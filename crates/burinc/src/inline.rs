//! Inlining: every call to a `fn` is replaced by its body, its parameters replaced by the
//! arguments (functions can't recurse). What's left calls only builtins, views (list sources) and
//! fragments.
//!
//! Capture: inside a query, `it` and `.f` name the query's own row. An argument that names the
//! row of a query around the call (`in_direct_with(it, viewer)` in a filter) and lands inside a
//! query of the function's body must keep naming that outer row: it is rewritten to `^k`, the
//! row k queries out from where it lands.

use crate::ast::*;
use crate::check::Model;
use std::collections::HashMap;

/// Names bound to (their value, the query depth they were written at).
pub type Env = HashMap<String, (Expr, usize)>;

pub fn env_of(names: impl IntoIterator<Item = (String, Expr)>, qd: usize) -> Env {
    names.into_iter().map(|(n, e)| (n, (e, qd))).collect()
}

/// `e` moved `k` queries deeper: its references to rows of queries around it point `k` further out.
fn shift(e: &Expr, k: usize) -> Expr {
    if k == 0 {
        return e.clone();
    }
    let s = |x: &Expr| shift(x, k);
    match e {
        Expr::Name(n, sp) if n == "it" => Expr::Name(format!("^{k}"), *sp),
        Expr::Name(n, sp) if n.starts_with('^') => Expr::Name(format!("^{}", n[1..].parse::<usize>().unwrap_or(0) + k), *sp),
        Expr::Dot(f, sp) => Expr::Field(Box::new(Expr::Name(format!("^{k}"), *sp)), f.clone(), *sp),
        Expr::Field(o, f, sp) => Expr::Field(Box::new(s(o)), f.clone(), *sp),
        Expr::Not(x, sp) => Expr::Not(Box::new(s(x)), *sp),
        Expr::Bin(op, l, r, sp) => Expr::Bin(op.clone(), Box::new(s(l)), Box::new(s(r)), *sp),
        Expr::If(c, a, b, sp) => Expr::If(Box::new(s(c)), Box::new(s(a)), Box::new(s(b)), *sp),
        Expr::Call(f, a, sp) => Expr::Call(f.clone(), a.iter().map(s).collect(), *sp),
        Expr::List(i, sp) => Expr::List(i.iter().map(s).collect(), *sp),
        // a query inside the value has its own rows: its free references would need shifting too
        // (not needed by any spec yet)
        other => other.clone(),
    }
}

pub fn expr(m: &Model, e: &Expr, env: &Env, depth: usize, qd: usize) -> Expr {
    assert!(depth < 64, "a function calls itself: functions can't recurse");
    let go = |x: &Expr| expr(m, x, env, depth, qd);
    match e {
        Expr::Name(n, _) => match env.get(n) {
            Some((v, at)) => shift(v, qd.saturating_sub(*at)),
            None => e.clone(),
        },
        Expr::Str(..) | Expr::Int(..) | Expr::Bool(..) | Expr::Dur(..) | Expr::None(_) | Expr::Dot(..) => e.clone(),
        Expr::Field(o, f, s) => Expr::Field(Box::new(go(o)), f.clone(), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(go(x)), *s),
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(go(l)), Box::new(go(r)), *s),
        Expr::If(c, a, b, s) => Expr::If(Box::new(go(c)), Box::new(go(a)), Box::new(go(b)), *s),
        Expr::List(items, s) => Expr::List(items.iter().map(go).collect(), *s),
        Expr::Record(fs, s) => Expr::Record(fs.iter().map(|(n, v)| (n.clone(), go(v))).collect(), *s),
        Expr::Query(q) => Expr::Query(Box::new(query(m, q, env, depth, qd))),
        Expr::Comp(f, x, q, s) => {
            // the comprehension's expression ranges over the query's rows
            let mut inner = env.clone();
            inner.remove("it");
            Expr::Comp(f.clone(), Box::new(expr(m, x, &inner, depth, qd + 1)), Box::new(query(m, q, env, depth, qd)), *s)
        }
        Expr::Call(f, args, s) => {
            let args: Vec<Expr> = args.iter().map(go).collect();
            if let Some(Decl::Fn { params, body, .. }) = m.fns.get(f) {
                let inner = env_of(params.iter().map(|p| p.name.clone()).zip(args), qd);
                expr(m, body, &inner, depth + 1, qd)
            } else {
                Expr::Call(f.clone(), args, *s)
            }
        }
    }
}

fn query(m: &Model, q: &Query, env: &Env, depth: usize, qd: usize) -> Query {
    // the filter and order are inside the query (its row is `it`); the source and bounds aren't
    let mut env_in = env.clone();
    env_in.remove("it");
    Query {
        source: match &q.source {
            Source::Entity(e) => Source::Entity(e.clone()),
            Source::Expr(e) => Source::Expr(expr(m, e, env, depth, qd)),
        },
        filter: q.filter.as_ref().map(|f| expr(m, f, &env_in, depth, qd + 1)),
        order: q.order.iter().map(|(k, d)| (expr(m, k, &env_in, depth, qd + 1), *d)).collect(),
        limit: q.limit.as_ref().map(|x| expr(m, x, env, depth, qd)),
        window_last: q.window_last.as_ref().map(|x| expr(m, x, env, depth, qd)),
        span: q.span,
    }
}

pub fn nodes(m: &Model, ns: &[TNode], env: &HashMap<String, Expr>) -> Vec<TNode> {
    let env = env_of(env.iter().map(|(k, v)| (k.clone(), v.clone())), 0);
    nodes_env(m, ns, &env)
}

fn nodes_env(m: &Model, ns: &[TNode], env: &Env) -> Vec<TNode> {
    ns.iter()
        .map(|n| match n {
            TNode::Text(t) => TNode::Text(t.clone()),
            TNode::Hole(e) => TNode::Hole(expr(m, e, env, 0, 0)),
            TNode::If(c, a, b) => TNode::If(expr(m, c, env, 0, 0), nodes_env(m, a, env), nodes_env(m, b, env)),
            TNode::For(v, l, lim, body) => {
                let mut inner = env.clone();
                inner.remove(v);
                TNode::For(v.clone(), expr(m, l, env, 0, 0), lim.clone(), nodes_env(m, body, &inner))
            }
        })
        .collect()
}
