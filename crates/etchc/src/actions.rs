//! Actions as fixed rules (DESIGN.md principle 3), lowered to LLVM IR, with their delta edges.
//!
//! `etch_act_<name>(ctx, db, w, viewer, args, deltas) -> i32` runs inside the writer's
//! transaction and returns its outcome (0: ok; otherwise an index into the outcomes):
//! - `require c else o`: the condition as one SQL expression (`SELECT …`), the viewer and the
//!   arguments bound;
//! - `create E {…}`: an INSERT; the id, seq and time come from the writer (`w`);
//! - `touch r`: an UPDATE of `updated`;
//! - `for each x in E where … { set x.f = v }`: one set-based UPDATE … RETURNING.
//!
//! Its delta edges are derived here, per write and register, and emitted as calls that tell the
//! writer what to rebuild after the commit (op codes in etch-rt's `abi::Delta`):
//! 1 build register `a` for a new row; 2 append `item` to screen list `a` at `key`; 3 rebuild
//! register `a` at `key`; 4 rebuild every kept instance of register `a`; 5 rebuild list `a` at `key`.

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
    /// the column that equals the screen's parameter (`.room == room`): the list's key
    pub keyed_by: Option<String>,
    /// ordered by creation, oldest first, a window: a new row is appended, the oldest dropped
    pub monotone: bool,
}

pub struct ActInfo {
    pub name: String,
    pub args: Vec<(String, Shape)>,
    pub outcomes: Vec<String>,
}

/// What a register's list reads: its entity, the column keyed to the register's key, the paths
/// it reads from its rows (fields, filter and order), and whether it is nested in another list.
struct ListRead {
    reg: usize,
    entity: String,
    keyed_by: Option<String>,
    paths: Vec<Vec<String>>,
    nested: bool,
}

/// `.f == x` conjuncts of a filter: (f, x).
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

/// The paths from a query's row that an expression reads (`.room.updated` → [room, updated]).
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
        for slot in &s.lists {
            let (q, ent) = sql::resolve(m, &slot.expr, &Scope::default())?;
            let root = if reg.per_viewer { "viewer".to_string() } else { reg.root.as_ref().map(|r| r.0.clone()).unwrap_or_default() };
            let mut eqs = vec![];
            if let Some(f) = &q.filter {
                eq_conjuncts(f, &mut eqs);
            }
            let keyed_by = if nested { None } else { eqs.iter().find(|(_, x)| matches!(x, Expr::Name(n, _) if *n == root)).map(|(f, _)| f.clone()) };
            let mut paths: Vec<Vec<String>> = slot.items.fields.iter().map(|(p, _)| p.split('.').skip(1).map(String::from).collect()).collect();
            if let Some(f) = &q.filter {
                row_paths(f, &mut paths);
            }
            for (k, _) in &q.order {
                row_paths(k, &mut paths);
            }
            out.push(ListRead { reg: ri, entity: ent, keyed_by, paths, nested });
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
                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone });
                }
            }
        }
    }
    Ok(out)
}

struct Act<'a, 'b> {
    m: &'a Model<'a>,
    md: &'b mut Module,
    b: String,
    n: usize,
    qid: &'b mut usize,
    args: Vec<(String, Shape)>,
    outcomes: Vec<String>,
    fails: Vec<(String, usize)>,
    crudspec: bool,
}

impl<'a, 'b> Act<'a, 'b> {
    fn v(&mut self) -> String {
        self.n += 1;
        format!("%a{}", self.n)
    }
    fn line(&mut self, s: &str) {
        let _ = writeln!(self.b, "  {s}");
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
                let g = self.md_str(s.as_bytes());
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

    fn md_str(&mut self, b: &[u8]) -> String {
        self.md.str_const_pub(b)
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

    /// A prepared statement for this SQL, with its parameters bound.
    fn stmt(&mut self, sel: &sql::Select) -> Result<String, String> {
        let g = self.md_str(sel.sql.as_bytes());
        *self.qid += 1;
        let st = self.v();
        let _ = writeln!(self.b, "  ; {}", sel.sql);
        self.line(&format!("{st} = call ptr @etch_stmt(ptr %db, i64 {}, ptr {g}, i64 {})", self.qid, sel.sql.len()));
        for (i, sql::Bind::Path(e)) in sel.binds.iter().enumerate() {
            self.bind(&st, i + 1, e)?;
        }
        Ok(st)
    }

    fn delta(&mut self, op: i32, a: usize, key: &str, item: &str) {
        self.line(&format!("call void @etch_delta(ptr %d, i32 {op}, i64 {a}, i64 {key}, i64 {item})"));
    }

    /// For each row a statement returns (or the one row a write names), the delta edges of a
    /// write to `ent`'s `cols`: `cols` "*" for a new row. `vals` are the columns returned, by name.
    #[allow(clippy::too_many_arguments)]
    fn edges(&mut self, ent: &str, cols: &[String], vals: &HashMap<String, String>, regs: &[RegInfo], lists: &[ListRead], screens: &[ScreenList], new_row: Option<&str>) -> Result<(), String> {
        let created = cols.iter().any(|c| c == "*");
        let touches = |p: &[String]| -> bool { created || p.last().map(|c| cols.contains(c)).unwrap_or(false) };
        // registers keyed by a row of this entity
        for (ri, r) in regs.iter().enumerate() {
            if r.root.as_ref().map(|(_, e)| e == ent).unwrap_or(false) {
                if let Some(id) = new_row {
                    self.delta(1, ri, id, "0");
                } else if r.schema.fields.iter().any(|(p, _)| { let ps: Vec<String> = p.split('.').skip(1).map(String::from).collect(); ps.len() == 1 && cols.contains(&ps[0]) }) {
                    let id = vals.get("id").cloned().unwrap_or_default();
                    self.delta(3, ri, &id, "0");
                }
            }
        }
        // registers' lists over this entity, or reaching it through a reference
        for l in lists {
            let reg = &regs[l.reg];
            for p in &l.paths {
                let direct = l.entity == ent && (p.len() == 1 || created) && touches(p);
                let via = p.len() == 2 && cols.contains(&p[1]) && !created && self.m.field_type(&l.entity, &p[0]).map(|t| matches!(t.strip_opt(), Type::Ref(e) if e == ent)).unwrap_or(false);
                if l.nested && (direct || via) {
                    self.delta(4, l.reg, "0", "0");
                } else if direct {
                    match (&l.keyed_by, vals.get(l.keyed_by.as_deref().unwrap_or(""))) {
                        (Some(_), Some(k)) => {
                            let k = k.clone();
                            self.delta(3, l.reg, &k, "0")
                        }
                        _ => self.delta(4, l.reg, "0", "0"),
                    }
                } else if via {
                    match &l.keyed_by {
                        Some(f) => {
                            // the keys of the list's rows that reach the written row
                            let id = vals.get("id").cloned().ok_or("the written row's id")?;
                            let s = sql::reach(&l.entity, f, &p[0]);
                            let g = self.md_str(s.as_bytes());
                            *self.qid += 1;
                            let st = self.v();
                            let _ = writeln!(self.b, "  ; {s}");
                            self.line(&format!("{st} = call ptr @etch_stmt(ptr %db, i64 {}, ptr {g}, i64 {})", self.qid, s.len()));
                            self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 1, i64 {id})"));
                            let (h, body, done) = (format!("rh{}", self.n), format!("rb{}", self.n), format!("rd{}", self.n));
                            self.n += 1;
                            let (rc, ok, k) = (self.v(), self.v(), self.v());
                            let _ = writeln!(self.b, "  br label %{h}\n{h}:\n  {rc} = call i32 @sqlite3_step(ptr {st})\n  {ok} = icmp eq i32 {rc}, 100\n  br i1 {ok}, label %{body}, label %{done}\n{body}:");
                            self.line(&format!("{k} = call i64 @etch_col_i64(ptr {st}, i32 0)"));
                            self.delta(3, l.reg, &k, "0");
                            let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call i32 @sqlite3_reset(ptr {st})");
                        }
                        None => self.delta(4, l.reg, "0", "0"),
                    }
                } else {
                    continue;
                }
                let _ = reg;
                break; // one edge per list
            }
        }
        // the screens' lists of this entity
        for (si, s) in screens.iter().enumerate() {
            if s.entity != ent {
                continue;
            }
            if let (Some(id), Some(f)) = (new_row, &s.keyed_by) {
                if let Some(k) = vals.get(f).cloned() {
                    self.delta(if s.monotone { 2 } else { 5 }, si, &k, id);
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
                        Else::Outcome(o) => {
                            if !self.outcomes.contains(o) {
                                self.outcomes.push(o.clone());
                            }
                            self.outcomes.iter().position(|x| x == o).unwrap()
                        }
                        _ => return Err("an action's require ends in an outcome (`else not_found`)".into()),
                    };
                    let cond = inline::expr(self.m, &r.cond, &inline::Env::new(), 0, 0);
                    let sel = sql::cond_select(self.m, &cond)?;
                    let st = self.stmt(&sel)?;
                    let (x, ok) = (self.v(), self.v());
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    self.line(&format!("{x} = call i64 @etch_col_i64(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("{ok} = icmp eq i64 {x}, 1"));
                    let pass = format!("pass{}", self.n);
                    let fail = format!("fail{}", self.n);
                    self.n += 1;
                    let _ = writeln!(self.b, "  br i1 {ok}, label %{pass}, label %{fail}\n{pass}:");
                    self.fails.push((fail, code));
                }
                Stmt::Create(ent, fields, _) => {
                    let gid = self.md_str(ent.as_bytes());
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
                    let g = self.md_str(sql_text.as_bytes());
                    *self.qid += 1;
                    let st = self.v();
                    let _ = writeln!(self.b, "  ; {sql_text}");
                    self.line(&format!("{st} = call ptr @etch_stmt(ptr %db, i64 {}, ptr {g}, i64 {})", self.qid, sql_text.len()));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 1, i64 {id})"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 2, i64 {seq})"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 3, i64 %now)"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 4, i64 %now)"));
                    let mut vals = HashMap::new();
                    vals.insert("id".to_string(), id.clone());
                    for (i, (f, e)) in fields.iter().enumerate() {
                        let (v, shape) = self.value(e)?;
                        if shape == Shape::I64 {
                            self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 {}, i64 {v})", i + 5));
                            vals.insert(f.clone(), v);
                        } else {
                            self.line(&format!("call void @etch_bind_str(ptr {st}, i32 {}, ptr {v})", i + 5));
                        }
                    }
                    self.line(&format!("call void @etch_exec(ptr {st})"));
                    self.edges(ent, &["*".into()], &vals, regs, lists, screens, Some(&id))?;
                }
                Stmt::Touch(e, _) => {
                    let ent = match self.m.type_of(e, scope).map_err(|x| x.msg)? {
                        Type::Ref(t) => t,
                        _ => return Err("touch takes a row".into()),
                    };
                    let (id, _) = self.value(e)?;
                    let sql_text = format!("UPDATE \"{ent}\" SET \"updated\" = ? WHERE id = ?");
                    let g = self.md_str(sql_text.as_bytes());
                    *self.qid += 1;
                    let st = self.v();
                    let _ = writeln!(self.b, "  ; {sql_text}");
                    self.line(&format!("{st} = call ptr @etch_stmt(ptr %db, i64 {}, ptr {g}, i64 {})", self.qid, sql_text.len()));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 1, i64 %now)"));
                    self.line(&format!("call i32 @sqlite3_bind_int64(ptr {st}, i32 2, i64 {id})"));
                    self.line(&format!("call void @etch_exec(ptr {st})"));
                    let mut vals = HashMap::new();
                    vals.insert("id".to_string(), id);
                    self.edges(&ent, &["updated".into()], &vals, regs, lists, screens, None)?;
                }
                Stmt::ForEach(var, q, inner, _) => {
                    // only `set var.f = v` inside: one set-based UPDATE over the query's rows
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };
                    let mut sets = vec![];
                    for s in inner {
                        match s {
                            Stmt::Set(Expr::Name(o, _), f, v, _) if o == var => {
                                sets.push((f.clone(), v.clone()));
                                let is_time = matches!(self.m.field_type(&ent, f).as_ref().map(|t| t.strip_opt().clone()), Some(Type::Time));
                                if self.crudspec && is_time {
                                    // crudspec's storage: a time's order column beside it
                                    sets.push((format!("{f}__seq"), Expr::Name("@seq".into(), Span::default())));
                                }
                            }
                            _ => return Err("only `set x.f = v` is lowered inside for each".into()),
                        }
                    }
                    let filter = q.filter.as_ref().map(|f| inline::expr(self.m, f, &inline::Env::new(), 0, 0));
                    // the columns the delta edges need back: the row's id and every list key column
                    let mut ret: Vec<String> = vec!["id".into()];
                    for l in lists.iter().filter(|l| l.entity == ent) {
                        if let Some(k) = &l.keyed_by {
                            if !ret.contains(k) {
                                ret.push(k.clone());
                            }
                        }
                    }
                    let sel = sql::update(self.m, &ent, filter.as_ref(), &sets, &ret)?;
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
                    let cols: Vec<String> = sets.iter().map(|(c, _)| c.clone()).collect();
                    self.edges(&ent, &cols, &vals, regs, lists, screens, None)?;
                    let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call void @etch_done(ptr {st}, i32 {rc})");
                }
                Stmt::Set(..) => return Err("set is lowered inside for each only (for now)".into()),
            }
        }
        Ok(())
    }
}

/// Every action's function; the info the runtime needs to call them.
pub fn lower(m: &Model, md: &mut Module, qid: &mut usize, screens: &[ScreenList]) -> Result<Vec<ActInfo>, String> {
    let regs = md.registers.clone();
    let lists = list_reads(m, &regs)?;
    let crudspec = matches!(m.consts.get("storage"), Some(Decl::Const { value: Expr::Str(s, _), .. }) if s == "crudspec");
    let mut infos = vec![];
    for d in &m.actions {
        if let Decl::Action { name, params, body, .. } = d {
            let args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text) { Shape::Str } else { Shape::I64 })).collect();
            let mut a = Act { m, md: &mut *md, b: String::new(), n: 0, qid: &mut *qid, args: args.clone(), outcomes: vec!["ok".into()], fails: vec![], crudspec };
            a.stmts(body, &Scope::of(params), &regs, &lists, screens)?;
            let mut f = String::new();
            let parts: Vec<&str> = args.iter().map(|(_, s)| if *s == Shape::Str { "%Str" } else { "i64" }).collect();
            let _ = writeln!(f, "%Args_{name} = type {{ {} }}", parts.join(", "));
            let _ = writeln!(f, "define i32 @etch_act_{name}(ptr noalias %ctx, ptr noalias %db, ptr noalias %w, i64 %viewer, ptr noalias %args, ptr noalias %d) {{\nentry:");
            let _ = writeln!(f, "  %now = call i64 @etch_w_now(ptr %w)");
            let body_ir = a.b.replace("%Args,", &format!("%Args_{name},"));
            f.push_str(&body_ir);
            let _ = writeln!(f, "  ret i32 0");
            for (l, code) in &a.fails {
                let _ = writeln!(f, "{l}:\n  ret i32 {code}");
            }
            let _ = writeln!(f, "}}\n");
            let outcomes = a.outcomes.clone();
            md.funcs.push_str(&f);
            infos.push(ActInfo { name: name.clone(), args, outcomes });
        }
    }
    Ok(infos)
}
