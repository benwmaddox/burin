//! Queries to SQL: each register's record and lists are filled by one SELECT each, selecting
//! exactly the record's fields (references followed by LEFT JOINs), with the query's filter,
//! order and bound translated. Values from outside the query (a parent record's field, the
//! viewer) are bound parameters.

use crate::ast::*;
use crate::check::{Model, Scope};
use crate::inline;

/// A value bound into a query: a path from a root outside it (`m`, `ms.room`, `viewer`).
#[derive(Clone, Debug)]
pub enum Bind {
    Path(Expr),
}

pub struct Select {
    pub sql: String,
    pub binds: Vec<Bind>,
}

// ---- read sets: every path the lowering turns into SQL while it is recording (a register's
// loader, a screen list's query): (the table it starts from, the dotted path from it), so the same
// column read along two paths (a message's creator's name, a boost's booster's name) is two reads.
// "*": the table's rows themselves (a row made or deleted can enter or leave). Recorded where
// paths become columns, so nothing read is missed.

thread_local! {
    static READS: std::cell::RefCell<Option<Vec<(String, String)>>> = const { std::cell::RefCell::new(None) };
}

fn note_path(ent: &str, path: &[String]) {
    note(ent, &path.join("."));
}

fn note(ent: &str, col: &str) {
    READS.with(|r| {
        if let Some(v) = r.borrow_mut().as_mut() {
            let x = (ent.to_string(), col.to_string());
            if !v.contains(&x) {
                v.push(x);
            }
        }
    });
}

/// Where a read path ends: (entity, column), "*" for a table's rows.
pub fn target(m: &Model, start: &str, path: &str) -> Option<(String, String)> {
    if path == "*" {
        return Some((start.to_string(), "*".into()));
    }
    let parts: Vec<&str> = path.split('.').collect();
    let mut ent = start.to_string();
    for f in &parts[..parts.len() - 1] {
        ent = ref_target(m, &ent, f).ok()?;
    }
    Some((ent, parts[parts.len() - 1].to_string()))
}

/// Runs `f`, returning what it read.
pub fn reading<R>(f: impl FnOnce() -> R) -> (R, Vec<(String, String)>) {
    let outer = READS.with(|r| r.borrow_mut().replace(vec![]));
    let x = f();
    let got = READS.with(|r| std::mem::replace(&mut *r.borrow_mut(), outer)).unwrap_or_default();
    // a recording inside another: the outer one reads it too
    for (e, c) in &got {
        note(e, c);
    }
    (x, got)
}

fn q(name: &str) -> String {
    format!("\"{name}\"")
}

/// LEFT JOINs for paths through references from the top-level row.
struct Joins {
    list: Vec<(Vec<String>, String)>,
    sql: String,
    next: usize,
}

impl Joins {
    fn column(&mut self, m: &Model, base: &str, base_ent: &str, path: &[String]) -> Result<String, String> {
        if path.is_empty() {
            return Ok(format!("{base}.id"));
        }
        note_path(base_ent, path);
        let (mut alias, mut ent) = (base.to_string(), base_ent.to_string());
        for i in 0..path.len() - 1 {
            let f = &path[i];
            let target = ref_target(m, &ent, f)?;
            let key: Vec<String> = std::iter::once(base.to_string()).chain(path[..=i].iter().cloned()).collect();
            let a = match self.list.iter().find(|(k, _)| *k == key) {
                Some((_, a)) => a.clone(),
                None => {
                    self.next += 1;
                    let a = format!("j{}", self.next);
                    self.sql.push_str(&format!(" LEFT JOIN {} {a} ON {a}.id = {alias}.{}", q(&target), q(f)));
                    self.list.push((key, a.clone()));
                    a
                }
            };
            alias = a;
            ent = target;
        }
        let last = &path[path.len() - 1];
        if m.field_type(&ent, last).is_none() {
            return Err(format!("{ent} has no field {last}"));
        }
        Ok(if last == "id" { format!("{alias}.id") } else { format!("{alias}.{}", q(last)) })
    }
}

fn ref_target(m: &Model, ent: &str, f: &str) -> Result<String, String> {
    match m.field_type(ent, f).as_ref().map(|t| t.strip_opt().clone()) {
        Some(Type::Ref(e)) => Ok(e),
        _ => Err(format!("{ent}.{f} isn't a reference")),
    }
}

/// A path inside a subquery (no joins there): through references as nested scalar subselects.
fn sub_column(m: &Model, alias: &str, ent: &str, p: &[String]) -> Result<String, String> {
    match p.len() {
        0 => Ok(format!("{alias}.id")),
        1 => {
            note_path(ent, p);
            Ok(format!("{alias}.{}", q(&p[0])))
        }
        _ => {
            note_path(ent, p);
            let target = ref_target(m, ent, &p[0])?;
            // (the inner part is the same path, already noted from its start)
            let inner = READS.with(|r| {
                let saved = r.borrow_mut().take();
                let x = sub_column(m, "x", &target, &p[1..]);
                *r.borrow_mut() = saved;
                x
            })?;
            Ok(format!("(SELECT {inner} FROM {} x WHERE x.id = {alias}.{})", q(&target), q(&p[0])))
        }
    }
}

/// (how many queries out, the path) when `e` is a path from a query's row: `.a.b` and `it.a` are
/// the innermost row's; `^k.a` the row k queries further out.
/// Whether a condition holds an order (`<`, `>=`, …) outside a subquery: NULL when a side is none.
fn orders(e: &Expr) -> bool {
    match e {
        Expr::Bin(BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, ..) => true,
        Expr::Bin(BinOp::And | BinOp::Or, l, r, _) => orders(l) || orders(r),
        Expr::Not(x, _) => orders(x),
        _ => false,
    }
}

fn row_ref(e: &Expr) -> Option<(usize, Vec<String>)> {
    match e {
        Expr::Dot(f, _) => Some((0, if f == "id" { vec![] } else { vec![f.clone()] })),
        Expr::Name(n, _) if n == "it" => Some((0, vec![])),
        Expr::Name(n, _) if n.starts_with('^') => Some((n[1..].parse().ok()?, vec![])),
        Expr::Field(o, f, _) => {
            let (k, mut p) = row_ref(o)?;
            if f != "id" {
                p.push(f.clone());
            }
            Some((k, p))
        }
        _ => None,
    }
}

struct Tx<'a> {
    m: &'a Model<'a>,
    joins: Joins,
    binds: Vec<Bind>,
    /// parameters that are rows (name, entity): their fields are subselects by their id
    params: Vec<(String, String)>,
    /// the rows of the queries we are in: (alias, entity), innermost last; the first is the top
    rows: Vec<(String, String)>,
    sub: usize,
}

impl<'a> Tx<'a> {
    /// A path from a row parameter (`room.type`): its root, its entity, the fields.
    fn param_path(&self, e: &Expr) -> Option<(String, String, Vec<String>)> {
        fn walk(e: &Expr) -> Option<(String, Vec<String>)> {
            match e {
                Expr::Name(n, _) => Some((n.clone(), vec![])),
                Expr::Field(o, f, _) => {
                    let (r, mut p) = walk(o)?;
                    p.push(f.clone());
                    Some((r, p))
                }
                _ => None,
            }
        }
        let (r, p) = walk(e)?;
        if p.is_empty() || p == ["id"] {
            return None;
        }
        let ent = self.params.iter().find(|(n, _)| *n == r)?.1.clone();
        Some((r, ent, p))
    }

    fn expr(&mut self, e: &Expr) -> Result<String, String> {
        if let Some((k, p)) = row_ref(e) {
            if k >= self.rows.len() {
                return Err("a reference to a row outside every query".into());
            }
            let at = self.rows.len() - 1 - k;
            let (a, ent) = self.rows[at].clone();
            return if a == "t0" { self.joins.column(self.m, &a, &ent, &p) } else { sub_column(self.m, &a, &ent, &p) };
        }
        Ok(match e {
            Expr::Str(s, _) => format!("'{}'", s.replace('\'', "''")),
            Expr::Int(n, _) => n.to_string(),
            Expr::Dur(n, _) => n.to_string(),
            Expr::Bool(b, _) => (*b as i32).to_string(),
            Expr::None(_) => "NULL".into(),
            Expr::Call(f, args, _) if f == "present" => format!("(trim(coalesce({}, '')) <> '')", self.expr(&args[0])?),
            // `first(query).field`: a scalar subselect
            Expr::Field(o, _, _) if matches!(&**o, Expr::Call(c, _, _) if c == "first") => scalar(self, e)?,
            Expr::Call(f, args, _) if f == "either" => format!("coalesce({}, {})", self.expr(&args[0])?, self.expr(&args[1])?),
            // a request's cookie: bound
            Expr::Call(f, _, _) if f == "cookie" => {
                self.binds.push(Bind::Path(e.clone()));
                "?".into()
            }
            Expr::Field(..) if self.param_path(e).is_some() => {
                // a row parameter's field: a subselect by the parameter's id (bound)
                let (root, ent, path) = self.param_path(e).unwrap();
                self.binds.push(Bind::Path(Expr::Name(root, e.span())));
                self.sub += 1;
                let a = format!("p{}", self.sub);
                let col = sub_column(self.m, &a, &ent, &path)?;
                format!("(SELECT {col} FROM {} {a} WHERE {a}.id = ?)", q(&ent))
            }
            Expr::Call(f, args, _) if f == "first" => {
                let qy = match &args[0] {
                    Expr::Query(x) => (**x).clone(),
                    _ => return Err("first takes a query".into()),
                };
                let (from, w) = self.subquery(&args[0])?;
                let (alias, _) = self.rows.last().cloned().unwrap();
                let mut order = vec![];
                for (k, d) in &qy.order {
                    order.push(format!("{}{}", self.expr(k)?, if *d { " DESC" } else { "" }));
                }
                self.rows.pop();
                let ord = if order.is_empty() { String::new() } else { format!(" ORDER BY {}", order.join(", ")) };
                format!("(SELECT {alias}.id {from}{w}{ord} LIMIT 1)")
            }
            Expr::Name(..) | Expr::Field(..) => {
                // a value from outside the query: bound
                self.binds.push(Bind::Path(e.clone()));
                "?".into()
            }
            // an order on none is NULL, which NOT keeps NULL: made false first, so `not (x < 10)`
            // is true for none, as a fragment's code has it
            Expr::Not(x, _) if orders(x) => format!("NOT coalesce(({}), 0)", self.expr(x)?),
            Expr::Not(x, _) => format!("NOT ({})", self.expr(x)?),
            Expr::Bin(op, l, r, _) => {
                let (a, b) = (self.expr(l)?, self.expr(r)?);
                let o = match op {
                    BinOp::Or => "OR",
                    BinOp::And => "AND",
                    BinOp::Eq => "IS",
                    BinOp::Ne => "IS NOT",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    BinOp::Ge => ">=",
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    // on INTEGERs: truncating, and NULL (none) by 0, as the fragments' code does
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Mod => "%",
                    // a list argument is bound as a JSON array of ids
                    BinOp::In => return Ok(format!("({a} IN (SELECT value FROM json_each({b})))")),
                };
                format!("({a} {o} {b})")
            }
            Expr::Call(f, args, _) if f == "lower" => format!("lower({})", self.expr(&args[0])?),
            // SQLite's scalar min and max: NULL when either is
            Expr::Call(f, args, _) if f == "max" || f == "min" => format!("{f}({}, {})", self.expr(&args[0])?, self.expr(&args[1])?),
            Expr::Call(f, args, _) if f == "clamp" => format!("max({}, min({}, {}))", self.expr(&args[1])?, self.expr(&args[0])?, self.expr(&args[2])?),
            Expr::Call(f, args, _) if f == "abs" => format!("abs({})", self.expr(&args[0])?),
            Expr::Call(f, args, _) if f == "exists" && self.semijoin(&args[0])?.is_some() => self.semijoin(&args[0])?.unwrap(),
            Expr::Call(f, args, _) if f == "exists" || f == "count" => {
                let (from, w) = self.subquery(&args[0])?;
                let r = format!("(SELECT {} {from}{w})", if f == "exists" { "1" } else { "count(*)" });
                self.rows.pop();
                if f == "exists" {
                    format!("EXISTS {r}")
                } else {
                    r
                }
            }
            Expr::Comp(f, x, qy, _) if f == "count_distinct" => {
                let (from, w) = self.subquery(&Expr::Query(qy.clone()))?;
                let col = self.expr(x)?;
                self.rows.pop();
                format!("(SELECT count(DISTINCT {col}) {from}{w})")
            }
            // an external function of values from outside the query (an action's arguments): the
            // caller computes it and binds the result; one of the rows' fields can't reach it
            Expr::Call(f, args, _) if self.m.externs.contains_key(f) => {
                if args.iter().any(reads_row) {
                    return Err(format!("`{f}` is an external function: it can't run inside a query on the query's rows (SQLite runs those); use it in a template or an action, or keep its result in a field"));
                }
                self.binds.push(Bind::Path(e.clone()));
                "?".to_string()
            }
            _ => return Err(format!("this can't be part of a query's SQL yet: {e:?}").chars().take(200).collect()),
        })
    }

    /// `exists(E where .f == <the outer row's path> and P)`, P not reading the outer row, as a
    /// semi-join: `<path> IN (SELECT f FROM E WHERE P)`. SQLite evaluates an uncorrelated IN
    /// once per query, where a correlated EXISTS runs once per outer row (thousands of lookups for
    /// a list of people each checked against their memberships). None when it doesn't apply.
    fn semijoin(&mut self, e: &Expr) -> Result<Option<String>, String> {
        let qy = match e {
            Expr::Query(qy) => (**qy).clone(),
            _ => return Ok(None),
        };
        let ent = match &qy.source {
            Source::Entity(en) => en.clone(),
            _ => return Ok(None),
        };
        let Some(filter) = &qy.filter else { return Ok(None) };
        let mut conj = vec![];
        split_and(filter, &mut conj);
        // the one conjunct that reads the outer row: `.f == outer`
        let mut corr: Option<(String, Expr)> = None;
        let mut rest = vec![];
        for c in conj {
            if !refs_outer(&c, 0) {
                rest.push(c);
                continue;
            }
            if corr.is_some() || self.rows.is_empty() {
                return Ok(None);
            }
            match &c {
                Expr::Bin(BinOp::Eq, l, r, _) => match (&**l, &**r) {
                    (Expr::Dot(f, _), x) | (x, Expr::Dot(f, _)) if f != "id" && !refs_inner(x) => corr = Some((f.clone(), x.clone())),
                    _ => return Ok(None),
                },
                _ => return Ok(None),
            }
        }
        let Some((f, outer)) = corr else { return Ok(None) };
        // the outer path, from where the exists is (one query out from inside it)
        let lhs = self.expr(&unshift(&outer))?;
        self.sub += 1;
        let a = format!("s{}", self.sub);
        note(&ent, "*");
        note(&ent, &f);
        self.rows.push((a.clone(), ent.clone()));
        let mut w = format!("{a}.{} IS NOT NULL", q(&f));
        for c in &rest {
            w = format!("{w} AND {}", self.expr(c)?);
        }
        self.rows.pop();
        Ok(Some(format!("({lhs} IN (SELECT {a}.{} FROM {} {a} WHERE {w}))", q(&f), q(&ent))))
    }

    /// A subquery's FROM and WHERE; its row is pushed (the caller pops it).
    fn subquery(&mut self, e: &Expr) -> Result<(String, String), String> {
        let qy = match e {
            Expr::Query(qy) => (**qy).clone(),
            Expr::Name(n, _) if self.m.entities.contains_key(n) => Query { source: Source::Entity(n.clone()), filter: None, order: vec![], limit: None, window_last: None, span: e.span() },
            _ => return Err("a subquery must be a query".into()),
        };
        let (qy, ent) = match &qy.source {
            Source::Entity(en) => (qy.clone(), en.clone()),
            Source::Expr(_) => resolve(self.m, &Expr::Query(Box::new(qy.clone())), &Scope::default())?,
        };
        self.sub += 1;
        let a = format!("s{}", self.sub);
        note(&ent, "*");
        self.rows.push((a.clone(), ent.clone()));
        let w = match &qy.filter {
            Some(f) => format!(" WHERE {}", self.expr(f)?),
            None => String::new(),
        };
        Ok((format!("FROM {} {a}", q(&ent)), w))
    }
}

fn split_and(e: &Expr, out: &mut Vec<Expr>) {
    if let Expr::Bin(BinOp::And, l, r, _) = e {
        split_and(l, out);
        split_and(r, out);
    } else {
        out.push(e.clone());
    }
}

/// Whether `e` (in a subquery, `depth` queries further in) reads a row outside the subquery.
fn refs_outer(e: &Expr, depth: usize) -> bool {
    match e {
        Expr::Name(n, _) if n.starts_with('^') => n[1..].parse::<usize>().map(|k| k > depth).unwrap_or(true),
        Expr::Field(o, _, _) | Expr::Not(o, _) => refs_outer(o, depth),
        Expr::Bin(_, l, r, _) => refs_outer(l, depth) || refs_outer(r, depth),
        Expr::Call(_, a, _) | Expr::List(a, _) => a.iter().any(|x| refs_outer(x, depth)),
        Expr::If(c, a, b, _) => refs_outer(c, depth) || refs_outer(a, depth) || refs_outer(b, depth),
        Expr::Query(q) => {
            q.filter.as_ref().map(|f| refs_outer(f, depth + 1)).unwrap_or(false)
                || matches!(&q.source, Source::Expr(x) if refs_outer(x, depth))
                || q.order.iter().any(|(k, _)| refs_outer(k, depth + 1))
        }
        Expr::Comp(_, x, q, _) => refs_outer(x, depth + 1) || refs_outer(&Expr::Query(q.clone()), depth),
        _ => false,
    }
}

/// Whether `e` reads the subquery's own row (`.f`, `it`).
fn refs_inner(e: &Expr) -> bool {
    match e {
        Expr::Dot(..) => true,
        Expr::Name(n, _) => n == "it",
        Expr::Field(o, _, _) | Expr::Not(o, _) => refs_inner(o),
        Expr::Bin(_, l, r, _) => refs_inner(l) || refs_inner(r),
        Expr::Call(_, a, _) => a.iter().any(refs_inner),
        _ => false,
    }
}

/// An outer row's path seen from one query out: `^1` becomes `it`, `^k` becomes `^(k-1)`.
fn unshift(e: &Expr) -> Expr {
    match e {
        Expr::Name(n, s) if n.starts_with('^') => {
            let k: usize = n[1..].parse().unwrap_or(1);
            Expr::Name(if k <= 1 { "it".into() } else { format!("^{}", k - 1) }, *s)
        }
        Expr::Field(o, f, s) => Expr::Field(Box::new(unshift(o)), f.clone(), *s),
        other => other.clone(),
    }
}

/// The SELECT that fills a list: the query (a view's, its parameters bound to the call's
/// arguments), the items' fields (paths from the loop variable, whose first part is dropped).
pub fn list_select(m: &Model, list: &Expr, fields: &[String], scope: &Scope) -> Result<Select, String> {
    let (qy, ent) = resolve(m, list, scope)?;
    note(&ent, "*");
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows: vec![("t0".into(), ent.clone())], sub: 0 };
    let mut cols = vec![];
    for f in fields {
        let p: Vec<String> = f.split('.').skip(1).map(String::from).filter(|s| s != "id").collect();
        cols.push(tx.joins.column(m, "t0", &ent, &p)?);
    }
    // a list only counted reads no field: its rows' ids
    if cols.is_empty() {
        cols.push("t0.id".into());
    }
    let w = match &qy.filter {
        Some(f) => format!(" WHERE {}", tx.expr(f)?),
        None => String::new(),
    };
    let mut order = vec![];
    for (k, desc) in &qy.order {
        order.push((tx.expr(k)?, *desc));
    }
    // a bound outside the query's rows: constant, or an expression of bound values and subqueries
    let outer = |tx: &mut Tx, e: &Expr| -> Result<String, String> {
        let saved = std::mem::take(&mut tx.rows);
        let r = tx.expr(e);
        tx.rows = saved;
        r
    };
    let limit = match &qy.limit {
        Some(e) => Some(outer(&mut tx, e)?),
        None => None,
    };
    let window = match &qy.window_last {
        Some(e) => Some(outer(&mut tx, e)?),
        None => None,
    };
    let from = format!("FROM {} t0{}", q(&ent), tx.joins.sql);
    let ord = |rev: bool| -> String {
        order.iter().map(|(o, d)| format!("{o}{}", if *d != rev { " DESC" } else { "" })).collect::<Vec<_>>().join(", ")
    };
    let sql = if let Some(n) = window {
        // the last N in the order, oldest first: the newest N, put back in order
        let inner_cols: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c} AS c{i}")).collect();
        let keys: Vec<String> = order.iter().enumerate().map(|(i, (o, _))| format!("{o} AS k{i}")).collect();
        let outer_order: Vec<String> = order.iter().enumerate().map(|(i, (_, d))| format!("k{i}{}", if *d { " DESC" } else { "" })).collect();
        format!(
            "SELECT {} FROM (SELECT {}, {} {from}{w} ORDER BY {} LIMIT {n}) ORDER BY {}",
            (0..cols.len()).map(|i| format!("c{i}")).collect::<Vec<_>>().join(", "),
            inner_cols.join(", "),
            keys.join(", "),
            ord(true),
            outer_order.join(", ")
        )
    } else {
        let mut s = format!("SELECT {} {from}{w}", cols.join(", "));
        if !order.is_empty() {
            s.push_str(&format!(" ORDER BY {}", ord(false)));
        }
        if let Some(n) = limit {
            s.push_str(&format!(" LIMIT {n}"));
        }
        s
    };
    Ok(Select { sql, binds: tx.binds })
}

/// The SELECT that fills a register's own record: its row by id (or no row, for a register keyed
/// by the viewer), with the record's fields; `scalars` are values of other queries (`first(Account).f`).
pub fn row_select(m: &Model, ent: Option<&str>, root: &str, fields: &[String], scalars: &[(usize, Expr)]) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows: vec![], sub: 0 };
    let mut cols = vec![];
    for (i, f) in fields.iter().enumerate() {
        if let Some((_, e)) = scalars.iter().find(|(k, _)| *k == i) {
            cols.push(scalar(&mut tx, e)?);
            continue;
        }
        let ent = ent.ok_or_else(|| format!("the record's field {f} has no row to come from"))?;
        let mut parts = f.split('.');
        let r = parts.next().unwrap_or("");
        if r != root {
            return Err(format!("the record's field {f} isn't from {root}"));
        }
        let p: Vec<String> = parts.map(String::from).collect();
        cols.push(tx.joins.column(m, "t0", ent, &p)?);
    }
    let sql = match ent {
        Some(e) => format!("SELECT {} FROM {} t0{} WHERE t0.id = ?", cols.join(", "), q(e), tx.joins.sql),
        None => format!("SELECT {}", cols.join(", ")),
    };
    Ok(Select { sql, binds: tx.binds })
}

/// A condition as one SQL expression: `SELECT <cond>` (its values from outside bound).
pub fn cond_select(m: &Model, cond: &Expr) -> Result<Select, String> {
    cond_select_with(m, cond, &[])
}

/// A condition as one SQL expression, with row parameters (name, entity) whose fields it reads.
pub fn cond_select_with(m: &Model, cond: &Expr, params: &[(String, String)]) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: params.to_vec(), rows: vec![], sub: 0 };
    let e = tx.expr(cond)?;
    Ok(Select { sql: format!("SELECT {e}"), binds: tx.binds })
}

/// A set over the rows a filter selects, as one UPDATE: `sets` are (column, value); the columns in
/// `returning` come back for each changed row (what the delta edges need).
/// `params`: the action's rows (its parameters, the viewer), whose fields a value or filter reads by id.
pub fn update(m: &Model, ent: &str, filter: Option<&Expr>, sets: &[(String, Expr)], returning: &[String], params: &[(String, String)]) -> Result<Select, String> {
    // no joins in an UPDATE: paths through references are correlated subselects (the alias isn't t0)
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: params.to_vec(), rows: vec![("u0".into(), ent.to_string())], sub: 0 };
    let mut set_sql = vec![];
    for (c, v) in sets {
        // a value may read the row's own fields (`.n + 1`); anything else is bound
        let x = tx.expr(v)?;
        set_sql.push(format!("{} = {x}", q(c)));
    }
    let w = match filter {
        Some(f) => format!(" WHERE {}", tx.expr(f)?),
        None => String::new(),
    };
    let ret = if returning.is_empty() { String::new() } else { format!(" RETURNING {}", returning.iter().map(|c| if c == "id" { "id".to_string() } else { q(c) }).collect::<Vec<_>>().join(", ")) };
    Ok(Select { sql: format!("UPDATE {} AS u0 SET {}{w}{ret}", q(ent), set_sql.join(", ")), binds: tx.binds })
}

/// `SELECT u0.id FROM E AS u0 WHERE …`: the rows a for each runs over (`rows`: parameters and
/// names that are rows, their fields read by id).
pub fn ids(m: &Model, ent: &str, filter: Option<&Expr>, rows: &[(String, String)]) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: rows.to_vec(), rows: vec![("u0".into(), ent.to_string())], sub: 0 };
    let w = match filter {
        Some(f) => format!(" WHERE {}", tx.expr(f)?),
        None => String::new(),
    };
    Ok(Select { sql: format!("SELECT u0.id FROM {} AS u0{w} ORDER BY u0.id", q(ent)), binds: tx.binds })
}

/// `DELETE FROM E AS u0 WHERE … RETURNING …`: the deleted rows' columns the delta edges need.
pub fn delete(m: &Model, ent: &str, filter: &Expr, returning: &[String]) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows: vec![("u0".into(), ent.to_string())], sub: 0 };
    let w = tx.expr(filter)?;
    let ret = returning.iter().map(|c| if c == "id" { "id".to_string() } else { q(c) }).collect::<Vec<_>>().join(", ");
    Ok(Select { sql: format!("DELETE FROM {} AS u0 WHERE {w} RETURNING {ret}", q(ent)), binds: tx.binds })
}

/// The keys of a list's rows that reach a written row through a reference: `SELECT F FROM L WHERE P = ?`.
pub fn reach(ent: &str, key_col: &str, via: &str) -> String {
    format!("SELECT {} FROM {} WHERE {} = ?", q(key_col), q(ent), q(via))
}

/// `first(query).field` as a scalar subselect.
fn scalar(tx: &mut Tx, e: &Expr) -> Result<String, String> {
    if let Expr::Field(o, f, _) = e {
        if let Expr::Call(c, a, _) = &**o {
            if c == "first" {
                let (from, w) = tx.subquery(&a[0])?;
                let (alias, ent) = tx.rows.last().cloned().unwrap();
                let col = sub_column(tx.m, &alias, &ent, &[f.clone()])?;
                tx.rows.pop();
                return Ok(format!("(SELECT {col} {from}{w} ORDER BY {alias}.created, {alias}.seq LIMIT 1)"));
            }
        }
    }
    Err("only first(…).field is a scalar for now".into())
}

/// A list expression's query and entity: a view call with its parameters replaced, or a query.
pub fn resolve(m: &Model, list: &Expr, scope: &Scope) -> Result<(Query, String), String> {
    match list {
        Expr::Call(f, args, _) => match m.views.get(f) {
            Some(Decl::View { params, query, .. }) => {
                // the view's entity, from its own declaration (the arguments may name loop
                // variables this scope doesn't know)
                let ent = m.query_entity(query, &Scope::of(params)).map_err(|e| e.msg)?;
                let env = inline::env_of(params.iter().map(|p| p.name.clone()).zip(args.iter().cloned()), 0);
                let qy = match inline::expr(m, &Expr::Query(Box::new(query.clone())), &env, 0, 0) {
                    Expr::Query(qy) => *qy,
                    _ => unreachable!(),
                };
                if let Source::Expr(inner) = &qy.source {
                    let (iq, _) = resolve(m, inner, scope)?;
                    return Ok((merge(iq, qy), ent));
                }
                Ok((qy, ent))
            }
            _ => Err(format!("`{f}` isn't a view")),
        },
        Expr::Query(qy) => {
            let qy = match inline::expr(m, list, &inline::Env::new(), 0, 0) {
                Expr::Query(x) => *x,
                _ => (**qy).clone(),
            };
            if let Source::Expr(inner) = &qy.source {
                let (iq, ent) = resolve(m, inner, scope)?;
                return Ok((merge(iq, qy), ent));
            }
            let ent = m.query_entity(&qy, scope).map_err(|e| e.msg)?;
            Ok((qy, ent))
        }
        _ => Err("a list must be a view or a query to be lowered".into()),
    }
}

/// `inner where f order by o limit n` as one query: both filters; the outer order if it has one.
fn merge(inner: Query, outer: Query) -> Query {
    let filter = match (inner.filter, outer.filter) {
        (Some(a), Some(b)) => Some(Expr::Bin(BinOp::And, Box::new(a), Box::new(b), outer.span)),
        (a, b) => a.or(b),
    };
    Query {
        source: inner.source,
        filter,
        // the outer order first; the inner one breaks ties (as a stable sort of the inner list)
        order: outer.order.into_iter().chain(inner.order).collect(),
        limit: outer.limit.or(inner.limit),
        window_last: outer.window_last.or(inner.window_last),
        span: outer.span,
    }
}

/// Whether an expression reads a query's row (`.f`, `it`).
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
