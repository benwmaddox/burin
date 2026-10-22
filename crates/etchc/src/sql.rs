//! Queries to SQL: each register's record and lists are filled by one SELECT each, selecting
//! exactly the record's fields (references followed by LEFT JOINs), with the query's filter,
//! order and bound translated. Values from outside the query (a parent record's field, the
//! viewer) are bound parameters.

use crate::ast::*;
use crate::check::{Model, Scope};

/// A value bound into a query: a path from a root outside it.
#[derive(Clone, Debug)]
pub enum Bind {
    /// a root's path (`m`, `ms.room`), read from its record
    Path(Expr),
    /// a constant
    Int(i64),
    Text(String),
}

pub struct Select {
    pub sql: String,
    pub binds: Vec<Bind>,
}

struct Joins {
    /// (path of reference fields from the row, its alias, its entity)
    list: Vec<(Vec<String>, String, String)>,
    sql: String,
    next: usize,
}

fn q(name: &str) -> String {
    format!("\"{name}\"")
}

impl Joins {
    /// The SQL column for a path of fields from the alias `base` (an entity), joining through
    /// references as needed. A reference's own value is its id column.
    fn column(&mut self, m: &Model, base: &str, base_ent: &str, path: &[String]) -> Result<String, String> {
        if path.is_empty() {
            return Ok(format!("{base}.id"));
        }
        let (mut alias, mut ent) = (base.to_string(), base_ent.to_string());
        for i in 0..path.len() - 1 {
            let f = &path[i];
            let t = m.field_type(&ent, f).ok_or_else(|| format!("{ent} has no field {f}"))?;
            let target = match t.strip_opt() {
                Type::Ref(e) => e.clone(),
                _ => return Err(format!("{ent}.{f} isn't a reference")),
            };
            let key: Vec<String> = std::iter::once(base.to_string()).chain(path[..=i].iter().cloned()).collect();
            let a = match self.list.iter().find(|(k, _, _)| *k == key) {
                Some((_, a, _)) => a.clone(),
                None => {
                    self.next += 1;
                    let a = format!("j{}", self.next);
                    self.sql.push_str(&format!(" LEFT JOIN {} {a} ON {a}.id = {alias}.{}", q(&target), q(f)));
                    self.list.push((key, a.clone(), target.clone()));
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

/// The path of an expression relative to the query's row (`.a.b`, `it.a`), if it is one.
fn row_path(e: &Expr) -> Option<Vec<String>> {
    match e {
        Expr::Dot(f, _) => Some(if f == "id" { vec![] } else { vec![f.clone()] }),
        Expr::Name(n, _) if n == "it" => Some(vec![]),
        Expr::Field(o, f, _) => {
            let mut p = row_path(o)?;
            if f != "id" {
                p.push(f.clone());
            }
            Some(p)
        }
        _ => None,
    }
}

struct Tx<'a> {
    m: &'a Model<'a>,
    joins: Joins,
    binds: Vec<Bind>,
    /// subqueries' aliases, innermost last: (alias, entity)
    rows: Vec<(String, String)>,
    sub: usize,
}

impl<'a> Tx<'a> {
    fn expr(&mut self, e: &Expr) -> Result<String, String> {
        if let Some(p) = row_path(e) {
            let (a, ent) = self.rows.last().cloned().unwrap();
            if self.rows.len() == 1 {
                return self.joins.column(self.m, &a, &ent, &p);
            }
            // inside a subquery: joins go into the subquery's own FROM; keep it simple: only
            // direct columns and one hop through a reference, as correlated subselects
            return sub_column(self.m, &a, &ent, &p);
        }
        Ok(match e {
            Expr::Str(s, _) => format!("'{}'", s.replace('\'', "''")),
            Expr::Int(n, _) => n.to_string(),
            Expr::Bool(b, _) => (*b as i32).to_string(),
            Expr::None(_) => "NULL".into(),
            Expr::Name(..) | Expr::Field(..) => {
                // a value from outside the query: bound
                self.binds.push(Bind::Path(e.clone()));
                "?".into()
            }
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
                };
                format!("({a} {o} {b})")
            }
            Expr::Call(f, args, _) if f == "lower" => format!("lower({})", self.expr(&args[0])?),
            Expr::Call(f, args, _) if f == "exists" => {
                let q = match &args[0] {
                    Expr::Query(q) => q,
                    _ => return Err("exists takes a query".into()),
                };
                let ent = match &q.source {
                    Source::Entity(e) => e.clone(),
                    _ => return Err("exists over a view isn't lowered yet".into()),
                };
                self.sub += 1;
                let a = format!("s{}", self.sub);
                self.rows.push((a.clone(), ent.clone()));
                let w = match &q.filter {
                    Some(f) => format!(" WHERE {}", self.expr(f)?),
                    None => String::new(),
                };
                self.rows.pop();
                format!("EXISTS (SELECT 1 FROM {} {a}{w})", q_name(&ent))
            }
            _ => return Err(format!("this can't be part of a query's SQL yet: {e:?}").chars().take(160).collect()),
        })
    }
}

fn q_name(e: &str) -> String {
    q(e)
}

/// A path inside a subquery: a direct column, or one reference followed, as a scalar subselect.
fn sub_column(m: &Model, alias: &str, ent: &str, p: &[String]) -> Result<String, String> {
    match p.len() {
        0 => Ok(format!("{alias}.id")),
        1 => Ok(format!("{alias}.{}", q(&p[0]))),
        2 => {
            let t = m.field_type(ent, &p[0]).ok_or("no such field")?;
            let target = match t.strip_opt() {
                Type::Ref(e) => e.clone(),
                _ => return Err(format!("{ent}.{} isn't a reference", p[0])),
            };
            Ok(format!("(SELECT x.{} FROM {} x WHERE x.id = {alias}.{})", q(&p[1]), q(&target), q(&p[0])))
        }
        _ => Err("a path of more than two fields in a subquery isn't lowered yet".into()),
    }
}

/// The SELECT that fills a list: the query (a view's, its parameters bound to `args`), the items'
/// fields (paths relative to the loop variable `var`).
pub fn list_select(m: &Model, list: &Expr, var: &str, fields: &[String], scope: &Scope) -> Result<Select, String> {
    // the view's query, its parameters replaced by the call's arguments
    let (q, ent) = resolve(m, list, scope)?;
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], rows: vec![("t0".into(), ent.clone())], sub: 0 };
    let mut cols = vec![];
    for f in fields {
        let p: Vec<String> = f.split('.').skip(1).map(String::from).filter(|s| s != "id").collect();
        let _ = var;
        cols.push(tx.joins.column(m, "t0", &ent, &p)?);
    }
    let w = match &q.filter {
        Some(f) => format!(" WHERE {}", tx.expr(f)?),
        None => String::new(),
    };
    let mut order = vec![];
    for (k, desc) in &q.order {
        order.push(format!("{}{}", tx.expr(k)?, if *desc { " DESC" } else { "" }));
    }
    let lim = |e: &Option<Expr>| -> Result<Option<String>, String> {
        Ok(match e {
            Some(Expr::Int(n, _)) => Some(n.to_string()),
            Some(_) => return Err("a list's bound must be a constant to lower it (for now)".into()),
            None => None,
        })
    };
    let from = format!("FROM {} t0{}", q_name(&ent), tx.joins.sql);
    let sql = if let Some(n) = lim(&q.window_last)? {
        // the last N in the order, oldest first: the newest N, reversed
        let rev: Vec<String> = q.order.iter().zip(&order).map(|((_, d), o)| if *d { o.trim_end_matches(" DESC").to_string() } else { format!("{o} DESC") }).collect();
        let inner_cols: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c} AS c{i}")).collect();
        let keys: Vec<String> = order.iter().enumerate().map(|(i, o)| format!("{} AS k{i}", o.trim_end_matches(" DESC"))).collect();
        let outer_order: Vec<String> = q.order.iter().enumerate().map(|(i, (_, d))| format!("k{i}{}", if *d { " DESC" } else { "" })).collect();
        format!(
            "SELECT {} FROM (SELECT {}, {} {from}{w} ORDER BY {} LIMIT {n}) ORDER BY {}",
            (0..cols.len()).map(|i| format!("c{i}")).collect::<Vec<_>>().join(", "),
            inner_cols.join(", "),
            keys.join(", "),
            rev.join(", "),
            outer_order.join(", ")
        )
    } else {
        let mut s = format!("SELECT {} {from}{w}", cols.join(", "));
        if !order.is_empty() {
            s.push_str(&format!(" ORDER BY {}", order.join(", ")));
        }
        if let Some(n) = lim(&q.limit)? {
            s.push_str(&format!(" LIMIT {n}"));
        }
        s
    };
    Ok(Select { sql, binds: tx.binds })
}

/// The SELECT that fills a register's own record: its row by id, with the record's fields.
pub fn row_select(m: &Model, ent: &str, root: &str, fields: &[String]) -> Result<Select, String> {
    let mut j = Joins { list: vec![], sql: String::new(), next: 0 };
    let mut cols = vec![];
    for f in fields {
        let mut parts = f.split('.');
        let r = parts.next().unwrap_or("");
        if r != root {
            return Err(format!("the record's field {f} isn't from {root}"));
        }
        let p: Vec<String> = parts.map(String::from).collect();
        cols.push(j.column(m, "t0", ent, &p)?);
    }
    Ok(Select { sql: format!("SELECT {} FROM {} t0{} WHERE t0.id = ?", cols.join(", "), q(ent), j.sql), binds: vec![] })
}

/// A list expression's query and entity: a view call with its parameters replaced, or a query.
pub fn resolve(m: &Model, list: &Expr, scope: &Scope) -> Result<(Query, String), String> {
    match list {
        Expr::Call(f, args, _) => match m.views.get(f) {
            Some(Decl::View { params, query, .. }) => {
                let env: std::collections::HashMap<String, Expr> = params.iter().map(|p| p.name.clone()).zip(args.iter().cloned()).collect();
                let q = match crate::inline::expr(m, &Expr::Query(Box::new(query.clone())), &env, 0) {
                    Expr::Query(q) => *q,
                    _ => unreachable!(),
                };
                let ent = m.query_entity(&q, scope).map_err(|e| e.msg)?;
                // a view over another view: its filter and order are added to the inner one's
                if let Source::Expr(inner) = &q.source {
                    let (iq, _) = resolve(m, inner, scope)?;
                    return Ok((merge(iq, q), ent));
                }
                Ok((q, ent))
            }
            _ => Err(format!("`{f}` isn't a view")),
        },
        Expr::Query(q) => {
            let ent = m.query_entity(q, scope).map_err(|e| e.msg)?;
            if let Source::Expr(inner) = &q.source {
                let (iq, _) = resolve(m, inner, scope)?;
                return Ok((merge(iq, (**q).clone()), ent));
            }
            Ok(((**q).clone(), ent))
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
        order: if outer.order.is_empty() { inner.order } else { outer.order },
        limit: outer.limit.or(inner.limit),
        window_last: outer.window_last.or(inner.window_last),
        span: outer.span,
    }
}
