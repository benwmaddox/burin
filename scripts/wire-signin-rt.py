p = '/mnt/d/code/etch/crates/etch-rt/src/app.rs'
s = open(p, encoding='utf-8').read()

def rep(old, new):
    global s
    assert old in s, old[:80]
    s = s.replace(old, new, 1)

# the trait
rep('''    /// Every session's token and its viewer.
    fn sessions(c: &Connection) -> Vec<(String, i64)>;''', '''    /// Every session: its row's id, its token, its viewer.
    fn sessions(c: &Connection) -> Vec<(i64, String, i64)>;
    /// One session row (by its id): its token and viewer.
    fn session_row(c: &Connection, id: i64) -> Option<(String, i64)>;
    /// The viewer of a token (a session the snapshot hasn't seen).
    fn session(c: &Connection, token: &str) -> Option<i64>;
    /// A public screen (no viewer, no parameters) at this path.
    fn public_screen(path: &[u8]) -> Option<usize>;
    /// Actions run without a viewer (signing in).
    fn public_act(name: &str) -> bool;
    /// An action's password checks, on the request's thread: each adds its hidden field (the
    /// hash it verified, or nothing).
    fn preflight(name: &str, c: &Connection, viewer: i64, form: &mut Vec<(String, String)>);
    /// Where "/" goes for a viewer (the request's cookies, as ids).
    fn home(c: &Connection, viewer: i64, cookie: &dyn Fn(&str) -> Option<i64>) -> Option<String>;''')

rep('''        snap.sessions = G::sessions(&w).into_iter().map(|(t, u)| (t.into_bytes().into_boxed_slice(), u)).collect();''',
    '''        snap.sessions = G::sessions(&w).into_iter().map(|(_, t, u)| (t.into_bytes().into_boxed_slice(), u)).collect();''')

rep('''    Act { name: String, viewer: i64, form: Vec<(String, String)>, reply: SyncSender<Result<String, &'static str>> },''',
    '''    Act { name: String, viewer: i64, form: Vec<(String, String)>, reply: SyncSender<Result<Done, &'static str>> },''')
rep('''#[derive(Default)]
pub struct Stats {''', '''/// A done action: where it goes, and its session cookie (Some(token): signed in; Some(empty):
/// signed out).
pub struct Done {
    pub to: String,
    pub cookie: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct Stats {''')

# the cold path: actions
rep('''            if let Some(name) = path.strip_prefix(b"/_do/") {
                let name = String::from_utf8_lossy(name).into_owned();
                let form = parse_form(req.body);
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", Some("unauthenticated")),
                };
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                if !self.send(Cmd::Act { name, viewer, form, reply: tx }) {
                    self.stats.refused.fetch_add(1, Ordering::Relaxed);
                    return respond(out, 503, "text/plain", &[], b"busy");
                }
                return match rx.recv() {
                    Ok(Ok(to)) => redirect(out, &self.back(req).unwrap_or(to), Some("ok")),
                    Ok(Err(e)) => redirect(out, "/", Some(e)),
                    Err(_) => respond(out, 500, "text/plain", &[], b"writer stopped"),
                };
            }''', '''            if let Some(name) = path.strip_prefix(b"/_do/") {
                let name = String::from_utf8_lossy(name).into_owned();
                let mut form = parse_form(req.body);
                // hidden arguments come only from here, never from the form
                form.retain(|(k, _)| !k.starts_with("__verified") && k != "__session");
                let viewer = match (viewer, G::public_act(&name)) {
                    (Some(v), _) => v,
                    (None, true) => 0,
                    (None, false) => return redirect(out, "/session/new", Some("unauthenticated")),
                };
                if let Some(t) = req.cookie(self.session_cookie) {
                    form.push(("__session".into(), String::from_utf8_lossy(t).into_owned()));
                }
                // password checks here, on this thread (slow by design), not on the writer
                self.with_reader(|c| G::preflight(&name, c, viewer, &mut form));
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                if !self.send(Cmd::Act { name, viewer, form, reply: tx }) {
                    self.stats.refused.fetch_add(1, Ordering::Relaxed);
                    return respond(out, 503, "text/plain", &[], b"busy");
                }
                let back = self.back(req);
                return match rx.recv() {
                    Ok(Ok(Done { to, cookie: Some(token) })) => {
                        let name = String::from_utf8_lossy(self.session_cookie);
                        let (to, c) = if token.is_empty() {
                            ("/session/new".to_string(), format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"))
                        } else {
                            let _ = to;
                            ("/".to_string(), format!("{name}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000", String::from_utf8_lossy(&token)))
                        };
                        redirect_with(out, &to, Some("ok"), &[("Set-Cookie", &c)])
                    }
                    Ok(Ok(Done { to, cookie: None })) => redirect(out, &back.unwrap_or(to), Some("ok")),
                    Ok(Err(e)) => redirect(out, &back.unwrap_or_else(|| "/".into()), Some(e)),
                    Err(_) => respond(out, 500, "text/plain", &[], b"writer stopped"),
                };
            }''')

# the cold path: "/" and public screens
rep('''        if req.method == b"GET" {
            if let Some((s, key)) = G::route(path) {''', '''        if req.method == b"GET" && path == b"/" {
            let Some(viewer) = viewer else { return redirect(out, "/session/new", None) };
            let cookie = |n: &str| req.cookie(n.as_bytes()).and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse().ok());
            return match self.with_reader(|c| G::home(c, viewer, &cookie)) {
                Some(to) => redirect(out, &to, None),
                None => respond(out, 404, "text/plain", &[], b"no rooms"),
            };
        }
        if req.method == b"GET" {
            if let Some(s) = G::public_screen(path) {
                return match self.with_reader(|c| self.public_page(c, s)) {
                    Some((body, headers)) => out.push_own(|b| {
                        b.extend_from_slice(b"HTTP/1.1 200 OK\\r\\nContent-Length: ");
                        push_int(b, body.len() as u64);
                        b.extend_from_slice(b"\\r\\n");
                        b.extend_from_slice(&headers);
                        b.extend_from_slice(b"Date: ");
                        http_date(b);
                        b.extend_from_slice(b"\\r\\n\\r\\n");
                        b.extend_from_slice(&body);
                    }),
                    None => respond(out, 500, "text/plain", &[], b"the page couldn't be built"),
                };
            }
            if let Some((s, key)) = G::route(path) {''')

rep('''    fn viewer_of(&self, req: &Req) -> Option<i64> {
        let t = req.cookie(self.session_cookie)?;
        if let Some(u) = self.snap.load().sessions.get(t) {
            return Some(*u);
        }
        // a session the snapshot hasn't seen: SQLite
        let t = String::from_utf8_lossy(t).into_owned();
        self.with_reader(|c| G::sessions(c).into_iter().find(|(k, _)| *k == t).map(|(_, u)| u))
    }''', '''    fn viewer_of(&self, req: &Req) -> Option<i64> {
        let t = req.cookie(self.session_cookie)?;
        if let Some(u) = self.snap.load().sessions.get(t) {
            return Some(*u);
        }
        // a session the snapshot hasn't seen yet: SQLite, by its token
        let t = String::from_utf8_lossy(t).into_owned();
        self.with_reader(|c| G::session(c, &t))
    }

    /// A public screen, built from SQLite (no viewer: each register's variant for nobody).
    fn public_page(&self, c: &Connection, s: usize) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
        for (kind, k) in G::screen_regs(s, 0) {
            let rec = load::<G>(c, &self.ctx, kind, k)?;
            snap.regs[kind].insert(k, Arc::new(build::<G>(&self.ctx, kind, rec.ptr(), &[])));
        }
        let (_, headers) = G::screen(c, s, 0)?;
        let mut page = Vec::with_capacity(16384);
        G::assemble(&snap, s, 0, 0, 0, &mut |seg| page.extend_from_slice(&seg.raw)).then_some((page, headers))
    }''')

# the writer: sessions by row id; replies with cookies
rep('''    let mut kept: HashMap<(usize, i64), Rec> = HashMap::new();''', '''    let mut kept: HashMap<(usize, i64), Rec> = HashMap::new();
    // the sessions' tokens by their row's id (a deleted row's token is forgotten by its id)
    let mut tokens: HashMap<i64, Box<[u8]>> = G::sessions(&c).into_iter().map(|(id, t, _)| (id, t.into_bytes().into_boxed_slice())).collect();''')
rep('''                    let r = with_db(&tx, |db| G::act(&name, &tx, &ctx, db, &mut w, viewer, vrec, &form, &mut deltas));
                    replies.push((reply, r.map(|_| G::after_act(&name, &form))));''', '''                    let r = with_db(&tx, |db| G::act(&name, &tx, &ctx, db, &mut w, viewer, vrec, &form, &mut deltas));
                    let cookie = match (w.signed_in.take(), std::mem::take(&mut w.signed_out)) {
                        (Some(t), _) => Some(t),
                        (None, true) => Some(vec![]),
                        _ => None,
                    };
                    replies.push((reply, r.map(|_| Done { to: G::after_act(&name, &form), cookie })));''')
rep('''                // what a screen's audience reads changed: its audience again (every key: i64::MIN)''', '''                // a session row made or changed (by its id): the snapshot's sessions
                10 => {
                    if let Some(old) = tokens.remove(&key) {
                        next.sessions.remove(&old);
                    }
                    if let Some((t, u)) = G::session_row(&c, key) {
                        let t = t.into_bytes().into_boxed_slice();
                        next.sessions.insert(t.clone(), u);
                        tokens.insert(key, t);
                    }
                }
                // a session row deleted
                11 => {
                    if let Some(old) = tokens.remove(&key) {
                        next.sessions.remove(&old);
                    }
                }
                // what a screen's audience reads changed: its audience again (every key: i64::MIN)''')
rep('''        stats.acts.fetch_add(replies.iter().filter(|r: &&(SyncSender<_>, Result<String, &str>)| r.1.is_ok()).count() as u64, Ordering::Relaxed);''',
    '''        stats.acts.fetch_add(replies.iter().filter(|r: &&(SyncSender<_>, Result<Done, &str>)| r.1.is_ok()).count() as u64, Ordering::Relaxed);''')

# redirect with headers
rep('''pub fn redirect(out: &mut Out, to: &str, outcome: Option<&str>) {
    out.push_own(|b| {
        b.extend_from_slice(b"HTTP/1.1 303 See Other\\r\\nContent-Length: 0\\r\\nLocation: ");
        b.extend_from_slice(to.as_bytes());
        b.extend_from_slice(b"\\r\\n");''', '''pub fn redirect(out: &mut Out, to: &str, outcome: Option<&str>) {
    redirect_with(out, to, outcome, &[]);
}

pub fn redirect_with(out: &mut Out, to: &str, outcome: Option<&str>, headers: &[(&str, &str)]) {
    out.push_own(|b| {
        b.extend_from_slice(b"HTTP/1.1 303 See Other\\r\\nContent-Length: 0\\r\\nLocation: ");
        b.extend_from_slice(to.as_bytes());
        b.extend_from_slice(b"\\r\\n");
        for (k, v) in headers {
            b.extend_from_slice(k.as_bytes());
            b.extend_from_slice(b": ");
            b.extend_from_slice(v.as_bytes());
            b.extend_from_slice(b"\\r\\n");
        }''')

# main: --hash-password
rep('''    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();''', '''    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    // a password's hash, to store (setting up an account by hand)
    if let Some(p) = arg("--hash-password") {
        println!("{}", crate::pass::hash(&p));
        return;
    }''')
open(p, 'w', encoding='utf-8').write(s)
print('ok')
