//! The hot-path rules (DESIGN.md principle 2) on the generated app: once warm, answering a room
//! page on the fast path allocates and frees nothing, and writes only its status line, length and
//! date (the body is borrowed from the snapshot). Needs ETCH_TEST_DATA: a copy of the golden data.

use campfire_app::gen::Spec;
use etch_rt::abi::Ctx;
use etch_rt::app::App;
use etch_rt::{Out, Req};
use std::alloc::{GlobalAlloc, Layout, System};
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

#[test]
fn the_fast_path_allocates_and_frees_nothing_and_copies_only_headers() {
    let data = match std::env::var("ETCH_TEST_DATA") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("skipped: ETCH_TEST_DATA isn't set");
            return;
        }
    };
    let host = "127.0.0.1:13790";
    let dir = std::path::Path::new(&data);
    let ctx = Arc::new(Ctx {
        signer: etch_rt::sign::Signer::new(&etch_rt::keys::secret(dir)),
        assets: Default::default(),
        configs: vec![host.to_string(), etch_rt::keys::vapid_public(dir)],
    });
    let db = dir.join("app.sqlite").to_string_lossy().into_owned();
    let c = rusqlite::Connection::open(&db).unwrap();
    let token: String = c.query_row("SELECT token FROM Session WHERE id = 481019661", [], |r| r.get(0)).unwrap();
    drop(c);
    let app = App::<Spec>::start(ctx, &db, host, false, b"session_token", 1_000_000);
    let cookie = format!("session_token={token}");
    for gzip in [false, true] {
        let req = Req { method: b"GET", path: b"/rooms/486777696", query: b"", host: host.as_bytes(), cookie: cookie.as_bytes(), content_type: b"", referer: b"", body: b"", gzip, worker: 0 };
        let mut out = Out::default();
        // the first requests warm the snapshot (the writer builds the room and the sidebar)
        let mut tries = 0;
        while !app.fast(&req, &mut out) {
            out.reset();
            tries += 1;
            assert!(tries < 200, "the page never became fast");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        out.reset();
        // one more, so the connection's buffers have grown
        assert!(app.fast(&req, &mut out));
        out.reset();
        std::thread::sleep(std::time::Duration::from_millis(50));
        for _ in 0..3 {
            let before = ALLOCS.load(Ordering::SeqCst);
            assert!(app.fast(&req, &mut out));
            let (segs, own, len) = (out.segments(), out.own_bytes(), out.len());
            out.reset();
            let after = ALLOCS.load(Ordering::SeqCst);
            assert_eq!(after - before, 0, "allocations or frees on the fast path (gzip {gzip})");
            assert!(own < 200, "{own} bytes written per request");
            eprintln!("fast page (gzip {gzip}): {len} bytes in {segs} segments, {own} bytes written by the request, 0 allocations");
        }
    }
}
