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
    out
}

fn kind_of(md: &Module, name: &str) -> Result<usize, String> {
    md.registers.iter().position(|r| r.name == name).ok_or_else(|| format!("{name} isn't a register"))
}

/// Rust for a SELECT's binds: each a screen parameter (the key) or the viewer.
fn binds_rs(sel: &sql::Select, key_param: &str) -> Result<String, String> {
    let mut v = vec![];
    for sql::Bind::Path(e) in &sel.binds {
        match e {
            Expr::Name(n, _) if n == key_param => v.push("key"),
            Expr::Name(n, _) if n == "viewer" => v.push("viewer"),
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
    let _ = writeln!(s, "    fn per_viewer(kind: usize) -> bool {{ matches!(kind, {}) }}", if pv.is_empty() { "usize::MAX".into() } else { pv.join(" | ") });

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
    let _ = writeln!(s, "    fn route(path: &[u8]) -> Option<(usize, i64)> {{");
    for (si, d) in screen_decls.iter().enumerate() {
        if let Decl::Screen { route, params, .. } = d {
            if params.len() != 1 {
                return Err("a screen has one row parameter (for now)".into());
            }
            let ph = format!("{{{}}}", params[0].name);
            let (pre, post) = route.split_once(&ph).ok_or("the route doesn't name its parameter")?;
            let _ = writeln!(s, "        if let Some(r) = path.strip_prefix(b{pre:?}).and_then(|r| r.strip_suffix(b{post:?})) {{\n            if !r.is_empty() && r.len() <= 18 && r.iter().all(|c| c.is_ascii_digit()) {{\n                return Some(({si}, std::str::from_utf8(r).ok()?.parse().ok()?));\n            }}\n        }}");
        }
    }
    let _ = writeln!(s, "        None\n    }}");

    // sessions
    let _ = writeln!(s, "    fn sessions(c: &rusqlite::Connection) -> Vec<(String, i64)> {{\n        let mut st = c.prepare_cached(r##\"SELECT \"{}\", \"{}\" FROM \"{}\" WHERE \"{}\" IS NOT NULL\"##).expect(\"sessions\");\n        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).expect(\"sessions\").flatten().collect()\n    }}", viewer.3, viewer.4, viewer.2, viewer.3);

    // register dispatch
    let _ = writeln!(s, "    fn rec_size(kind: usize) -> usize {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => std::mem::size_of::<In_{}>(),", r.name);
    }
    let _ = writeln!(s, "        _ => 0 }} }}");
    let _ = writeln!(s, "    fn tmps(kind: usize) -> usize {{ match kind {{ {}_ => 0 }} }}", md.registers.iter().enumerate().map(|(i, r)| format!("{i} => {}, ", r.tmps)).collect::<String>());
    let _ = writeln!(s, "    unsafe fn load(kind: usize, ctx: &etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, key: i64, rec: *mut u8, arena: *mut etch_rt::abi::Arena) -> bool {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => etch_load_{}(ctx, db, key, rec as *mut In_{}, arena),", r.name, r.name);
    }
    let _ = writeln!(s, "        _ => false }} }}");
    let _ = writeln!(s, "    unsafe fn bits(kind: usize, rec: *const u8, viewer: *const u8) -> i64 {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => etch_bits_{}(rec as *const In_{}, viewer as *const Viewer),", r.name, r.name);
    }
    let _ = writeln!(s, "        _ => 0 }} }}");
    let _ = writeln!(s, "    unsafe fn build(kind: usize, ctx: &etch_rt::abi::Ctx, rec: *const u8, viewer: *const u8, out: &mut Vec<u8>, tmps: &[*mut Vec<u8>], splits: &mut Vec<usize>) {{ match kind {{");
    for (i, r) in md.registers.iter().enumerate() {
        let _ = writeln!(s, "        {i} => etch_build_{}(ctx, rec as *const In_{}, viewer as *const Viewer, out, tmps.as_ptr(), splits),", r.name, r.name);
    }
    let _ = writeln!(s, "        _ => {{}} }} }}");
    let _ = writeln!(s, "    fn viewer_size() -> usize {{ std::mem::size_of::<Viewer>() }}");
    let _ = writeln!(s, "    unsafe fn load_viewer(ctx: &etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, key: i64, rec: *mut u8, arena: *mut etch_rt::abi::Arena) -> bool {{ etch_load_viewer(ctx, db, key, rec as *mut Viewer, arena) }}");
    let _ = writeln!(s, "    unsafe fn patch(reg: i64, list: i64, field: i64, rec: *mut u8, item: i64, value: i64) -> bool {{ patch(reg, list, field, rec, item, value) }}");
    let _ = writeln!(s, "    unsafe fn find(reg: i64, list: i64, field: i64, rec: *mut u8, row: i64) -> bool {{ find(reg, list, field, rec, row) }}");

    // screens
    let mut screen_fn = String::new();
    let mut regs_fn = String::new();
    let mut lists_fn = String::new();
    let mut vregs_fn = String::new();
    let mut asm_fn = String::new();
    let mut refuse_fn = String::new();
    let mut consts: Vec<String> = vec![];
    for (si, d) in screen_decls.iter().enumerate() {
        let (params, requires, cookies, body) = match d {
            Decl::Screen { params, requires, cookies, body, .. } => (params, requires, cookies, body),
            _ => continue,
        };
        let p = &params[0].name;
        let pent = match params[0].ty.strip_opt() {
            Type::Ref(e) => e.clone(),
            _ => return Err("a screen's parameter is a row".into()),
        };
        let pmap = vec![(p.clone(), pent.clone())];
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
        let (asql, abinds) = audience.ok_or("a precomputed screen needs a guard naming its audience (member(viewer, …))")?;
        let mut headers = String::from("Content-Type: text/html; charset=utf-8\\r\\n");
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
        let _ = write!(screen_fn, "            let mut st = c.prepare_cached(r##\"{asql}\"##).ok()?;\n            let audience: Vec<i64> = st.query_map({abinds}, |r| r.get(0)).ok()?.flatten().collect();\n            Some((audience, format!(\"{headers}\").into_bytes()))\n        }}\n");
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
        let mut vregs = vec![];
        let mut lists_rs = String::new();
        let mut asm = String::new();
        for n in &body.nodes {
            match n {
                TNode::Text(t) => {
                    consts.push(t.clone());
                    let _ = writeln!(asm, "                push(&consts()[{}]);", consts.len() - 1);
                }
                TNode::Hole(Expr::Call(f, args, _)) if m.fragments.contains_key(f) => {
                    let k = kind_of(md, f)?;
                    let reg = &md.registers[k];
                    if reg.per_viewer {
                        vregs.push(k);
                        let splice_kinds: Vec<usize> = reg.splices.iter().map(|n| kind_of(md, n)).collect::<Result<_, _>>()?;
                        for sk in &splice_kinds {
                            regs.push(format!("({sk}, key)"));
                        }
                        let _ = writeln!(asm, "                let Some(r) = snap.reg({k}, viewer) else {{ return false }};");
                        let _ = writeln!(asm, "                for (i, seg) in r.variants.iter().enumerate() {{\n                    if i > 0 {{\n                        let site = r.by_place[i - 1] as usize;\n                        let sk = [{}][site];\n                        let Some(sp) = snap.reg(sk, key) else {{ return false }};\n                        push(&sp.variants[0]);\n                    }}\n                    push(seg);\n                }}", splice_kinds.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "));
                    } else if args.is_empty() {
                        regs.push(format!("({k}, 0)"));
                        let _ = writeln!(asm, "                let Some(r) = snap.reg({k}, 0) else {{ return false }};\n                push(r.for_place(place));");
                    } else if matches!(&args[0], Expr::Name(n, _) if n == p) {
                        regs.push(format!("({k}, key)"));
                        let _ = writeln!(asm, "                let Some(r) = snap.reg({k}, key) else {{ return false }};\n                push(r.for_place(place));");
                    } else {
                        return Err(format!("a screen calls {f} with something other than its parameter"));
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
                    let _ = writeln!(asm, "                let Some(items) = snap.lists[{li}].get(&key) else {{ return false }};\n                for id in items.iter() {{\n                    let Some(r) = snap.reg({k}, *id) else {{ return false }};\n                    push(r.for_place(place));\n                }}");
                }
                _ => return Err("a screen's template holds fragments, loops of a fragment, and text".into()),
            }
        }
        let _ = writeln!(regs_fn, "        {si} => vec![{}],", regs.join(", "));
        let _ = writeln!(vregs_fn, "        {si} => vec![{}],", vregs.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "));
        let _ = write!(lists_fn, "        {si} => {{\n            let mut out = vec![];\n{lists_rs}            out\n        }}\n");
        let _ = write!(asm_fn, "            {si} => {{\n{asm}                true\n            }}\n");
    }
    let _ = writeln!(s, "    fn screen(c: &rusqlite::Connection, s: usize, key: i64) -> Option<(Vec<i64>, Vec<u8>)> {{\n        match s {{\n{screen_fn}        _ => None,\n        }}\n    }}");
    let _ = writeln!(s, "    fn refuse(c: &rusqlite::Connection, s: usize, key: i64, viewer: i64) -> Option<String> {{\n        match s {{\n{refuse_fn}        _ => None,\n        }}\n    }}");
    let _ = writeln!(s, "    fn screen_regs(s: usize, key: i64) -> Vec<(usize, i64)> {{ let _ = key; match s {{\n{regs_fn}        _ => vec![] }} }}");
    let _ = writeln!(s, "    fn viewer_regs(s: usize) -> Vec<usize> {{ match s {{\n{vregs_fn}        _ => vec![] }} }}");
    let _ = writeln!(s, "    fn screen_lists(c: &rusqlite::Connection, s: usize, key: i64) -> Vec<(usize, Vec<i64>)> {{\n        match s {{\n{lists_fn}        _ => vec![],\n        }}\n    }}");
    let _ = writeln!(s, "    fn assemble<'a>(snap: &'a etch_rt::store::Snap, s: usize, key: i64, viewer: i64, place: usize, push: &mut dyn FnMut(&'a etch_rt::store::Seg)) -> bool {{\n        let _ = (viewer, place);\n        match s {{\n{asm_fn}            _ => false,\n        }}\n    }}");

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
    let _ = writeln!(s, "    fn after_act(_name: &str, _form: &[(String, String)]) -> String {{ \"/\".to_string() }}");
    let _ = writeln!(s, "    fn schema() -> &'static str {{ r##\"{}\"## }}", schema(m));
    let _ = writeln!(s, "}}\n");
    // the screens' constant text, made once
    let _ = writeln!(s, "fn consts() -> &'static [etch_rt::store::Seg] {{\n    static C: std::sync::OnceLock<Vec<etch_rt::store::Seg>> = std::sync::OnceLock::new();\n    C.get_or_init(|| vec![{}])\n}}", consts.iter().map(|c| format!("etch_rt::store::Seg::new({c:?}.as_bytes().to_vec())")).collect::<Vec<_>>().join(", "));
    Ok(s)
}

/// The tables an app's entities are kept in (created if missing; an existing table is left as it is).
pub fn schema(m: &Model) -> String {
    let mut s = String::new();
    let mut ents: Vec<&&Entity> = m.entities.values().collect();
    ents.sort_by(|a, b| a.name.cmp(&b.name));
    for e in ents {
        let mut cols = vec!["id INTEGER PRIMARY KEY".to_string(), "seq INTEGER NOT NULL".into(), "created INTEGER NOT NULL".into(), "updated INTEGER NOT NULL".into()];
        for f in &e.fields {
            let ty = match f.ty.strip_opt() {
                Type::Text => "TEXT",
                _ => "INTEGER",
            };
            let null = if matches!(f.ty, Type::Opt(_)) { "" } else { " NOT NULL" };
            let check = match (f.max, f.ty.strip_opt()) {
                (Some(n), Type::Text) => format!(" CHECK (length(\"{}\") <= {n})", f.name),
                _ => String::new(),
            };
            cols.push(format!("\"{}\" {ty}{null}{check}", f.name));
        }
        s.push_str(&format!("CREATE TABLE IF NOT EXISTS \"{}\" ({}) STRICT;\n", e.name, cols.join(", ")));
        for f in &e.fields {
            if matches!(f.ty.strip_opt(), Type::Ref(_)) {
                s.push_str(&format!("CREATE INDEX IF NOT EXISTS \"ix_{}_{}\" ON \"{}\" (\"{}\");\n", e.name, f.name, e.name, f.name));
            }
        }
        s.push_str(&format!("CREATE INDEX IF NOT EXISTS \"ix_{}_created\" ON \"{}\" (created, seq);\n", e.name, e.name));
    }
    s
}

fn split_and(e: &Expr, out: &mut Vec<Expr>) {
    if let Expr::Bin(BinOp::And, l, r, _) = e {
        split_and(l, out);
        split_and(r, out);
    } else {
        out.push(e.clone());
    }
}
