//! The app's glue for the runtime (`etch_rt::app::Glue`), generated as Rust: routes, screens'
//! audiences, guards and headers, the read stage's assembly in template order, and dispatch to
//! the generated LLVM functions. Everything app-specific the runtime needs comes from here.

use crate::actions::{ActInfo, ScreenList};
use crate::ast::*;
use crate::check::{Model, Scope};
use crate::inline;
use crate::llvm::{show, Module};
use crate::sql;
use std::fmt::Write;

/// The fragments that are registers, in the order the module generates them: what screens call,
/// and their lists' items.
pub fn registers_of(m: &Model) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut add = |f: &str| {
        if !out.iter().any(|x| x == f) {
            out.push(f.to_string());
        }
    };
    for d in &m.screens {
        if let Decl::Screen { body, .. } = d {
            for n in &body.nodes {
                match n {
                    TNode::Hole(Expr::Call(f, _, _)) if m.fragments.contains_key(f) => add(f),
                    TNode::For(_, _, _, b) => {
                        for x in b {
                            if let TNode::Hole(Expr::Call(f, _, _)) = x {
                                if m.fragments.contains_key(f) {
                                    add(f);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // the cached fragments a per-request fragment merges (called with a row, or nothing)
    fn walk(m: &Model, ns: &[TNode], out: &mut Vec<String>, seen: &mut Vec<String>) {
        for n in ns {
            match n {
                TNode::Hole(Expr::Call(f, _, _)) => {
                    if let Some(Decl::Fragment { params, per_viewer, body, .. }) = m.fragments.get(f) {
                        let rowish = params.len() <= 1 && params.iter().all(|p| matches!(p.ty.strip_opt(), Type::Ref(_)));
                        if !per_viewer && rowish {
                            if !out.iter().any(|x| x == f) {
                                out.push(f.clone());
                            }
                        } else if !seen.contains(f) {
                            // inlined into the per-request fragment: what it calls is merged too
                            seen.push(f.clone());
                            walk(m, &crate::inline::nodes(m, &body.nodes, &std::collections::HashMap::new()), out, seen);
                            seen.pop();
                        }
                    }
                }
                TNode::If(_, a, b) => {
                    walk(m, a, out, seen);
                    walk(m, b, out, seen);
                }
                TNode::For(_, _, _, b) => walk(m, b, out, seen),
                TNode::Text(_) | TNode::Hole(_) => {}
            }
        }
    }
    let per_request: Vec<String> = out.iter().filter(|f| matches!(m.fragments.get(*f), Some(Decl::Fragment { per_viewer: true, .. }))).cloned().collect();
    for f in per_request {
        if let Some(Decl::Fragment { body, .. }) = m.fragments.get(&f) {
            walk(m, &crate::inline::nodes(m, &body.nodes, &std::collections::HashMap::new()), &mut out, &mut vec![f.clone()]);
        }
    }
    out
}

fn kind_of(md: &Module, name: &str) -> Result<usize, String> {
    md.registers.iter().position(|r| r.name == name).ok_or_else(|| format!("{name} isn't a register"))
}

/// Rust for a SELECT's binds: each a screen parameter (the key) or the viewer.
fn binds_rs(sel: &sql::Select, key_param: &str) -> Result<String, String> {
    let mut v: Vec<String> = vec![];
    for sql::Bind::Path(e) in &sel.binds {
        match e {
            Expr::Name(n, _) if n == key_param => v.push("key".into()),
            Expr::Name(n, _) if n == "viewer" => v.push("viewer".into()),
            Expr::Call(f, a, _) if f == "cookie" => match a.first() {
                Some(Expr::Str(c, _)) => v.push(format!("cookie({c:?})")),
                _ => return Err("cookie takes a literal name".into()),
            },
            other => return Err(format!("a screen query's parameter isn't lowered yet: {}", show(other))),
        }
    }
    Ok(format!("rusqlite::params![{}]", v.join(", ")))
}

pub fn glue(m: &Model, md: &Module, screens: &[ScreenList], acts: &[ActInfo]) -> Result<String, String> {
    let mut s = String::new();
    let viewer = match m.viewer {
        Some(Decl::Viewer { entity, cookie, via_entity, via_field, to_field, .. }) => (entity.clone(), cookie.clone(), via_entity.clone(), via_field.clone(), to_field.clone()),
        _ => return Err("an app needs a viewer declaration".into()),
    };
    let screen_decls: Vec<&Decl> = m.screens.clone();
    let _ = writeln!(s, "\n// ---- the glue for the runtime (etch_rt::app::Glue)\n");
    let _ = writeln!(s, "pub const SESSION_COOKIE: &[u8] = b{:?};", viewer.1);
    let _ = writeln!(s, "pub const CONFIGS: &[&str] = &[{}];", m.app.decls.iter().filter_map(|d| if let Decl::Config { name, .. } = d { Some(format!("{name:?}")) } else { None }).collect::<Vec<_>>().join(", "));
    let _ = writeln!(s, "pub struct Spec;\n");
    let _ = writeln!(s, "unsafe impl etch_rt::app::Glue for Spec {{");
    let _ = writeln!(s, "    const REGS: usize = {};\n    const LISTS: usize = {};\n    const SCREENS: usize = {};", md.registers.len(), screens.len(), screen_decls.len());
    let pv: Vec<String> = md.registers.iter().enumerate().filter(|(_, r)| r.per_viewer).map(|(i, _)| i.to_string()).collect();
    let _ = writeln!(s, "    fn per_request(kind: usize) -> bool {{ matches!(kind, {}) }}", if pv.is_empty() { "usize::MAX".into() } else { pv.join(" | ") });

    // screen lists: their item kind, screen, bound
    let mut li = String::new();
    let mut ls = String::new();
    let mut lb = String::new();
    for (i, l) in screens.iter().enumerate() {
        // the screen and the item fragment of the loop showing it
        let mut found = None;
        for (si, d) in screen_decls.iter().enumerate() {
            if let Decl::Screen { body, .. } = d {
                for n in &body.nodes {
                    if let TNode::For(_, list, _, b) = n {
                        if show(list) == l.name {
                            if let Some(TNode::Hole(Expr::Call(f, _, _))) = b.iter().find(|x| matches!(x, TNode::Hole(Expr::Call(..)))) {
                                found = Some((si, f.clone()));
                            }
                        }
                    }
                }
            }
        }
        let (si, f) = found.ok_or_else(|| format!("the list {} shows no fragment", l.name))?;
        let bound = m.bound_of(&l.expr, &Scope::default()).ok_or("an unbounded screen list")?;
        let _ = write!(li, "{i} => {}, ", kind_of(md, &f)?);
        let _ = write!(ls, "{i} => {si}, ");
        let _ = write!(lb, "{i} => {bound}, ");
    }
    let _ = writeln!(s, "    fn list_item(list: usize) -> usize {{ match list {{ {li}_ => 0 }} }}");
    let _ = writeln!(s, "    fn list_screen(list: usize) -> usize {{ match list {{ {ls}_ => 0 }} }}");
    let _ = writeln!(s, "    fn list_bound(list: usize) -> usize {{ match list {{ {lb}_ => 0 }} }}");

    // routes: a literal prefix, the key's digits, a literal suffix
    let _ = writeln!(s, "    fn public_screen(path: &[u8]) -> Option<usize> {{");
    for (si, d) in screen_decls.iter().enumerate() {
        if let Decl::Screen { route, public: true, .. } = d {
            let _ = writeln!(s, "        if path == b{route:?} {{ return Some({si}); }}");
        }
    }
    let _ = writeln!(s, "        None\n    }}");
    let _ = writeln!(s, "    fn route(path: &[u8]) -> Option<(usize, i64)> {{");
    for (si, d) in screen_decls.iter().enumerate() {
        if let Decl::Screen { route, params, public: false, .. } = d {
            if params.is_empty() {
                let _ = writeln!(s, "        if path == b{route:?} {{ return Some(({si}, 0)); }}");
                continue;
            }
            if params.len() != 1 {
                return Err("a screen has one row parameter, or none (for now)".into());
            }
            let ph = format!("{{{}}}", params[0].name);
            let (pre, post) = route.split_once(&ph).ok_or("the route doesn't name its parameter")?;
            let _ = writeln!(s, "        if let Some(r) = path.strip_prefix(b{pre:?}).and_then(|r| r.strip_suffix(b{post:?})) {{\n            if !r.is_empty() && r.len() <= 18 && r.iter().all(|c| c.is_ascii_digit()) {{\n                return Some(({si}, std::str::from_utf8(r).ok()?.parse().ok()?));\n            }}\n        }}");
        }
    }
    let _ = writeln!(s, "        None\n    }}");

    // sessions
    let (vt, vu, ve) = (&viewer.3, &viewer.4, &viewer.2);
    let _ = writeln!(s, "    fn sessions(c: &rusqlite::Connection) -> Vec<(i64, String, i64)> {{\n        let mut st = c.prepare_cached(r##\"SELECT id, \"{vt}\", \"{vu}\" FROM \"{ve}\" WHERE \"{vt}\" IS NOT NULL AND \"{vu}\" IS NOT NULL\"##).expect(\"sessions\");\n        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).expect(\"sessions\").flatten().collect()\n    }}");
    let _ = writeln!(s, "    fn session_row(c: &rusqlite::Connection, id: i64) -> Option<(String, i64)> {{\n        c.prepare_cached(r##\"SELECT \"{vt}\", \"{vu}\" FROM \"{ve}\" WHERE id = ? AND \"{vt}\" IS NOT NULL AND \"{vu}\" IS NOT NULL\"##).ok()?.query_row([id], |r| Ok((r.get(0)?, r.get(1)?))).ok()\n    }}");
    let _ = writeln!(s, "    fn session(c: &rusqlite::Connection, token: &str) -> Option<i64> {{\n        c.prepare_cached(r##\"SELECT \"{vu}\" FROM \"{ve}\" WHERE \"{vt}\" = ? AND \"{vu}\" IS NOT NULL\"##).ok()?.query_row([token], |r| r.get(0)).ok()\n    }}");

    // register dispatch
    let _ = writeln!(s, "    fn rec_size(kind: usize) -> usize {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => std::mem::size_of::<In_{}>(),", r.name);
    }
    let _ = writeln!(s, "        _ => 0 }} }}");
    let _ = writeln!(s, "    fn tmps(kind: usize) -> usize {{ match kind {{ {}_ => 0 }} }}", md.registers.iter().enumerate().map(|(i, r)| format!("{i} => {}, ", r.tmps)).collect::<String>());
    let _ = writeln!(s, "    unsafe fn load(kind: usize, ctx: &etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, key: i64, viewer: i64, rec: *mut u8, arena: *mut etch_rt::abi::Arena) -> bool {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => etch_load_{}(ctx, db, key, viewer, rec as *mut In_{}, arena),", r.name, r.name);
    }
    let _ = writeln!(s, "        _ => false }} }}");

    let _ = writeln!(s, "    unsafe fn build(kind: usize, ctx: &etch_rt::abi::Ctx, rec: *const u8, viewer: *const u8, out: &mut Vec<u8>, tmps: &[*mut Vec<u8>], splits: &mut Vec<usize>) {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => etch_build_{}(ctx, rec as *const In_{}, viewer as *const Viewer, out, tmps.as_ptr(), splits),", r.name, r.name);
    }
    let _ = writeln!(s, "        _ => {{}} }} }}");
    let _ = writeln!(s, "    fn viewer_size() -> usize {{ std::mem::size_of::<Viewer>() }}");
    let _ = writeln!(s, "    unsafe fn load_viewer(ctx: &etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, key: i64, rec: *mut u8, arena: *mut etch_rt::abi::Arena) -> bool {{ etch_load_viewer(ctx, db, key, key, rec as *mut Viewer, arena) }}");


    // screens
    let mut screen_fn = String::new();
    let mut regs_fn = String::new();
    let mut lists_fn = String::new();

    let mut asm_fn = String::new();
    let mut refuse_fn = String::new();
    let mut guards_fn = String::new();
    let mut colds: Vec<String> = vec![];
    let mut consts: Vec<String> = vec![];
    for (si, d) in screen_decls.iter().enumerate() {
        let (params, requires, cookies, body, public, cold, ctype, max_age) = match d {
            Decl::Screen { params, requires, cookies, body, public, cold, ctype, max_age, .. } => (params, requires, cookies, body, *public, *cold, ctype.clone(), *max_age),
            _ => continue,
        };
        // per request (public or cold): no audience, the guards checked in order each time
        let per_request = public || cold;
        let p = &params.first().map(|x| x.name.clone()).unwrap_or_default();
        let pent = match params.first().map(|x| x.ty.strip_opt()) {
            Some(Type::Ref(e)) => e.clone(),
            None if per_request => String::new(),
            _ => return Err("a screen's parameter is a row".into()),
        };
        // the parameter and the viewer: rows whose fields a guard reads by id
        let pmap = vec![(p.clone(), pent.clone()), ("viewer".to_string(), viewer.0.clone())];
        // the guards: the audience (a viewer's membership), the write-time ones, and all of them
        // in order (for the cold path's answer)
        let mut audience = None;
        let mut guards = vec![];
        let mut all = vec![];
        for r in requires {
            let cond = inline::expr(m, &r.cond, &inline::Env::new(), 0, 0);
            let sel = sql::cond_select_with(m, &cond, &pmap)?;
            all.push((sel.sql.clone(), binds_rs(&sel, p)?, r.otherwise.clone()));
            let on_viewer = sel.binds.iter().any(|sql::Bind::Path(e)| matches!(e, Expr::Name(n, _) if n == "viewer"));
            if per_request {
                continue;
            }
            if !on_viewer {
                guards.push((sel.sql.clone(), binds_rs(&sel, p)?));
                continue;
            }
            // `exists(E where .F == viewer and …)`: the audience is F over the other conjuncts
            if let Expr::Call(f, a, _) = &cond {
                if f == "exists" {
                    if let Some(Expr::Query(q)) = a.first() {
                        let mut conj = vec![];
                        split_and(q.filter.as_ref().ok_or("an audience guard needs a filter")?, &mut conj);
                        let vi = conj.iter().position(|c| matches!(c, Expr::Bin(BinOp::Eq, l, r, _) if matches!(&**r, Expr::Name(n, _) if n == "viewer") && matches!(&**l, Expr::Dot(..))));
                        if let Some(vi) = vi {
                            let col = match &conj[vi] {
                                Expr::Bin(_, l, _, _) => match &**l {
                                    Expr::Dot(c, _) => c.clone(),
                                    _ => unreachable!(),
                                },
                                _ => unreachable!(),
                            };
                            conj.remove(vi);
                            let filter = conj.into_iter().reduce(|x, y| Expr::Bin(BinOp::And, Box::new(x), Box::new(y), q.span));
                            let aq = Query { source: q.source.clone(), filter, order: vec![(Expr::Dot(col.clone(), q.span), false)], limit: None, window_last: None, span: q.span };
                            let sel = sql::list_select(m, &Expr::Query(Box::new(aq)), &[format!("it.{col}")], &Scope::of(params))?;
                            audience = Some((sel.sql.clone(), binds_rs(&sel, p)?));
                            continue;
                        }
                    }
                }
            }
            return Err("a screen's guard on the viewer must be `exists(E where .f == viewer and …)` (its audience)".into());
        }
        let (asql, abinds) = match audience {
            Some(a) => a,
            None if per_request => (String::new(), String::new()),
            None => return Err("a precomputed screen needs a guard naming its audience (member(viewer, …))".into()),
        };
        let mut headers = format!("Content-Type: {}\\r\\n", ctype.unwrap_or_else(|| "text/html; charset=utf-8".into()));
        if let Some(n) = max_age {
            headers.push_str(&format!("Cache-Control: private, max-age={n}\\r\\n"));
        }
        let mut cookie_fmt = vec![];
        for (n, e) in cookies {
            // text(the parameter) or text(the parameter.id): the key
            let names_key = |x: &Expr| match x {
                Expr::Name(n, _) => n == p,
                Expr::Field(o, f, _) => f == "id" && matches!(&**o, Expr::Name(n, _) if n == p),
                _ => false,
            };
            match e {
                Expr::Call(f, a, _) if f == "text" && a.first().map(names_key).unwrap_or(false) => {
                    headers.push_str(&format!("Set-Cookie: {n}={{key}}; Path=/; Max-Age=31536000\\r\\n"));
                    cookie_fmt.push(());
                }
                Expr::Str(v, _) => headers.push_str(&format!("Set-Cookie: {n}={v}; Path=/; Max-Age=31536000\\r\\n")),
                _ => return Err(format!("cookie {n}: only text(the parameter) or a constant for now")),
            }
        }
        headers.push_str("Vary: Accept-Encoding\\r\\n");
        let _ = write!(screen_fn, "        {si} => {{\n");
        for (sql_text, b) in &guards {
            let _ = write!(screen_fn, "            let ok: i64 = c.prepare_cached(r##\"{sql_text}\"##).ok()?.query_row({b}, |r| r.get(0)).ok()?;\n            if ok != 1 {{ return None; }}\n");
        }
        if per_request {
            let _ = write!(screen_fn, "            let _ = c;\n            Some((vec![], format!(\"{headers}\").into_bytes()))\n        }}\n");
        } else {
            let _ = write!(screen_fn, "            let mut st = c.prepare_cached(r##\"{asql}\"##).ok()?;\n            let audience: Vec<i64> = st.query_map({abinds}, |r| r.get(0)).ok()?.flatten().collect();\n            Some((audience, format!(\"{headers}\").into_bytes()))\n        }}\n");
        }
        let _ = write!(guards_fn, "        {si} => {{\n");
        for (sql_text, b, otherwise) in &all {
            let ans = match otherwise {
                Else::Go(to) => format!("Some({to:?}.to_string())"),
                _ => "None".into(),
            };
            let _ = write!(guards_fn, "            let ok: i64 = c.prepare_cached(r##\"{sql_text}\"##).ok().and_then(|mut st| st.query_row({b}, |r| r.get(0)).ok()).unwrap_or(0);\n            if ok != 1 {{ return Err({ans}); }}\n");
        }
        let _ = write!(guards_fn, "            Ok(())\n        }}\n");
        if cold {
            colds.push(si.to_string());
        }
        let _ = write!(refuse_fn, "        {si} => {{\n");
        for (sql_text, b, otherwise) in &all {
            let ans = match otherwise {
                Else::Go(to) => format!("Some({to:?}.to_string())"),
                _ => "None".into(),
            };
            let _ = write!(refuse_fn, "            let ok: i64 = c.prepare_cached(r##\"{sql_text}\"##).ok().and_then(|mut st| st.query_row({b}, |r| r.get(0)).ok()).unwrap_or(0);\n            if ok != 1 {{ return {ans}; }}\n");
        }
        let _ = write!(refuse_fn, "            None\n        }}\n");

        // the read stage, in template order
        let mut regs = vec![];
        let mut lists_rs = String::new();
        let mut asm = String::new();
        for n in &body.nodes {
            match n {
                TNode::Text(t) => {
                    consts.push(t.clone());
                    let _ = writeln!(asm, "                emit(etch_rt::store::Part::Seg(&consts()[{}]));", consts.len() - 1);
                }
                TNode::Hole(Expr::Call(f, args, _)) if m.fragments.contains_key(f) => {
                    let k = kind_of(md, f)?;
                    let reg = &md.registers[k];
                    let at = if args.is_empty() {
                        "0"
                    } else if matches!(&args[0], Expr::Name(n, _) if n == p) {
                        "key"
                    } else {
                        return Err(format!("a screen calls {f} with something other than its parameter"));
                    };
                    if reg.per_viewer {
                        // rendered for the viewer now (it merges the cached pieces it calls)
                        let _ = writeln!(asm, "                emit(etch_rt::store::Part::Render({k}, {at}));");
                    } else {
                        regs.push(format!("({k}, {at})"));
                        let _ = writeln!(asm, "                let Some(r) = snap.reg({k}, {at}) else {{ return false }};\n                emit(etch_rt::store::Part::Seg(&r.seg));");
                    }
                }
                TNode::For(var, list, _, b) => {
                    let li = screens.iter().position(|l| l.name == show(list)).ok_or("an unknown screen list")?;
                    let k = match b.iter().find_map(|x| if let TNode::Hole(Expr::Call(f, _, _)) = x { Some(f.clone()) } else { None }) {
                        Some(f) => kind_of(md, &f)?,
                        None => return Err("a screen's loop shows one fragment".into()),
                    };
                    let sel = sql::list_select(m, list, &[var.clone()], &Scope::of(params))?;
                    let _ = write!(lists_rs, "            {{ let mut st = c.prepare_cached(r##\"{}\"##).expect(\"a list\"); out.push(({li}, st.query_map({}, |r| r.get(0)).expect(\"a list\").flatten().collect())); }}\n", sel.sql, binds_rs(&sel, p)?);
                    if md.registers[k].per_viewer {
                        return Err(format!("a screen's list shows cached fragments: {} is rendered per request", md.registers[k].name));
                    }
                    let _ = writeln!(asm, "                let Some(items) = snap.lists[{li}].get(&key) else {{ return false }};\n                if let Some(b) = snap.block({li}, key, {k}) {{\n                    emit(etch_rt::store::Part::Seg(b));\n                }} else {{\n                    for id in items.iter() {{\n                        let Some(r) = snap.reg({k}, *id) else {{ return false }};\n                        emit(etch_rt::store::Part::Seg(&r.seg));\n                    }}\n                }}");
                }
                _ => return Err("a screen's template holds fragments, loops of a fragment, and text".into()),
            }
        }
        let _ = writeln!(regs_fn, "        {si} => vec![{}],", regs.join(", "));

        let _ = write!(lists_fn, "        {si} => {{\n            let mut out = vec![];\n{lists_rs}            out\n        }}\n");
        let _ = write!(asm_fn, "            {si} => {{\n{asm}                true\n            }}\n");
    }
    let _ = writeln!(s, "    fn screen(c: &rusqlite::Connection, s: usize, key: i64) -> Option<(Vec<i64>, Vec<u8>)> {{\n        match s {{\n{screen_fn}        _ => None,\n        }}\n    }}");
    let _ = writeln!(s, "    fn refuse(c: &rusqlite::Connection, s: usize, key: i64, viewer: i64) -> Option<String> {{\n        match s {{\n{refuse_fn}        _ => None,\n        }}\n    }}");
    let _ = writeln!(s, "    fn guards(c: &rusqlite::Connection, s: usize, key: i64, viewer: i64) -> Result<(), Option<String>> {{\n        let _ = (c, key, viewer);\n        match s {{\n{guards_fn}        _ => Err(None),\n        }}\n    }}");
    let _ = writeln!(s, "    fn cold(s: usize) -> bool {{ matches!(s, {}) }}", if colds.is_empty() { "usize::MAX".to_string() } else { colds.join(" | ") });
    let _ = writeln!(s, "    fn screen_regs(s: usize, key: i64) -> Vec<(usize, i64)> {{ let _ = key; match s {{\n{regs_fn}        _ => vec![] }} }}");

    let _ = writeln!(s, "    fn screen_lists(c: &rusqlite::Connection, s: usize, key: i64) -> Vec<(usize, Vec<i64>)> {{\n        match s {{\n{lists_fn}        _ => vec![],\n        }}\n    }}");
    let _ = writeln!(s, "    fn assemble<'a>(snap: &'a etch_rt::store::Snap, s: usize, key: i64, emit: &mut dyn FnMut(etch_rt::store::Part<'a>)) -> bool {{\n        let _ = (snap, key);\n        match s {{\n{asm_fn}            _ => false,\n        }}\n    }}");

    // actions
    let _ = writeln!(s, "    fn act(name: &str, c: &rusqlite::Connection, ctx: &etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, w: &mut etch_rt::abi::Writer, viewer: i64, vrec: *const u8, form: &[(String, String)], d: &mut etch_rt::abi::Deltas) -> Result<(), &'static str> {{\n        let _ = c;\n        match name {{");
    for a in acts {
        let _ = writeln!(s, "            {:?} => {{", a.name);
        let mut fields = vec![];
        for (n, sh) in &a.args {
            match sh {
                crate::llvm::Shape::I64 => {
                    let _ = writeln!(s, "                let {n}: i64 = etch_rt::app::field(form, {n:?}).and_then(|v| v.parse().ok()).ok_or(\"invalid\")?;");
                    fields.push(n.clone());
                }
                crate::llvm::Shape::Str if a.list_args.contains(n) => {
                    let _ = writeln!(s, "                let {n}_s = etch_rt::app::field_ids(form, {n:?});");
                    fields.push(format!("{n}: Str::of(&{n}_s)"));
                }
                crate::llvm::Shape::Str => {
                    let _ = writeln!(s, "                let {n}_s = etch_rt::app::field(form, {n:?}).unwrap_or(\"\");");
                    fields.push(format!("{n}: Str::of({n}_s)"));
                }
            }
        }
        let _ = writeln!(s, "                let args = Args_{} {{ {} }};", a.name, fields.join(", "));
        let _ = writeln!(s, "                let r = unsafe {{ etch_act_{}(ctx, db, w, viewer, vrec as *const Viewer, &args, d) }};\n                if r == 0 {{ Ok(()) }} else {{ Err(OUTCOMES_{}[r as usize]) }}\n            }}", a.name, a.name.to_uppercase());
    }
    let _ = writeln!(s, "            _ => Err(\"not_found\"),\n        }}\n    }}");
    let publics: Vec<String> = acts.iter().filter(|a| a.public).map(|a| format!("{:?}", a.name)).collect();
    let _ = writeln!(s, "    fn public_act(name: &str) -> bool {{ matches!(name, {}) }}", if publics.is_empty() { "\"\"".to_string() } else { publics.join(" | ") });
    // each authenticates: the hash read and the text verified on the request's thread; the hash
    // that passed (or nothing) handed to the writer as a hidden argument
    let _ = writeln!(s, "    fn preflight(name: &str, c: &rusqlite::Connection, viewer: i64, form: &mut Vec<(String, String)>) {{\n        let _ = (c, viewer, &form);\n        match name {{");
    for a in acts.iter().filter(|a| !a.verifies.is_empty()) {
        let _ = writeln!(s, "            {:?} => {{", a.name);
        let rows: Vec<(String, String)> = m.actions.iter().find_map(|d| match d {
            Decl::Action { name, params, .. } if *name == a.name => Some(params.iter().filter_map(|p| if let Type::Ref(e) = p.ty.strip_opt() { Some((p.name.clone(), e.clone())) } else { None }).collect()),
            _ => None,
        }).unwrap_or_default();
        for (i, h, text) in &a.verifies {
            let sel = sql::cond_select_with(m, h, &rows)?;
            let mut binds = vec![];
            for sql::Bind::Path(e) in &sel.binds {
                match e {
                    Expr::Name(n, _) if n == "viewer" => binds.push("viewer.to_string()".to_string()),
                    Expr::Name(n, _) if a.args.iter().any(|(x, _)| x == n) => binds.push(format!("etch_rt::app::field(form, {n:?}).unwrap_or(\"\").to_string()")),
                    other => return Err(format!("authenticates reads {} (only arguments and the viewer)", show(other))),
                }
            }
            let _ = writeln!(s, "                let hash: Option<String> = c.prepare_cached(r##\"{}\"##).ok().and_then(|mut st| st.query_row(rusqlite::params![{}], |r| r.get::<_, Option<String>>(0)).ok()).flatten();", sel.sql, binds.join(", "));
            let _ = writeln!(s, "                let ok = etch_rt::pass::verify(hash.as_deref(), etch_rt::app::field(form, {text:?}).unwrap_or(\"\"));");
            let _ = writeln!(s, "                form.push((\"__verified_{i}\".to_string(), if ok {{ hash.unwrap_or_default() }} else {{ String::new() }}));");
        }
        let _ = writeln!(s, "            }}");
    }
    let _ = writeln!(s, "            _ => {{}}\n        }}");
    // each hash(text): hashed here (slow by design), off the writer
    let _ = writeln!(s, "        match name {{");
    for a in acts.iter().filter(|a| !a.hashes.is_empty()) {
        let _ = writeln!(s, "            {:?} => {{", a.name);
        for (i, arg) in &a.hashes {
            let _ = writeln!(s, "                let t = etch_rt::app::field(form, {arg:?}).unwrap_or(\"\").to_string();\n                form.push((\"__hashed_{i}\".to_string(), if t.is_empty() {{ String::new() }} else {{ etch_rt::pass::hash(&t) }}));");
        }
        let _ = writeln!(s, "            }}");
    }
    let _ = writeln!(s, "            _ => {{}}\n        }}\n    }}");
    // where "/" goes
    match m.home {
        Some(Decl::Home { screen, arg, .. }) => {
            let route = screen_decls.iter().find_map(|d| match d {
                Decl::Screen { name, route, params, .. } if name == screen => Some(route.replace(&format!("{{{}}}", params[0].name), "{}")),
                _ => None,
            }).ok_or("home's screen")?;
            let x = inline::expr(m, arg, &inline::Env::new(), 0, 0);
            let sel = sql::cond_select_with(m, &x, &[])?;
            let _ = writeln!(s, "    fn home(c: &rusqlite::Connection, viewer: i64, cookie: &dyn Fn(&str) -> Option<i64>) -> Option<String> {{\n        let id: Option<i64> = c.prepare_cached(r##\"{}\"##).ok()?.query_row({}, |r| r.get(0)).ok()?;\n        id.map(|id| format!({route:?}, id))\n    }}", sel.sql, binds_rs(&sel, "")?);
        }
        _ => {
            let _ = writeln!(s, "    fn home(_c: &rusqlite::Connection, _viewer: i64, _cookie: &dyn Fn(&str) -> Option<i64>) -> Option<String> {{ None }}");
        }
    }
    let _ = writeln!(s, "    fn after_act(_name: &str, _form: &[(String, String)]) -> String {{ \"/\".to_string() }}");
    let (schema_sql, adds) = schema_and_columns(m);
    let _ = writeln!(s, "    fn schema() -> &'static str {{ r##\"{}\"## }}", schema_sql);
    let _ = writeln!(s, "    fn columns() -> &'static [(&'static str, &'static str, &'static str)] {{ &[{}] }}", adds.join(", "));
    let _ = writeln!(s, "}}\n");
    // the screens' constant text, made once
    let _ = writeln!(s, "fn consts() -> &'static [etch_rt::store::Seg] {{\n    static C: std::sync::OnceLock<Vec<etch_rt::store::Seg>> = std::sync::OnceLock::new();\n    C.get_or_init(|| vec![{}])\n}}", consts.iter().map(|c| format!("etch_rt::store::Seg::new({c:?}.as_bytes().to_vec())")).collect::<Vec<_>>().join(", "));
    Ok(s)
}

/// The tables an app's entities are kept in (created if missing; an existing table is left as it is).
/// The schema, and each field's column as (table, column, the ALTER that adds it): a database
/// made before a field was added gets its column when the app starts.
pub fn schema_and_columns(m: &Model) -> (String, Vec<String>) {
    let mut s = String::new();
    let mut adds = vec![];
    let mut ents: Vec<&&Entity> = m.entities.values().collect();
    ents.sort_by(|a, b| a.name.cmp(&b.name));
    for e in ents {
        let mut cols = vec!["id INTEGER PRIMARY KEY".to_string(), "seq INTEGER NOT NULL".into(), "created INTEGER NOT NULL".into(), "updated INTEGER NOT NULL".into()];
        for f in &e.fields {
            let ty = match f.ty.strip_opt() {
                Type::Text | Type::Password => "TEXT",
                _ => "INTEGER",
            };
            // a number or flag starts at 0 (false) when a row is made without it, or a column added
            let null = match f.ty {
                Type::Opt(_) => "",
                Type::Int | Type::Bool | Type::Duration => " NOT NULL DEFAULT 0",
                _ => " NOT NULL",
            };
            let check = match (f.max, f.ty.strip_opt()) {
                (Some(n), Type::Text) => format!(" CHECK (length(\"{}\") <= {n})", f.name),
                _ => String::new(),
            };
            cols.push(format!("\"{}\" {ty}{null}{check}", f.name));
            adds.push(format!("(\"{}\", \"{}\", r##\"ALTER TABLE \"{}\" ADD COLUMN \"{}\" {ty}{null}{check}\"##)", e.name, f.name, e.name, f.name));
        }
        s.push_str(&format!("CREATE TABLE IF NOT EXISTS \"{}\" ({}) STRICT;\n", e.name, cols.join(", ")));
        for f in &e.fields {
            if matches!(f.ty.strip_opt(), Type::Ref(_)) {
                s.push_str(&format!("CREATE INDEX IF NOT EXISTS \"ix_{}_{}\" ON \"{}\" (\"{}\");\n", e.name, f.name, e.name, f.name));
            }
        }
        s.push_str(&format!("CREATE INDEX IF NOT EXISTS \"ix_{}_created\" ON \"{}\" (created, seq);\n", e.name, e.name));
    }
    (s, adds)
}

fn split_and(e: &Expr, out: &mut Vec<Expr>) {
    if let Expr::Bin(BinOp::And, l, r, _) = e {
        split_and(l, out);
        split_and(r, out);
    } else {
        out.push(e.clone());
    }
}
