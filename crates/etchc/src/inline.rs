//! Inlining: every call to a `fn` is replaced by its body, its parameters replaced by the
//! arguments (functions can't recurse: the checker refuses a cycle). What's left calls only
//! builtins, views (list sources) and fragments.

use crate::ast::*;
use crate::check::Model;
use std::collections::HashMap;

pub fn expr(m: &Model, e: &Expr, env: &HashMap<String, Expr>, depth: usize) -> Expr {
    assert!(depth < 64, "a function calls itself: functions can't recurse");
    let go = |x: &Expr| expr(m, x, env, depth);
    match e {
        Expr::Name(n, _) => env.get(n).cloned().unwrap_or_else(|| e.clone()),
        Expr::Str(..) | Expr::Int(..) | Expr::Bool(..) | Expr::Dur(..) | Expr::None(_) | Expr::Dot(..) => e.clone(),
        Expr::Field(o, f, s) => Expr::Field(Box::new(go(o)), f.clone(), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(go(x)), *s),
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(go(l)), Box::new(go(r)), *s),
        Expr::If(c, a, b, s) => Expr::If(Box::new(go(c)), Box::new(go(a)), Box::new(go(b)), *s),
        Expr::List(items, s) => Expr::List(items.iter().map(go).collect(), *s),
        Expr::Record(fs, s) => Expr::Record(fs.iter().map(|(n, v)| (n.clone(), go(v))).collect(), *s),
        Expr::Query(q) => Expr::Query(Box::new(query(m, q, env, depth))),
        Expr::Comp(f, x, q, s) => Expr::Comp(f.clone(), Box::new(go(x)), Box::new(query(m, q, env, depth)), *s),
        Expr::Call(f, args, s) => {
            let args: Vec<Expr> = args.iter().map(go).collect();
            if let Some(Decl::Fn { params, body, .. }) = m.fns.get(f) {
                let inner: HashMap<String, Expr> = params.iter().map(|p| p.name.clone()).zip(args).collect();
                expr(m, body, &inner, depth + 1)
            } else {
                Expr::Call(f.clone(), args, *s)
            }
        }
    }
}

fn query(m: &Model, q: &Query, env: &HashMap<String, Expr>, depth: usize) -> Query {
    // inside a query `it` and `.f` are the query's own row: names bound outside still substitute
    let mut env = env.clone();
    env.remove("it");
    Query {
        source: match &q.source {
            Source::Entity(e) => Source::Entity(e.clone()),
            Source::Expr(e) => Source::Expr(expr(m, e, &env, depth)),
        },
        filter: q.filter.as_ref().map(|f| expr(m, f, &env, depth)),
        order: q.order.iter().map(|(k, d)| (expr(m, k, &env, depth), *d)).collect(),
        limit: q.limit.as_ref().map(|x| expr(m, x, &env, depth)),
        window_last: q.window_last.as_ref().map(|x| expr(m, x, &env, depth)),
        span: q.span,
    }
}

pub fn nodes(m: &Model, ns: &[TNode], env: &HashMap<String, Expr>) -> Vec<TNode> {
    ns.iter()
        .map(|n| match n {
            TNode::Text(t) => TNode::Text(t.clone()),
            TNode::Hole(e) => TNode::Hole(expr(m, e, env, 0)),
            TNode::If(c, a, b) => TNode::If(expr(m, c, env, 0), nodes(m, a, env), nodes(m, b, env)),
            TNode::For(v, l, lim, body) => {
                let mut inner = env.clone();
                inner.remove(v);
                TNode::For(v.clone(), expr(m, l, env, 0), lim.clone(), nodes(m, body, &inner))
            }
        })
        .collect()
}
