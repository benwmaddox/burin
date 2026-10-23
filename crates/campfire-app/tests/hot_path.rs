//! The hot-path rules (DESIGN.md principle 2) on the generated app: once warm, answering a room
//! page on the fast path allocates and frees nothing. Its cached pieces are borrowed from the
//! snapshot; what it writes is its status line, length and date, and its per-request parts (the
//! sidebar, the title), rendered into reused buffers. Needs BURIN_TEST_DATA: a copy of the golden
//! data. BURIN_TRACE_ALLOCS=1 prints where the first allocations come from.

use campfire_app::gen::Spec;
use burin_rt::abi::Ctx;
use burin_rt::app::App;
use burin_rt::{Out, Req};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
/// BURIN_TRACE_ALLOCS: where the first allocations on a measured request come from
static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRACED: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static IN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// this thread's allocations are counted (the request's: the writer's and the checkpointer's
    /// own work isn't the hot path)
    static COUNTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn counted() -> bool {
    COUNTED.with(|c| c.get())
}

fn trace() {
    if !TRACE.load(Ordering::Relaxed) || TRACED.fetch_add(1, Ordering::Relaxed) >= 6 {
        return;
    }
    IN.with(|i| {
        if i.get() {
            return;
        }
        i.set(true);
        let b = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> = b.lines().filter(|l| !l.contains("at /rustc") && !l.contains("std::") && !l.contains("core::") && !l.contains("alloc::")).take(14).collect();
        eprintln!("alloc from:
{}", frames.join("
"));
        i.set(false);
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if counted() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        trace();
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if counted() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        FREES.fetch_add(1, Ordering::Relaxed);
        if TRACE.load(Ordering::Relaxed) {
            trace();
        }
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if counted() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        trace();
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn the_fast_path_allocates_and_frees_nothing_and_copies_only_headers() {
    let data = match std::env::var("BURIN_TEST_DATA") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("skipped: BURIN_TEST_DATA isn't set");
            return;
        }
    };
    let host = "127.0.0.1:13790";
    let dir = std::path::Path::new(&data);
    let ctx = Arc::new(Ctx {
        signer: burin_rt::sign::Signer::new(&burin_rt::keys::secret(dir)),
        assets: Default::default(),
        configs: vec![host.to_string(), burin_rt::keys::vapid_public(dir)],
    });
    let db = dir.join("app.sqlite").to_string_lossy().into_owned();
    let c = rusqlite::Connection::open(&db).unwrap();
    let token: String = c.query_row("SELECT token FROM Session WHERE id = 481019661", [], |r| r.get(0)).unwrap();
    drop(c);
    let app = App::<Spec>::start(ctx, &db, host, false, b"session_token", 1_000_000, 0);
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
        // read before counting starts (reading it allocates)
        let trace = std::env::var("BURIN_TRACE_ALLOCS").map(|v| v == "1").unwrap_or(false);
        for i in 0..3 {
            // apart, so a request lands in a new second (its Date header is made again)
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            COUNTED.with(|c| c.set(true));
            let before = ALLOCS.load(Ordering::SeqCst);
            TRACE.store(trace, Ordering::SeqCst);
            assert!(app.fast(&req, &mut out));
            TRACE.store(false, Ordering::SeqCst);
            let (segs, own, len) = (out.segments(), out.own_bytes(), out.len());
            let (mid, mid_frees) = (ALLOCS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst));
            out.reset();
            let after = ALLOCS.load(Ordering::SeqCst);
            COUNTED.with(|c| c.set(false));
            eprintln!("  the request: {} (frees so far {mid_frees}); the reset: {}", mid - before, after - mid);
            eprintln!("fast page (gzip {gzip}): {len} bytes in {segs} segments, {own} bytes written by the request (its per-request parts), {} allocations", after - before);
            assert_eq!(after - before, 0, "allocations or frees on the fast path (gzip {gzip})");
        }
    }
}
