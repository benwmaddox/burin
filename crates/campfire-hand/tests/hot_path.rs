//! The hot-path rules (DESIGN.md principle 2), as a test: once a connection's buffers have grown,
//! answering a room page on the fast path allocates nothing and copies almost nothing (only the
//! status line, the length and the date are written; the body is borrowed from the snapshot).
//!
//! Needs a crudspec Campfire data directory with the bench user: ETCH_TEST_DATA, or
//! ~/hw/golden-data (skipped when neither exists).

use arc_swap::ArcSwap;
use campfire_hand::{db, keys, render, serve, state};
use etch_rt::sign::Signer;
use etch_rt::{Out, Req};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

const ROOM: i64 = 486777696;
const HOST: &[u8] = b"127.0.0.1:13790";

#[test]
fn the_fast_path_allocates_and_frees_nothing_and_copies_only_headers() {
    let data = std::env::var("ETCH_TEST_DATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("hw/golden-data"));
    if !data.join("app.sqlite").exists() {
        eprintln!("skipped: no data at {}", data.display());
        return;
    }
    let ctx = Arc::new(render::Ctx {
        signer: Signer::new(&keys::secret(&data)),
        assets: HashMap::new(),
        vapid_public: keys::vapid_public(&data),
    });
    let c = db::open(data.join("app.sqlite").to_str().unwrap(), false).unwrap();
    let users = state::load_users(&c).unwrap();
    let sessions = state::load_sessions(&c).unwrap();
    let token: String = c.query_row("SELECT token FROM Session WHERE id = 481019661", [], |r| r.get(0)).unwrap();
    let uid = sessions[token.as_bytes()];
    let host = String::from_utf8(HOST.to_vec()).unwrap();
    let room = state::build_room(&c, &ctx, &host, &users, ROOM).unwrap().expect("the bench room is precomputed");
    let side = state::build_sidebar(&c, &ctx, &users, uid).unwrap();
    let mut snap = state::Snap { version: 1, host, sessions, users, rooms: HashMap::new(), sidebars: HashMap::new(), end: Arc::new(render::page_end(&ctx)) };
    snap.rooms.insert(ROOM, Arc::new(room));
    snap.sidebars.insert(uid, Arc::new(side));
    let (tx, _rx) = std::sync::mpsc::sync_channel(16);
    let app = serve::App {
        ctx,
        snap: Arc::new(ArcSwap::from_pointee(snap)),
        writer: tx,
        db_path: data.join("app.sqlite").to_string_lossy().into_owned(),
        verify: false,
        counts: Default::default(),
        wstats: Default::default(),
    };
    let cookie = format!("session_token={token}");
    let req = Req { method: b"GET", path: b"/rooms/486777696", query: b"", host: HOST, cookie: cookie.as_bytes(), content_type: b"", body: b"", worker: 0 };
    let mut out = Out::default();
    // the first page grows the connection's buffers
    assert!(app.fast(&req, &mut out));
    out.reset();
    for _ in 0..3 {
        let before = ALLOCS.load(Ordering::SeqCst);
        assert!(app.fast(&req, &mut out));
        let segs = out.segments();
        let own = out.own_bytes();
        let len = out.len();
        out.reset(); // lets the snapshot go (a reference count, not a free: the app holds it)
        let after = ALLOCS.load(Ordering::SeqCst);
        assert_eq!(after - before, 0, "allocations or frees on the fast path");
        assert!(own < 200, "{own} bytes written per request");
        eprintln!("fast page: {len} bytes in {segs} segments, {own} bytes written by the request, 0 allocations");
    }
}
