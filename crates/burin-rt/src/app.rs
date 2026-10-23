//! The server every app runs, the same for every app: the fast path, the cold path (the golden
//! model), the writer and its delta edges. What is app-specific is generated (`Glue`), and called
//! by static dispatch, so it inlines.
//!
//! A page is merged from two kinds of parts: cached pieces (fragments that never read the viewer:
//! built once at write time, kept, precompressed, borrowed into every response) and fragments
//! rendered for the viewer per request (into reused per-thread buffers), which themselves merge
//! the cached pieces they call. Nothing per viewer is kept.

use crate::abi::{Arena, Ctx, Db, Deltas, Writer};
use crate::out::{push_int, Out};
use crate::store::{Part, Reg, Seg, Snap};
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
}

/// What the compiler generates for an app.
///
/// # Safety
/// The record pointers passed to these functions are records of the register kind named.
pub unsafe trait Glue: Send + Sync + 'static {
    const REGS: usize;
    const LISTS: usize;
    const SCREENS: usize;
    /// Register kinds rendered per request (they read the viewer); the rest are cached.
    fn per_request(kind: usize) -> bool;
    /// The register kind a screen list's items are.
    fn list_item(list: usize) -> usize;
    /// The screen (and its parameter) a screen list belongs to.
    fn list_screen(list: usize) -> usize;
    /// A screen list's bound (its window or limit).
    fn list_bound(list: usize) -> usize;
    /// A live list's view (its page element is `live_<view>`), None for one that isn't live.
    fn list_live(list: usize) -> Option<&'static str>;
    /// The action a screen's live stream runs as its viewer when it opens and closes.
    fn watch(screen: usize) -> Option<(&'static str, &'static str)>;
    /// A request's screen and its key, from its path.
    fn route(path: &[u8]) -> Option<(usize, i64)>;
    /// Every session: its row's id, its token, its viewer.
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
    /// Screens rendered per request, for a viewer (settings pages: `cold`).
    fn cold(s: usize) -> bool;
    /// A screen's guards in order, for a viewer: Ok, or the first unmet one's answer (a redirect,
    /// or None: not found).
    fn guards(c: &Connection, s: usize, key: i64, viewer: i64) -> Result<(), Option<String>>;
    /// Where "/" goes for a viewer (the request's cookies, as ids).
    fn home(c: &Connection, viewer: i64, cookie: &dyn Fn(&str) -> Option<i64>) -> Option<String>;
    /// A register's record size, its loader and its builder.
    fn rec_size(kind: usize) -> usize;
    /// # Safety: `rec` is a zeroed record of `kind`'s size.
    unsafe fn load(kind: usize, ctx: &Ctx, db: *mut Db, key: i64, viewer: i64, rec: *mut u8, arena: *mut Arena) -> bool;
    /// # Safety: `rec` is a loaded record of `kind`; `viewer` a viewer record (or null, for a
    /// cached register: it never reads it); `tmps` has `tmps(kind)` buffers.
    unsafe fn build(kind: usize, ctx: &Ctx, rec: *const u8, viewer: *const u8, out: &mut Vec<u8>, tmps: &[*mut Vec<u8>], pieces: &mut Vec<usize>);
    fn tmps(kind: usize) -> usize;
    fn viewer_size() -> usize;
    /// # Safety: `rec` a zeroed viewer record.
    unsafe fn load_viewer(ctx: &Ctx, db: *mut Db, key: i64, rec: *mut u8, arena: *mut Arena) -> bool;
    /// A screen instance: its audience (sorted) and headers; None when its write-time guards
    /// don't hold.
    fn screen(c: &Connection, s: usize, key: i64) -> Option<(Vec<i64>, Vec<u8>)>;
    /// The cached registers a screen instance needs, keyed by its parameter: (kind, key).
    fn screen_regs(s: usize, key: i64) -> Vec<(usize, i64)>;
    /// The screen's lists' items, from SQLite: (list, item keys).
    fn screen_lists(c: &Connection, s: usize, key: i64) -> Vec<(usize, Vec<i64>)>;
    /// The page's parts in order; false when a cached piece isn't in the snapshot.
    fn assemble<'a>(snap: &'a Snap, s: usize, key: i64, emit: &mut dyn FnMut(Part<'a>)) -> bool;
    /// Runs an action (its form fields); Ok(0) or its outcome's name.
    fn act(name: &str, c: &Connection, ctx: &Ctx, db: *mut Db, w: &mut Writer, viewer: i64, vrec: *const u8, form: &[(String, String)], d: &mut Deltas) -> Result<(), &'static str>;
    /// The page to go to after an action (its form fields).
    fn after_act(name: &str, form: &[(String, String)]) -> String;
    /// The tables the app keeps its entities in (created if missing).
    fn schema() -> &'static str;
    /// Each field's column: (table, column, the ALTER TABLE that adds it).
    fn columns() -> &'static [(&'static str, &'static str, &'static str)];
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

/// Columns for fields added since the database was made (a number starts at 0).
fn add_columns<G: Glue>(c: &Connection) {
    for (table, col, add) in G::columns() {
        let has: bool = c.prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?")).and_then(|mut st| st.exists([col])).expect("table info");
        if !has {
            c.execute_batch(add).unwrap_or_else(|e| panic!("adding {table}.{col}: {e}"));
            eprintln!("added the column {table}.{col}");
        }
    }
}

/// A register's record, loaded from SQLite (a read); `viewer` for one rendered per request.
pub fn load<G: Glue>(c: &Connection, ctx: &Ctx, kind: usize, key: i64, viewer: i64) -> Option<Rec> {
    let (rec, p, a) = Rec::new(G::rec_size(kind));
    let ok = with_db(c, |db| unsafe { G::load(kind, ctx, db, key, viewer, p, a) });
    ok.then_some(rec)
}

/// A viewer's record (a read): the forwarded records' values, a per-request fragment's viewer.
pub fn load_viewer<G: Glue>(c: &Connection, ctx: &Ctx, key: i64) -> Rec {
    let (rec, p, a) = Rec::new(G::viewer_size());
    with_db(c, |db| unsafe { G::load_viewer(ctx, db, key, p, a) });
    rec
}

/// A cached register's bytes, as they are.
fn build_raw<G: Glue>(ctx: &Ctx, kind: usize, rec: *const u8) -> Vec<u8> {
    let mut bufs: Vec<Vec<u8>> = (0..G::tmps(kind).max(1)).map(|_| Vec::with_capacity(256)).collect();
    let ptrs: Vec<*mut Vec<u8>> = bufs.iter_mut().map(|b| b as *mut Vec<u8>).collect();
    let mut out = Vec::with_capacity(4096);
    let mut pieces = vec![];
    unsafe { G::build(kind, ctx, rec, std::ptr::null(), &mut out, &ptrs, &mut pieces) };
    out
}

/// A cached register's value from its record: one piece, compressed once.
pub fn build<G: Glue>(ctx: &Ctx, kind: usize, rec: *const u8) -> Reg {
    // (a list item keeps its element's id: Reg::plain)
    let raw = build_raw::<G>(ctx, kind, rec);
    // a list's item is sent inside its list's block: not compressed on its own
    if (0..G::LISTS).any(|l| G::list_item(l) == kind) { Reg::plain(raw) } else { Reg::one(raw) }
}

/// Builds a screen instance into `snap` from SQLite: its audience and headers, the cached
/// registers keyed by its parameter, its lists and their items. False if its write-time guards
/// don't hold.
pub fn build_screen<G: Glue>(c: &Connection, ctx: &Ctx, snap: &mut Snap, s: usize, key: i64) -> bool {
    let (audience, headers) = match G::screen(c, s, key) {
        Some(x) => x,
        None => {
            snap.screens[s].remove(&key);
            return false;
        }
    };
    for (kind, k) in G::screen_regs(s, key) {
        if let Some(rec) = load::<G>(c, ctx, kind, k, 0) {
            snap.regs[kind].insert(k, Arc::new(build::<G>(ctx, kind, rec.ptr())));
        }
    }
    for (list, items) in G::screen_lists(c, s, key) {
        let kind = G::list_item(list);
        for id in &items {
            if let Some(rec) = load::<G>(c, ctx, kind, *id, 0) {
                snap.regs[kind].insert(*id, Arc::new(build::<G>(ctx, kind, rec.ptr())));
            }
        }
        snap.lists[list].insert(key, Arc::new(items));
    }
    snap.screens[s].insert(key, Arc::new(crate::store::Screen { audience, headers }));
    true
}

// ---- merging a page: cached pieces and fragments rendered for the viewer

/// Where a page's parts go: a cached piece (borrowed: its bytes outlive the response) or bytes
/// made for this request.
pub trait Sink<'a> {
    fn seg(&mut self, s: &'a Seg);
    fn bytes(&mut self, b: &[u8]);
}

/// A gzip response: cached pieces' precompressed bytes, borrowed; bytes made for this request in
/// stored (uncompressed) blocks; the CRC combined from the pieces' and continued over the rest.
struct GzOut<'o> {
    out: &'o mut Out,
    crc: crate::gz::Crc,
}

impl<'a> Sink<'a> for GzOut<'_> {
    fn seg(&mut self, s: &'a Seg) {
        let Some(gz) = &s.gz else { return self.bytes(&s.raw) };
        // Safety: the response holds the snapshot the piece is from
        unsafe { self.out.push_borrowed(&gz.deflate) };
        self.crc.add(gz);
    }
    fn bytes(&mut self, b: &[u8]) {
        for chunk in b.chunks(65535) {
            self.out.push_own(|o| {
                o.extend_from_slice(&crate::gz::stored_header(chunk.len()));
                o.extend_from_slice(chunk);
            });
            self.crc.add_raw(chunk);
        }
    }
}

/// A plain response: pieces borrowed, the rest copied in.
struct RawOut<'o> {
    out: &'o mut Out,
}

impl<'a> Sink<'a> for RawOut<'_> {
    fn seg(&mut self, s: &'a Seg) {
        unsafe { self.out.push_borrowed(&s.raw) };
    }
    fn bytes(&mut self, b: &[u8]) {
        self.out.push_own(|o| o.extend_from_slice(b));
    }
}

/// A page in one buffer (the cold path, the golden model, the scrubber).
struct VecSink(Vec<u8>);

impl<'a> Sink<'a> for VecSink {
    fn seg(&mut self, s: &'a Seg) {
        self.0.extend_from_slice(&s.raw);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// A thread's buffers for rendering per-request fragments, reused (no allocation once warm).
struct Scratch {
    rec: Vec<u64>,
    arena: Arena,
    viewer: Vec<u64>,
    varena: Arena,
    out: Vec<u8>,
    pieces: Vec<usize>,
    tmps: Vec<Vec<u8>>,
    ptrs: Vec<*mut Vec<u8>>,
}

thread_local! {
    static SCRATCH: RefCell<Option<Scratch>> = const { RefCell::new(None) };
}

impl Scratch {
    fn new() -> Scratch {
        Scratch { rec: Vec::with_capacity(512), arena: Arena::new(), viewer: Vec::with_capacity(64), varena: Arena::new(), out: Vec::with_capacity(65536), pieces: Vec::with_capacity(256), tmps: vec![], ptrs: vec![] }
    }

    fn load_viewer<G: Glue>(&mut self, c: &Connection, ctx: &Ctx, viewer: i64) {
        self.viewer.clear();
        self.viewer.resize(G::viewer_size().div_ceil(8).max(1), 0);
        self.varena.reset();
        let (p, a) = (self.viewer.as_mut_ptr() as *mut u8, &mut self.varena as *mut Arena);
        with_db(c, |db| unsafe { G::load_viewer(ctx, db, viewer, p, a) });
    }

    /// A per-request fragment rendered: its bytes in `out`, its cached pieces' places in `pieces`.
    fn render<G: Glue>(&mut self, c: &Connection, ctx: &Ctx, kind: usize, key: i64, viewer: i64) -> bool {
        self.rec.clear();
        self.rec.resize(G::rec_size(kind).div_ceil(8).max(1), 0);
        self.arena.reset();
        let (p, a) = (self.rec.as_mut_ptr() as *mut u8, &mut self.arena as *mut Arena);
        if !with_db(c, |db| unsafe { G::load(kind, ctx, db, key, viewer, p, a) }) {
            return false;
        }
        self.out.clear();
        self.pieces.clear();
        while self.tmps.len() < G::tmps(kind).max(1) {
            self.tmps.push(Vec::with_capacity(256));
        }
        self.ptrs.clear();
        for t in self.tmps.iter_mut() {
            t.clear();
            self.ptrs.push(t as *mut Vec<u8>);
        }
        unsafe { G::build(kind, ctx, self.rec.as_ptr() as *const u8, self.viewer.as_ptr() as *const u8, &mut self.out, &self.ptrs, &mut self.pieces) };
        true
    }
}

/// `<turbo-stream-source src="/_burin/live?s=…&k=…&since=…">`, written into `b` (no allocation):
/// its length.
fn live_tag(b: &mut [u8; 160], s: usize, key: i64, since: u64) -> usize {
    use std::io::Write;
    let mut w = &mut b[..];
    let _ = write!(w, "<turbo-stream-source src=\"/_burin/live?s={s}&amp;k={key}&amp;since={since}\"></turbo-stream-source>");
    160 - w.len()
}

/// A page's parts, in order, to `sink`: cached pieces from `snap`; fragments rendered for the
/// viewer now, each merged with the cached pieces it calls (a piece the snapshot doesn't keep is
/// built now, as it is, and noted in `keep` for the writer to keep). False when a part can't be
/// made (a cached piece of the page itself missing, a row gone).
pub fn emit_page<'a, G: Glue>(snap: &'a Snap, c: &Connection, ctx: &Ctx, s: usize, key: i64, viewer: i64, sink: &mut dyn Sink<'a>, keep: &mut Vec<(usize, i64)>) -> bool {
    SCRATCH.with(|sc| {
        let mut guard = sc.borrow_mut();
        let sc = guard.get_or_insert_with(Scratch::new);
        let (mut ok, mut viewer_loaded) = (true, false);
        let done = G::assemble(snap, s, key, &mut |part| {
            if !ok {
                return;
            }
            match part {
                Part::Seg(seg) => sink.seg(seg),
                Part::Live => {
                    let mut b = [0u8; 160];
                    let n = live_tag(&mut b, s, key, snap.live_seq);
                    sink.bytes(&b[..n]);
                }
                Part::Render(kind, k) => {
                    if !viewer_loaded {
                        sc.load_viewer::<G>(c, ctx, viewer);
                        viewer_loaded = true;
                    }
                    if !sc.render::<G>(c, ctx, kind, k, viewer) {
                        ok = false;
                        return;
                    }
                    let mut at = 0;
                    for pair in sc.pieces.chunks(2) {
                        let (place, pk) = crate::abi::split_at(pair[0]);
                        let pkey = pair[1] as i64;
                        if place > at {
                            sink.bytes(&sc.out[at..place]);
                        }
                        at = place;
                        match snap.reg(pk, pkey) {
                            Some(r) => sink.seg(&r.seg),
                            None => match load::<G>(c, ctx, pk, pkey, 0) {
                                Some(rec) => {
                                    sink.bytes(&build_raw::<G>(ctx, pk, rec.ptr()));
                                    keep.push((pk, pkey));
                                }
                                None => {
                                    ok = false;
                                    return;
                                }
                            },
                        }
                    }
                    if at < sc.out.len() {
                        sink.bytes(&sc.out[at..]);
                    }
                }
            }
        });
        done && ok
    })
}

// ---- the server

pub enum Cmd {
    Act { name: String, viewer: i64, form: Vec<(String, String)>, reply: SyncSender<Result<Done, &'static str>> },
    /// a screen instance to build (a read missed it)
    Warm { screen: usize, key: i64 },
    /// a cached piece a per-request fragment merged that the snapshot didn't keep
    Keep { kind: usize, key: i64 },
}

/// A done action: where it goes, and its session cookie (Some(token): signed in; Some(empty):
/// signed out).
pub struct Done {
    pub to: String,
    pub cookie: Option<Vec<u8>>,
    /// where the action said to go (`go`)
    pub go: Option<String>,
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
    /// registers built from forwarded records: no read
    pub forwarded: AtomicU64,
    /// fragments rendered for a viewer (per request)
    pub rendered: AtomicU64,
    /// writes that cleared every screen (an edge the writer has no rule for): should stay 0
    pub cleared: AtomicU64,
    /// screen instances whose audience changed
    pub audiences: AtomicU64,
    /// list blocks made, and the writer's microseconds compressing them
    pub blocks: AtomicU64,
    pub block_us: AtomicU64,
    /// batches that changed nothing kept (no new snapshot)
    pub quiet: AtomicU64,
    /// live events published (each to the open pages on its topic)
    pub live: AtomicU64,
    /// kept pages the scrubber compared with a fresh build, and the ones it found different (and
    /// rebuilt): a missed edge
    pub scrubbed: AtomicU64,
    pub scrub_fixed: AtomicU64,
    /// verified pages a concurrent write made incomparable (not counted as verified)
    pub verify_raced: AtomicU64,
    /// the writer's commits, and the commits its published snapshot includes
    pub commits: AtomicU64,
    pub published: AtomicU64,
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
    /// the static files (`--assets`), set once at start
    pub files: std::sync::OnceLock<crate::files::Files>,
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

fn page_response(out: &mut Out, body: &[u8], headers: &[u8]) {
    out.push_own(|b| {
        b.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: ");
        push_int(b, body.len() as u64);
        b.extend_from_slice(b"\r\n");
        b.extend_from_slice(headers);
        b.extend_from_slice(b"Date: ");
        http_date(b);
        b.extend_from_slice(b"\r\n\r\n");
        b.extend_from_slice(body);
    });
}

impl<G: Glue> App<G> {
    /// Starts the writer (and the checkpointer); returns the app to serve.
    pub fn start(ctx: Arc<Ctx>, db_path: &str, host: &str, verify: bool, session_cookie: &'static [u8], seq: i64, scrub_ms: u64) -> Arc<App<G>> {
        let w = open_write(db_path);
        w.execute_batch(G::schema()).expect("the schema");
        add_columns::<G>(&w);
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
        snap.sessions = G::sessions(&w).into_iter().map(|(_, t, u)| (t.into_bytes().into_boxed_slice(), u)).collect();
        let snap = Arc::new(ArcSwap::from_pointee(snap));
        let (tx, rx) = std::sync::mpsc::sync_channel(1024);
        let stats = Arc::new(Stats::default());
        {
            let (ctx, snap, stats) = (ctx.clone(), snap.clone(), stats.clone());
            std::thread::Builder::new().name("burin-writer".into()).spawn(move || writer::<G>(w, ctx, snap, rx, stats, seq, scrub_ms)).unwrap();
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
        Arc::new(App { ctx, snap, writer: tx, db_path: db_path.to_string(), host: host.to_string(), verify, stats, session_cookie, files: std::sync::OnceLock::new(), _g: std::marker::PhantomData })
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

    /// The fast path: a screen instance from the snapshot, its cached pieces borrowed, its
    /// per-request fragments rendered for the viewer.
    pub fn fast(&self, req: &Req, out: &mut Out) -> bool {
        if req.method != b"GET" || !req.query.is_empty() || req.cookie(b"flash").is_some() {
            return false;
        }
        let (s, key) = match G::route(req.path) {
            Some(r) if !G::cold(r.0) => r,
            _ => return false,
        };
        let token = match req.cookie(self.session_cookie) {
            Some(t) => t,
            None => return false,
        };
        // (verifying: the writer's commits and publishes before the page, to tell a race from a miss)
        let marks = if self.verify { (self.stats.commits.load(Ordering::SeqCst), self.stats.published.load(Ordering::SeqCst)) } else { (0, 0) };
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
                self.send(Cmd::Warm { screen: s, key });
                return false;
            }
        };
        if inst.audience.binary_search(&uid).is_err() {
            return false; // not in its audience: the cold path answers
        }
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
        let mut keep = Vec::new();
        let ok = self.with_reader(|c| {
            if req.gzip {
                out.push_static(&crate::gz::HEADER);
                let mut gz = GzOut { out: &mut *out, crc: crate::gz::Crc::default() };
                let ok = emit_page::<G>(&snap, c, &self.ctx, s, key, uid, &mut gz, &mut keep);
                let trailer = gz.crc.trailer();
                out.push_static(&crate::gz::FINAL);
                out.push_own(|b| b.extend_from_slice(&trailer));
                ok
            } else {
                emit_page::<G>(&snap, c, &self.ctx, s, key, uid, &mut RawOut { out: &mut *out }, &mut keep)
            }
        });
        for (kind, k) in keep {
            self.send(Cmd::Keep { kind, key: k });
        }
        if !ok {
            self.send(Cmd::Warm { screen: s, key });
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
            self.check(s, key, uid, out, body_len, req.gzip, marks);
        }
        out.hold(snap);
        self.stats.fast.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The golden-model check: the fast page must equal a fresh build from SQLite.
    fn check(&self, s: usize, key: i64, uid: i64, out: &Out, body_len: usize, gzip: bool, (c0, p0): (u64, u64)) {
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
        let fresh = self.with_reader(|c| fresh::<G>(c, &self.ctx, s, key, uid));
        // a write committed while this page was served or built afresh (or committed but not yet
        // published when it was served): the two can't be compared
        if c0 != p0 || self.stats.commits.load(Ordering::SeqCst) != c0 {
            self.stats.verify_raced.fetch_add(1, Ordering::Relaxed);
            return;
        }
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

    fn cold(&self, req: &Req, out: &mut Out) {
        self.stats.cold.fetch_add(1, Ordering::Relaxed);
        let path = req.path;
        if req.method == b"GET" && path == b"/up" {
            return respond(out, 200, "text/plain", &[], b"ok");
        }
        if req.method == b"GET" && path == b"/_burin/stats" {
            let st = &self.stats;
            let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
            let s = format!(
                r#"{{"fast":{},"cold":{},"verified":{},"mismatched":{},"refused":{},"batches":{},"acts":{},"write_reads":{},"forwarded":{},"rendered":{},"cleared":{},"audiences":{},"scrubbed":{},"scrub_fixed":{},"verify_raced":{},"blocks":{},"block_us":{},"quiet":{},"live":{},"kept":{},"version":{}}}"#,
                g(&st.fast), g(&st.cold), g(&st.verified), g(&st.mismatched), g(&st.refused), g(&st.batches), g(&st.acts), g(&st.write_reads), g(&st.forwarded), g(&st.rendered), g(&st.cleared), g(&st.audiences), g(&st.scrubbed), g(&st.scrub_fixed), g(&st.verify_raced), g(&st.blocks), g(&st.block_us), g(&st.quiet), g(&st.live),
                self.snap.load().regs.iter().map(|r| r.len()).sum::<usize>(), self.snap.load().version
            );
            return respond(out, 200, "application/json", &[], s.as_bytes());
        }
        let viewer = self.viewer_of(req);
        if req.method == b"POST" {
            if let Some(name) = path.strip_prefix(b"/_do/") {
                let name = String::from_utf8_lossy(name).into_owned();
                let mut form = match multipart_boundary(req.content_type) {
                    Some(b) => parse_multipart(req.body, b),
                    None => parse_form(req.body),
                };
                // hidden arguments come only from here, never from the form
                form.retain(|(k, _)| !k.starts_with("__verified") && !k.starts_with("__hashed") && k != "__session");
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
                    Ok(Ok(Done { to, cookie: Some(token), .. })) => {
                        let name = String::from_utf8_lossy(self.session_cookie);
                        let (to, c) = if token.is_empty() {
                            ("/session/new".to_string(), format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"))
                        } else {
                            let _ = to;
                            ("/".to_string(), format!("{name}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000", String::from_utf8_lossy(&token)))
                        };
                        redirect_with(out, &to, Some("ok"), &[("Set-Cookie", &c)])
                    }
                    Ok(Ok(Done { go: Some(to), cookie: None, .. })) => redirect(out, &to, Some("ok")),
                    Ok(Ok(Done { to, cookie: None, go: None })) => redirect(out, &back.unwrap_or(to), Some("ok")),
                    Ok(Err(e)) => redirect(out, &back.unwrap_or_else(|| "/".into()), Some(e)),
                    Err(_) => respond(out, 500, "text/plain", &[], b"writer stopped"),
                };
            }
        }
        if req.method == b"GET" && path == b"/" {
            let Some(viewer) = viewer else { return redirect(out, "/session/new", None) };
            let cookie = |n: &str| req.cookie(n.as_bytes()).and_then(|v| std::str::from_utf8(v).ok()).and_then(|v| v.parse().ok());
            return match self.with_reader(|c| G::home(c, viewer, &cookie)) {
                Some(to) => redirect(out, &to, None),
                None => respond(out, 404, "text/plain", &[], b"no rooms"),
            };
        }
        if req.method == b"GET" {
            if let Some(s) = G::public_screen(path) {
                return match self.with_reader(|c| per_request::<G>(c, &self.ctx, s, 0, 0)) {
                    Ok((body, headers)) => page_response(out, &body, &headers),
                    Err(_) => respond(out, 500, "text/plain", &[], b"the page couldn't be built"),
                };
            }
            if let Some((s, key)) = G::route(path).filter(|(s, _)| G::cold(*s)) {
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", None),
                };
                return match self.with_reader(|c| G::guards(c, s, key, viewer).and_then(|_| per_request::<G>(c, &self.ctx, s, key, viewer))) {
                    Ok((body, headers)) => page_response(out, &body, &headers),
                    Err(Some(to)) => redirect(out, &to, None),
                    Err(None) => respond(out, 404, "text/plain", &[], b"not found"),
                };
            }
            if let Some((s, key)) = G::route(path) {
                let viewer = match viewer {
                    Some(v) => v,
                    None => return redirect(out, "/session/new", None),
                };
                let r = self.with_reader(|c| match fresh::<G>(c, &self.ctx, s, key, viewer) {
                    Some(p) => Ok(p),
                    None => Err(G::refuse(c, s, key, viewer)),
                });
                return match r {
                    Ok((body, headers)) => page_response(out, &body, &headers),
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
        // a session the snapshot hasn't seen yet: SQLite, by its token
        let t = String::from_utf8_lossy(t).into_owned();
        self.with_reader(|c| G::session(c, &t))
    }
}

/// A page built fresh from SQLite (the cold path, and the golden model): None when the viewer
/// isn't in its audience or its guards don't hold.
fn fresh<G: Glue>(c: &Connection, ctx: &Ctx, s: usize, key: i64, uid: i64) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
    // the live events it includes: read before the database is, so one that commits meanwhile is
    // sent again (harmless: Turbo replaces an element of the same id) rather than missed
    snap.live_seq = crate::live::HUB.last();
    if !build_screen::<G>(c, ctx, &mut snap, s, key) {
        return None;
    }
    let inst = snap.screens[s].get(&key)?.clone();
    inst.audience.binary_search(&uid).ok()?;
    let mut page = VecSink(Vec::with_capacity(400_000));
    emit_page::<G>(&snap, c, ctx, s, key, uid, &mut page, &mut vec![]).then_some((page.0, inst.headers.clone()))
}

/// A page built per request (a cold screen, a public one): its cached pieces built now too.
fn per_request<G: Glue>(c: &Connection, ctx: &Ctx, s: usize, key: i64, viewer: i64) -> Result<(Vec<u8>, Vec<u8>), Option<String>> {
    let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);
    snap.live_seq = crate::live::HUB.last();
    for (kind, k) in G::screen_regs(s, key) {
        let rec = load::<G>(c, ctx, kind, k, 0).ok_or(None)?;
        snap.regs[kind].insert(k, Arc::new(build::<G>(ctx, kind, rec.ptr())));
    }
    for (list, items) in G::screen_lists(c, s, key) {
        let kind = G::list_item(list);
        for id in &items {
            if let Some(rec) = load::<G>(c, ctx, kind, *id, 0) {
                snap.regs[kind].insert(*id, Arc::new(build::<G>(ctx, kind, rec.ptr())));
            }
        }
        snap.lists[list].insert(key, Arc::new(items));
    }
    let (_, headers) = G::screen(c, s, key).ok_or(None)?;
    let mut page = VecSink(Vec::with_capacity(65536));
    if emit_page::<G>(&snap, c, ctx, s, key, viewer, &mut page, &mut vec![]) {
        Ok((page.0, headers))
    } else {
        Err(None)
    }
}

impl<G: Glue> App<G> {
    /// A watched screen's action, as `viewer` (a stream opened or closed): not waited for.
    fn watch(&self, s: usize, key: i64, viewer: i64) {
        if let Some((name, param)) = G::watch(s) {
            let (reply, _) = std::sync::mpsc::sync_channel(1);
            self.send(Cmd::Act { name: name.to_string(), viewer, form: vec![(param.to_string(), key.to_string())], reply });
        }
    }

    /// A static file: gzip when the browser takes it, kept for a year (its link names its hash).
    fn file(&self, req: &Req, out: &mut Out) {
        let Some(f) = self.files.get().and_then(|fs| fs.get(req.path)) else {
            return respond(out, 404, "text/plain", &[], b"not found");
        };
        let (body, gz) = match (&f.gz, req.gzip) {
            (Some(g), true) => (g, true),
            _ => (&f.raw, false),
        };
        out.push_own(|b| {
            b.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: ");
            push_int(b, body.len() as u64);
            b.extend_from_slice(b"\r\nContent-Type: ");
            b.extend_from_slice(f.ctype.as_bytes());
            b.extend_from_slice(b"\r\nCache-Control: public, max-age=31536000, immutable\r\n");
            if f.gz.is_some() {
                b.extend_from_slice(b"Vary: Accept-Encoding\r\n");
            }
            if gz {
                b.extend_from_slice(b"Content-Encoding: gzip\r\n");
            }
            b.extend_from_slice(b"Date: ");
            http_date(b);
            b.extend_from_slice(b"\r\n\r\n");
            b.extend_from_slice(body);
        });
    }
}

impl<G: Glue> crate::Handler for App<G> {
    /// `/_burin/live?s=&k=&since=`: the screen instance's live lists, for a viewer in its
    /// audience; first what they missed since the page (or the browser's last event).
    fn live(&self, req: &Req) -> Option<crate::http::Live> {
        let q = |name: &str| parse_form(req.query).into_iter().find(|(k, _)| k == name).and_then(|(_, v)| v.parse::<i64>().ok());
        let (s, key, since) = (q("s")? as usize, q("k")?, q("since").unwrap_or(0).max(0) as u64);
        if s >= G::SCREENS {
            return None;
        }
        let viewer = self.viewer_of(req)?;
        let snap = self.snap.load();
        let inst = snap.screens[s].get(&key)?;
        inst.audience.binary_search(&viewer).ok()?;
        let topics: Vec<u64> = (0..G::LISTS).filter(|&l| G::list_screen(l) == s && G::list_live(l).is_some()).map(|l| crate::live::list_topic(l, key)).collect();
        let since = since.max(req.last_event_id);
        let mut head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\n\r\nretry: 2000\n\n".to_vec();
        let last = match crate::live::HUB.replay(&topics, since) {
            crate::live::Replay::Events(es) => {
                let mut last = since;
                for e in es {
                    head.extend_from_slice(&e.bytes);
                    last = last.max(e.id);
                }
                last
            }
            crate::live::Replay::Refresh => {
                let last = crate::live::HUB.last();
                head.extend_from_slice(&crate::live::event_bytes(last, &crate::live::turbo_stream("refresh", "", None)));
                last
            }
        };
        self.watch(s, key, viewer);
        Some(crate::http::Live { topics, viewer, head, last, tag: (s, key) })
    }

    fn live_closed(&self, live: &crate::http::Live) {
        self.watch(live.tag.0, live.tag.1, live.viewer);
    }

    fn handle(&self, req: &Req, out: &mut Out) {
        if req.method == b"GET" && req.path.starts_with(b"/assets/") {
            return self.file(req, out);
        }
        if !self.fast(req, out) {
            out.reset();
            self.cold(req, out);
        }
    }
}

// ---- the writer

/// The writer: actions in batches (group commit), then their delta edges into the next snapshot.
/// It keeps only cached pieces: nothing per viewer, so a write's work doesn't grow with the
/// people who can see it.
fn writer<G: Glue>(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>, seq: i64, scrub_ms: u64) {
    let mut w = Writer::new(seq);
    // viewers' records: an action's viewer (forwarding copies values from it)
    let mut viewers: HashMap<i64, Rec> = HashMap::new();
    // the sessions' tokens by their row's id (a deleted row's token is forgotten by its id)
    let mut tokens: HashMap<i64, Box<[u8]>> = G::sessions(&c).into_iter().map(|(id, t, _)| (id, t.into_bytes().into_boxed_slice())).collect();
    let mut batch = Vec::with_capacity(256);
    // the scrubber's place in the kept pages
    let mut cursor = 0usize;
    loop {
        let first = if scrub_ms == 0 {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        } else {
            match rx.recv_timeout(std::time::Duration::from_millis(scrub_ms)) {
                Ok(c) => c,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    scrub::<G>(&c, &ctx, &snap, &stats, &mut cursor);
                    continue;
                }
                Err(_) => break,
            }
        };
        batch.push(first);
        while batch.len() < 256 {
            match rx.try_recv() {
                Ok(c) => batch.push(c),
                Err(_) => break,
            }
        }
        let t0 = std::time::Instant::now();
        let n_cmds = batch.len();
        let cur = snap.load_full();
        let mut replies = vec![];
        let mut deltas = Deltas::default();
        let (mut warm, mut keep): (Vec<(usize, i64)>, Vec<(usize, i64)>) = (vec![], vec![]);
        w.begin(crate::time::now_ms());
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).expect("begin");
        for cmd in batch.drain(..) {
            match cmd {
                Cmd::Act { name, viewer, form, reply } => {
                    let vrec = viewers.entry(viewer).or_insert_with(|| load_viewer::<G>(&tx, &ctx, viewer)).ptr();
                    // each action is all or nothing: a savepoint, rolled back (with its delta
                    // edges) when it doesn't end ok, a failed require after a write or a write
                    // a constraint refused
                    let marks = deltas.list.len();
                    tx.execute_batch("SAVEPOINT act").expect("savepoint");
                    let mut r = with_db(&tx, |db| G::act(&name, &tx, &ctx, db, &mut w, viewer, vrec, &form, &mut deltas));
                    if let Some(e) = crate::abi::take_write_failure() {
                        eprintln!("action {name}: a write failed ({e}); rolled back");
                        r = Err("invalid");
                    }
                    let cookie = match (w.signed_in.take(), std::mem::take(&mut w.signed_out)) {
                        (Some(t), _) => Some(t),
                        (None, true) => Some(vec![]),
                        _ => None,
                    };
                    let go = w.go.take();
                    if r.is_ok() {
                        tx.execute_batch("RELEASE act").expect("release");
                    } else {
                        tx.execute_batch("ROLLBACK TO act; RELEASE act").expect("rollback");
                        deltas.list.truncate(marks);
                    }
                    replies.push((reply, r.map(|_| Done { to: G::after_act(&name, &form), cookie, go })));
                }
                Cmd::Warm { screen, key } => warm.push((screen, key)),
                Cmd::Keep { kind, key } => keep.push((kind, key)),
            }
        }
        tx.commit().expect("commit");
        stats.commits.fetch_add(1, Ordering::SeqCst);
        let t_commit = t0.elapsed();
        // a batch that changed nothing kept (a member marking a room read: no piece, list or
        // audience reads what it wrote): the published snapshot is still right, so it isn't
        // copied and swapped (copying it is most of such a batch's cost)
        if warm.is_empty() && keep.is_empty() && deltas.list.iter().all(|d| matches!(d.0, 1 | 6 | 15)) {
            for &(op, _, key, _) in &deltas.list {
                if op == 15 {
                    if key == i64::MIN {
                        viewers.clear();
                    } else {
                        viewers.remove(&key);
                    }
                }
            }
            stats.published.store(stats.commits.load(Ordering::SeqCst), Ordering::SeqCst);
            stats.batches.fetch_add(1, Ordering::Relaxed);
            stats.quiet.fetch_add(1, Ordering::Relaxed);
            stats.acts.fetch_add(replies.iter().filter(|r: &&(SyncSender<_>, Result<Done, &str>)| r.1.is_ok()).count() as u64, Ordering::Relaxed);
            for (to, r) in replies {
                let _ = to.send(r);
            }
            continue;
        }
        let mut next = cur.next();
        // what open pages are sent (topic, Turbo Stream), and audiences that changed
        let mut live: Vec<(u64, Vec<u8>)> = vec![];
        let mut revokes: Vec<(u64, Arc<Vec<i64>>)> = vec![];
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
                    // a new item in a kept screen list: its piece built (from the forwarded
                    // record, or read), added in order
                    let s = G::list_screen(a);
                    if !next.screens[s].contains_key(&key) {
                        continue;
                    }
                    let kind = G::list_item(a);
                    let reg = match forwarded.get(&item) {
                        Some(rec) => {
                            stats.forwarded.fetch_add(1, Ordering::Relaxed);
                            build::<G>(&ctx, kind, *rec as *const u8)
                        }
                        None => {
                            stats.write_reads.fetch_add(1, Ordering::Relaxed);
                            match load::<G>(&c, &ctx, kind, item, 0) {
                                Some(rec) => build::<G>(&ctx, kind, rec.ptr()),
                                None => continue,
                            }
                        }
                    };
                    let view = G::list_live(a);
                    if let Some(v) = view {
                        let target = format!("live_{v}");
                        live.push((crate::live::list_topic(a, key), crate::live::turbo_stream(if op == 2 { "append" } else { "prepend" }, &target, Some(&reg.seg.raw))));
                    }
                    next.regs[kind].insert(item, Arc::new(reg));
                    let mut items: Vec<i64> = next.lists[a].get(&key).map(|l| (**l).clone()).unwrap_or_default();
                    // oldest first (op 2): appended, the oldest drops; newest first (op 7): first, the last drops
                    let mut gone = vec![];
                    if op == 2 {
                        items.push(item);
                        while items.len() > G::list_bound(a).max(1) {
                            let old = items.remove(0);
                            gone.extend(next.regs[kind].remove(&old));
                        }
                    } else {
                        items.insert(0, item);
                        while items.len() > G::list_bound(a).max(1) {
                            let old = items.pop().unwrap();
                            gone.extend(next.regs[kind].remove(&old));
                        }
                    }
                    if view.is_some() {
                        for r in gone {
                            if let Some(id) = &r.dom_id {
                                live.push((crate::live::list_topic(a, key), crate::live::turbo_stream("remove", id, None)));
                            }
                        }
                    }
                    next.lists[a].insert(key, Arc::new(items));
                    recheck.insert((s, key));
                }
                // a cached register at a key, or every kept one of it
                3 => {
                    shared.insert((a, key));
                }
                4 => {
                    shared.extend(next.regs[a].keys().map(|k| (a, *k)));
                }
                // a viewer's row changed: their record is read again (no piece depends on it)
                15 => {
                    if key == i64::MIN {
                        viewers.clear();
                    } else {
                        viewers.remove(&key);
                    }
                }
                // a new row's register: no screen keeps it yet (a list that shows it adds it,
                // ops 2 and 7; a screen built later reads it)
                1 => {}
                // a row left a screen list: the list again
                5 => {
                    relist.insert((a, key));
                }
                // a deleted row's register
                9 => {
                    dropped.insert((a, key));
                }
                // a session row made or changed (by its id): the snapshot's sessions
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
                // what a screen's audience reads changed: its audience again (every key: i64::MIN)
                12 => {
                    audiences.insert((a, key));
                }
                // a deleted row's screen instance (and its lists)
                13 => {
                    next.screens[a].remove(&key);
                    for l in 0..G::LISTS {
                        if G::list_screen(l) == a {
                            next.lists[l].remove(&key);
                        }
                    }
                }
                // anything else: the screens rebuild on demand (counted: a write should never need it)
                _ => {
                    stats.cleared.fetch_add(1, Ordering::Relaxed);
                    for s in &mut next.screens {
                        s.clear();
                    }
                    // open pages can't be told what changed: they reload
                    for l in (0..G::LISTS).filter(|&l| G::list_live(l).is_some()) {
                        for k in next.lists[l].keys() {
                            live.push((crate::live::list_topic(l, *k), crate::live::turbo_stream("refresh", "", None)));
                        }
                    }
                }
            }
        }
        // every key of a list (a fallback edge: i64::MIN)
        let relist: Vec<(usize, i64)> = relist
            .into_iter()
            .flat_map(|(l, k)| if k == i64::MIN { next.lists[l].keys().map(|k2| (l, *k2)).collect::<Vec<_>>() } else { vec![(l, k)] })
            .collect();
        for (list, key) in relist {
            let s = G::list_screen(list);
            if !next.screens[s].contains_key(&key) {
                continue;
            }
            let kind = G::list_item(list);
            let Some((_, items)) = G::screen_lists(&c, s, key).into_iter().find(|(l, _)| *l == list) else { continue };
            // open pages: what left the list is removed; anything new (somewhere in it) reloads
            if G::list_live(list).is_some() {
                let before: Vec<i64> = next.lists[list].get(&key).map(|l| (**l).clone()).unwrap_or_default();
                for id in before.iter().filter(|id| !items.contains(id)) {
                    if let Some(dom) = cur.regs[kind].get(id).and_then(|r| r.dom_id.clone()) {
                        live.push((crate::live::list_topic(list, key), crate::live::turbo_stream("remove", &dom, None)));
                    }
                }
                if items.iter().any(|id| !before.contains(id)) {
                    live.push((crate::live::list_topic(list, key), crate::live::turbo_stream("refresh", "", None)));
                }
            }
            for id in &items {
                if !next.regs[kind].contains_key(id) {
                    if let Some(rec) = load::<G>(&c, &ctx, kind, *id, 0) {
                        next.regs[kind].insert(*id, Arc::new(build::<G>(&ctx, kind, rec.ptr())));
                        stats.write_reads.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            next.lists[list].insert(key, Arc::new(items));
            recheck.insert((s, key));
        }
        // the live lists that show a kind of item, and where an item is in them
        let live_lists = |kind: usize| (0..G::LISTS).filter(move |&l| G::list_item(l) == kind && G::list_live(l).is_some());
        for (kind, key) in dropped {
            if let Some(dom) = next.regs[kind].remove(&key).and_then(|r| r.dom_id.clone()) {
                for l in live_lists(kind) {
                    for (lk, items) in &next.lists[l] {
                        if items.contains(&key) {
                            live.push((crate::live::list_topic(l, *lk), crate::live::turbo_stream("remove", &dom, None)));
                        }
                    }
                }
            }
            shared.remove(&(kind, key));
        }
        for (kind, key) in shared {
            if !next.regs[kind].contains_key(&key) {
                continue; // not kept: built when a page needs it
            }
            stats.write_reads.fetch_add(1, Ordering::Relaxed);
            match load::<G>(&c, &ctx, kind, key, 0) {
                Some(rec) => {
                    let reg = build::<G>(&ctx, kind, rec.ptr());
                    // open pages showing it: replaced
                    if let Some(dom) = &reg.dom_id {
                        for l in live_lists(kind) {
                            for (lk, items) in &next.lists[l] {
                                if items.contains(&key) {
                                    live.push((crate::live::list_topic(l, *lk), crate::live::turbo_stream("replace", dom, Some(&reg.seg.raw))));
                                }
                            }
                        }
                    }
                    next.regs[kind].insert(key, Arc::new(reg));
                }
                None => {
                    next.regs[kind].remove(&key);
                }
            }
        }
        // audiences that may have changed: read again (the pieces don't depend on who sees them,
        // so only the instance's audience changes: a viewer who left can't be served it)
        for (s, key) in audiences {
            let keys: Vec<i64> = if key == i64::MIN { next.screens[s].keys().copied().collect() } else { vec![key] };
            for key in keys {
                let Some(inst) = next.screens[s].get(&key).cloned() else { continue };
                match G::screen(&c, s, key) {
                    Some((aud, _)) if aud == inst.audience => {}
                    Some((audience, headers)) => {
                        stats.audiences.fetch_add(1, Ordering::Relaxed);
                        // open streams of anyone it no longer has are closed
                        let aud = Arc::new(audience.clone());
                        revokes.extend((0..G::LISTS).filter(|&l| G::list_screen(l) == s && G::list_live(l).is_some()).map(|l| (crate::live::list_topic(l, key), aud.clone())));
                        next.screens[s].insert(key, Arc::new(crate::store::Screen { audience, headers }));
                    }
                    None => {
                        let none = Arc::new(vec![]);
                        revokes.extend((0..G::LISTS).filter(|&l| G::list_screen(l) == s && G::list_live(l).is_some()).map(|l| (crate::live::list_topic(l, key), none.clone())));
                        next.screens[s].remove(&key);
                    }
                }
            }
        }
        // screens whose lists changed: their write-time guards again (a guard may read the list)
        for (s, key) in recheck {
            if G::screen(&c, s, key).is_none() {
                next.screens[s].remove(&key);
            }
        }
        for (s, key) in warm {
            if !next.screens[s].contains_key(&key) {
                build_screen::<G>(&c, &ctx, &mut next, s, key);
            }
        }
        // pieces per-request fragments merged that weren't kept: kept from now on
        for (kind, key) in keep {
            if !next.regs[kind].contains_key(&key) {
                if let Some(rec) = load::<G>(&c, &ctx, kind, key, 0) {
                    next.regs[kind].insert(key, Arc::new(build::<G>(&ctx, kind, rec.ptr())));
                }
            }
        }
        let tb = std::time::Instant::now();
        let made = next.refresh_blocks(G::list_item);
        if made > 0 {
            stats.blocks.fetch_add(made as u64, Ordering::Relaxed);
            stats.block_us.fetch_add(tb.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
        // the live events, numbered: the snapshot includes up to the last (its pages' streams get
        // what comes after)
        let events: Vec<crate::live::Event> = if live.is_empty() {
            vec![]
        } else {
            let first = crate::live::HUB.reserve(live.len());
            live.into_iter()
                .enumerate()
                .map(|(i, (topic, stream))| {
                    let id = first + i as u64;
                    crate::live::Event { id, topic, bytes: crate::live::event_bytes(id, &stream) }
                })
                .collect()
        };
        next.live_seq = crate::live::HUB.last();
        stats.live.fetch_add(events.len() as u64, Ordering::Relaxed);
        snap.store(Arc::new(next));
        crate::live::HUB.publish(events, revokes);
        stats.published.store(stats.commits.load(Ordering::SeqCst), Ordering::SeqCst);
        // a slow batch, logged with where its time went (the bench keeps the server's log)
        let took = t0.elapsed();
        if took.as_millis() >= 20 {
            eprintln!("slow batch: {} ms ({} ms to commit), {n_cmds} commands, {} deltas", took.as_millis(), t_commit.as_millis(), deltas.list.len());
        }
        stats.batches.fetch_add(1, Ordering::Relaxed);
        stats.acts.fetch_add(replies.iter().filter(|r: &&(SyncSender<_>, Result<Done, &str>)| r.1.is_ok()).count() as u64, Ordering::Relaxed);
        for (to, r) in replies {
            let _ = to.send(r);
        }
        let _ = &mut c;
    }
}

/// The scrubber (while the writer is idle): one kept page, for one member of its audience in
/// turn, is built afresh from SQLite and compared with what readers are served. A difference is
/// a missed edge: logged, counted, and the page rebuilt.
fn scrub<G: Glue>(c: &Connection, ctx: &Ctx, snap: &ArcSwap<Snap>, stats: &Stats, cursor: &mut usize) {
    let cur = snap.load_full();
    let mut all: Vec<(usize, i64)> = (0..G::SCREENS).flat_map(|s| cur.screens[s].keys().map(move |k| (s, *k))).collect();
    if all.is_empty() {
        return;
    }
    all.sort_unstable();
    let (s, key) = all[*cursor % all.len()];
    let round = *cursor / all.len();
    *cursor += 1;
    let inst = cur.screens[s][&key].clone();
    if inst.audience.is_empty() {
        return;
    }
    let uid = inst.audience[round % inst.audience.len()];
    // the page as served (a piece missing: nothing to compare)
    let mut fast = VecSink(Vec::with_capacity(400_000));
    if !emit_page::<G>(&cur, c, ctx, s, key, uid, &mut fast, &mut vec![]) {
        return;
    }
    stats.scrubbed.fetch_add(1, Ordering::Relaxed);
    let fresh = fresh::<G>(c, ctx, s, key, uid);
    if fresh.as_ref().map(|f| &f.0) == Some(&fast.0) {
        return;
    }
    stats.scrub_fixed.fetch_add(1, Ordering::Relaxed);
    let at = fresh.as_ref().map(|f| f.0.iter().zip(&fast.0).position(|(a, b)| a != b).unwrap_or(f.0.len().min(fast.0.len())));
    eprintln!("scrub: screen {s} key {key} viewer {uid}: the kept page differed from a fresh build (at byte {at:?}); rebuilt");
    let mut next = cur.next();
    if !build_screen::<G>(c, ctx, &mut next, s, key) {
        next.screens[s].remove(&key);
    }
    snap.store(Arc::new(next));
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
    redirect_with(out, to, outcome, &[]);
}

pub fn redirect_with(out: &mut Out, to: &str, outcome: Option<&str>, headers: &[(&str, &str)]) {
    out.push_own(|b| {
        b.extend_from_slice(b"HTTP/1.1 303 See Other\r\nContent-Length: 0\r\nLocation: ");
        b.extend_from_slice(to.as_bytes());
        b.extend_from_slice(b"\r\n");
        for (k, v) in headers {
            b.extend_from_slice(k.as_bytes());
            b.extend_from_slice(b": ");
            b.extend_from_slice(v.as_bytes());
            b.extend_from_slice(b"\r\n");
        }
        if let Some(o) = outcome {
            b.extend_from_slice(b"X-Burin-Outcome: ");
            b.extend_from_slice(o.as_bytes());
            b.extend_from_slice(b"\r\n");
        }
        b.extend_from_slice(b"\r\n");
    });
}

/// application/x-www-form-urlencoded
/// A multipart/form-data body's boundary (a browser's form with enctype="multipart/form-data").
fn multipart_boundary(ctype: &[u8]) -> Option<&[u8]> {
    let s = ctype.strip_prefix(b"multipart/form-data")?;
    let i = s.windows(9).position(|w| w.eq_ignore_ascii_case(b"boundary="))?;
    let b = &s[i + 9..];
    let b = b.split(|&c| c == b';').next()?;
    let b = b.strip_prefix(b"\"").and_then(|x| x.strip_suffix(b"\"")).unwrap_or(b);
    (!b.is_empty()).then_some(b)
}

/// A multipart/form-data body's text fields, by name (a file's part is skipped: no attachments).
pub fn parse_multipart(body: &[u8], boundary: &[u8]) -> Vec<(String, String)> {
    let mut delim = b"--".to_vec();
    delim.extend_from_slice(boundary);
    let find = |hay: &[u8], needle: &[u8], from: usize| hay.get(from..).and_then(|h| h.windows(needle.len()).position(|w| w == needle)).map(|i| i + from);
    let mut out = vec![];
    let Some(mut at) = find(body, &delim, 0) else { return out };
    loop {
        at += delim.len();
        if body.get(at..at + 2) == Some(b"--") {
            break; // the closing delimiter
        }
        let Some(head_end) = find(body, b"\r\n\r\n", at) else { break };
        let head = &body[at..head_end];
        let start = head_end + 4;
        let Some(next) = find(body, &delim, start) else { break };
        // the part ends with CRLF before the next delimiter
        let end = if next >= 2 && &body[next - 2..next] == b"\r\n" { next - 2 } else { next };
        let head = String::from_utf8_lossy(head);
        let disp = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-disposition:")).unwrap_or("");
        let attr = |k: &str| {
            let pat = format!("{k}=\"");
            disp.find(&pat).map(|i| &disp[i + pat.len()..]).and_then(|r| r.find('"').map(|j| r[..j].to_string()))
        };
        if let (Some(name), None) = (attr("name"), attr("filename")) {
            out.push((name, String::from_utf8_lossy(&body[start..end.max(start)]).into_owned()));
        }
        at = next;
    }
    out
}

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

/// A list argument: every value of its field (or `__arg_<name>`), each a row's id (a `#` or `@`
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
        // the same order as files::Files::load: a name found twice links the file served
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
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
/// key; any other from the environment (BURIN_CONFIG_<NAME>).
pub fn main<G: Glue>(configs: &[&str], session_cookie: &'static [u8]) {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    // a password's hash, to store (setting up an account by hand)
    if let Some(p) = arg("--hash-password") {
        println!("{}", crate::pass::hash(&p));
        return;
    }
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
    // SQLite, before any connection opens: no memory statistics (they take a process-wide lock on
    // every allocation, which every worker rendering per-request fragments contends on), and
    // multi-thread mode (each connection belongs to one thread: no per-connection locks)
    let (a, b) = unsafe {
        (rusqlite::ffi::sqlite3_config(rusqlite::ffi::SQLITE_CONFIG_MEMSTATUS, 0 as std::os::raw::c_int), rusqlite::ffi::sqlite3_config(rusqlite::ffi::SQLITE_CONFIG_MULTITHREAD))
    };
    if a != 0 || b != 0 {
        eprintln!("sqlite3_config refused ({a}, {b}): SQLite was already started");
    }
    let start = std::time::Instant::now();
    let dir = std::path::Path::new(&data);
    let secret = crate::keys::secret(dir);
    let values: Vec<String> = configs
        .iter()
        .map(|c| match *c {
            "host" => host.clone(),
            "vapid_public" => std::env::var("VAPID_PUBLIC_KEY").unwrap_or_else(|_| crate::keys::vapid_public(dir)),
            other => std::env::var(format!("BURIN_CONFIG_{}", other.to_uppercase())).unwrap_or_default(),
        })
        .collect();
    let ctx = Arc::new(Ctx { signer: crate::sign::Signer::new(&secret), assets: assets.as_ref().map(|a| asset_urls(std::path::Path::new(a))).unwrap_or_default(), configs: values });
    let db_path = dir.join("app.sqlite").to_string_lossy().into_owned();
    // the next seq: past every table's
    let seq = {
        let c = open_write(&db_path);
        let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
        tables.iter().filter_map(|t| c.query_row(&format!("SELECT max(seq) FROM \"{t}\""), [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()).max().unwrap_or(0)
    };
    let scrub_ms: u64 = arg("--scrub-ms").map(|v| v.parse().expect("--scrub-ms N")).unwrap_or(100);
    let app = App::<G>::start(ctx, &db_path, &host, verify, session_cookie, seq, scrub_ms);
    let _ = app.files.set(assets.as_deref().map(|a| crate::files::Files::load(std::path::Path::new(a))).unwrap_or_else(crate::files::Files::empty));
    println!("started in {} ms ({workers} workers{})", start.elapsed().as_millis(), if verify { ", verifying" } else { "" });
    crate::http::serve(format!("0.0.0.0:{port}").parse().unwrap(), workers, app).expect("serve");
}

#[cfg(test)]
mod form_tests {
    use super::*;

    #[test]
    fn multipart_fields_as_a_browser_sends_them() {
        let ct = b"multipart/form-data; boundary=----WebKitFormBoundaryX1";
        let b = multipart_boundary(ct).unwrap();
        assert_eq!(b, b"----WebKitFormBoundaryX1");
        let body = b"------WebKitFormBoundaryX1\r\nContent-Disposition: form-data; name=\"room\"\r\n\r\n42\r\n\
------WebKitFormBoundaryX1\r\nContent-Disposition: form-data; name=\"body\"\r\n\r\nline one\r\nline two \xe2\x9c\x93\r\n\
------WebKitFormBoundaryX1\r\nContent-Disposition: form-data; name=\"attachment\"; filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n\
------WebKitFormBoundaryX1\r\nContent-Disposition: form-data; name=\"empty\"\r\n\r\n\r\n\
------WebKitFormBoundaryX1--\r\n";
        let f = parse_multipart(body, b);
        assert_eq!(f, vec![("room".to_string(), "42".to_string()), ("body".into(), "line one\r\nline two \u{2713}".into()), ("empty".into(), "".into())]);
        assert!(multipart_boundary(b"application/x-www-form-urlencoded").is_none());
    }
}
