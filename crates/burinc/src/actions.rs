//! Actions as fixed rules (DESIGN.md principle 3), lowered to LLVM IR, with their delta edges.
//!
//! `burin_act_<name>(ctx, db, w, viewer, vrec, args, deltas) -> i32` runs inside the writer's
//! transaction and returns its outcome (0: ok; otherwise an index into the outcomes):
//! - `require c else o`: the condition as one SQL expression (`SELECT …`), values bound;
//! - `create E {…}`: an INSERT; the id, seq and time come from the writer (`w`);
//! - `touch r`: an UPDATE of `updated`;
//! - `for each x in E where … { set x.f = v }`: one set-based UPDATE … RETURNING.
//!
//! Its delta edges are derived here, per write and register. **Forwarding** (no read after the
//! write) where the compiler can prove the values:
//! - a created row's register record is filled by the action from what it wrote, the viewer's
//!   kept record, and empty lists (a new row has none) — op 6, carrying the record;
//! - a set of an item field no filter or order reads patches the kept record — op 8.
//! Otherwise the writer rebuilds from SQLite (a read): op 1 build a new row's register; 3 rebuild a
//! register at a key; 4 rebuild every kept instance; 5 rebuild a screen list. Op 2 appends an
//! item to a screen list (monotone order). `emit` prints which edges forward and which read.

use crate::ast::*;
use crate::check::{Model, Scope};
use crate::inline;
use crate::llvm::{show, Module, RegInfo, Schema, Shape};
use crate::sql;
use std::collections::HashMap;
use std::fmt::Write;

/// A list a screen shows (its registers' keys, in order), kept per key.
#[derive(Clone)]
pub struct ScreenList {
    pub name: String,
    pub expr: Expr,
    pub entity: String,
    pub keyed_by: Option<String>,
    pub monotone: bool,
    /// newest first, with a limit: a new row goes first
    pub newest_first: bool,
    /// every (entity, column) its query reads
    pub reads: Vec<(String, String)>,
}

pub struct ActInfo {
    pub name: String,
    pub args: Vec<(String, Shape)>,
    pub outcomes: Vec<String>,
    /// how each delta edge is met: forwarded, patched, or read (for the report)
    pub edges: Vec<String>,
    /// run without a viewer
    pub public: bool,
    /// each `authenticates(hash, text)`: (its index, the hash's expression in the action's
    /// arguments alone, the text's argument). The runtime reads the hash and verifies the text on
    /// the request's thread, then hands the writer the hash it verified (`__verified_<i>`): the
    /// writer checks only that the stored hash is still that one
    pub verifies: Vec<(usize, Expr, String)>,
    /// arguments that are lists of rows: from repeated fields, as JSON arrays of ids
    pub list_args: Vec<String>,
    /// each `hash(text)`: (its index, the text's argument); the runtime hashes it off the writer
    /// (`__hashed_<i>`, empty for empty text)
    pub hashes: Vec<(usize, String)>,
}

/// `authenticates(h, p)` replaced by `h == __verified_<i>`, in order; the replaced (h, p) pairs.
fn verified(e: &Expr, out: &mut Vec<(Expr, Expr)>) -> Expr {
    match e {
        Expr::Call(f, a, s) if f == "authenticates" => {
            out.push((a[0].clone(), a[1].clone()));
            Expr::Bin(BinOp::Eq, Box::new(a[0].clone()), Box::new(Expr::Name(format!("__verified_{}", out.len() - 1), *s)), *s)
        }
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(verified(l, out)), Box::new(verified(r, out)), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(verified(x, out)), *s),
        other => other.clone(),
    }
}

/// Names replaced by expressions (an action's lets, by what they read).
fn subst(e: &Expr, env: &HashMap<String, Expr>) -> Expr {
    match e {
        Expr::Name(n, _) => env.get(n).cloned().unwrap_or_else(|| e.clone()),
        Expr::Field(o, f, s) => Expr::Field(Box::new(subst(o, env)), f.clone(), *s),
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(subst(l, env)), Box::new(subst(r, env)), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(subst(x, env)), *s),
        Expr::Call(f, a, s) => Expr::Call(f.clone(), a.iter().map(|x| subst(x, env)).collect(), *s),
        other => other.clone(),
    }
}

/// What a register's list reads.
struct ListRead {
    reg: usize,
    entity: String,
    keyed_by: Option<String>,
    /// paths its items' fields read (from the row)
    item_paths: Vec<Vec<String>>,
    /// paths its filter and order read
    query_paths: Vec<Vec<String>>,
    /// paths its filter alone reads
    filter_paths: Vec<Vec<String>>,
    nested: bool,
    /// a top-level list's index in the register's record, and its items' record
    slot: Option<usize>,
    items: Schema,
    var: String,
}

/// A patch function needed: (register, list, field).
type Patch = (usize, usize, usize);

fn eq_conjuncts(e: &Expr, out: &mut Vec<(String, Expr)>) {
    match e {
        Expr::Bin(BinOp::And, l, r, _) => {
            eq_conjuncts(l, out);
            eq_conjuncts(r, out);
        }
        Expr::Bin(BinOp::Eq, l, r, _) => {
            if let Expr::Dot(f, _) = &**l {
                out.push((f.clone(), (**r).clone()));
            } else if let Expr::Dot(f, _) = &**r {
                out.push((f.clone(), (**l).clone()));
            }
        }
        _ => {}
    }
}

fn row_paths(e: &Expr, out: &mut Vec<Vec<String>>) {
    fn path(e: &Expr) -> Option<Vec<String>> {
        match e {
            Expr::Dot(f, _) => Some(vec![f.clone()]),
            // `it.f` (a function's body inlined with the row as its argument)
            Expr::Field(o, f, _) if matches!(&**o, Expr::Name(n, _) if n == "it") => Some(vec![f.clone()]),
            Expr::Field(o, f, _) => {
                let mut p = path(o)?;
                p.push(f.clone());
                Some(p)
            }
            _ => None,
        }
    }
    if let Some(p) = path(e) {
        out.push(p);
        return;
    }
    match e {
        Expr::Field(o, _, _) | Expr::Not(o, _) => row_paths(o, out),
        Expr::Bin(_, l, r, _) => {
            row_paths(l, out);
            row_paths(r, out);
        }
        Expr::Call(_, a, _) => a.iter().for_each(|x| row_paths(x, out)),
        _ => {}
    }
}

fn list_reads(m: &Model, regs: &[RegInfo]) -> Result<Vec<ListRead>, String> {
    let mut out = vec![];
    fn walk(m: &Model, ri: usize, reg: &RegInfo, s: &Schema, nested: bool, out: &mut Vec<ListRead>) -> Result<(), String> {
        for (k, slot) in s.lists.iter().enumerate() {
            let (mut q, ent) = sql::resolve(m, &slot.expr, &Scope::default())?;
            // the filter with its functions expanded: the fields it reads through them count
            q.filter = q.filter.map(|x| inline::expr(m, &x, &inline::Env::new(), 0, 0));
            let root = if reg.per_viewer { "viewer".to_string() } else { reg.root.as_ref().map(|r| r.0.clone()).unwrap_or_default() };
            let mut eqs = vec![];
            if let Some(f) = &q.filter {
                eq_conjuncts(f, &mut eqs);
            }
            let keyed_by = if nested { None } else { eqs.iter().find(|(_, x)| matches!(x, Expr::Name(n, _) if *n == root)).map(|(f, _)| f.clone()) };
            let item_paths: Vec<Vec<String>> = slot.items.fields.iter().map(|(p, _)| p.split('.').skip(1).map(String::from).collect()).collect();
            let mut query_paths = vec![];
            if let Some(f) = &q.filter {
                row_paths(f, &mut query_paths);
            }
            let filter_paths = query_paths.clone();
            for (o, _) in &q.order {
                row_paths(o, &mut query_paths);
            }
            out.push(ListRead { reg: ri, entity: ent, keyed_by, item_paths, query_paths, filter_paths, nested, slot: if nested { None } else { Some(k) }, items: slot.items.clone(), var: slot.var.clone() });
            walk(m, ri, reg, &slot.items, true, out)?;
        }
        Ok(())
    }
    for (i, r) in regs.iter().enumerate() {
        // rendered per request: never kept, so no write reaches one
        if r.per_viewer {
            continue;
        }
        walk(m, i, r, &r.schema, false, &mut out)?;
    }
    Ok(out)
}

/// The screens' lists: `{{for x in view(param)}}` in a screen's template.
pub fn screen_lists(m: &Model) -> Result<Vec<ScreenList>, String> {
    let mut out = vec![];
    for d in &m.screens {
        if let Decl::Screen { params, body, .. } = d {
            for n in &body.nodes {
                if let TNode::For(_, list, _, _) = n {
                    let (mut q, ent) = sql::resolve(m, list, &Scope::of(params))?;
                    q.filter = q.filter.map(|x| inline::expr(m, &x, &inline::Env::new(), 0, 0));
                    let mut eqs = vec![];
                    if let Some(f) = &q.filter {
                        eq_conjuncts(f, &mut eqs);
                    }
                    let keyed_by = eqs.iter().find(|(_, x)| matches!(x, Expr::Name(n, _) if params.iter().any(|p| &p.name == n))).map(|(f, _)| f.clone());
                    let monotone = matches!(q.order.first(), Some((Expr::Dot(f, _), false)) if f == "created") && q.window_last.is_some();
                    let newest_first = matches!(q.order.first(), Some((Expr::Dot(f, _), true)) if f == "created") && q.limit.is_some() && q.window_last.is_none();
                    let var = match n {
                        TNode::For(v, ..) => v.clone(),
                        _ => unreachable!(),
                    };
                    let (sel, reads) = sql::reading(|| sql::list_select(m, list, &[var], &Scope::of(params)));
                    sel?;
                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone, newest_first, reads });
                }
            }
        }
    }
    Ok(out)
}

/// A screen's audience (its guard `exists(E where .F == viewer and …)`): the entity it reads,
/// the column equal to the screen's parameter (its key), and every column its filter reads.
#[derive(Clone)]
pub struct Audience {
    pub screen: usize,
    pub entity: String,
    pub key_col: Option<String>,
    pub cols: Vec<String>,
}

/// Every screen's audience, by the screen's index.
pub fn audiences(m: &Model) -> Vec<Audience> {
    let mut out = vec![];
    for (si, d) in m.screens.iter().enumerate() {
        let Decl::Screen { params, requires, .. } = d else { continue };
        for r in requires {
            let cond = inline::expr(m, &r.cond, &inline::Env::new(), 0, 0);
            let Expr::Call(f, a, _) = &cond else { continue };
            let Some(Expr::Query(q)) = a.first() else { continue };
            let (Source::Entity(ent), Some(filter)) = (&q.source, &q.filter) else { continue };
            if f != "exists" || !mentions_viewer(filter) {
                continue;
            }
            let mut eqs = vec![];
            eq_conjuncts(filter, &mut eqs);
            let key_col = eqs.iter().find(|(_, x)| matches!(x, Expr::Name(n, _) if params.iter().any(|p| &p.name == n))).map(|(c, _)| c.clone());
            let mut paths = vec![];
            row_paths(filter, &mut paths);
            let mut cols: Vec<String> = paths.into_iter().filter_map(|p| p.first().cloned()).collect();
            cols.sort();
            cols.dedup();
            out.push(Audience { screen: si, entity: ent.clone(), key_col, cols });
        }
    }
    out
}

fn mentions_viewer(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "viewer",
        Expr::Field(o, _, _) | Expr::Not(o, _) => mentions_viewer(o),
        Expr::Bin(_, l, r, _) => mentions_viewer(l) || mentions_viewer(r),
        Expr::Call(_, a, _) => a.iter().any(mentions_viewer),
        _ => false,
    }
}

struct Act<'a, 'b> {
    m: &'a Model<'a>,
    md: &'b mut Module,
    b: String,
    n: usize,
    qid: &'b mut usize,
    args: Vec<(String, Shape)>,
    /// the parameters that are rows (name, entity): a guard reads their fields by their id
    rows: Vec<(String, String)>,
    outcomes: Vec<String>,
    fails: Vec<(String, usize)>,
    /// the entities this action creates (a new row's lists of these may not be empty)
    creates: Vec<String>,
    patches: Vec<Patch>,
    /// (register, list, reference field): kept lists searched for a row they reference
    finds: Vec<Patch>,
    edges: Vec<String>,
    audiences: Vec<Audience>,
    /// lets: by name, the pointer their value is kept at, their expression (for the runtime's
    /// verification) and type
    lets: HashMap<String, (String, Expr, Type)>,
    /// the authenticates seen so far (their index is their hidden argument's)
    verifies: Vec<(usize, Expr, String)>,
    /// the entry block's allocas
    allocas: String,
    /// the registers ('r') and screen lists ('l') a write's precise edges reached: in full (None),
    /// or for one read path (its start table, its path)
    touched: Vec<(char, usize, Option<(String, String)>)>,
    hashes: Vec<(usize, String)>,
    /// the action's parameters, for the types of values it computes
    scope: Scope,
}

impl<'a, 'b> Act<'a, 'b> {
    fn v(&mut self) -> String {
        self.n += 1;
        format!("%a{}", self.n)
    }
    fn line(&mut self, s: &str) {
        for l in s.split('\n') {
            if l.trim_start().contains(" = alloca ") {
                let _ = writeln!(self.allocas, "  {}", l.trim_start());
            } else {
                let _ = writeln!(self.b, "  {}", l.trim_start());
            }
        }
    }
    fn note(&mut self, s: String) {
        if !self.edges.contains(&s) {
            self.edges.push(s);
        }
    }

    /// An argument's, the viewer's or the time's value: i64, or a pointer to a %Str.
    fn value(&mut self, e: &Expr) -> Result<(String, Shape), String> {
        if let Expr::Name(n, _) = e {
            if let Some((p, _, _)) = self.lets.get(n).cloned() {
                let x = self.v();
                self.line(&format!("{x} = load i64, ptr {p}"));
                return Ok((x, Shape::I64));
            }
        }
        match e {
            Expr::Name(n, _) if n == "viewer" => Ok(("%viewer".into(), Shape::I64)),
            Expr::Name(n, _) if n == "now" => Ok(("%now".into(), Shape::I64)),
            Expr::Name(n, _) if n == "@seq" => {
                let x = self.v();
                self.line(&format!("{x} = call i64 @burin_w_seq_now(ptr %w)"));
                Ok((x, Shape::I64))
            }
            // a number SQL computed (RETURNING), by its value
            Expr::Name(n, _) if n.starts_with("@i64") => Ok((n[4..].to_string(), Shape::I64)),
            // a text made once (a token), by the pointer it was made at
            Expr::Name(n, _) if n.starts_with("@ptr") => Ok((n[4..].to_string(), Shape::Str)),
            Expr::Int(n, _) | Expr::Dur(n, _) => Ok((n.to_string(), Shape::I64)),
            Expr::Bool(b, _) => Ok(((*b as i64).to_string(), Shape::I64)),
            Expr::Name(n, _) => {
                let i = self.args.iter().position(|(a, _)| a == n).ok_or_else(|| format!("`{n}` isn't an argument here"))?;
                let shape = self.args[i].1;
                let p = self.v();
                self.line(&format!("{p} = getelementptr inbounds %Args, ptr %args, i32 0, i32 {i}"));
                if shape == Shape::I64 {
                    let x = self.v();
                    self.line(&format!("{x} = load i64, ptr {p}"));
                    Ok((x, shape))
                } else {
                    Ok((p, shape))
                }
            }
            Expr::Str(s, _) => {
                let g = self.md.str_const_pub(s.as_bytes());
                let p = self.v();
                self.line(&format!("{p} = alloca %Str"));
                let (s1, s2) = (self.v(), self.v());
                self.line(&format!("{s1} = insertvalue %Str undef, ptr {g}, 0\n  {s2} = insertvalue %Str {s1}, i64 {}, 1\n  store %Str {s2}, ptr {p}", s.len()));
                Ok((p, Shape::Str))
            }
            Expr::Call(f, _, _) if f == "hash" => {
                let n = self.unhash(e)?;
                self.value(&n)
            }
            // arithmetic (`now + 5min`, `p.level * 2`): computed by SQLite, as `require` and `set`
            // are, so an action follows one set of rules for numbers
            Expr::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod, ..) => self.computed(e),
            Expr::Call(f, _, _) if matches!(f.as_str(), "min" | "max" | "clamp" | "abs") => self.computed(e),
            Expr::Call(f, a, _) if f == "token" => {
                let n = match a.first() {
                    Some(Expr::Int(n, _)) => *n,
                    _ => return Err("token takes a constant length".into()),
                };
                let p = self.v();
                self.line(&format!("{p} = alloca %Str\n  call void @burin_w_token(ptr %w, i64 {n}, ptr {p})"));
                Ok((p, Shape::Str))
            }
            // an external function: its arguments as the C ABI takes them (text as its bytes'
            // pointer and length); a text result written into the writer's buffer, then kept
            Expr::Call(f, a, _) if self.m.externs.contains_key(f) => {
                let Some(Decl::Extern { params, ret, .. }) = self.m.externs.get(f) else { unreachable!() };
                let (params, ret) = (params.clone(), ret.clone());
                let mut list = vec![];
                for (p, x) in params.iter().zip(a) {
                    let (v, shape) = self.value(x)?;
                    if shape == Shape::Str {
                        let (s, ptr, len) = (self.v(), self.v(), self.v());
                        self.line(&format!("{s} = load %Str, ptr {v}\n  {ptr} = extractvalue %Str {s}, 0\n  {len} = extractvalue %Str {s}, 1"));
                        list.push(format!("ptr {ptr}, i64 {len}"));
                    } else if matches!(p.ty, Type::Text) {
                        return Err(format!("`{f}`'s `{}` is text", p.name));
                    } else {
                        list.push(format!("i64 {v}"));
                    }
                }
                if matches!(ret, Type::Text) {
                    let (o, d) = (self.v(), self.v());
                    let sep = if list.is_empty() { "" } else { ", " };
                    self.line(&format!("{o} = call ptr @burin_w_ext_out(ptr %w)\n  call void @{}({}{sep}ptr {o})", self.m.extern_symbol(f), list.join(", ")));
                    self.line(&format!("{d} = alloca %Str\n  call void @burin_w_ext_take(ptr %w, ptr {d})"));
                    Ok((d, Shape::Str))
                } else {
                    let r = self.v();
                    self.line(&format!("{r} = call i64 @{}({})", self.m.extern_symbol(f), list.join(", ")));
                    Ok((r, Shape::I64))
                }
            }
            _ => Err(format!("an action can't compute `{}` yet", show(e))),
        }
    }

    /// A number, time or duration computed in one `SELECT` (none: NULL, read back as i64::MIN).
    fn computed(&mut self, e: &Expr) -> Result<(String, Shape), String> {
        let mut sc = self.scope.clone();
        for (k, (_, _, t)) in &self.lets {
            sc = sc.with(k, t.clone());
        }
        // text joined with `+` would be SQLite's numeric `+`
        if matches!(self.m.type_of(e, &sc).map(|t| t.strip_opt().clone()), Ok(Type::Text)) {
            return Err(format!("an action can't join text (`{}`) yet", show(e)));
        }
        let x = inline::expr(self.m, e, &inline::Env::new(), 0, 0);
        let sel = sql::cond_select_with(self.m, &x, &self.rows)?;
        let st = self.stmt(&sel)?;
        let v = self.v();
        self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
        self.line(&format!("{v} = call i64 @burin_col_int(ptr {st}, i32 0)"));
        self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
        Ok((v, Shape::I64))
    }

    /// `hash(arg)` as its hidden argument (the runtime hashes the text off the writer).
    fn unhash(&mut self, e: &Expr) -> Result<Expr, String> {
        match e {
            Expr::Call(f, a, s) if f == "hash" => {
                let arg = match a.first() {
                    Some(Expr::Name(n, _)) if self.args.iter().any(|(x, sh)| x == n && *sh == Shape::Str) => n.clone(),
                    _ => return Err("hash takes a text argument".into()),
                };
                let i = match self.hashes.iter().position(|(_, x)| *x == arg) {
                    Some(i) => i,
                    None => {
                        self.hashes.push((self.hashes.len(), arg));
                        self.hashes.len() - 1
                    }
                };
                Ok(Expr::Name(format!("__hashed_{i}"), *s))
            }
            other => Ok(other.clone()),
        }
    }

    /// A precise edge covers this read path of register `reg`.
    fn cover(&mut self, reg: usize, start: &str, path: &str) {
        self.touched.push(('r', reg, Some((start.to_string(), path.to_string()))));
    }

    fn outcome(&mut self, o: &str) -> usize {
        if !self.outcomes.iter().any(|x| x == o) {
            self.outcomes.push(o.to_string());
        }
        self.outcomes.iter().position(|x| x == o).unwrap()
    }

    /// Text written to a field with `max N`: at most N characters, else the outcome `invalid`
    /// (checked here, before SQLite: the same rule as the table's CHECK).
    fn check_max(&mut self, ent: &str, f: &str, e: &Expr) -> Result<(), String> {
        let max = self.m.entities.get(ent).and_then(|x| x.fields.iter().find(|x| x.name == f)).and_then(|x| if matches!(x.ty.strip_opt(), Type::Text) { x.max } else { None });
        let (Some(n), Expr::Name(a, _)) = (max, e) else { return Ok(()) };
        if !self.args.iter().any(|(x, _)| x == a) {
            return Ok(());
        }
        let (v, _) = self.value(e)?;
        let (c, ok) = (self.v(), self.v());
        self.line(&format!("{c} = call i64 @burin_str_chars(ptr {v})"));
        self.line(&format!("{ok} = icmp sle i64 {c}, {n}"));
        let code = self.outcome("invalid");
        let (pass, fail) = (format!("pass{}", self.n), format!("fail{}", self.n));
        self.n += 1;
        let _ = writeln!(self.b, "  br i1 {ok}, label %{pass}, label %{fail}
{pass}:");
        self.fails.push((fail, code));
        Ok(())
    }

    fn bind(&mut self, st: &str, i: usize, e: &Expr) -> Result<(), String> {
        let (v, shape) = self.value(e)?;
        if shape == Shape::I64 {
            // none (i64::MIN) is bound as NULL
            self.line(&format!("call void @burin_bind_i64(ptr {st}, i32 {i}, i64 {v})"));
        } else {
            self.line(&format!("call void @burin_bind_str(ptr {st}, i32 {i}, ptr {v})"));
        }
        Ok(())
    }

    fn stmt_text(&mut self, sql_text: &str) -> String {
        let g = self.md.str_const_pub(sql_text.as_bytes());
        *self.qid += 1;
        let st = self.v();
        let _ = writeln!(self.b, "  ; {sql_text}");
        self.line(&format!("{st} = call ptr @burin_stmt(ptr %db, i64 {}, ptr {g}, i64 {})", self.qid, sql_text.len()));
        st
    }

    fn stmt(&mut self, sel: &sql::Select) -> Result<String, String> {
        let st = self.stmt_text(&sel.sql);
        for (i, sql::Bind::Path(e)) in sel.binds.iter().enumerate() {
            self.bind(&st, i + 1, e)?;
        }
        Ok(st)
    }

    fn delta(&mut self, op: i32, a: usize, key: &str, item: &str) {
        // ops that rebuild a register or list in full cover all its reads (op 3 covers the read
        // path its caller names: `cover`)
        // op 4 rebuilds every kept instance: all its reads covered. Ops 1, 6 and 9 concern one new
        // or deleted row's own instance: the register's other instances may still read what the
        // write changed (a scan of the table: the fail-safe below sees it)
        match op {
            4 => self.touched.push(('r', a, None)),
            2 | 5 | 7 => self.touched.push(('l', a, None)),
            _ => {}
        }
        self.line(&format!("call void @burin_delta(ptr %d, i32 {op}, i64 {a}, i64 {key}, i64 {item})"));
    }

    /// A created row's register record, filled from what the action wrote, if every field can
    /// be: the record's pointer (in the writer's arena).
    fn forward(&mut self, reg: &RegInfo, fields: &[(String, Expr)], id: &str, seq: &str) -> Result<Option<String>, String> {
        let (root, ent) = match &reg.root {
            Some(r) => r.clone(),
            None => return Ok(None),
        };
        if !reg.schema.scalars.is_empty() {
            return Ok(None);
        }
        // each list of the new row: empty, if it's keyed to the row and this action makes none
        for slot in &reg.schema.lists {
            let (q, lent) = sql::resolve(self.m, &slot.expr, &Scope::default())?;
            let mut eqs = vec![];
            if let Some(f) = &q.filter {
                eq_conjuncts(f, &mut eqs);
            }
            let keyed = eqs.iter().any(|(_, x)| matches!(x, Expr::Name(n, _) if *n == root));
            if !keyed || self.creates.contains(&lent) {
                return Ok(None);
            }
        }
        // each field: plan where its value comes from, before emitting anything
        enum Src {
            Id,
            Now,
            Seq,
            Val(Expr),
            None(Shape),
            Viewer(String),
        }
        let mut plan = vec![];
        for (path, shape) in &reg.schema.fields {
            let parts: Vec<&str> = path.split('.').collect();
            if parts[0] != root {
                return Ok(None);
            }
            let src = match &parts[1..] {
                [] => Src::Id,
                ["created"] | ["updated"] => Src::Now,
                ["seq"] => Src::Seq,
                [f] => match fields.iter().find(|(n, _)| n == f) {
                    Some((_, e)) => Src::Val(e.clone()),
                    None if self.m.field_type(&ent, f).is_some() => Src::None(*shape),
                    None => return Ok(None),
                },
                [f, rest @ ..] => match fields.iter().find(|(n, _)| n == f) {
                    Some((_, Expr::Name(v, _))) if v == "viewer" => Src::Viewer(std::iter::once("viewer").chain(rest.iter().copied()).collect::<Vec<_>>().join(".")),
                    _ => return Ok(None),
                },
            };
            plan.push((src, *shape));
        }
        let ty = format!("%In_{}", reg.name);
        let (size, sz, rec) = (self.v(), self.v(), self.v());
        self.line(&format!("{size} = getelementptr {ty}, ptr null, i64 1\n  {sz} = ptrtoint ptr {size} to i64\n  {rec} = call ptr @burin_w_alloc(ptr %w, i64 {sz})"));
        for (i, (src, shape)) in plan.into_iter().enumerate() {
            let p = self.v();
            self.line(&format!("{p} = getelementptr inbounds {ty}, ptr {rec}, i32 0, i32 {i}"));
            match src {
                Src::Id => self.line(&format!("store i64 {id}, ptr {p}")),
                Src::Now => self.line(&format!("store i64 %now, ptr {p}")),
                Src::Seq => self.line(&format!("store i64 {seq}, ptr {p}")),
                Src::None(Shape::I64) => self.line(&format!("store i64 -9223372036854775808, ptr {p}")),
                Src::None(Shape::Str) => {}
                Src::Val(e) => {
                    let (v, sh) = self.value(&e)?;
                    if sh == Shape::I64 {
                        self.line(&format!("store i64 {v}, ptr {p}"));
                    } else {
                        // copied into the writer's arena: the record outlives the action's arguments
                        self.line(&format!("call void @burin_w_copy(ptr %w, ptr {v}, ptr {p})"));
                    }
                }
                Src::Viewer(key) => {
                    // the viewer's kept record has it (added to the viewer's record here)
                    let vi = self.md.viewer_field(&key, shape);
                    let (q, x) = (self.v(), self.v());
                    self.line(&format!("{q} = getelementptr inbounds %Viewer, ptr %vrec, i32 0, i32 {vi}"));
                    if shape == Shape::Str {
                        self.line(&format!("call void @burin_w_copy(ptr %w, ptr {q}, ptr {p})"));
                    } else {
                        self.line(&format!("{x} = load i64, ptr {q}\n  store i64 {x}, ptr {p}"));
                    }
                }
            }
        }
        Ok(Some(rec))
    }

    /// The delta edges of a write to `ent`'s `cols` ("*": a new row), for each row it changed.
    /// `vals`: the row's columns known here, by name; `sets`: the values written (for patches).
    #[allow(clippy::too_many_arguments)]
    fn edges(&mut self, ent: &str, cols: &[String], vals: &HashMap<String, String>, sets: &[(String, Expr)], regs: &[RegInfo], lists: &[ListRead], screens: &[ScreenList], new: Option<(&str, &str, &[(String, Expr)])>, removed: bool) -> Result<(), String> {
        let created = new.is_some();
        self.touched.clear();
        // rows enter or leave lists
        let rows_change = created || removed;
        let verb = if created { "create" } else if removed { "delete" } else { "set" };
        // registers keyed by a row of this entity
        for (ri, r) in regs.iter().enumerate() {
            if r.per_viewer || !r.root.as_ref().map(|(_, e)| e == ent).unwrap_or(false) {
                continue;
            }
            if removed {
                let id = vals.get("id").cloned().ok_or("the deleted row's id")?;
                self.delta(9, ri, &id, "0");
                self.note(format!("delete {ent} → {}: dropped (no read)", r.name));
            } else if let Some((id, seq, fields)) = new {
                match self.forward(r, fields, id, seq)? {
                    Some(rec) => {
                        let x = self.v();
                        self.line(&format!("{x} = ptrtoint ptr {rec} to i64"));
                        self.delta(6, ri, id, &x);
                        self.note(format!("create {ent} → {}: forwarded (its record filled from the write)", r.name));
                    }
                    None => {
                        self.delta(1, ri, id, "0");
                        self.note(format!("create {ent} → {}: READ (its record isn't all known from the write)", r.name));
                    }
                }
            } else if r.schema.fields.iter().any(|(p, _)| {
                let ps: Vec<String> = p.split('.').skip(1).map(String::from).collect();
                ps.len() == 1 && cols.contains(&ps[0])
            }) {
                let id = vals.get("id").cloned().unwrap_or_default();
                self.delta(3, ri, &id, "0");
                for c in cols {
                    self.cover(ri, ent, c);
                }
                self.note(format!("set {ent}.{} → {}: READ (rebuilt)", cols.join(","), r.name));
            }
        }
        // a row-keyed register's fields through a reference to the written row (a message's
        // creator's name): every kept one rebuilt
        for (ri, r) in regs.iter().enumerate().filter(|(_, r)| !r.per_viewer) {
            let Some((_, rent)) = &r.root else { continue };
            if rows_change {
                continue;
            }
            for (p, _) in &r.schema.fields {
                let ps: Vec<&str> = p.split('.').skip(1).collect();
                if ps.len() == 2 && cols.iter().any(|c| c == ps[1]) && self.m.field_type(rent, ps[0]).map(|t| matches!(t.strip_opt(), Type::Ref(e) if e == ent)).unwrap_or(false) {
                    self.delta(4, ri, "0", "0");
                    self.note(format!("set {ent}.{} → {} (through {rent}.{}): READ (every kept one rebuilt)", cols.join(","), r.name, ps[0]));
                    break;
                }
            }
        }
        // registers' lists over this entity, or reaching it through a reference
        for l in lists {
            let reg_name = regs[l.reg].name.clone();
            let reads_item = |c: &String| l.item_paths.iter().any(|p| p.len() == 1 && &p[0] == c);
            let reads_query = |c: &String| l.query_paths.iter().any(|p| p.len() == 1 && &p[0] == c);
            let direct = l.entity == ent && (rows_change || cols.iter().any(|c| reads_item(c) || reads_query(c)));
            // (a deleted row has no references left to it: the checker proved it)
            let via: Option<String> = if rows_change || l.entity == ent {
                None
            } else {
                l.item_paths.iter().chain(l.query_paths.iter()).find(|p| {
                    p.len() == 2 && cols.contains(&p[1]) && self.m.field_type(&l.entity, &p[0]).map(|t| matches!(t.strip_opt(), Type::Ref(e) if e == ent)).unwrap_or(false)
                }).map(|p| p[0].clone())
            };
            if !direct && via.is_none() {
                continue;
            }
            if l.nested {
                self.delta(4, l.reg, "0", "0");
                self.note(format!("{verb} {ent} → {reg_name} (a nested list): READ (every kept one rebuilt)"));
                continue;
            }
            if direct {
                let key = l.keyed_by.as_ref().and_then(|f| vals.get(f).cloned());
                // a patch: only item fields change, nothing the list's filter or order reads
                match key {
                    Some(k) => {
                        self.delta(3, l.reg, &k, "0");
                        self.cover(l.reg, &l.entity, "*");
                        for c in cols {
                            self.cover(l.reg, &l.entity, c);
                        }
                        self.note(format!("{verb} {ent} → {reg_name}: READ (rebuilt at its key)"));
                    }
                    None => {
                        self.delta(4, l.reg, "0", "0");
                        self.note(format!("{verb} {ent} → {reg_name}: READ (every kept one rebuilt)"));
                    }
                }
            } else if let (Some(p0), Some(f)) = (via, &l.keyed_by) {
                // the keys of the list's rows that reach the written row
                let id = vals.get("id").cloned().ok_or("the written row's id")?;
                let s = sql::reach(&l.entity, f, &p0);
                let st = self.stmt_text(&s);
                self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 1, i64 {id})"));
                let (h, body, done) = (format!("rh{}", self.n), format!("rb{}", self.n), format!("rd{}", self.n));
                self.n += 1;
                let (rc, ok, k) = (self.v(), self.v(), self.v());
                let _ = writeln!(self.b, "  br label %{h}\n{h}:\n  {rc} = call i32 @sqlite3_step(ptr {st})\n  {ok} = icmp eq i32 {rc}, 100\n  br i1 {ok}, label %{body}, label %{done}\n{body}:");
                self.line(&format!("{k} = call i64 @burin_col_i64(ptr {st}, i32 0)"));
                self.delta(3, l.reg, &k, "0");
                for c in cols {
                    self.cover(l.reg, &l.entity, &format!("{p0}.{c}"));
                }
                let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call i32 @sqlite3_reset(ptr {st})");
                self.note(format!("set {ent}.{} → {reg_name} (through {}.{p0}): READ (each reaching key rebuilt)", cols.join(","), l.entity));
            } else {
                self.delta(4, l.reg, "0", "0");
                self.note(format!("set {ent}.{} → {reg_name}: READ (every kept one rebuilt)", cols.join(",")));
            }
        }
        // the sessions the snapshot knows: a session row made, changed or deleted (op 10: read it
        // by its id; op 11: forget its token)
        if let Some(Decl::Viewer { via_entity, via_field, to_field, .. }) = self.m.viewer {
            if via_entity == ent && (rows_change || cols.iter().any(|c| c == via_field || c == to_field)) {
                let id = vals.get("id").cloned().ok_or("the session row's id")?;
                self.delta(if removed { 11 } else { 10 }, 0, &id, "0");
                self.note(format!("{verb} {ent} → the sessions: {}", if removed { "forgotten (no read)" } else { "READ (the row, by its id)" }));
            }
        }
        // the screens' audiences that read this entity: read again at the key (every key when
        // the key column itself changes: op 12 with no key)
        for a in self.audiences.clone() {
            if a.entity != ent || !(rows_change || cols.iter().any(|c| a.cols.contains(c))) {
                continue;
            }
            let moved = !rows_change && a.key_col.as_ref().map(|k| cols.contains(k)).unwrap_or(true);
            let key = if moved { None } else { a.key_col.as_ref().and_then(|k| vals.get(k).cloned()) };
            match key {
                Some(k) => {
                    self.delta(12, a.screen, &k, "0");
                    self.note(format!("{verb} {ent} → the audience of screen {}: READ (the audience at its key; the page rebuilt if it changed)", a.screen));
                }
                None => {
                    self.delta(12, a.screen, "-9223372036854775808", "0");
                    self.note(format!("{verb} {ent} → the audience of screen {}: READ (every kept instance's audience)", a.screen));
                }
            }
        }
        if removed {
            for (si, d) in self.m.screens.iter().enumerate() {
                if let Decl::Screen { params, .. } = d {
                    if matches!(params.first().map(|p| p.ty.strip_opt()), Some(Type::Ref(e)) if e == ent) {
                        let id = vals.get("id").cloned().ok_or("the deleted row's id")?;
                        self.delta(13, si, &id, "0");
                        self.note(format!("delete {ent} → screen {si} at it: dropped (no read)"));
                    }
                }
            }
        }
        // the screens' lists of this entity
        for (si, s) in screens.iter().enumerate() {
            if s.entity != ent {
                continue;
            }
            if removed {
                // a row left the list: the next row in its order may come in (a read)
                if let Some(k) = s.keyed_by.as_ref().and_then(|f| vals.get(f).cloned()) {
                    let id = vals.get("id").cloned().ok_or("the deleted row's id")?;
                    self.delta(5, si, &k, &id);
                    self.note(format!("delete {ent} → screen list {}: READ (the list again)", s.name));
                }
                continue;
            }
            if let (Some((id, _, _)), Some(f)) = (new, &s.keyed_by) {
                if let Some(k) = vals.get(f).cloned() {
                    let (op, how) = if s.monotone {
                        (2, "appended (oldest first: the new row is last)")
                    } else if s.newest_first {
                        (7, "prepended (newest first: the new row is first)")
                    } else {
                        (5, "READ (rebuilt)")
                    };
                    self.delta(op, si, &k, id);
                    self.note(format!("create {ent} → screen list {}: {how}", s.name));
                }
            }
        }
        // the fail-safe: anything that reads what was written (its read set, recorded where its
        // SQL was made) and that no precise edge reached is rebuilt in full. A missing rule costs
        // speed, never correctness; the build reports it
        // a row made or deleted reaches what scans its entity ("*"); a column read through a
        // reference reaches only through the referring row (that row's own write covers it)
        let m = self.m;
        let reaches = |start: &str, path: &str| match sql::target(m, start, path) {
            Some((e, c)) => e == ent && if rows_change { c == "*" } else { cols.contains(&c) },
            None => false,
        };
        let hit = |reads: &[(String, String)]| reads.iter().any(|(s, p)| reaches(s, p));
        let uncovered = |touched: &[(char, usize, Option<(String, String)>)], kind: char, i: usize, reads: &[(String, String)]| {
            if touched.iter().any(|(k, x, c)| *k == kind && *x == i && c.is_none()) {
                return false;
            }
            reads.iter().any(|(s, p)| reaches(s, p) && !touched.iter().any(|(k, x, c)| *k == kind && *x == i && c.as_ref() == Some(&(s.clone(), p.clone()))))
        };
        for (ri, r) in regs.iter().enumerate() {
            if !r.per_viewer && uncovered(&self.touched, 'r', ri, &r.reads) {
                self.delta(4, ri, "0", "0");
                self.note(format!("{verb} {ent}.{} → {}: FALLBACK (every kept one rebuilt; no precise rule)", cols.join(","), r.name));
            }
        }
        for (si, s) in screens.iter().enumerate() {
            if uncovered(&self.touched, 'l', si, &s.reads) {
                self.delta(5, si, "-9223372036854775808", "0");
                self.note(format!("{verb} {ent}.{} → screen list {}: FALLBACK (every kept one read again; no precise rule)", cols.join(","), s.name));
            }
        }
        // (a new row changes no one's record)
        if !created && hit(&self.md.viewer_reads.clone()) {
            let own = self.m.viewer_entity() == Some(ent);
            match vals.get("id").cloned().filter(|_| own) {
                // the viewer's own row: its record, its registers and the pages it can see
                Some(id) => {
                    self.delta(15, 0, &id, "0");
                    self.note(format!("{verb} {ent}.{} → that viewer: READ (their record, their own registers, the pages they can see)", cols.join(",")));
                }
                None => {
                    self.delta(15, 0, "-9223372036854775808", "0");
                    self.note(format!("{verb} {ent}.{} → the viewers' records: FALLBACK (all read again, the screens rebuilt)", cols.join(",")));
                }
            }
        }
        Ok(())
    }

    fn stmts(&mut self, body: &[Stmt], scope: &Scope, regs: &[RegInfo], lists: &[ListRead], screens: &[ScreenList]) -> Result<(), String> {
        for s in body {
            match s {
                Stmt::Require(r) => {
                    let code = match &r.otherwise {
                        Else::Outcome(o) => self.outcome(o),
                        _ => return Err("an action's require ends in an outcome (`else not_found`)".into()),
                    };
                    let cond = inline::expr(self.m, &r.cond, &inline::Env::new(), 0, 0);
                    let mut pairs = vec![];
                    let base = self.verifies.len();
                    let cond = {
                        // indexes continue from earlier requires
                        let mut all: Vec<(Expr, Expr)> = (0..base).map(|_| (Expr::None(Span::default()), Expr::None(Span::default()))).collect();
                        let c = verified(&cond, &mut all);
                        pairs = all.split_off(base);
                        c
                    };
                    let env: HashMap<String, Expr> = self.lets.iter().map(|(n, (_, e, _))| (n.clone(), e.clone())).collect();
                    for (i, (h, p)) in pairs.into_iter().enumerate() {
                        let arg = match p {
                            Expr::Name(a, _) if self.args.iter().any(|(x, s)| *x == a && *s == Shape::Str) => a,
                            _ => return Err("authenticates checks an argument's text".into()),
                        };
                        self.verifies.push((base + i, subst(&h, &env), arg));
                    }
                    let sel = sql::cond_select_with(self.m, &cond, &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (x, ok) = (self.v(), self.v());
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    self.line(&format!("{x} = call i64 @burin_col_i64(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("{ok} = icmp eq i64 {x}, 1"));
                    let (pass, fail) = (format!("pass{}", self.n), format!("fail{}", self.n));
                    self.n += 1;
                    let _ = writeln!(self.b, "  br i1 {ok}, label %{pass}, label %{fail}\n{pass}:");
                    self.fails.push((fail, code));
                }
                Stmt::Create(ent, fields, name, _) => {
                    for (f, v) in fields {
                        self.check_max(ent, f, v)?;
                    }
                    let gid = self.md.str_const_pub(ent.as_bytes());
                    let (id, seq) = (self.v(), self.v());
                    self.line(&format!("{id} = call i64 @burin_w_id(ptr %w, ptr %db, ptr {gid}, i64 {})", ent.len()));
                    self.line(&format!("{seq} = call i64 @burin_w_seq(ptr %w)"));
                    let mut cols = vec!["id".to_string(), "seq".into(), "created".into(), "updated".into()];
                    cols.extend(fields.iter().map(|(f, _)| f.clone()));
                    let sql_text = format!(
                        "INSERT INTO \"{ent}\" ({}) VALUES ({})",
                        cols.iter().map(|c| if c == "id" { c.clone() } else { format!("\"{c}\"") }).collect::<Vec<_>>().join(", "),
                        vec!["?"; cols.len()].join(", ")
                    );
                    let st = self.stmt_text(&sql_text);
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 1, i64 {id})"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 2, i64 {seq})"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 3, i64 %now)"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 4, i64 %now)"));
                    let mut vals = HashMap::new();
                    vals.insert("id".to_string(), id.clone());
                    // a value used twice (a token) is made once: bound, and forwarded
                    let mut fixed: Vec<(String, Expr)> = vec![];
                    for (i, (f, e)) in fields.iter().enumerate() {
                        let (v, shape) = self.value(e)?;
                        if shape == Shape::I64 {
                            self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 {}, i64 {v})", i + 5));
                            vals.insert(f.clone(), v.clone());
                            // a computed value (`now + 5min`), forwarded as the value bound
                            fixed.push((f.clone(), if matches!(e, Expr::Bin(..) | Expr::Call(..)) { Expr::Name(format!("@i64{v}"), Span::default()) } else { e.clone() }));
                        } else {
                            self.line(&format!("call void @burin_bind_str(ptr {st}, i32 {}, ptr {v})", i + 5));
                            // the made value, by its pointer, for the forwarded record
                            fixed.push((f.clone(), Expr::Name(format!("@ptr{v}"), Span::default())));
                        }
                    }
                    self.line(&format!("call void @burin_exec(ptr {st})"));
                    self.edges(ent, &["*".into()], &vals, &[], regs, lists, screens, Some((&id, &seq, &fixed)), false)?;
                    if let Some(n) = name {
                        let p = format!("%let_{n}_{}", self.n);
                        self.n += 1;
                        self.line(&format!("{p} = alloca i64"));
                        self.line(&format!("store i64 {id}, ptr {p}"));
                        self.rows.push((n.clone(), ent.clone()));
                        self.lets.insert(n.clone(), (p, Expr::None(Span::default()), Type::Ref(ent.clone())));
                    }
                }
                Stmt::Touch(e, _) => {
                    let ent = match self.m.type_of(e, scope).map_err(|x| x.msg)? {
                        Type::Ref(t) => t,
                        _ => return Err("touch takes a row".into()),
                    };
                    let (id, _) = self.value(e)?;
                    let st = self.stmt_text(&format!("UPDATE \"{ent}\" SET \"updated\" = ? WHERE id = ?"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 1, i64 %now)"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 2, i64 {id})"));
                    self.line(&format!("call void @burin_exec(ptr {st})"));
                    let mut vals = HashMap::new();
                    vals.insert("id".to_string(), id);
                    self.edges(&ent, &["updated".into()], &vals, &[("updated".into(), Expr::Name("now".into(), Span::default()))], regs, lists, screens, None, false)?;
                }
                Stmt::Let(n, e, _) => {
                    let mut sc = scope.clone();
                    for (k, (_, _, t)) in &self.lets {
                        sc = sc.with(k, t.clone());
                    }
                    let t = self.m.type_of(e, &sc).map_err(|x| x.msg)?;
                    let x = inline::expr(self.m, e, &inline::Env::new(), 0, 0);
                    let sel = sql::cond_select_with(self.m, &x, &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (p, v) = (format!("%let_{n}_{}", self.n), self.v());
                    self.line(&format!("{p} = alloca i64"));
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    // a computed number that overflowed (a REAL) is none, as in a page
                    self.line(&format!("{v} = call i64 @burin_col_int(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("store i64 {v}, ptr {p}"));
                    if let Type::Ref(ent) = t.strip_opt() {
                        self.rows.push((n.clone(), ent.clone()));
                    }
                    self.lets.insert(n.clone(), (p, x, t));
                }
                Stmt::SignIn(e, span) => {
                    let Some(Decl::Viewer { via_entity, via_field, to_field, .. }) = self.m.viewer else { return Err("sign in needs a viewer".into()) };
                    let (tok, _) = self.value(&Expr::Call("token".into(), vec![Expr::Int(32, *span)], *span))?;
                    let fields = vec![(to_field.clone(), e.clone()), (via_field.clone(), Expr::Name(format!("@ptr{tok}"), *span))];
                    self.stmts(&[Stmt::Create(via_entity.clone(), fields, None, *span)], scope, regs, lists, screens)?;
                    self.line(&format!("call void @burin_w_sign_in(ptr %w, ptr {tok})"));
                }
                Stmt::If(c, a, b, _) => {
                    let cond = inline::expr(self.m, c, &inline::Env::new(), 0, 0);
                    let sel = sql::cond_select_with(self.m, &cond, &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (x, ok) = (self.v(), self.v());
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    self.line(&format!("{x} = call i64 @burin_col_i64(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("{ok} = icmp eq i64 {x}, 1"));
                    let (lt, lf, le) = (format!("ift{}", self.n), format!("iff{}", self.n), format!("ife{}", self.n));
                    self.n += 1;
                    let _ = writeln!(self.b, "  br i1 {ok}, label %{lt}, label %{lf}\n{lt}:");
                    // names made in a branch stay in it
                    let (lets, rows) = (self.lets.clone(), self.rows.clone());
                    self.stmts(a, scope, regs, lists, screens)?;
                    (self.lets, self.rows) = (lets.clone(), rows.clone());
                    let _ = writeln!(self.b, "  br label %{le}\n{lf}:");
                    self.stmts(b, scope, regs, lists, screens)?;
                    (self.lets, self.rows) = (lets, rows);
                    let _ = writeln!(self.b, "  br label %{le}\n{le}:");
                }
                Stmt::Go(screen, arg, _) => {
                    let (route, param) = self.m.screens.iter().find_map(|d| match d {
                        Decl::Screen { name, route, params, .. } if name == screen => Some((route.clone(), params[0].name.clone())),
                        _ => None,
                    }).ok_or("go's screen")?;
                    let (pre, post) = route.split_once(&format!("{{{param}}}")).ok_or("go's route")?;
                    let (gp, gs) = (self.md.str_const_pub(pre.as_bytes()), self.md.str_const_pub(post.as_bytes()));
                    let (id, _) = self.value(arg)?;
                    self.line(&format!("call void @burin_w_go(ptr %w, ptr {gp}, i64 {}, i64 {id}, ptr {gs}, i64 {})", pre.len(), post.len()));
                }
                Stmt::SignOut(span) => {
                    let Some(Decl::Viewer { via_entity, via_field, .. }) = self.m.viewer else { return Err("sign out needs a viewer".into()) };
                    let filter = Expr::Bin(BinOp::Eq, Box::new(Expr::Dot(via_field.clone(), *span)), Box::new(Expr::Name("__session".into(), *span)), *span);
                    let q = Query { source: Source::Entity(via_entity.clone()), filter: Some(filter), order: vec![], limit: None, window_last: None, span: *span };
                    let row = "__s".to_string();
                    self.stmts(&[Stmt::ForEach(row.clone(), q, vec![Stmt::Delete(Expr::Name(row, *span), *span)], *span)], scope, regs, lists, screens)?;
                    self.line("call void @burin_w_sign_out(ptr %w)");
                }
                // `set x.f = v` and `delete x` on one row: a for each over its id
                Stmt::Set(o, _, _, span) | Stmt::Delete(o, span) => {
                    let ent = match self.m.type_of(o, scope).map_err(|x| x.msg)? {
                        Type::Ref(t) => t,
                        _ => return Err("set and delete take a row".into()),
                    };
                    let row = "__row".to_string();
                    let filter = Expr::Bin(BinOp::Eq, Box::new(Expr::Dot("id".into(), *span)), Box::new(o.clone()), *span);
                    let q = Query { source: Source::Entity(ent), filter: Some(filter), order: vec![], limit: None, window_last: None, span: *span };
                    let inner = match s {
                        Stmt::Set(_, f, v, sp) => Stmt::Set(Expr::Name(row.clone(), *sp), f.clone(), v.clone(), *sp),
                        _ => Stmt::Delete(Expr::Name(row.clone(), *span), *span),
                    };
                    self.stmts(&[Stmt::ForEach(row, q, vec![inner], *span)], scope, regs, lists, screens)?;
                }
                Stmt::ForEach(var, q, inner, _) if !inner.iter().all(|s| matches!(s, Stmt::Set(Expr::Name(o, _), ..) if o == var)) && !(inner.len() == 1 && matches!(&inner[0], Stmt::Delete(Expr::Name(o, _), _) if o == var)) => {
                    // row by row: the rows' ids first (in the writer's arena), then the body for each
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };
                    let filter = q.filter.as_ref().map(|f| inline::expr(self.m, f, &inline::Env::new(), 0, 0));
                    let sel = sql::ids(self.m, &ent, filter.as_ref(), &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (ids, p, n) = (self.v(), self.v(), self.v());
                    self.line(&format!("{ids} = call %List @burin_w_ids(ptr %w, ptr {st})"));
                    self.line(&format!("{p} = extractvalue %List {ids}, 0"));
                    self.line(&format!("{n} = extractvalue %List {ids}, 1"));
                    let k = self.n;
                    self.n += 1;
                    let (slot, ix) = (format!("%let_{var}_{k}"), format!("%ix{k}"));
                    self.line(&format!("{slot} = alloca i64"));
                    self.line(&format!("{ix} = alloca i64"));
                    self.line(&format!("store i64 0, ptr {ix}"));
                    let (h, body, done) = (format!("fh{k}"), format!("fb{k}"), format!("fd{k}"));
                    let (i, more, at, id) = (self.v(), self.v(), self.v(), self.v());
                    let _ = writeln!(self.b, "  br label %{h}\n{h}:\n  {i} = load i64, ptr {ix}\n  {more} = icmp slt i64 {i}, {n}\n  br i1 {more}, label %{body}, label %{done}\n{body}:");
                    self.line(&format!("{at} = getelementptr inbounds i64, ptr {p}, i64 {i}"));
                    self.line(&format!("{id} = load i64, ptr {at}"));
                    self.line(&format!("store i64 {id}, ptr {slot}"));
                    let (lets, rows) = (self.lets.clone(), self.rows.clone());
                    self.lets.insert(var.clone(), (slot, Expr::None(Span::default()), Type::Ref(ent.clone())));
                    self.rows.push((var.clone(), ent.clone()));
                    self.stmts(inner, &scope.with(var, Type::Ref(ent.clone())), regs, lists, screens)?;
                    (self.lets, self.rows) = (lets, rows);
                    let (i2, nx) = (self.v(), self.v());
                    let _ = writeln!(self.b, "  {i2} = load i64, ptr {ix}\n  {nx} = add i64 {i2}, 1\n  store i64 {nx}, ptr {ix}\n  br label %{h}\n{done}:");
                }
                Stmt::ForEach(var, q, inner, _) => {
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };
                    let mut sets = vec![];
                    let mut delete = false;
                    for s in inner {
                        match s {
                            Stmt::Set(Expr::Name(o, _), f, v, _) if o == var => {
                                // the row's own fields (`set r.n = r.n + 1`): `.n`, computed by SQL
                                sets.push((f.clone(), of_row(v, var)));
                            }
                            Stmt::Delete(Expr::Name(o, _), _) if o == var && inner.len() == 1 => delete = true,
                            _ => return Err("inside for each: `set x.f = v`s, or one `delete x`".into()),
                        }
                    }
                    for (f, v) in &sets {
                        self.check_max(&ent, f, v)?;
                    }
                    let sets: Vec<(String, Expr)> = sets.iter().map(|(c, v)| Ok((c.clone(), self.unhash(v)?))).collect::<Result<_, String>>()?;
                    let filter = q.filter.as_ref().map(|f| inline::expr(self.m, f, &inline::Env::new(), 0, 0));
                    // the changed rows' columns the delta edges key by: lists' and screens' keys
                    let mut ret: Vec<String> = vec!["id".into()];
                    // a value computed from the row comes back with RETURNING: forwarded as it is
                    // (arithmetic too: after the UPDATE, `p.n - 1` would read the new p.n)
                    let arith = |v: &Expr| matches!(v, Expr::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod, ..)) || matches!(v, Expr::Call(f, _, _) if matches!(f.as_str(), "min" | "max" | "clamp" | "abs"));
                    let computed: Vec<String> = sets.iter().filter(|(_, v)| reads_row(v) || arith(v)).map(|(c, _)| c.clone()).collect();
                    for c in &computed {
                        match self.m.field_type(&ent, c).map(|t| t.strip_opt().clone()) {
                            Some(Type::Text) | Some(Type::Password) => return Err(format!("`{c}`: a text computed from its row isn't lowered yet")),
                            _ => {}
                        }
                        if !ret.contains(c) {
                            ret.push(c.clone());
                        }
                    }
                    let audience_keys: Vec<String> = self.audiences.iter().filter(|a| a.entity == ent).filter_map(|a| a.key_col.clone()).collect();
                    let keys = lists.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone()).chain(screens.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone())).chain(audience_keys);
                    for k in keys {
                        if !ret.contains(&k) {
                            ret.push(k);
                        }
                    }
                    let sel = if delete {
                        sql::delete(self.m, &ent, filter.as_ref().ok_or("delete needs a filter")?, &ret)?
                    } else {
                        sql::update(self.m, &ent, filter.as_ref(), &sets, &ret, &self.rows)?
                    };
                    let st = self.stmt(&sel)?;
                    let (h, body, done) = (format!("uh{}", self.n), format!("ub{}", self.n), format!("ud{}", self.n));
                    self.n += 1;
                    let (rc, ok) = (self.v(), self.v());
                    let _ = writeln!(self.b, "  br label %{h}\n{h}:\n  {rc} = call i32 @sqlite3_step(ptr {st})\n  {ok} = icmp eq i32 {rc}, 100\n  br i1 {ok}, label %{body}, label %{done}\n{body}:");
                    let mut vals = HashMap::new();
                    for (i, c) in ret.iter().enumerate() {
                        let x = self.v();
                        self.line(&format!("{x} = call i64 @burin_col_i64(ptr {st}, i32 {i})"));
                        vals.insert(c.clone(), x);
                    }
                    if delete {
                        self.edges(&ent, &["*".into()], &vals, &[], regs, lists, screens, None, true)?;
                    } else {
                        let cols: Vec<String> = sets.iter().map(|(c, _)| c.clone()).filter(|c| !c.ends_with("__seq")).collect();
                        let real_sets: Vec<(String, Expr)> = sets
                            .iter()
                            .filter(|(c, _)| !c.ends_with("__seq"))
                            .map(|(c, v)| if computed.contains(c) { (c.clone(), Expr::Name(format!("@i64{}", vals[c]), Span::default())) } else { (c.clone(), v.clone()) })
                            .collect();
                        self.edges(&ent, &cols, &vals, &real_sets, regs, lists, screens, None, false)?;
                    }
                    let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call void @burin_done(ptr {st}, i32 {rc})");
                }
            }
        }
        Ok(())
    }
}

/// A set's value with the row's own name made `it` (`r.n + 1` → `.n + 1`).
fn of_row(e: &Expr, var: &str) -> Expr {
    let r = |x: &Expr| Box::new(of_row(x, var));
    match e {
        Expr::Field(o, f, s) if matches!(&**o, Expr::Name(n, _) if n == var) => Expr::Dot(f.clone(), *s),
        Expr::Name(n, s) if n == var => Expr::Name("it".into(), *s),
        Expr::Field(o, f, s) => Expr::Field(r(o), f.clone(), *s),
        Expr::Bin(op, a, b, s) => Expr::Bin(op.clone(), r(a), r(b), *s),
        Expr::Not(a, s) => Expr::Not(r(a), *s),
        Expr::If(c, a, b, s) => Expr::If(r(c), r(a), r(b), *s),
        Expr::Call(f, a, s) => Expr::Call(f.clone(), a.iter().map(|x| of_row(x, var)).collect(), *s),
        _ => e.clone(),
    }
}

/// Whether a value reads the row it's written to (then SQL computes it).
fn reads_row(e: &Expr) -> bool {
    match e {
        Expr::Dot(..) => true,
        Expr::Name(n, _) => n == "it",
        Expr::Field(o, _, _) | Expr::Not(o, _) => reads_row(o),
        Expr::Bin(_, a, b, _) => reads_row(a) || reads_row(b),
        Expr::If(c, a, b, _) => reads_row(c) || reads_row(a) || reads_row(b),
        Expr::Call(_, a, _) => a.iter().any(reads_row),
        _ => false,
    }
}

fn creates_of(body: &[Stmt], out: &mut Vec<String>) {
    for s in body {
        match s {
            Stmt::Create(e, _, _, _) => out.push(e.clone()),
            Stmt::ForEach(_, _, inner, _) => creates_of(inner, out),
            Stmt::If(_, a, b, _) => {
                creates_of(a, out);
                creates_of(b, out);
            }
            _ => {}
        }
    }
}

/// A patch function: sets one i64 field of the item with this id in one list of a kept record.
fn patch_fn(reg: &RegInfo, (_, slot, field): Patch, store: bool) -> String {
    let name = &reg.name;
    let list_ty = format!("%In_{name}");
    let item_ty = format!("%In_{name}_{slot}");
    let id_field = reg.schema.lists[slot].items.fields.iter().position(|(p, _)| *p == reg.schema.lists[slot].var).unwrap_or(0);
    let lslot = reg.schema.fields.len() + slot;
    // a patch finds the item by its id and stores the value; a find looks for an item whose
    // reference field holds the row
    let (kind, match_field, found) = if store {
        ("patch", id_field, format!("  %fp = getelementptr inbounds {item_ty}, ptr %it, i32 0, i32 {field}
  store i64 %value, ptr %fp"))
    } else {
        ("find", field, String::new())
    };
    format!(
        "define i1 @burin_{kind}_{name}_{slot}_{field}(ptr %rec, i64 %item, i64 %value) {{
entry:
  %lp = getelementptr inbounds {list_ty}, ptr %rec, i32 0, i32 {lslot}
  %l = load %List, ptr %lp
  %items = extractvalue %List %l, 0
  %n = extractvalue %List %l, 1
  br label %head
head:
  %i = phi i64 [ 0, %entry ], [ %next, %step ]
  %more = icmp slt i64 %i, %n
  br i1 %more, label %body, label %none
body:
  %it = getelementptr inbounds {item_ty}, ptr %items, i64 %i
  %idp = getelementptr inbounds {item_ty}, ptr %it, i32 0, i32 {match_field}
  %id = load i64, ptr %idp
  %hit = icmp eq i64 %id, %item
  br i1 %hit, label %found, label %step
step:
  %next = add i64 %i, 1
  br label %head
found:
{found}
  ret i1 true
none:
  ret i1 false
}}
"
    )
}

/// Every action's function, its patch functions; the info the runtime needs to call them.
pub fn lower(m: &Model, md: &mut Module, qid: &mut usize, screens: &[ScreenList]) -> Result<(Vec<ActInfo>, Vec<Patch>, Vec<Patch>), String> {
    let regs = md.registers.clone();
    let lists = list_reads(m, &regs)?;
    let mut infos = vec![];
    let mut all_patches: Vec<Patch> = vec![];
    let mut all_finds: Vec<Patch> = vec![];
    let auds = audiences(m);
    for d in &m.actions {
        if let Decl::Action { name, params, body, public, .. } = d {
            let mut args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text | Type::List(_)) { Shape::Str } else { Shape::I64 })).collect();
            let list_args: Vec<String> = params.iter().filter(|p| matches!(p.ty.strip_opt(), Type::List(_))).map(|p| p.name.clone()).collect();
            // hidden arguments the runtime fills (never from the form): the request's session
            // token (sign out), each verified password hash
            fn hidden(body: &[Stmt], out: &mut Vec<String>, n: &mut usize) {
                for s in body {
                    match s {
                        Stmt::SignOut(_) if !out.iter().any(|x| x == "__session") => out.push("__session".into()),
                        Stmt::Require(r) => {
                            let mut v = vec![];
                            verified(&r.cond, &mut v);
                            for _ in v {
                                out.push(format!("__verified_{n}"));
                                *n += 1;
                            }
                        }
                        Stmt::ForEach(_, _, inner, _) => hidden(inner, out, n),
                        Stmt::If(_, a, b, _) => {
                            hidden(a, out, n);
                            hidden(b, out, n);
                        }
                        _ => {}
                    }
                }
            }
            let mut hid = vec![];
            hidden(body, &mut hid, &mut 0);
            // each distinct hash(argument), in order of appearance
            fn hashed(body: &[Stmt], out: &mut Vec<String>) {
                fn ex(e: &Expr, out: &mut Vec<String>) {
                    match e {
                        Expr::Call(f, a, _) if f == "hash" => {
                            if let Some(Expr::Name(n, _)) = a.first() {
                                if !out.contains(n) {
                                    out.push(n.clone());
                                }
                            }
                        }
                        Expr::Call(_, a, _) => a.iter().for_each(|x| ex(x, out)),
                        Expr::Bin(_, l, r, _) => {
                            ex(l, out);
                            ex(r, out);
                        }
                        _ => {}
                    }
                }
                for s in body {
                    match s {
                        Stmt::Set(_, _, v, _) => ex(v, out),
                        Stmt::Create(_, fs, _, _) => fs.iter().for_each(|(_, v)| ex(v, out)),
                        Stmt::ForEach(_, _, inner, _) => hashed(inner, out),
                        Stmt::If(_, a, b, _) => {
                            hashed(a, out);
                            hashed(b, out);
                        }
                        _ => {}
                    }
                }
            }
            let mut hs = vec![];
            hashed(body, &mut hs);
            hid.extend((0..hs.len()).map(|i| format!("__hashed_{i}")));
            args.extend(hid.into_iter().map(|h| (h, Shape::Str)));
            let mut creates = vec![];
            creates_of(body, &mut creates);
            let rows: Vec<(String, String)> = params.iter().filter_map(|p| if let Type::Ref(e) = p.ty.strip_opt() { Some((p.name.clone(), e.clone())) } else { None }).chain(match m.viewer {
                Some(Decl::Viewer { entity, .. }) => Some(("viewer".to_string(), entity.clone())),
                _ => None,
            }).collect();
            let mut a = Act { m, md: &mut *md, b: String::new(), n: 0, qid: &mut *qid, args: args.clone(), rows, outcomes: vec!["ok".into()], fails: vec![], creates, patches: vec![], finds: vec![], edges: vec![], audiences: auds.clone(), lets: HashMap::new(), verifies: vec![], allocas: String::new(), touched: vec![], hashes: vec![], scope: Scope::of(params) };
            a.stmts(body, &Scope::of(params), &regs, &lists, screens)?;
            let mut f = String::new();
            let parts: Vec<&str> = args.iter().map(|(_, s)| if *s == Shape::Str { "%Str" } else { "i64" }).collect();
            let _ = writeln!(f, "%Args_{name} = type {{ {} }}", parts.join(", "));
            let _ = writeln!(f, "define i32 @burin_act_{name}(ptr noalias %ctx, ptr noalias %db, ptr noalias %w, i64 %viewer, ptr noalias %vrec, ptr noalias %args, ptr noalias %d) {{\nentry:");
            let _ = writeln!(f, "  %now = call i64 @burin_w_now(ptr %w)");
            f.push_str(&a.allocas);
            f.push_str(&a.b.replace("%Args,", &format!("%Args_{name},")));
            let _ = writeln!(f, "  ret i32 0");
            for (l, code) in &a.fails {
                let _ = writeln!(f, "{l}:\n  ret i32 {code}");
            }
            let _ = writeln!(f, "}}\n");
            let (outcomes, edges, patches, verifies, hashes) = (a.outcomes.clone(), a.edges.clone(), a.patches.clone(), a.verifies.clone(), a.hashes.clone());
            for f in a.finds.clone() {
                if !all_finds.contains(&f) {
                    all_finds.push(f);
                }
            }
            md.funcs.push_str(&f);
            for p in patches {
                if !all_patches.contains(&p) {
                    all_patches.push(p);
                }
            }
            infos.push(ActInfo { name: name.clone(), args, outcomes, edges, public: *public, verifies, list_args, hashes });
        }
    }
    for p in &all_patches {
        md.funcs.push_str(&patch_fn(&regs[p.0], *p, true));
    }
    for p in &all_finds {
        md.funcs.push_str(&patch_fn(&regs[p.0], *p, false));
    }
    Ok((infos, all_patches, all_finds))
}
