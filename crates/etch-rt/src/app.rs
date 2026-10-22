//! The server every app runs, the same for every app: the fast path, the cold path (the golden
//! model), the writer and its delta edges. What is app-specific is generated (`Glue`), and called
//! by static dispatch, so it inlines.

use crate::abi::{Arena, Ctx, Db, Deltas, Writer};
use crate::out::{push_int, Out};
use crate::store::{Reg, Seg, Snap};
use crate::{http_date, Req};
use arc_swap::ArcSwap;
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;

/// A record a register is built from: its bytes (an instance of the register's input struct) and
/// the arena its text lives in.
pub struct Rec {
    pub bytes: Box<[u64]>,
    _arena: Box<Arena>,
}

impl Rec {
    pub fn new(size: usize) -> (Rec, *mut u8, *mut Arena) {
        let mut bytes = vec![0u64; size.div_ceil(8).max(1)].into_boxed_slice();
        let mut arena = Box::new(Arena::new());
        let p = bytes.as_mut_ptr() as *mut u8;
        let a: *mut Arena = &mut *arena;
        (Rec { bytes, _arena: arena }, p, a)
    }
    pub fn ptr(&self) -> *const u8 {
        self.bytes.as_ptr() as *const u8
    }
    pub fn ptr_mut(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr() as *mut u8
    }
}

/// What the compiler generates for an app.
///
/// # Safety
/// The record pointers passed to these functions are records of the register kind named.
pub unsafe trait Glue: Send + Sync + 'static {
    const REGS: usize;
    const LISTS: usize;
    const SCREENS: usize;
    /// Register kinds keyed by the viewer.
    fn per_viewer(kind: usize) -> bool;
    /// The register kind a screen list's items are.
    fn list_item(list: usize) -> usize;
    /// The screen (and its parameter) a screen list belongs to.
    fn list_screen(list: usize) -> usize;
    /// A screen list's bound (its window or limit).
    fn list_bound(list: usize) -> usize;
    /// A request's screen and its key, from its path.
    fn route(path: &[u8]) -> Option<(usize, i64)>;
    /// Every session's token and its viewer.
    fn sessions(c: &Connection) -> Vec<(String, i64)>;
    /// A register's record size, and its loader, variant key and builder.
    fn rec_size(kind: usize) -> usize;
    /// # Safety: `rec` is a zeroed record of `kind`'s size.
    unsafe fn load(kind: usize, ctx: &Ctx, db: *mut Db, key: i64, rec: *mut u8, arena: *mut Arena) -> bool;
    /// # Safety: `rec` is a loaded record of `kind`; `viewer` a viewer record.
    unsafe fn bits(kind: usize, rec: *const u8, viewer: *const u8) -> i64;
    /// # Safety: as `bits`; `tmps` has `tmps(kind)` buffers.
    unsafe fn build(kind: usize, ctx: &Ctx, rec: *const u8, viewer: *const u8, out: &mut Vec<u8>, tmps: &[*mut Vec<u8>], splits: &mut Vec<usize>);
    fn tmps(kind: usize) -> usize;
    fn viewer_size() -> usize;
    /// # Safety: `rec` a zeroed viewer record.
    unsafe fn load_viewer(ctx: &Ctx, db: *mut Db, key: i64, rec: *mut u8, arena: *mut Arena) -> bool;
    /// # Safety: kept records of the kinds the patch names.
    unsafe fn patch(reg: i64, list: i64, field: i64, rec: *mut u8, item: i64, value: i64) -> bool;
    /// # Safety: as `patch`.
    unsafe fn find(reg: i64, list: i64, field: i64, rec: *mut u8, row: i64) -> bool;
    /// A screen instance: its audience (sorted), whether its write-time guards hold, its headers.
    fn screen(c: &Connection, s: usize, key: i64) -> Option<(Vec<i64>, Vec<u8>)>;
    /// The registers a screen instance needs, keyed by its parameter: (kind, key).
    fn screen_regs(s: usize, key: i64) -> Vec<(usize, i64)>;
    /// The screen's lists' items, from SQLite: (list, item keys).
    fn screen_lists(c: &Connection, s: usize, key: i64) -> Vec<(usize, Vec<i64>)>;
    /// The register kinds a screen needs per viewer.
    fn viewer_regs(s: usize) -> Vec<usize>;
    /// The page: its pieces in order, for the viewer at `place` in the audience; false when a
    /// piece isn't in the snapshot.
    fn assemble<'a>(snap: &'a Snap, s: usize, key: i64, viewer: i64, place: usize, push: &mut dyn FnMut(&'a Seg)) -> bool;
    /// Runs an action (its form fields); Ok(0) or its outcome's name.
    fn act(name: &str, c: &Connection, ctx: &Ctx, db: *mut Db, w: &mut Writer, viewer: i64, vrec: *const u8, form: &[(String, String)], d: &mut Deltas) -> Result<(), &'static str>;
    /// The page to go to after an action (its form fields).
    fn after_act(name: &str, form: &[(String, String)]) -> String;
    /// The tables the app keeps its entities in (created if missing).
    fn schema() -> &'static str;
    /// A screen's unmet guard's answer for a viewer, on the cold path: a redirect, or 501.
    fn refuse(c: &Connection, s: usize, key: i64, viewer: i64) -> Option<String>;
}

// ---- statements and arenas, per thread and connection

thread_local! {
    static DB: RefCell<Option<(usize, Db)>> = const { RefCell::new(None) };
}

/// Runs `f` with this thread's prepared statements for connection `c`.
pub fn with_db<T>(c: &Connection, f: impl FnOnce(*mut Db) -> T) -> T {
    let h = unsafe { c.handle() };
    DB.with(|d| {
        let mut d = d.borrow_mut();
        if d.as_ref().map(|(k, _)| *k != h as usize).unwrap_or(true) {
            *d = Some((h as usize, unsafe { Db::new(h) }));
        }
        let db: *mut Db = &mut d.as_mut().unwrap().1;
        f(db)
    })
}

fn tmps(n: usize) -> (Vec<Vec<u8>>, Vec<*mut Vec<u8>>) {
    let mut bufs: Vec<Vec<u8>> = (0..n.max(1)).map(|_| Vec::with_capacity(256)).collect();
    let ptrs = bufs.iter_mut().map(|b| b as *mut Vec<u8>).collect();
    (bufs, ptrs)
}

/// A register's record, loaded from SQLite (a read).
pub fn load<G: Glue>(c: &Connection, ctx: &Ctx, kind: usize, key: i64) -> Option<Rec> {
    let (rec, p, a) = Rec::new(G::rec_size(kind));
    let ok = with_db(c, |db| unsafe { G::load(kind, ctx, db, key, p, a) });
    ok.then_some(rec)
}

/// A viewer's record (a read).
pub fn load_viewer<G: Glue>(c: &Connection, ctx: &Ctx, key: i64) -> Rec {
    let (rec, p, a) = Rec::new(G::viewer_size());
    with_db(c, |db| unsafe { G::load_viewer(ctx, db, key, p, a) });
    rec
}

/// A register's value from its record: a variant per distinct key among the viewers (or, keyed
/// by a viewer, its pieces between splices).
pub fn build<G: Glue>(ctx: &Ctx, kind: usize, rec: *const u8, viewers: &[*const u8]) -> Reg {
    let (_bufs, ptrs) = tmps(G::tmps(kind));
    let mut splits = vec![];
    if G::per_viewer(kind) {
        let mut out = Vec::with_capacity(8192);
        unsafe { G::build(kind, ctx, rec, viewers[0], &mut out, &ptrs, &mut splits) };
        // its pieces between splices; by_place holds each splice's site, in order
        let mut variants = vec![];
        let mut sites = vec![];
        let mut at = 0;
        for s in splits {
            let (s, site) = crate::abi::split_at(s);
            variants.push(Seg::new(out[at..s].to_vec()));
            sites.push(site as u8);
            at = s;
        }
        variants.push(Seg::new(out[at..].to_vec()));
        return Reg { variants, by_place: sites, overflow: false };
    }
    let mut keys: Vec<i64> = vec![];
    let mut variants = vec![];
    let mut by = Vec::with_capacity(viewers.len());
    for v in viewers {
        let k = unsafe { G::bits(kind, rec, *v) };
        let i = match keys.iter().position(|x| *x == k) {
            Some(i) => i,
            None => {
                keys.push(k);
                let mut out = Vec::with_capacity(4096);
                unsafe { G::build(kind, ctx, rec, *v, &mut out, &ptrs, &mut splits) };
                variants.push(Seg::new(out));
                keys.len() - 1
            }
        };
        by.push(i as u8);
    }
    if variants.is_empty() {
        // no audience yet: the variant for no viewer
        let mut out = Vec::with_capacity(4096);
        let none = vec![0u64; G::viewer_size().div_ceil(8).max(1)];
        unsafe { G::build(kind, ctx, rec, none.as_ptr() as *const u8, &mut out, &ptrs, &mut splits) };
        variants.push(Seg::new(out));
    }
    let overflow = variants.len() > 256;
    Reg { variants, by_place: if overflow { vec![] } else { by }, overflow }
}

/// Builds a screen instance into `snap` from SQLite: its audience and headers, the registers keyed
/// by its parameter, its lists and their items. False if its write-time guards don't hold.
pub fn build_screen<G: Glue>(c: &Connection, ctx: &Ctx, snap: &mut Snap, viewers: &mut HashMap<i64, Rec>, s: usize, key: i64) -> bool {
    let (audience, headers) = match G::screen(c, s, key) {
        Some(x) => x,
        None => {
            snap.screens[s].remove(&key);
            return false;
        }
    };
    for u in &audience {
        viewers.entry(*u).or_insert_with(|| load_viewer::<G>(c, ctx, *u));
    }
    let vs: Vec<*const u8> = audience.iter().map(|u| viewers[u].ptr()).collect();
    for (kind, k) in G::screen_regs(s, key) {
        if let Some(rec) = load::<G>(c, ctx, kind, k) {
            snap.regs[kind].insert(k, Arc::new(build::<G>(ctx, kind, rec.ptr(), &vs)));
        }
    }
    for (list, items) in G::screen_lists(c, s, key) {
        let kind = G::list_item(list);
        for id in &items {
            if let Some(rec) = load::<G>(c, ctx, kind, *id) {
                snap.regs[kind].insert(*id, Arc::new(build::<G>(ctx, kind, rec.ptr(), &vs)));
            }
        }
        snap.lists[list].insert(key, Arc::new(items));
    }
    snap.screens[s].insert(key, Arc::new(crate::store::Screen { audience, headers }));
    true
}

// ---- the server

pub enum Cmd {
    Act { name: String, viewer: i64, form: Vec<(String, String)>, reply: SyncSender<Result<String, &'static str>> },
    Warm { screen: usize, key: i64, viewer: i64 },
}

#[derive(Default)]
pub struct Stats {
    pub fast: AtomicU64,
    pub cold: AtomicU64,
    pub verified: AtomicU64,
    pub mismatched: AtomicU64,
    pub refused: AtomicU64,
    pub batches: AtomicU64,
    pub acts: AtomicU64,
    /// registers loaded from SQLite after a commit (a read after the write)
    pub write_reads: AtomicU64,
    /// registers built from forwarded records, or patched kept records: no read
    pub forwarded: AtomicU64,
    pub patched: AtomicU64,
    /// writes that cleared every screen (an edge the writer has no rule for): should stay 0
    pub cleared: AtomicU64,
    /// screen instances rebuilt because their audience changed
    pub audiences: AtomicU64,
}

pub struct App<G: Glue> {
    pub ctx: Arc<Ctx>,
    pub snap: Arc<ArcSwap<Snap>>,
    pub writer: SyncSender<Cmd>,
    pub db_path: String,
    pub host: String,
    pub verify: bool,
    pub stats: Arc<Stats>,
    pub session_cookie: &'static [u8],
    _g: std::marker::PhantomData<G>,
}

thread_local! {
    static READER: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

fn open_read(path: &str) -> Connection {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).expect("open the database to read");
    c.busy_timeout(std::time::Duration::from_secs(5)).ok();
    c
}

pub fn open_write(path: &str) -> Connection {
    let c = Connection::open(path).expect("open the database");
    c.busy_timeout(std::time::Duration::from_secs(5)).ok();
    c.pragma_update(None, "journal_mode", "WAL").ok();
    c.pragma_update(None, "synchronous", "NORMAL").ok();
    c.pragma_update(None, "wal_autocheckpoint", 0).ok();
    c
}

impl<G: Glue> App<G> {
    /// Starts the writer (and the checkpointer); returns the app to serve.
    pub fn start(ctx: Arc<Ctx>, db_path: &str, host: &str, verify: bool, session_cookie: &'static [u8], seq: i64) -> Arc<App<G>> {
        let w = open_write(db_path);
        w.execute_batch(G::schema()).expect("the schema");
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
        snap.sessions = G::sessions(&w).into_iter().map(|(t, u)| (t.into_bytes().into_boxed_slice(), u)).collect();
        let snap = Arc::new(ArcSwap::from_pointee(snap));
        let (tx, rx) = std::sync::mpsc::sync_channel(1024);
        let stats = Arc::new(Stats::default());
        {
            let (ctx, snap, stats) = (ctx.clone(), snap.clone(), stats.clone());
            std::thread::Builder::new().name("etch-writer".into()).spawn(move || writer::<G>(w, ctx, snap, rx, stats, seq)).unwrap();
        }
        {
            let snap = snap.clone();
            let ck = open_write(db_path);
            std::thread::spawn(move || {
                let mut last = 0;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let v = snap.load().version;
                    if v != last {
                        last = v;
                        let _ = ck.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
                    }
                }
            });
        }
        Arc::new(App { ctx, snap, writer: tx, db_path: db_path.to_string(), host: host.to_string(), verify, stats, session_cookie, _g: std::marker::PhantomData })
    }

    fn with_reader<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        READER.with(|r| {
            let mut r = r.borrow_mut();
            if r.is_none() {
                *r = Some(open_read(&self.db_path));
            }
            f(r.as_ref().unwrap())
        })
    }

    fn send(&self, cmd: Cmd) -> bool {
        match self.writer.try_send(cmd) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// The fast path: a screen instance and the viewer's registers, from the snapshot.
    pub fn fast(&self, req: &Req, out: &mut Out) -> bool {
        if req.method != b"GET" || !req.query.is_empty() || req.cookie(b"flash").is_some() {
            return false;
        }
        let (s, key) = match G::route(req.path) {
            Some(r) => r,
            None => return false,
        };
        let token = match req.cookie(self.session_cookie) {
            Some(t) => t,
            None => return false,
        };
        let snap = self.snap.load_full();
        if req.host != self.host.as_bytes() {
            return false;
        }
        let uid = match snap.sessions.get(token) {
            Some(u) => *u,
            None => return false,
        };
        let inst = match snap.screens[s].get(&key) {
            Some(i) => i.clone(),
            None => {
                self.send(Cmd::Warm { screen: s, key, viewer: uid });
                return false;
            }
        };
        let place = match inst.audience.binary_search(&uid) {
            Ok(p) => p,
            Err(_) => return false, // not in its audience: the cold path answers
        };
        let first = out.placeholder();
        // Safety: every borrowed range is owned by `snap`, which the response holds
        unsafe { out.push_borrowed(&inst.headers) };
        if req.gzip {
            out.push_static(b"Content-Encoding: gzip\r\n");
        }
        out.push_own(|b| {
            b.extend_from_slice(b"Date: ");
            http_date(b);
            b.extend_from_slice(b"\r\n\r\n");
        });
        let head_len = out.len();
        let ok = if req.gzip {
            out.push_static(&crate::gz::HEADER);
            let mut crc = crate::gz::Crc::default();
            let ok = G::assemble(&snap, s, key, uid, place, &mut |seg| {
                unsafe { out.push_borrowed(&seg.gz.deflate) };
                crc.add(&seg.gz);
            });
            out.push_static(&crate::gz::FINAL);
            out.push_own(|b| b.extend_from_slice(&crc.trailer()));
            ok
        } else {
            G::assemble(&snap, s, key, uid, place, &mut |seg| unsafe { out.push_borrowed(&seg.raw) })
        };
        if !ok {
            self.send(Cmd::Warm { screen: s, key, viewer: uid });
            out.reset();
            return false;
        }
        let body_len = out.len() - head_len;
        out.fill(first, |b| {
            b.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: ");
            push_int(b, body_len as u64);
            b.extend_from_slice(b"\r\n");
        });
        if self.verify {
            self.check(s, key, uid, out, body_len, req.gzip, &inst.headers);
        }
        out.hold(snap);
        self.stats.fast.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The golden-model check: the fast page must equal a fresh build from SQLite.
    fn check(&self, s: usize, key: i64, uid: i64, out: &Out, body_len: usize, gzip: bool, _headers: &[u8]) {
        let all = out.to_vec();
        let mut fast = all[all.len() - body_len..].to_vec();
        if gzip {
            use std::io::Read;
            let mut plain = vec![];
            if flate2::read::GzDecoder::new(&fast[..]).read_to_end(&mut plain).is_err() {
                plain = b"(not valid gzip)".to_vec();
            }
            fast = plain;
        }
        let fresh = self.with_reader(|c| self.fresh(c, s, key, uid));
        self.stats.verified.fetch_add(1, Ordering::Relaxed);
        match fresh {
            Some((page, _)) if page == fast => {}
            other => {
                let n = self.stats.mismatched.fetch_add(1, Ordering::Relaxed);
                if n < 5 {
                    let f = other.map(|x| x.0).unwrap_or_default();
                    let at = f.iter().zip(&fast).position(|(a, b)| a != b).unwrap_or(f.len().min(fast.len()));
                    let ctx = |b: &[u8]| String::from_utf8_lossy(&b[at.saturating_sub(60)..(at + 60).min(b.len())]).into_owned();
                    eprintln!("verify: screen {s} key {key} viewer {uid}: differs at byte {at}\n fast:  {}\n fresh: {}", ctx(&fast), ctx(&f));
                }
            }
        }
    }

    /// The page built fresh from SQLite (the cold path, and the golden model): None when the
    /// viewer isn't in its audience or its guards don't hold.
    fn fresh(&self, c: &Connection, s: usize, key: i64, uid: i64) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
        let mut viewers = HashMap::new();
        if !build_screen::<G>(c, &self.ctx, &mut snap, &mut viewers, s, key) {
            return None;
        }
        let inst = snap.screens[s].get(&key)?.clone();
        let place = inst.audience.binary_search(&uid).ok()?;
        viewers.entry(uid).or_insert_with(|| load_viewer::<G>(c, &self.ctx, uid));
        for kind in G::viewer_regs(s) {
            if let Some(rec) = load::<G>(c, &self.ctx, kind, uid) {
                snap.regs[kind].insert(uid, Arc::new(build::<G>(&self.ctx, kind, rec.ptr(), &[viewers[&uid].ptr()])));
            }
        }
        let mut page = Vec::with_capacity(400_000);
        if !G::assemble(&snap, s, key, uid, place, &mut |seg| page.extend_from_slice(&seg.raw)) {
            return None;
        }
        Some((page, inst.headers.clone()))
    }

    fn cold(&self, req: &Req, out: &mut Out) {
        self.stats.cold.fetch_add(1, Ordering::Relaxed);
        let path = req.path;
        if req.method == b"GET" && path == b"/up" {
            return respond(out, 200, "text/plain", &[], b"ok");
        }
        if req.method == b"GET" && path == b"/_etch/stats" {
            let st = &self.stats;
            let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
            let s = format!(
                r#"{{"fast":{},"cold":{},"verified":{},"mismatched":{},"refused":{},"batches":{},"acts":{},"write_reads":{},"forwarded":{},"patched":{},"cleared":{},"audiences":{},"version":{}}}"#,
                g(&st.fast), g(&st.cold), g(&st.verified), g(&st.mismatched), g(&st.refused), g(&st.batches), g(&st.acts), g(&st.write_reads), g(&st.forwarded), g(&st.patched), g(&st.cleared), g(&st.audiences), self.snap.load().version
            );
            return respond(out, 200, "application/json", &[], s.as_bytes());
        }
        let viewer = self.viewer_of(req);
        if req.method == b"POST" {
            if let Some(name) = path.strip_prefix(b"/_do/") {
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
            }
        }
        if req.method == b"GET" {
            if let Some((s, key)) = G::route(path) {
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", None),
                };
                let r = self.with_reader(|c| match self.fresh(c, s, key, viewer) {
                    Some(p) => Ok(p),
                    None => Err(G::refuse(c, s, key, viewer)),
                });
                return match r {
                    Ok((body, headers)) => {
                        out.push_own(|b| {
                            b.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: ");
                            push_int(b, body.len() as u64);
                            b.extend_from_slice(b"\r\n");
                            b.extend_from_slice(&headers);
                            b.extend_from_slice(b"Date: ");
                            http_date(b);
                            b.extend_from_slice(b"\r\n\r\n");
                            b.extend_from_slice(&body);
                        });
                    }
                    Err(Some(to)) => redirect(out, &to, None),
                    Err(None) => respond(out, 501, "text/plain", &[], b"not in this phase"),
                };
            }
        }
        respond(out, 404, "text/plain", &[], b"not found")
    }

    /// The page an action came from, to go back to: the Referer's path, only when it names this
    /// host (never another site's address).
    fn back(&self, req: &Req) -> Option<String> {
        let r = std::str::from_utf8(req.referer).ok()?;
        let rest = r.strip_prefix("http://").or_else(|| r.strip_prefix("https://"))?;
        let (host, path) = rest.split_at(rest.find('/')?);
        (host == self.host && !path.starts_with("//") && !path.contains('\\')).then(|| path.to_string())
    }

    fn viewer_of(&self, req: &Req) -> Option<i64> {
        let t = req.cookie(self.session_cookie)?;
        if let Some(u) = self.snap.load().sessions.get(t) {
            return Some(*u);
        }
        // a session the snapshot hasn't seen: SQLite
        let t = String::from_utf8_lossy(t).into_owned();
        self.with_reader(|c| G::sessions(c).into_iter().find(|(k, _)| *k == t).map(|(_, u)| u))
    }
}

impl<G: Glue> crate::Handler for App<G> {
    fn handle(&self, req: &Req, out: &mut Out) {
        if !self.fast(req, out) {
            out.reset();
            self.cold(req, out);
        }
    }
}

// ---- the writer

/// The writer: actions in batches (group commit), then their delta edges into the next snapshot.
fn writer<G: Glue>(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>, seq: i64) {
    let mut w = Writer::new(seq);
    // kept between batches: viewers' records, per-viewer registers' records (patched in place)
    let mut viewers: HashMap<i64, Rec> = HashMap::new();
    let mut kept: HashMap<(usize, i64), Rec> = HashMap::new();
    let mut batch = Vec::with_capacity(256);
    while let Ok(first) = rx.recv() {
        batch.push(first);
        while batch.len() < 256 {
            match rx.try_recv() {
                Ok(c) => batch.push(c),
                Err(_) => break,
            }
        }
        let cur = snap.load_full();
        let mut replies = vec![];
        let mut deltas = Deltas::default();
        let mut warm: Vec<(usize, i64, i64)> = vec![];
        w.begin(crate::time::now_ms());
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).expect("begin");
        for cmd in batch.drain(..) {
            match cmd {
                Cmd::Act { name, viewer, form, reply } => {
                    let vrec = viewers.entry(viewer).or_insert_with(|| load_viewer::<G>(&tx, &ctx, viewer)).ptr();
                    let r = with_db(&tx, |db| G::act(&name, &tx, &ctx, db, &mut w, viewer, vrec, &form, &mut deltas));
                    replies.push((reply, r.map(|_| G::after_act(&name, &form))));
                }
                Cmd::Warm { screen, key, viewer } => warm.push((screen, key, viewer)),
            }
        }
        tx.commit().expect("commit");
        let mut next = cur.next();
        let mut reload: HashSet<(usize, i64)> = HashSet::new();
        let mut dirty: HashSet<(usize, i64)> = HashSet::new();
        let mut forwarded: HashMap<i64, i64> = HashMap::new();
        let mut recheck: HashSet<(usize, i64)> = HashSet::new();
        let (mut shared, mut relist, mut dropped, mut audiences): (HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>) = Default::default();
        for &(op, a, key, item) in &deltas.list {
            let a = a as usize;
            match op {
                6 => {
                    forwarded.insert(key, item);
                }
                2 | 7 => {
                    // a new item appended to a screen list: its register built for the screen's audience
                    let s = G::list_screen(a);
                    let inst = match next.screens[s].get(&key) {
                        Some(i) => i.clone(),
                        None => continue,
                    };
                    let kind = G::list_item(a);
                    for u in &inst.audience {
                        viewers.entry(*u).or_insert_with(|| load_viewer::<G>(&c, &ctx, *u));
                    }
                    let vs: Vec<*const u8> = inst.audience.iter().map(|u| viewers[u].ptr()).collect();
                    let reg = match forwarded.get(&item) {
                        Some(rec) => {
                            stats.forwarded.fetch_add(1, Ordering::Relaxed);
                            build::<G>(&ctx, kind, *rec as *const u8, &vs)
                        }
                        None => {
                            stats.write_reads.fetch_add(1, Ordering::Relaxed);
                            match load::<G>(&c, &ctx, kind, item) {
                                Some(rec) => build::<G>(&ctx, kind, rec.ptr(), &vs),
                                None => continue,
                            }
                        }
                    };
                    next.regs[kind].insert(item, Arc::new(reg));
                    let mut items: Vec<i64> = next.lists[a].get(&key).map(|l| (**l).clone()).unwrap_or_default();
                    // oldest first (op 2): appended, the oldest drops; newest first (op 7): first, the last drops
                    if op == 2 {
                        items.push(item);
                        while items.len() > G::list_bound(a).max(1) {
                            let old = items.remove(0);
                            next.regs[kind].remove(&old);
                        }
                    } else {
                        items.insert(0, item);
                        while items.len() > G::list_bound(a).max(1) {
                            let old = items.pop().unwrap();
                            next.regs[kind].remove(&old);
                        }
                    }
                    next.lists[a].insert(key, Arc::new(items));
                    recheck.insert((s, key));
                }
                3 if G::per_viewer(a) => {
                    if kept.contains_key(&(a, key)) {
                        reload.insert((a, key));
                    }
                }
                4 if G::per_viewer(a) => {
                    reload.extend(kept.keys().filter(|(k, _)| *k == a).copied());
                }
                // a shared register at a key: rebuilt for the audience of the screen showing it
                3 => {
                    shared.insert((a, key));
                }
                // a row left a screen list: the list again
                5 => {
                    relist.insert((a, key));
                }
                // a deleted row's register
                9 => {
                    dropped.insert((a, key));
                }
                // what a screen's audience reads changed: its audience again (every key: i64::MIN)
                12 => {
                    audiences.insert((a, key));
                }
                // anything else: the screens rebuild on demand (counted: a write should never need it)
                _ => {
                    stats.cleared.fetch_add(1, Ordering::Relaxed);
                    for s in &mut next.screens {
                        s.clear();
                    }
                }
            }
        }
        for (list, key) in relist {
            let s = G::list_screen(list);
            let Some(inst) = next.screens[s].get(&key).cloned() else { continue };
            let kind = G::list_item(list);
            let Some((_, items)) = G::screen_lists(&c, s, key).into_iter().find(|(l, _)| *l == list) else { continue };
            for u in &inst.audience {
                viewers.entry(*u).or_insert_with(|| load_viewer::<G>(&c, &ctx, *u));
            }
            let vs: Vec<*const u8> = inst.audience.iter().map(|u| viewers[u].ptr()).collect();
            for id in &items {
                if !next.regs[kind].contains_key(id) {
                    if let Some(rec) = load::<G>(&c, &ctx, kind, *id) {
                        next.regs[kind].insert(*id, Arc::new(build::<G>(&ctx, kind, rec.ptr(), &vs)));
                        stats.write_reads.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            next.lists[list].insert(key, Arc::new(items));
            recheck.insert((s, key));
        }
        for (kind, key) in dropped {
            next.regs[kind].remove(&key);
            shared.remove(&(kind, key));
        }
        for (kind, key) in shared {
            if !next.regs[kind].contains_key(&key) {
                continue; // not kept: built when a screen needs it
            }
            let Some(audience) = audience_of::<G>(&next, kind, key) else {
                next.regs[kind].remove(&key);
                continue;
            };
            for u in &audience {
                viewers.entry(*u).or_insert_with(|| load_viewer::<G>(&c, &ctx, *u));
            }
            let vs: Vec<*const u8> = audience.iter().map(|u| viewers[u].ptr()).collect();
            stats.write_reads.fetch_add(1, Ordering::Relaxed);
            match load::<G>(&c, &ctx, kind, key) {
                Some(rec) => {
                    next.regs[kind].insert(key, Arc::new(build::<G>(&ctx, kind, rec.ptr(), &vs)));
                }
                None => {
                    next.regs[kind].remove(&key);
                }
            }
        }
        // audiences that may have changed: read again; a kept instance whose audience changed is
        // rebuilt (its variants are indexed by place in the audience); a viewer who left can't be
        // served it, one who joined can
        for (s, key) in audiences {
            let keys: Vec<i64> = if key == i64::MIN { next.screens[s].keys().copied().collect() } else { vec![key] };
            for key in keys {
                let Some(inst) = next.screens[s].get(&key).cloned() else { continue };
                match G::screen(&c, s, key) {
                    Some((aud, _)) if aud == inst.audience => {}
                    _ => {
                        stats.audiences.fetch_add(1, Ordering::Relaxed);
                        build_screen::<G>(&c, &ctx, &mut next, &mut viewers, s, key);
                    }
                }
            }
        }
        let mut found: HashMap<(usize, i64, i64), bool> = HashMap::new();
        for &(reg, key, item, list, field, value) in &deltas.patches {
            if let Some(k) = kept.get_mut(&(reg as usize, key)) {
                let hit = unsafe { G::patch(reg, list, field, k.ptr_mut(), item, value) };
                *found.entry((reg as usize, key, item)).or_insert(false) |= hit;
            }
        }
        for ((reg, key, _), hit) in found {
            if hit {
                dirty.insert((reg, key));
            } else {
                reload.insert((reg, key));
            }
        }
        for &(reg, row, list, field) in &deltas.finds {
            for ((k, uid), r) in kept.iter_mut() {
                if *k == reg as usize && unsafe { G::find(reg, list, field, r.ptr_mut(), row) } {
                    reload.insert((*k, *uid));
                }
            }
        }
        // screens whose lists changed: their write-time guards again (a guard may read the list)
        for (s, key) in recheck {
            if G::screen(&c, s, key).is_none() {
                next.screens[s].remove(&key);
            }
        }
        for (s, key, viewer) in warm {
            if !next.screens[s].contains_key(&key) {
                build_screen::<G>(&c, &ctx, &mut next, &mut viewers, s, key);
            }
            for kind in G::viewer_regs(s) {
                if !kept.contains_key(&(kind, viewer)) {
                    reload.insert((kind, viewer));
                }
            }
        }
        for (kind, key) in &reload {
            if let Some(rec) = load::<G>(&c, &ctx, *kind, *key) {
                kept.insert((*kind, *key), rec);
                dirty.insert((*kind, *key));
                stats.write_reads.fetch_add(1, Ordering::Relaxed);
            }
        }
        for (kind, key) in dirty {
            if let Some(r) = kept.get(&(kind, key)) {
                let v = viewers.entry(key).or_insert_with(|| load_viewer::<G>(&c, &ctx, key)).ptr();
                next.regs[kind].insert(key, Arc::new(build::<G>(&ctx, kind, r.ptr(), &[v])));
                if !reload.contains(&(kind, key)) {
                    stats.patched.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        snap.store(Arc::new(next));
        stats.batches.fetch_add(1, Ordering::Relaxed);
        stats.acts.fetch_add(replies.iter().filter(|r: &&(SyncSender<_>, Result<String, &str>)| r.1.is_ok()).count() as u64, Ordering::Relaxed);
        for (to, r) in replies {
            let _ = to.send(r);
        }
        let _ = &mut c;
    }
}

/// The audience a kept shared register is built for: the screen instance whose list holds it, or
/// whose registers include it.
fn audience_of<G: Glue>(snap: &Snap, kind: usize, key: i64) -> Option<Vec<i64>> {
    for l in 0..G::LISTS {
        if G::list_item(l) != kind {
            continue;
        }
        for (sk, items) in &snap.lists[l] {
            if items.contains(&key) {
                return snap.screens[G::list_screen(l)].get(sk).map(|i| i.audience.clone());
            }
        }
    }
    for s in 0..G::SCREENS {
        for (sk, inst) in &snap.screens[s] {
            if G::screen_regs(s, *sk).contains(&(kind, key)) {
                return Some(inst.audience.clone());
            }
        }
    }
    None
}

// ---- responses

pub fn respond(out: &mut Out, status: u16, ctype: &str, headers: &[(&str, &str)], body: &[u8]) {
    let reason = match status {
        200 => "OK",
        303 => "See Other",
        404 => "Not Found",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "",
    };
    let mut h = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: {ctype}\r\n", body.len());
    for (k, v) in headers {
        h.push_str(k);
        h.push_str(": ");
        h.push_str(v);
        h.push_str("\r\n");
    }
    out.push_own(|b| {
        b.extend_from_slice(h.as_bytes());
        b.extend_from_slice(b"Date: ");
        http_date(b);
        b.extend_from_slice(b"\r\n\r\n");
        b.extend_from_slice(body);
    });
}

pub fn redirect(out: &mut Out, to: &str, outcome: Option<&str>) {
    out.push_own(|b| {
        b.extend_from_slice(b"HTTP/1.1 303 See Other\r\nContent-Length: 0\r\nLocation: ");
        b.extend_from_slice(to.as_bytes());
        b.extend_from_slice(b"\r\n");
        if let Some(o) = outcome {
            b.extend_from_slice(b"X-Etch-Outcome: ");
            b.extend_from_slice(o.as_bytes());
            b.extend_from_slice(b"\r\n");
        }
        b.extend_from_slice(b"\r\n");
    });
}

/// application/x-www-form-urlencoded
pub fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    body.split(|&c| c == b'&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = match p.iter().position(|&c| c == b'=') {
                Some(i) => (&p[..i], &p[i + 1..]),
                None => (p, &p[p.len()..]),
            };
            (unescape(k), unescape(v))
        })
        .collect()
}

fn unescape(s: &[u8]) -> String {
    let mut o = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' => o.push(b' '),
            b'%' if i + 2 < s.len() => match std::str::from_utf8(&s[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    o.push(v);
                    i += 2;
                }
                None => o.push(b'%'),
            },
            c => o.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

/// A form field's value: an action's argument by its name, or as `__arg_<name>` (the pages'
/// hidden fields), a row's id there with its `#` or `@` mark dropped.
pub fn field<'a>(form: &'a [(String, String)], name: &str) -> Option<&'a str> {
    if let Some((_, v)) = form.iter().find(|(k, _)| k == name) {
        return Some(v.as_str());
    }
    let v = form.iter().find(|(k, _)| k.strip_prefix("__arg_") == Some(name)).map(|(_, v)| v.as_str())?;
    Some(v.strip_prefix('#').or_else(|| v.strip_prefix('@')).unwrap_or(v))
}

// ---- a generated app's main

/// Every asset's address with its content's hash, by file name.
fn asset_urls(dir: &std::path::Path) -> HashMap<String, String> {
    use sha2::{Digest, Sha256};
    let mut m = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let (Some(name), Ok(bytes)) = (p.file_name().and_then(|n| n.to_str()), std::fs::read(&p)) {
                let h = Sha256::digest(&bytes);
                let hex: String = h.iter().take(6).map(|b| format!("{b:02x}")).collect();
                m.entry(name.to_string()).or_insert(format!("/assets/{name}?v={hex}"));
            }
        }
    }
    m
}

/// Serves a generated app:
///   app --data DIR --assets DIR [--port 3000] [--host HOST] [--workers N] [--verify] [--exit-after S]
/// Configs: `host` is --host (default 127.0.0.1:PORT); `vapid_public` the data folder's web-push
/// key; any other from the environment (ETCH_CONFIG_<NAME>).
pub fn main<G: Glue>(configs: &[&str], session_cookie: &'static [u8]) {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = arg("--data").expect("--data DIR");
    let assets = arg("--assets");
    let port: u16 = arg("--port").map(|p| p.parse().expect("--port N")).unwrap_or(3000);
    let host = arg("--host").unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let workers: usize = arg("--workers").map(|w| w.parse().expect("--workers N")).unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
    if let Some(secs) = arg("--exit-after").and_then(|s| s.parse::<u64>().ok()) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            std::process::exit(0);
        });
    }
    let verify = args.iter().any(|a| a == "--verify");
    let start = std::time::Instant::now();
    let dir = std::path::Path::new(&data);
    let secret = crate::keys::secret(dir);
    let values: Vec<String> = configs
        .iter()
        .map(|c| match *c {
            "host" => host.clone(),
            "vapid_public" => std::env::var("VAPID_PUBLIC_KEY").unwrap_or_else(|_| crate::keys::vapid_public(dir)),
            other => std::env::var(format!("ETCH_CONFIG_{}", other.to_uppercase())).unwrap_or_default(),
        })
        .collect();
    let ctx = Arc::new(Ctx { signer: crate::sign::Signer::new(&secret), assets: assets.map(|a| asset_urls(std::path::Path::new(&a))).unwrap_or_default(), configs: values });
    let db_path = dir.join("app.sqlite").to_string_lossy().into_owned();
    // the next seq: past every table's
    let seq = {
        let c = open_write(&db_path);
        let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
        tables.iter().filter_map(|t| c.query_row(&format!("SELECT max(seq) FROM \"{t}\""), [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()).max().unwrap_or(0)
    };
    let app = App::<G>::start(ctx, &db_path, &host, verify, session_cookie, seq);
    println!("started in {} ms ({workers} workers{})", start.elapsed().as_millis(), if verify { ", verifying" } else { "" });
    crate::http::serve(format!("0.0.0.0:{port}").parse().unwrap(), workers, app).expect("serve");
}
