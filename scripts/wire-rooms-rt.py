p = '/mnt/d/code/etch/crates/etch-rt/src/app.rs'
s = open(p, encoding='utf-8').read()

def rep(old, new):
    global s
    assert old in s, old[:80]
    s = s.replace(old, new, 1)

rep('''    /// Where "/" goes for a viewer (the request's cookies, as ids).''', '''    /// Screens rendered per request, for a viewer (settings pages: `cold`).
    fn cold(s: usize) -> bool;
    /// A screen's guards in order, for a viewer: Ok, or the first unmet one's answer (a redirect,
    /// or None: not found).
    fn guards(c: &Connection, s: usize, key: i64, viewer: i64) -> Result<(), Option<String>>;
    /// Where "/" goes for a viewer (the request's cookies, as ids).''')

rep('''pub struct Done {
    pub to: String,
    pub cookie: Option<Vec<u8>>,
}''', '''pub struct Done {
    pub to: String,
    pub cookie: Option<Vec<u8>>,
    /// where the action said to go (`go`)
    pub go: Option<String>,
}''')
rep('''                    replies.push((reply, r.map(|_| Done { to: G::after_act(&name, &form), cookie })));''', '''                    let go = w.go.take();
                    replies.push((reply, r.map(|_| Done { to: G::after_act(&name, &form), cookie, go })));''')
rep('''                    Ok(Ok(Done { to, cookie: Some(token) })) => {''', '''                    Ok(Ok(Done { to, cookie: Some(token), .. })) => {''')
rep('''                    Ok(Ok(Done { to, cookie: None })) => redirect(out, &back.unwrap_or(to), Some("ok")),''', '''                    Ok(Ok(Done { go: Some(to), cookie: None, .. })) => redirect(out, &to, Some("ok")),
                    Ok(Ok(Done { to, cookie: None, go: None })) => redirect(out, &back.unwrap_or(to), Some("ok")),''')

# op 13: a deleted row's screen instance
rep('''                // a session row deleted
                11 => {''', '''                // a deleted row's screen instance (and its lists)
                13 => {
                    next.screens[a].remove(&key);
                    for l in 0..G::LISTS {
                        if G::list_screen(l) == a {
                            next.lists[l].remove(&key);
                        }
                    }
                }
                // a session row deleted
                11 => {''')

# the fast path leaves cold screens alone
rep('''        let (s, key) = match G::route(req.path) {
            Some(r) => r,
            None => return false,
        };''', '''        let (s, key) = match G::route(req.path) {
            Some(r) if !G::cold(r.0) => r,
            _ => return false,
        };''')

# the cold path renders cold screens per request
rep('''            if let Some((s, key)) = G::route(path) {
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", None),
                };''', '''            if let Some((s, key)) = G::route(path).filter(|(s, _)| G::cold(*s)) {
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", None),
                };
                return match self.with_reader(|c| self.cold_page(c, s, key, viewer)) {
                    Ok((body, headers)) => out.push_own(|b| {
                        b.extend_from_slice(b"HTTP/1.1 200 OK\\r\\nContent-Length: ");
                        push_int(b, body.len() as u64);
                        b.extend_from_slice(b"\\r\\n");
                        b.extend_from_slice(&headers);
                        b.extend_from_slice(b"Date: ");
                        http_date(b);
                        b.extend_from_slice(b"\\r\\n\\r\\n");
                        b.extend_from_slice(&body);
                    }),
                    Err(Some(to)) => redirect(out, &to, None),
                    Err(None) => respond(out, 404, "text/plain", &[], b"not found"),
                };
            }
            if let Some((s, key)) = G::route(path) {
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", None),
                };''')

rep('''    /// A public screen, built from SQLite (no viewer: each register's variant for nobody).''', '''    /// A cold screen for a viewer, built from SQLite after its guards (in order).
    fn cold_page(&self, c: &Connection, s: usize, key: i64, viewer: i64) -> Result<(Vec<u8>, Vec<u8>), Option<String>> {
        G::guards(c, s, key, viewer)?;
        let v = load_viewer::<G>(c, &self.ctx, viewer);
        let vs = [v.ptr()];
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
        for (kind, k) in G::screen_regs(s, key) {
            let rec = load::<G>(c, &self.ctx, kind, k).ok_or(None)?;
            snap.regs[kind].insert(k, Arc::new(build::<G>(&self.ctx, kind, rec.ptr(), &vs)));
        }
        for kind in G::viewer_regs(s) {
            let rec = load::<G>(c, &self.ctx, kind, viewer).ok_or(None)?;
            snap.regs[kind].insert(viewer, Arc::new(build::<G>(&self.ctx, kind, rec.ptr(), &vs)));
        }
        for (list, items) in G::screen_lists(c, s, key) {
            let kind = G::list_item(list);
            for id in &items {
                if let Some(rec) = load::<G>(c, &self.ctx, kind, *id) {
                    snap.regs[kind].insert(*id, Arc::new(build::<G>(&self.ctx, kind, rec.ptr(), &vs)));
                }
            }
            snap.lists[list].insert(key, Arc::new(items));
        }
        let (_, headers) = G::screen(c, s, key).ok_or(None)?;
        let mut page = Vec::with_capacity(65536);
        if G::assemble(&snap, s, key, viewer, 0, &mut |seg| page.extend_from_slice(&seg.raw)) {
            Ok((page, headers))
        } else {
            Err(None)
        }
    }

    /// A public screen, built from SQLite (no viewer: each register's variant for nobody).''')

rep('''/// A form field's value: an action's argument by its name, or as `__arg_<name>`''', '''/// A list argument: every value of its field (or `__arg_<name>`), each a row's id (a `#` or `@`
/// mark dropped), as a JSON array (`[1,2]`); values that aren't ids are left out.
pub fn field_ids(form: &[(String, String)], name: &str) -> String {
    let ids: Vec<String> = form
        .iter()
        .filter(|(k, _)| k == name || k.strip_prefix("__arg_") == Some(name))
        .filter_map(|(_, v)| v.trim_start_matches(['#', '@']).parse::<i64>().ok())
        .map(|i| i.to_string())
        .collect();
    format!("[{}]", ids.join(","))
}

/// A form field's value: an action's argument by its name, or as `__arg_<name>`''')
open(p, 'w', encoding='utf-8').write(s)

# the glue: cold screens (per request, any guards), routes without a parameter, guards in order
p = '/mnt/d/code/etch/crates/etchc/src/glue.rs'
s = open(p, encoding='utf-8').read()
rep('''        if let Decl::Screen { route, params, public: false, .. } = d {
            if params.len() != 1 {
                return Err("a screen has one row parameter (for now)".into());
            }''', '''        if let Decl::Screen { route, params, public: false, .. } = d {
            if params.is_empty() {
                let _ = writeln!(s, "        if path == b{route:?} {{ return Some(({si}, 0)); }}");
                continue;
            }
            if params.len() != 1 {
                return Err("a screen has one row parameter, or none (for now)".into());
            }''')
rep('''        let (params, requires, cookies, body, public) = match d {
            Decl::Screen { params, requires, cookies, body, public, .. } => (params, requires, cookies, body, *public),
            _ => continue,
        };''', '''        let (params, requires, cookies, body, public, cold) = match d {
            Decl::Screen { params, requires, cookies, body, public, cold, .. } => (params, requires, cookies, body, *public, *cold),
            _ => continue,
        };
        // per request (public or cold): no audience, the guards checked in order each time
        let per_request = public || cold;''')
rep('''            None if public => String::new(),
            _ => return Err("a screen's parameter is a row".into()),''', '''            None if per_request => String::new(),
            _ => return Err("a screen's parameter is a row".into()),''')
rep('''            let on_viewer = sel.binds.iter().any(|sql::Bind::Path(e)| matches!(e, Expr::Name(n, _) if n == "viewer"));
            if !on_viewer {''', '''            let on_viewer = sel.binds.iter().any(|sql::Bind::Path(e)| matches!(e, Expr::Name(n, _) if n == "viewer"));
            if per_request {
                continue;
            }
            if !on_viewer {''')
rep('''            None if public => (String::new(), String::new()),''', '''            None if per_request => (String::new(), String::new()),''')
rep('''        if public {
            let _ = write!(screen_fn,''', '''        if per_request {
            let _ = write!(screen_fn,''')
rep('''        let _ = write!(refuse_fn, "        {si} => {{\\n");''', '''        let _ = write!(guards_fn, "        {si} => {{\\n");
        for (sql_text, b, otherwise) in &all {
            let ans = match otherwise {
                Else::Go(to) => format!("Some({to:?}.to_string())"),
                _ => "None".into(),
            };
            let _ = write!(guards_fn, "            let ok: i64 = c.prepare_cached(r##\\"{sql_text}\\"##).ok().and_then(|mut st| st.query_row({b}, |r| r.get(0)).ok()).unwrap_or(0);\\n            if ok != 1 {{ return Err({ans}); }}\\n");
        }
        let _ = write!(guards_fn, "            Ok(())\\n        }}\\n");
        if cold {
            colds.push(si.to_string());
        }
        let _ = write!(refuse_fn, "        {si} => {{\\n");''')
rep('''    let mut refuse_fn = String::new();''', '''    let mut refuse_fn = String::new();
    let mut guards_fn = String::new();
    let mut colds: Vec<String> = vec![];''')
rep('''    let _ = writeln!(s, "    fn screen_regs(s: usize, key: i64)''', '''    let _ = writeln!(s, "    fn guards(c: &rusqlite::Connection, s: usize, key: i64, viewer: i64) -> Result<(), Option<String>> {{\\n        let _ = (c, key, viewer);\\n        match s {{\\n{guards_fn}        _ => Err(None),\\n        }}\\n    }}");
    let _ = writeln!(s, "    fn cold(s: usize) -> bool {{ matches!(s, {}) }}", if colds.is_empty() { "usize::MAX".to_string() } else { colds.join(" | ") });
    let _ = writeln!(s, "    fn screen_regs(s: usize, key: i64)''')
open(p, 'w', encoding='utf-8').write(s)
print('ok')
