root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('glue.rs', [
    # binds: cookies too
    ('''            Expr::Name(n, _) if n == "viewer" => v.push("viewer"),
            other => return Err(format!("a screen query's parameter isn't lowered yet: {}", show(other))),
        }
    }
    Ok(format!("rusqlite::params![{}]", v.join(", ")))''', '''            Expr::Name(n, _) if n == "viewer" => v.push("viewer".into()),
            Expr::Call(f, a, _) if f == "cookie" => match a.first() {
                Some(Expr::Str(c, _)) => v.push(format!("cookie({c:?})")),
                _ => return Err("cookie takes a literal name".into()),
            },
            other => return Err(format!("a screen query's parameter isn't lowered yet: {}", show(other))),
        }
    }
    Ok(format!("rusqlite::params![{}]", v.join(", ")))'''),
    ('''    let mut v = vec![];
    for sql::Bind::Path(e) in &sel.binds {
        match e {
            Expr::Name(n, _) if n == key_param => v.push("key"),''', '''    let mut v: Vec<String> = vec![];
    for sql::Bind::Path(e) in &sel.binds {
        match e {
            Expr::Name(n, _) if n == key_param => v.push("key".into()),'''),
    # routes: public screens by their literal path; the rest by their parameter
    ('''    let _ = writeln!(s, "    fn route(path: &[u8]) -> Option<(usize, i64)> {{");
    for (si, d) in screen_decls.iter().enumerate() {
        if let Decl::Screen { route, params, .. } = d {''', '''    let _ = writeln!(s, "    fn public_screen(path: &[u8]) -> Option<usize> {{");
    for (si, d) in screen_decls.iter().enumerate() {
        if let Decl::Screen { route, public: true, .. } = d {
            let _ = writeln!(s, "        if path == b{route:?} {{ return Some({si}); }}");
        }
    }
    let _ = writeln!(s, "        None\\n    }}");
    let _ = writeln!(s, "    fn route(path: &[u8]) -> Option<(usize, i64)> {{");
    for (si, d) in screen_decls.iter().enumerate() {
        if let Decl::Screen { route, params, public: false, .. } = d {'''),
    # sessions: with their row ids; one by id; one by token
    ('''    let _ = writeln!(s, "    fn sessions(c: &rusqlite::Connection) -> Vec<(String, i64)> {{\\n        let mut st = c.prepare_cached(r##\\"SELECT \\"{}\\", \\"{}\\" FROM \\"{}\\" WHERE \\"{}\\" IS NOT NULL\\"##).expect(\\"sessions\\");\\n        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).expect(\\"sessions\\").flatten().collect()\\n    }}", viewer.3, viewer.4, viewer.2, viewer.3);''',
     '''    let (vt, vu, ve) = (&viewer.3, &viewer.4, &viewer.2);
    let _ = writeln!(s, "    fn sessions(c: &rusqlite::Connection) -> Vec<(i64, String, i64)> {{\\n        let mut st = c.prepare_cached(r##\\"SELECT id, \\"{vt}\\", \\"{vu}\\" FROM \\"{ve}\\" WHERE \\"{vt}\\" IS NOT NULL AND \\"{vu}\\" IS NOT NULL\\"##).expect(\\"sessions\\");\\n        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).expect(\\"sessions\\").flatten().collect()\\n    }}");
    let _ = writeln!(s, "    fn session_row(c: &rusqlite::Connection, id: i64) -> Option<(String, i64)> {{\\n        c.prepare_cached(r##\\"SELECT \\"{vt}\\", \\"{vu}\\" FROM \\"{ve}\\" WHERE id = ? AND \\"{vt}\\" IS NOT NULL AND \\"{vu}\\" IS NOT NULL\\"##).ok()?.query_row([id], |r| Ok((r.get(0)?, r.get(1)?))).ok()\\n    }}");
    let _ = writeln!(s, "    fn session(c: &rusqlite::Connection, token: &str) -> Option<i64> {{\\n        c.prepare_cached(r##\\"SELECT \\"{vu}\\" FROM \\"{ve}\\" WHERE \\"{vt}\\" = ? AND \\"{vu}\\" IS NOT NULL\\"##).ok()?.query_row([token], |r| r.get(0)).ok()\\n    }}");'''),
    # screens: a public one has no parameter, guards or audience
    ('''        let (params, requires, cookies, body) = match d {
            Decl::Screen { params, requires, cookies, body, .. } => (params, requires, cookies, body),
            _ => continue,
        };
        let p = &params[0].name;
        let pent = match params[0].ty.strip_opt() {
            Type::Ref(e) => e.clone(),
            _ => return Err("a screen's parameter is a row".into()),
        };''', '''        let (params, requires, cookies, body, public) = match d {
            Decl::Screen { params, requires, cookies, body, public, .. } => (params, requires, cookies, body, *public),
            _ => continue,
        };
        let p = &params.first().map(|x| x.name.clone()).unwrap_or_default();
        let pent = match params.first().map(|x| x.ty.strip_opt()) {
            Some(Type::Ref(e)) => e.clone(),
            None if public => String::new(),
            _ => return Err("a screen's parameter is a row".into()),
        };'''),
    ('''        let (asql, abinds) = audience.ok_or("a precomputed screen needs a guard naming its audience (member(viewer, …))")?;''',
     '''        let (asql, abinds) = match audience {
            Some(a) => a,
            None if public => (String::new(), String::new()),
            None => return Err("a precomputed screen needs a guard naming its audience (member(viewer, …))".into()),
        };'''),
    ('''        let _ = write!(screen_fn, "            let mut st = c.prepare_cached(r##\\"{asql}\\"##).ok()?;\\n            let audience: Vec<i64> = st.query_map({abinds}, |r| r.get(0)).ok()?.flatten().collect();\\n            Some((audience, format!(\\"{headers}\\").into_bytes()))\\n        }}\\n");''',
     '''        if public {
            let _ = write!(screen_fn, "            let _ = c;\\n            Some((vec![], format!(\\"{headers}\\").into_bytes()))\\n        }}\\n");
        } else {
            let _ = write!(screen_fn, "            let mut st = c.prepare_cached(r##\\"{asql}\\"##).ok()?;\\n            let audience: Vec<i64> = st.query_map({abinds}, |r| r.get(0)).ok()?.flatten().collect();\\n            Some((audience, format!(\\"{headers}\\").into_bytes()))\\n        }}\\n");
        }'''),
    # actions: public ones, and the password preflight
    ('''    let _ = writeln!(s, "            _ => Err(\\"not_found\\"),\\n        }}\\n    }}");''', '''    let _ = writeln!(s, "            _ => Err(\\"not_found\\"),\\n        }}\\n    }}");
    let publics: Vec<String> = acts.iter().filter(|a| a.public).map(|a| format!("{:?}", a.name)).collect();
    let _ = writeln!(s, "    fn public_act(name: &str) -> bool {{ matches!(name, {}) }}", if publics.is_empty() { "\\"\\"".to_string() } else { publics.join(" | ") });
    // each authenticates: the hash read and the text verified on the request's thread; the hash
    // that passed (or nothing) handed to the writer as a hidden argument
    let _ = writeln!(s, "    fn preflight(name: &str, c: &rusqlite::Connection, viewer: i64, form: &mut Vec<(String, String)>) {{\\n        let _ = (c, viewer, &form);\\n        match name {{");
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
                    Expr::Name(n, _) if a.args.iter().any(|(x, _)| x == n) => binds.push(format!("etch_rt::app::field(form, {n:?}).unwrap_or(\\"\\").to_string()")),
                    other => return Err(format!("authenticates reads {} (only arguments and the viewer)", show(other))),
                }
            }
            let _ = writeln!(s, "                let hash: Option<String> = c.prepare_cached(r##\\"{}\\"##).ok().and_then(|mut st| st.query_row(rusqlite::params![{}], |r| r.get::<_, Option<String>>(0)).ok()).flatten();", sel.sql, binds.join(", "));
            let _ = writeln!(s, "                let ok = etch_rt::pass::verify(hash.as_deref(), etch_rt::app::field(form, {text:?}).unwrap_or(\\"\\"));");
            let _ = writeln!(s, "                form.push((\\"__verified_{i}\\".to_string(), if ok {{ hash.unwrap_or_default() }} else {{ String::new() }}));");
        }
        let _ = writeln!(s, "            }}");
    }
    let _ = writeln!(s, "            _ => {{}}\\n        }}\\n    }}");
    // where "/" goes
    match m.home {
        Some(Decl::Home { screen, arg, .. }) => {
            let route = screen_decls.iter().find_map(|d| match d {
                Decl::Screen { name, route, params, .. } if name == screen => Some(route.replace(&format!("{{{}}}", params[0].name), "{}")),
                _ => None,
            }).ok_or("home's screen")?;
            let x = inline::expr(m, arg, &inline::Env::new(), 0, 0);
            let sel = sql::cond_select_with(m, &x, &[])?;
            let _ = writeln!(s, "    fn home(c: &rusqlite::Connection, viewer: i64, cookie: &dyn Fn(&str) -> Option<i64>) -> Option<String> {{\\n        let id: Option<i64> = c.prepare_cached(r##\\"{}\\"##).ok()?.query_row({}, |r| r.get(0)).ok()?;\\n        id.map(|id| format!({route:?}, id))\\n    }}", sel.sql, binds_rs(&sel, "")?);
        }
        _ => {
            let _ = writeln!(s, "    fn home(_c: &rusqlite::Connection, _viewer: i64, _cookie: &dyn Fn(&str) -> Option<i64>) -> Option<String> {{ None }}");
        }
    }'''),
])
print('ok')
