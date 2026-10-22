//! The dataflow netlist (docs/IR.md): every template and expression placed in a stage, the
//! registers (materialized fragments) with their keys, variant bits and splices, and each
//! action's delta edges. Its report is the compiler's "timing report".

use crate::ast::*;
use crate::check::{Model, Scope};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// What a value depends on. Ordered from cheapest to dearest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Input {
    /// a parameter of the fragment or screen (a route parameter, or a row passed in)
    Param(String),
    /// the viewer, read only through a Bool (a variant bit)
    ViewerBool,
    /// the viewer, otherwise
    Viewer,
    /// the request beyond its route and viewer (none on the fast path)
    Request,
    /// the clock or randomness (never on the fast path)
    Clock,
}

/// The inputs of a value, and whether it reads the database (queries, rows through references).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deps {
    pub inputs: BTreeSet<Input>,
    pub reads_db: bool,
    /// it uses start-time values (config, the secret, the assets)
    pub start: bool,
    /// the tables and columns it reads ("Table.column"; "Table.*" for a whole row)
    pub reads: BTreeSet<String>,
}

impl Deps {
    fn join(&mut self, o: Deps) {
        self.inputs.extend(o.inputs);
        self.reads_db |= o.reads_db;
        self.start |= o.start;
        self.reads.extend(o.reads);
    }
    pub fn stage(&self) -> Stage {
        if self.inputs.contains(&Input::Clock) || self.inputs.contains(&Input::Request) {
            Stage::Cold
        } else if self.inputs.is_empty() && !self.reads_db {
            if self.start {
                Stage::Start
            } else {
                Stage::Compile
            }
        } else {
            Stage::Write
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Compile,
    Start,
    Write,
    Read,
    Cold,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Compile => "compile",
            Stage::Start => "start",
            Stage::Write => "write",
            Stage::Read => "read",
            Stage::Cold => "cold",
        }
    }
}

/// A register: a fragment kept in the snapshot under a key.
#[derive(Debug)]
pub struct Register {
    pub fragment: String,
    /// the key: the fragment's parameters, and the viewer when it's `per viewer`
    pub key: Vec<String>,
    /// the Bool holes and conditions that depend on the viewer: one bit each
    pub variant_bits: Vec<String>,
    /// holes filled at read time from another register (their text, and what they depend on)
    pub splices: Vec<(String, Vec<String>)>,
    /// the tables and columns it reads (what its delta edges watch)
    pub reads: BTreeSet<String>,
    /// constant bytes it carries, and the most bytes it can have (from the spec's bounds)
    pub const_bytes: usize,
    pub max_bytes: Option<usize>,
    /// it's built inside a loop of this many items at most (per key of the enclosing register)
    pub in_list: Option<i64>,
}

pub struct ScreenNet {
    pub name: String,
    pub route: String,
    pub stage: Stage,
    /// why it can't use the fast path, if it can't
    pub cold_because: Option<String>,
    /// the audience: who may see it, from its guard
    pub audience: Option<String>,
    /// the byte ranges of a page: constant pieces, registers, splices (worst case)
    pub segments: Option<usize>,
    /// the lists it shows, with their bounds
    pub lists: Vec<(String, Option<i64>)>,
}

pub struct Delta {
    pub action: String,
    pub write: String,
    pub registers: Vec<String>,
    pub how: String,
}

pub struct Net {
    pub registers: Vec<Register>,
    pub screens: Vec<ScreenNet>,
    pub deltas: Vec<Delta>,
    /// fragments only ever called with constants: start-time constants, inlined
    pub inlined: Vec<String>,
}

struct Ctx<'a> {
    m: &'a Model<'a>,
}

/// Names a scope binds that are rows or route parameters (their deps are the name itself).
fn is_viewer(e: &Expr) -> bool {
    matches!(e, Expr::Name(n, _) if n == "viewer")
}

impl<'a> Ctx<'a> {
    /// What an expression depends on. `params` are the names that are inputs here (the
    /// fragment's or screen's parameters and loop variables), and their entity.
    fn deps(&self, e: &Expr, scope: &Scope, params: &BTreeMap<String, Option<String>>) -> Deps {
        let mut d = Deps::default();
        match e {
            Expr::Str(..) | Expr::Int(..) | Expr::Bool(..) | Expr::Dur(..) | Expr::None(_) => {}
            Expr::Name(n, _) => {
                if params.contains_key(n) {
                    d.inputs.insert(Input::Param(n.clone()));
                } else if n == "viewer" {
                    d.inputs.insert(Input::Viewer);
                } else if n == "now" {
                    d.inputs.insert(Input::Clock);
                } else if n == "it" {
                    // inside a query: the query's own row
                } else if self.m.entities.contains_key(n) {
                    d.reads_db = true;
                    d.reads.insert(format!("{n}.*"));
                } else if let Some(Decl::Const { value, .. }) = self.m.consts.get(n) {
                    d.join(self.deps(value, scope, params));
                }
            }
            Expr::Dot(f, _) => {
                if let Some(Type::Ref(ent)) = scope.get("it") {
                    d.reads_db = true;
                    d.reads.insert(format!("{ent}.{f}"));
                }
            }
            Expr::Field(o, f, _) => {
                if let Expr::Name(n, _) = &**o {
                    if n == "config" {
                        d.start = true;
                        return d;
                    }
                }
                d.join(self.deps(o, scope, params));
                if let Ok(Type::Ref(ent)) = self.m.type_of(o, scope).map(|t| t.strip_opt().clone()) {
                    // a row's id is the reference itself; any other field is a read
                    if f != "id" {
                        d.reads_db = true;
                        d.reads.insert(format!("{ent}.{f}"));
                    }
                }
            }
            Expr::Call(f, args, _) => {
                for a in args {
                    d.join(self.deps(a, scope, params));
                }
                match f.as_str() {
                    "asset" | "sign" => d.start = true,
                    "token" => {
                        d.inputs.insert(Input::Clock);
                    }
                    "count" | "exists" | "first" => d.reads_db = true,
                    _ => {
                        if let Some(Decl::Fn { params: ps, body, .. }) = self.m.fns.get(f) {
                            // a function's body, with its parameters bound to the arguments' deps
                            let inner = Scope::of(ps);
                            let mut local: BTreeMap<String, Option<String>> = BTreeMap::new();
                            for p in ps {
                                local.insert(p.name.clone(), None);
                            }
                            let body_d = self.deps(body, &inner, &local);
                            let mut out = Deps { reads_db: body_d.reads_db, start: body_d.start, reads: body_d.reads.clone(), ..Default::default() };
                            for i in body_d.inputs {
                                match i {
                                    Input::Param(p) => {
                                        let k = ps.iter().position(|q| q.name == p).unwrap();
                                        out.join(self.deps(&args[k], scope, params));
                                    }
                                    other => {
                                        out.inputs.insert(other);
                                    }
                                }
                            }
                            d.join(out);
                        } else if let Some(Decl::View { params: ps, query, .. }) = self.m.views.get(f) {
                            d.reads_db = true;
                            let inner = Scope::of(ps);
                            let mut local: BTreeMap<String, Option<String>> = BTreeMap::new();
                            for p in ps {
                                local.insert(p.name.clone(), None);
                            }
                            let qd = self.query_deps(query, &inner, &local);
                            d.reads.extend(qd.reads);
                            for i in qd.inputs {
                                if let Input::Param(p) = i {
                                    let k = ps.iter().position(|q| q.name == p).unwrap();
                                    d.join(self.deps(&args[k], scope, params));
                                } else {
                                    d.inputs.insert(i);
                                }
                            }
                        }
                    }
                }
            }
            Expr::Bin(op, l, r, _) => {
                let (a, b) = (self.deps(l, scope, params), self.deps(r, scope, params));
                // `x == viewer` and friends: the viewer only through a Bool
                let viewer_cmp = matches!(op, BinOp::Eq | BinOp::Ne) && (is_viewer(l) || is_viewer(r));
                d.join(a);
                d.join(b);
                if viewer_cmp {
                    d.inputs.remove(&Input::Viewer);
                    d.inputs.insert(Input::ViewerBool);
                }
            }
            Expr::Not(x, _) => d.join(self.deps(x, scope, params)),
            Expr::If(c, a, b, _) => {
                d.join(self.deps(c, scope, params));
                d.join(self.deps(a, scope, params));
                d.join(self.deps(b, scope, params));
            }
            Expr::List(items, _) => {
                for i in items {
                    d.join(self.deps(i, scope, params));
                }
            }
            Expr::Record(fs, _) => {
                for (_, v) in fs {
                    d.join(self.deps(v, scope, params));
                }
            }
            Expr::Query(q) => d.join(self.query_deps(q, scope, params)),
            Expr::Comp(_, x, q, _) => {
                d.join(self.query_deps(q, scope, params));
                if let Ok(ent) = self.m.query_entity(q, scope) {
                    d.join(self.deps(x, &scope.with("it", Type::Ref(ent)), params));
                }
            }
        }
        d
    }

    fn query_deps(&self, q: &Query, scope: &Scope, params: &BTreeMap<String, Option<String>>) -> Deps {
        let mut d = Deps { reads_db: true, ..Default::default() };
        match &q.source {
            Source::Entity(e) => {
                d.reads.insert(format!("{e}.*"));
            }
            Source::Expr(e) => d.join(self.deps(e, scope, params)),
        }
        if let Ok(ent) = self.m.query_entity(q, scope) {
            let inner = scope.with("it", Type::Ref(ent));
            for x in q.filter.iter().chain(q.order.iter().map(|(k, _)| k)) {
                d.join(self.deps(x, &inner, params));
            }
        }
        for x in q.limit.iter().chain(q.window_last.iter()) {
            d.join(self.deps(x, scope, params));
        }
        // a query filtered by the viewer reads per viewer, not through a Bool
        if d.inputs.remove(&Input::ViewerBool) {
            d.inputs.insert(Input::Viewer);
        }
        d
    }
}

/// Walks a fragment's template: its variant bits, splices, constant bytes and the most bytes.
struct Walk<'a, 'b> {
    c: &'b Ctx<'a>,
    key: BTreeSet<String>,
    per_viewer: bool,
    bits: Vec<String>,
    splices: Vec<(String, Vec<String>)>,
    reads: BTreeSet<String>,
    const_bytes: usize,
    /// fragments called: (name, items per page when in a loop, called with constants only)
    calls: Vec<(String, Option<i64>, bool)>,
    bounded: bool,
    cold: Option<String>,
    /// the product of the bounds of the loops around the current node
    loop_bound: Option<i64>,
    /// the lists looped over: (shown, bound, the list, its scope)
    lists: Vec<(String, Option<i64>, Expr, Scope)>,
}

fn show_expr(e: &Expr) -> String {
    match e {
        Expr::Str(s, _) => format!("{s:?}"),
        Expr::Int(n, _) => n.to_string(),
        Expr::Bool(b, _) => b.to_string(),
        Expr::Dur(n, _) => format!("{n}ms"),
        Expr::None(_) => "none".into(),
        Expr::Name(n, _) => n.clone(),
        Expr::Dot(f, _) => format!(".{f}"),
        Expr::Field(o, f, _) => format!("{}.{f}", show_expr(o)),
        Expr::Call(f, a, _) => format!("{f}({})", a.iter().map(show_expr).collect::<Vec<_>>().join(", ")),
        Expr::Bin(op, l, r, _) => {
            let o = match op {
                BinOp::Or => "or",
                BinOp::And => "and",
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::Add => "+",
                BinOp::Sub => "-",
            };
            format!("{} {o} {}", show_expr(l), show_expr(r))
        }
        Expr::Not(x, _) => format!("not {}", show_expr(x)),
        Expr::If(..) => "if …".into(),
        Expr::List(..) => "[…]".into(),
        Expr::Record(..) => "{…}".into(),
        Expr::Query(_) => "query".into(),
        Expr::Comp(f, ..) => format!("{f}(… for …)"),
    }
}

impl<'a, 'b> Walk<'a, 'b> {
    fn covered(&self, d: &Deps) -> Result<(), Vec<String>> {
        let mut outside = vec![];
        for i in &d.inputs {
            match i {
                Input::Param(p) if !self.key.contains(p) => outside.push(p.clone()),
                Input::Viewer if !self.per_viewer => outside.push("viewer".into()),
                Input::Request => outside.push("the request".into()),
                Input::Clock => outside.push("the clock".into()),
                _ => {}
            }
        }
        if outside.is_empty() {
            Ok(())
        } else {
            Err(outside)
        }
    }

    /// A Bool that depends on the viewer is a variant bit: built for each kind of viewer at write
    /// time, chosen at read time. (A list that depends on the viewer is not: that needs a register
    /// keyed by the viewer.)
    fn as_bit(&mut self, e: &Expr, d: &mut Deps, scope: &Scope) {
        let is_bool = matches!(self.c.m.type_of(e, scope).map(|t| t.strip_opt().clone()), Ok(Type::Bool));
        if is_bool && !self.per_viewer && (d.inputs.remove(&Input::Viewer) | d.inputs.remove(&Input::ViewerBool)) {
            let per = self.loop_bound.map(|n| format!("   (per item: up to {n})")).unwrap_or_default();
            self.bits.push(format!("{}{per}", show_expr(e)));
        }
    }

    fn nodes(&mut self, nodes: &[TNode], scope: &Scope, params: &BTreeMap<String, Option<String>>) {
        for n in nodes {
            match n {
                TNode::Text(t) => self.const_bytes += t.len(),
                TNode::Hole(e) => {
                    // a fragment called here is its own register, unless it only has constants
                    if let Expr::Call(f, args, _) = e {
                        if self.c.m.fragments.contains_key(f) {
                            let constant = args.iter().all(|a| matches!(a, Expr::Str(..) | Expr::Int(..)));
                            self.calls.push((f.clone(), None, constant));
                            for a in args {
                                let d = self.c.deps(a, scope, params);
                                self.reads.extend(d.reads);
                            }
                            continue;
                        }
                    }
                    let mut d = self.c.deps(e, scope, params);
                    self.reads.extend(d.reads.iter().cloned());
                    self.as_bit(e, &mut d, scope);
                    match self.covered(&d) {
                        Ok(()) => {}
                        Err(out) if out.iter().all(|o| o != "the clock" && o != "the request" && o != "viewer") => {
                            self.splices.push((show_expr(e), out));
                        }
                        Err(out) => {
                            self.cold.get_or_insert(format!("`{}` reads {}", show_expr(e), out.join(", ")));
                        }
                    }
                }
                TNode::If(c, a, b) => {
                    let mut d = self.c.deps(c, scope, params);
                    self.reads.extend(d.reads.iter().cloned());
                    self.as_bit(c, &mut d, scope);
                    if let Err(out) = self.covered(&d) {
                        self.cold.get_or_insert(format!("`{{{{if {}}}}}` reads {}", show_expr(c), out.join(", ")));
                    }
                    self.nodes(a, scope, params);
                    self.nodes(b, scope, params);
                }
                TNode::For(v, list, _, body) => {
                    let d = self.c.deps(list, scope, params);
                    self.reads.extend(d.reads.iter().cloned());
                    if let Err(out) = self.covered(&d) {
                        self.cold.get_or_insert(format!("`{{{{for {v} in {}}}}}` reads {}", show_expr(list), out.join(", ")));
                    }
                    let bound = self.c.m.bound_of(list, scope);
                    self.lists.push((show_expr(list), bound, list.clone(), scope.clone()));
                    let saved_bound = self.loop_bound;
                    self.loop_bound = Some(saved_bound.unwrap_or(1) * bound.unwrap_or(1));
                    if bound.is_none() {
                        self.bounded = false;
                    }
                    let t = match self.c.m.type_of(list, scope) {
                        Ok(Type::List(t)) => *t,
                        _ => Type::None,
                    };
                    let mut inner = params.clone();
                    // a loop variable is an input of what the loop builds (a row of the list)
                    inner.insert(v.clone(), None);
                    let before = self.calls.len();
                    let saved_key = self.key.clone();
                    self.key.insert(v.clone());
                    let before_bytes = self.const_bytes;
                    self.nodes(body, &scope.with(v, t), &inner);
                    let body_bytes = self.const_bytes - before_bytes;
                    if let Some(b) = bound {
                        self.const_bytes += body_bytes * (b.max(1) as usize - 1);
                    }
                    self.key = saved_key;
                    self.loop_bound = saved_bound;
                    for c in &mut self.calls[before..] {
                        c.1 = Some(c.1.unwrap_or(1) * bound.unwrap_or(1));
                    }
                }
            }
        }
    }
}

fn walk<'a, 'b>(c: &'b Ctx<'a>, key: BTreeSet<String>, per_viewer: bool) -> Walk<'a, 'b> {
    Walk { c, key, per_viewer, bits: vec![], splices: vec![], reads: BTreeSet::new(), const_bytes: 0, calls: vec![], bounded: true, cold: None, loop_bound: None, lists: vec![] }
}

pub fn build(m: &Model) -> Net {
    let c = Ctx { m };
    let mut registers = vec![];
    let mut screens = vec![];
    let mut cold_frags: BTreeMap<String, String> = BTreeMap::new();
    let mut calls: Vec<(String, Option<i64>, bool)> = vec![];
    // every fragment is a register: keyed by its parameters, or by the viewer alone when it's
    // `per viewer` (then what depends on a parameter is spliced in at read time)
    for d in &m.app.decls {
        if let Decl::Fragment { name, params, per_viewer, body, .. } = d {
            let scope = Scope::of(params);
            let ps: BTreeMap<String, Option<String>> = params.iter().map(|p| (p.name.clone(), None)).collect();
            let key: BTreeSet<String> = if *per_viewer { BTreeSet::new() } else { params.iter().map(|p| p.name.clone()).collect() };
            let mut w = walk(&c, key, *per_viewer);
            w.nodes(&body.nodes, &scope, &ps);
            if let Some(why) = w.cold.take() {
                cold_frags.insert(name.clone(), why);
            }
            let key: Vec<String> = if *per_viewer { vec!["viewer".into()] } else { params.iter().map(|p| format!("{}: {}", p.name, p.ty.show())).collect() };
            calls.extend(w.calls.iter().cloned());
            registers.push(Register {
                fragment: name.clone(),
                key,
                variant_bits: w.bits,
                splices: w.splices,
                reads: w.reads,
                const_bytes: w.const_bytes,
                max_bytes: None,
                in_list: None,
            });
        }
    }
    let mut screen_lists = vec![];
    for d in &m.screens {
        if let Decl::Screen { name, params, route, cold, requires, body, .. } = d {
            let scope = Scope::of(params);
            let ps: BTreeMap<String, Option<String>> = params.iter().map(|p| (p.name.clone(), None)).collect();
            let mut w = walk(&c, params.iter().map(|p| p.name.clone()).collect(), true);
            w.nodes(&body.nodes, &scope, &ps);
            calls.extend(w.calls.iter().cloned());
            let mut cold_because = w.cold.take();
            if *cold {
                cold_because = Some("declared cold".into());
            }
            if cold_because.is_none() {
                for (callee, _, _) in &w.calls {
                    if let Some(why) = cold_frags.get(callee) {
                        cold_because = Some(format!("fragment {callee}: {why}"));
                    }
                }
            }
            let audience = requires.iter().find_map(|r| audience_of(m, &r.cond));
            // byte ranges: one per register instance, plus two per splice (it and the piece after)
            let mut segs = 0usize;
            for (callee, bound, _) in &w.calls {
                let n = bound.unwrap_or(1) as usize;
                let r = registers.iter().find(|r| &r.fragment == callee);
                segs += n * (1 + r.map(|r| 2 * r.splices.len()).unwrap_or(0));
            }
            let lists: Vec<(String, Option<i64>)> = w.lists.iter().map(|(s, b, _, _)| (s.clone(), *b)).collect();
            for (s, b, e, sc) in &w.lists {
                screen_lists.push((name.clone(), s.clone(), *b, e.clone(), sc.clone()));
            }
            screens.push(ScreenNet {
                name: name.clone(),
                route: route.clone(),
                stage: if cold_because.is_some() { Stage::Cold } else { Stage::Read },
                cold_because,
                audience,
                segments: Some(segs + 1),
                lists,
            });
        }
    }
    // a fragment only ever called with constants is itself a constant (start time): inlined
    let called: BTreeSet<String> = calls.iter().map(|(c, _, _)| c.clone()).collect();
    let inlined: BTreeSet<String> = called.iter().filter(|f| calls.iter().filter(|(c, _, _)| c == *f).all(|(_, _, k)| *k)).cloned().collect();
    for (callee, bound, constant) in &calls {
        if !constant {
            if let Some(r) = registers.iter_mut().find(|r| &r.fragment == callee) {
                if bound.is_some() {
                    r.in_list = *bound;
                }
            }
        }
    }
    registers.retain(|r| !inlined.contains(&r.fragment));
    let mut deltas = deltas(m, &registers);
    deltas.extend(list_deltas(m, &screen_lists));
    Net { registers, screens, deltas, inlined: inlined.into_iter().collect() }
}

/// What creating a row does to the lists screens show: appended when the list's order is by
/// creation (a new row is the newest: `created` is set to now), else rebuilt.
fn list_deltas(m: &Model, lists: &[(String, String, Option<i64>, Expr, Scope)]) -> Vec<Delta> {
    let mut out = vec![];
    for d in &m.actions {
        if let Decl::Action { name, params, body, .. } = d {
            let mut writes = vec![];
            collect_writes(m, body, &Scope::of(params), &mut writes);
            for (table, _) in writes.iter().filter(|(_, c)| c.iter().any(|c| c == "*")) {
                for (screen, shown, bound, e, sc) in lists {
                    let q = match e {
                        Expr::Call(f, _, _) => match m.views.get(f) {
                            Some(Decl::View { query, .. }) => query.clone(),
                            _ => continue,
                        },
                        Expr::Query(q) => (**q).clone(),
                        _ => continue,
                    };
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => match m.query_entity(&q, sc) {
                            Ok(e) => e,
                            Err(_) => continue,
                        },
                    };
                    if &ent != table {
                        continue;
                    }
                    let by_created = matches!(q.order.first(), Some((Expr::Dot(f, _), false)) if f == "created");
                    let how = match (by_created, &q.window_last, bound) {
                        (true, Some(_), Some(n)) => format!("append the new row's fragment; drop the oldest past {n} (monotone: ordered by created)"),
                        (true, None, _) if q.limit.is_none() => "append (monotone: ordered by created)".to_string(),
                        _ => "rebuild the list for the affected keys".to_string(),
                    };
                    out.push(Delta { action: name.clone(), write: format!("create {table}"), registers: vec![format!("screen {screen}: {shown}")], how });
                }
            }
        }
    }
    out
}

/// `member(viewer, room)` → "Membership where .room == room → .user"
fn audience_of(m: &Model, cond: &Expr) -> Option<String> {
    if let Expr::Call(f, args, _) = cond {
        if let Some(Decl::Fn { params, body: Expr::Call(ex, inner, _), .. }) = m.fns.get(f) {
            if ex == "exists" {
                if let Some(Expr::Query(q)) = inner.first() {
                    if let (Source::Entity(ent), Some(filter)) = (&q.source, &q.filter) {
                        // the parameter the viewer is passed as, and the other one
                        let vi = args.iter().position(is_viewer)?;
                        let vp = &params[vi].name;
                        let mut by_viewer = None;
                        let mut others = vec![];
                        conjuncts(filter, &mut |c| {
                            if let Expr::Bin(BinOp::Eq, l, r, _) = c {
                                if let (Expr::Dot(f, _), Expr::Name(n, _)) = (&**l, &**r) {
                                    if n == vp {
                                        by_viewer = Some(f.clone());
                                    } else if let Some(k) = params.iter().position(|p| &p.name == n) {
                                        others.push(format!(".{f} == {}", show_expr(&args[k])));
                                    }
                                }
                            }
                        });
                        return Some(format!("{ent} where {} → .{}", others.join(" and "), by_viewer?));
                    }
                }
            }
        }
    }
    None
}

fn conjuncts<'e>(e: &'e Expr, f: &mut impl FnMut(&'e Expr)) {
    if let Expr::Bin(BinOp::And, l, r, _) = e {
        conjuncts(l, f);
        conjuncts(r, f);
    } else {
        f(e);
    }
}

/// For each action's writes, the registers that read what it writes.
fn deltas(m: &Model, registers: &[Register]) -> Vec<Delta> {
    let mut out = vec![];
    for d in &m.actions {
        if let Decl::Action { name, params, body, .. } = d {
            let mut writes: Vec<(String, Vec<String>)> = vec![];
            collect_writes(m, body, &Scope::of(params), &mut writes);
            for (table, cols) in writes {
                let regs: Vec<String> = registers
                    .iter()
                    .filter(|r| r.reads.iter().any(|rd| {
                        let (t, c) = rd.split_once('.').unwrap_or((rd, "*"));
                        t == table && (c == "*" || cols.iter().any(|w| w == "*" || w == c))
                    }))
                    .map(|r| r.fragment.clone())
                    .collect();
                if regs.is_empty() {
                    continue;
                }
                let what = if cols.iter().any(|c| c == "*") { format!("create {table}") } else { format!("set {table}.{}", cols.join(", ")) };
                out.push(Delta { action: name.clone(), write: what, registers: regs, how: "rebuild the affected keys".into() });
            }
        }
    }
    out
}

fn collect_writes(m: &Model, body: &[Stmt], scope: &Scope, out: &mut Vec<(String, Vec<String>)>) {
    for s in body {
        match s {
            Stmt::Create(e, _, _) => out.push((e.clone(), vec!["*".into()])),
            Stmt::Touch(e, _) => {
                if let Ok(Type::Ref(t)) = m.type_of(e, scope) {
                    out.push((t, vec!["updated".into()]));
                }
            }
            Stmt::Set(o, f, _, _) => {
                if let Ok(Type::Ref(t)) = m.type_of(o, scope) {
                    out.push((t, vec![f.clone()]));
                }
            }
            Stmt::ForEach(v, q, inner, _) => {
                if let Ok(ent) = m.query_entity(q, scope) {
                    collect_writes(m, inner, &scope.with(v, Type::Ref(ent)), out);
                }
            }
            Stmt::Delete(e, _) => {
                if let Ok(Type::Ref(t)) = m.type_of(e, scope) {
                    out.push((t, vec!["*".into()]));
                }
            }
            Stmt::SignIn(..) | Stmt::SignOut(_) => {
                if let Some(Decl::Viewer { via_entity, .. }) = m.viewer {
                    out.push((via_entity.clone(), vec!["*".into()]));
                }
            }
            Stmt::Require(_) | Stmt::Let(..) => {}
        }
    }
}

/// The report: per screen, per register, per action.
pub fn report(net: &Net) -> String {
    let mut s = String::new();
    for sc in &net.screens {
        let _ = writeln!(s, "screen {} at {}", sc.name, sc.route);
        let _ = writeln!(s, "  read stage: {}", match &sc.cold_because {
            None => "clean: lookups, variant choice and byte ranges only (fast path)".to_string(),
            Some(w) => format!("COLD: {w}"),
        });
        if let Some(a) = &sc.audience {
            let _ = writeln!(s, "  audience: {a}");
        }
        for (l, b) in &sc.lists {
            let _ = writeln!(s, "  list: {l}  ({})", b.map(|n| format!("at most {n}")).unwrap_or("UNBOUNDED".into()));
        }
        if let Some(n) = sc.segments {
            let _ = writeln!(s, "  byte ranges per page, at most: {n}");
        }
        let _ = writeln!(s);
    }
    if !net.inlined.is_empty() {
        let _ = writeln!(s, "inlined as start-time constants (only ever called with constants): {}\n", net.inlined.join(", "));
    }
    let _ = writeln!(s, "registers (built at write time, kept in the snapshot):");
    for r in &net.registers {
        let key = if r.key.is_empty() { "none (one for the app: start time)".to_string() } else { r.key.join(", ") };
        let _ = writeln!(s, "  {}  key: {key}{}", r.fragment, r.in_list.map(|n| format!("  (up to {n} per page)")).unwrap_or_default());
        if !r.variant_bits.is_empty() {
            let _ = writeln!(s, "    variant bits ({}, at most {} variants):", r.variant_bits.len(), 1u64 << r.variant_bits.len().min(63));
            for b in &r.variant_bits {
                let _ = writeln!(s, "      {b}");
            }
        }
        for (e, by) in &r.splices {
            let _ = writeln!(s, "    splice at read time: {e}  (depends on {})", by.join(", "));
        }
        let _ = writeln!(s, "    constant bytes, worst case: {}", r.const_bytes);
        if !r.reads.is_empty() {
            let _ = writeln!(s, "    reads: {}", r.reads.iter().cloned().collect::<Vec<_>>().join(" "));
        }
    }
    let _ = writeln!(s, "\ndelta edges (what each action's writes change):");
    for d in &net.deltas {
        let _ = writeln!(s, "  {}: {} → {} ({})", d.action, d.write, d.registers.join(", "), d.how);
    }
    s
}
