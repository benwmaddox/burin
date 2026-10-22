//! Actions as fixed rules (DESIGN.md principle 3), lowered to LLVM IR, with their delta edges.
//!
//! `etch_act_<name>(ctx, db, w, viewer, vrec, args, deltas) -> i32` runs inside the writer's
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
}

pub struct ActInfo {
    pub name: String,
    pub args: Vec<(String, Shape)>,
    pub outcomes: Vec<String>,
    /// how each delta edge is met: forwarded, patched, or read (for the report)
    pub edges: Vec<String>,
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
            let (q, ent) = sql::resolve(m, &slot.expr, &Scope::default())?;
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
                    let (q, ent) = sql::resolve(m, list, &Scope::of(params))?;
                    let mut eqs = vec![];
                    if let Some(f) = &q.filter {
                        eq_conjuncts(f, &mut eqs);
                    }
                    let keyed_by = eqs.iter().find(|(_, x)| matches!(x, Expr::Name(n, _) if params.iter().any(|p| &p.name == n))).map(|(f, _)| f.clone());
                    let monotone = matches!(q.order.first(), Some((Expr::Dot(f, _), false)) if f == "created") && q.window_last.is_some();
                    let newest_first = matches!(q.order.first(), Some((Expr::Dot(f, _), true)) if f == "created") && q.limit.is_some() && q.window_last.is_none();
                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone, newest_first });
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
}

impl<'a, 'b> Act<'a, 'b> {
    fn v(&mut self) -> String {
        self.n += 1;
        format!("%a{}", self.n)
    }
    fn line(&mut self, s: &str) {
        let _ = writeln!(self.b, "  {s}");
    }
    fn note(&mut self, s: String) {
        if !self.edges.contains(&s) {
            self.edges.push(s);
        }
    }

    /// An argument's, the viewer's or the time's value: i64, or a pointer to a %Str.
    fn value(&mut self, e: &Expr) -> Result<(String, Shape), String> {
        match e {
            Expr::Name(n, _) if n == "viewer" => Ok(("%viewer".into(), Shape::I64)),
            Expr::Name(n, _) if n == "now" => Ok(("%now".into(), Shape::I64)),
            Expr::Name(n, _) if n == "@seq" => {
                let x = self.v();
                self.line(&format!("{x} = call i64 @etch_w_seq_now(ptr %w)"));
                Ok((x, Shape::I64))
            }
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
            Expr::Call(f, a, _) if f == "token" => {
                let n = match a.first() {
                    Some(Expr::Int(n, _)) => *n,
                    _ => return Err("token takes a constant length".into()),
                };
                let p = self.v();
                self.line(&format!("{p} = alloca %Str\n  call void @etch_w_token(ptr %w, i64 {n}, ptr {p})"));
                Ok((p, Shape::Str))
            }
            _ => Err(format!("an action can't compute `{}` yet", show(e))),
        }
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
        self.line(&format!("{c} = call i64 @etch_str_chars(ptr {v})"));
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
            self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 {i}, i64 {v})"));
        } else {
            self.line(&format!("call void @etch_bind_str(ptr {st}, i32 {i}, ptr {v})"));
        }
        Ok(())
    }

    fn stmt_text(&mut self, sql_text: &str) -> String {
        let g = self.md.str_const_pub(sql_text.as_bytes());
        *self.qid += 1;
        let st = self.v();
        let _ = writeln!(self.b, "  ; {sql_text}");
        self.line(&format!("{st} = call ptr @etch_stmt(ptr %db, i64 {}, ptr {g}, i64 {})", self.qid, sql_text.len()));
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
        self.line(&format!("call void @etch_delta(ptr %d, i32 {op}, i64 {a}, i64 {key}, i64 {item})"));
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
        self.line(&format!("{size} = getelementptr {ty}, ptr null, i64 1\n  {sz} = ptrtoint ptr {size} to i64\n  {rec} = call ptr @etch_w_alloc(ptr %w, i64 {sz})"));
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
                        self.line(&format!("call void @etch_w_copy(ptr %w, ptr {v}, ptr {p})"));
                    }
                }
                Src::Viewer(key) => {
                    // the viewer's kept record has it (added to the viewer's record here)
                    let vi = self.md.viewer_field(&key, shape);
                    let (q, x) = (self.v(), self.v());
                    self.line(&format!("{q} = getelementptr inbounds %Viewer, ptr %vrec, i32 0, i32 {vi}"));
                    if shape == Shape::Str {
                        self.line(&format!("call void @etch_w_copy(ptr %w, ptr {q}, ptr {p})"));
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
        // rows enter or leave lists
        let rows_change = created || removed;
        let verb = if created { "create" } else if removed { "delete" } else { "set" };
        // registers keyed by a row of this entity
        for (ri, r) in regs.iter().enumerate() {
            if !r.root.as_ref().map(|(_, e)| e == ent).unwrap_or(false) {
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
                self.note(format!("set {ent}.{} → {}: READ (rebuilt)", cols.join(","), r.name));
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
                let patchable = !rows_change
                    && key.is_some()
                    && cols.iter().all(|c| !reads_query(c))
                    && cols.iter().filter(|c| reads_item(c)).all(|c| l.items.fields.iter().any(|(p, s)| p.split('.').skip(1).collect::<Vec<_>>() == [c.as_str()] && *s == Shape::I64));
                match (key, patchable) {
                    (Some(k), true) => {
                        let id = vals.get("id").cloned().ok_or("the written row's id")?;
                        for (c, e) in sets {
                            if !reads_item(c) {
                                continue;
                            }
                            let fi = l.items.fields.iter().position(|(p, _)| p.split('.').skip(1).collect::<Vec<_>>() == [c.as_str()]).unwrap();
                            let (v, _) = self.value(e)?;
                            let slot = l.slot.unwrap();
                            self.line(&format!("call void @etch_delta_patch(ptr %d, i64 {}, i64 {k}, i64 {id}, i64 {slot}, i64 {fi}, i64 {v})", l.reg));
                            if !self.patches.contains(&(l.reg, slot, fi)) {
                                self.patches.push((l.reg, slot, fi));
                            }
                        }
                        self.note(format!("set {ent}.{} → {reg_name}: patched (the kept record's item, then its bytes)", cols.join(",")));
                    }
                    (Some(k), false) => {
                        self.delta(3, l.reg, &k, "0");
                        self.note(format!("{verb} {ent} → {reg_name}: READ (its list changes order or rows)"));
                    }
                    (None, _) => {
                        self.delta(4, l.reg, "0", "0");
                        self.note(format!("{verb} {ent} → {reg_name}: READ (every kept one rebuilt)"));
                    }
                }
            } else if let Some((slot, pi)) = via.as_ref().and_then(|p0| {
                // the written column only orders the list (no filter reads it, no item shows it):
                // rows can't enter or leave it; only kept lists holding the row reorder. They are
                // found in the kept records (the item's reference), not by a query
                let only_order = cols.iter().all(|c| {
                    let path = [p0.clone(), c.clone()];
                    !l.filter_paths.iter().any(|p| *p == path) && !l.item_paths.iter().any(|p| *p == path)
                });
                let pi = l.items.fields.iter().position(|(p, s)| p.split('.').skip(1).collect::<Vec<_>>() == [p0.as_str()] && *s == Shape::I64)?;
                if only_order { l.slot.map(|s| (s, pi)) } else { None }
            }) {
                let id = vals.get("id").cloned().ok_or("the written row's id")?;
                self.line(&format!("call void @etch_delta_find(ptr %d, i64 {}, i64 {id}, i64 {slot}, i64 {pi})", l.reg));
                if !self.finds.contains(&(l.reg, slot, pi)) {
                    self.finds.push((l.reg, slot, pi));
                }
                self.note(format!("set {ent}.{} → {reg_name} (it orders the list): only kept lists holding the row (found in the kept records), READ for those", cols.join(",")));
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
                self.line(&format!("{k} = call i64 @etch_col_i64(ptr {st}, i32 0)"));
                self.delta(3, l.reg, &k, "0");
                let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call i32 @sqlite3_reset(ptr {st})");
                self.note(format!("set {ent}.{} → {reg_name} (through {}.{p0}): READ (each reaching key rebuilt)", cols.join(","), l.entity));
            } else {
                self.delta(4, l.reg, "0", "0");
                self.note(format!("set {ent}.{} → {reg_name}: READ (every kept one rebuilt)", cols.join(",")));
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
                    let sel = sql::cond_select_with(self.m, &cond, &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (x, ok) = (self.v(), self.v());
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    self.line(&format!("{x} = call i64 @etch_col_i64(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("{ok} = icmp eq i64 {x}, 1"));
                    let (pass, fail) = (format!("pass{}", self.n), format!("fail{}", self.n));
                    self.n += 1;
                    let _ = writeln!(self.b, "  br i1 {ok}, label %{pass}, label %{fail}\n{pass}:");
                    self.fails.push((fail, code));
                }
                Stmt::Create(ent, fields, _) => {
                    for (f, v) in fields {
                        self.check_max(ent, f, v)?;
                    }
                    let gid = self.md.str_const_pub(ent.as_bytes());
                    let (id, seq) = (self.v(), self.v());
                    self.line(&format!("{id} = call i64 @etch_w_id(ptr %w, ptr %db, ptr {gid}, i64 {})", ent.len()));
                    self.line(&format!("{seq} = call i64 @etch_w_seq(ptr %w)"));
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
                            fixed.push((f.clone(), e.clone()));
                        } else {
                            self.line(&format!("call void @etch_bind_str(ptr {st}, i32 {}, ptr {v})", i + 5));
                            // the made value, by its pointer, for the forwarded record
                            fixed.push((f.clone(), Expr::Name(format!("@ptr{v}"), Span::default())));
                        }
                    }
                    self.line(&format!("call void @etch_exec(ptr {st})"));
                    self.edges(ent, &["*".into()], &vals, &[], regs, lists, screens, Some((&id, &seq, &fixed)), false)?;
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
                    self.line(&format!("call void @etch_exec(ptr {st})"));
                    let mut vals = HashMap::new();
                    vals.insert("id".to_string(), id);
                    self.edges(&ent, &["updated".into()], &vals, &[("updated".into(), Expr::Name("now".into(), Span::default()))], regs, lists, screens, None, false)?;
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
                                sets.push((f.clone(), v.clone()));
                            }
                            Stmt::Delete(Expr::Name(o, _), _) if o == var && inner.len() == 1 => delete = true,
                            _ => return Err("inside for each: `set x.f = v`s, or one `delete x`".into()),
                        }
                    }
                    for (f, v) in &sets {
                        self.check_max(&ent, f, v)?;
                    }
                    let filter = q.filter.as_ref().map(|f| inline::expr(self.m, f, &inline::Env::new(), 0, 0));
                    // the changed rows' columns the delta edges key by: lists' and screens' keys
                    let mut ret: Vec<String> = vec!["id".into()];
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
                        sql::update(self.m, &ent, filter.as_ref(), &sets, &ret)?
                    };
                    let st = self.stmt(&sel)?;
                    let (h, body, done) = (format!("uh{}", self.n), format!("ub{}", self.n), format!("ud{}", self.n));
                    self.n += 1;
                    let (rc, ok) = (self.v(), self.v());
                    let _ = writeln!(self.b, "  br label %{h}\n{h}:\n  {rc} = call i32 @sqlite3_step(ptr {st})\n  {ok} = icmp eq i32 {rc}, 100\n  br i1 {ok}, label %{body}, label %{done}\n{body}:");
                    let mut vals = HashMap::new();
                    for (i, c) in ret.iter().enumerate() {
                        let x = self.v();
                        self.line(&format!("{x} = call i64 @etch_col_i64(ptr {st}, i32 {i})"));
                        vals.insert(c.clone(), x);
                    }
                    if delete {
                        self.edges(&ent, &["*".into()], &vals, &[], regs, lists, screens, None, true)?;
                    } else {
                        let cols: Vec<String> = sets.iter().map(|(c, _)| c.clone()).filter(|c| !c.ends_with("__seq")).collect();
                        let real_sets: Vec<(String, Expr)> = sets.iter().filter(|(c, _)| !c.ends_with("__seq")).cloned().collect();
                        self.edges(&ent, &cols, &vals, &real_sets, regs, lists, screens, None, false)?;
                    }
                    let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call void @etch_done(ptr {st}, i32 {rc})");
                }
            }
        }
        Ok(())
    }
}

fn creates_of(body: &[Stmt], out: &mut Vec<String>) {
    for s in body {
        match s {
            Stmt::Create(e, _, _) => out.push(e.clone()),
            Stmt::ForEach(_, _, inner, _) => creates_of(inner, out),
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
        "define i1 @etch_{kind}_{name}_{slot}_{field}(ptr %rec, i64 %item, i64 %value) {{
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
        if let Decl::Action { name, params, body, .. } = d {
            let args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text) { Shape::Str } else { Shape::I64 })).collect();
            let mut creates = vec![];
            creates_of(body, &mut creates);
            let rows: Vec<(String, String)> = params.iter().filter_map(|p| if let Type::Ref(e) = p.ty.strip_opt() { Some((p.name.clone(), e.clone())) } else { None }).chain(match m.viewer {
                Some(Decl::Viewer { entity, .. }) => Some(("viewer".to_string(), entity.clone())),
                _ => None,
            }).collect();
            let mut a = Act { m, md: &mut *md, b: String::new(), n: 0, qid: &mut *qid, args: args.clone(), rows, outcomes: vec!["ok".into()], fails: vec![], creates, patches: vec![], finds: vec![], edges: vec![], audiences: auds.clone() };
            a.stmts(body, &Scope::of(params), &regs, &lists, screens)?;
            let mut f = String::new();
            let parts: Vec<&str> = args.iter().map(|(_, s)| if *s == Shape::Str { "%Str" } else { "i64" }).collect();
            let _ = writeln!(f, "%Args_{name} = type {{ {} }}", parts.join(", "));
            let _ = writeln!(f, "define i32 @etch_act_{name}(ptr noalias %ctx, ptr noalias %db, ptr noalias %w, i64 %viewer, ptr noalias %vrec, ptr noalias %args, ptr noalias %d) {{\nentry:");
            let _ = writeln!(f, "  %now = call i64 @etch_w_now(ptr %w)");
            f.push_str(&a.b.replace("%Args,", &format!("%Args_{name},")));
            let _ = writeln!(f, "  ret i32 0");
            for (l, code) in &a.fails {
                let _ = writeln!(f, "{l}:\n  ret i32 {code}");
            }
            let _ = writeln!(f, "}}\n");
            let (outcomes, edges, patches) = (a.outcomes.clone(), a.edges.clone(), a.patches.clone());
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
            infos.push(ActInfo { name: name.clone(), args, outcomes, edges });
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
