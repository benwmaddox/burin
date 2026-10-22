//! Phase 1a: Campfire's room page and post_message, written by hand as etch's compiler should
//! generate them (docs/DESIGN.md). Serves the data of a crudspec Campfire data directory.
//!
//!   campfire-hand --data DIR --assets DIR [--port 3000] [--host HOST] [--workers N] [--verify]
//!
//! The time zone is TZ's, fixed at start. --verify compares every fast page with a fresh render.

use campfire_hand::{db, keys, render, serve, state, writer};

use arc_swap::ArcSwap;
use etch_rt::sign::Signer;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = arg("--data").expect("--data DIR");
    let assets = arg("--assets").expect("--assets DIR");
    let port: u16 = arg("--port").map(|p| p.parse().expect("--port N")).unwrap_or(3000);
    let host = arg("--host").unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let workers: usize = arg("--workers").map(|w| w.parse().expect("--workers N")).unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
    // --exit-after SECS: exit cleanly (profiling builds write their profiles at exit)
    if let Some(secs) = arg("--exit-after").and_then(|s| s.parse::<u64>().ok()) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            std::process::exit(0);
        });
    }
    let verify = args.iter().any(|a| a == "--verify") || std::env::var("ETCH_VERIFY").as_deref() == Ok("1");
    let start = std::time::Instant::now();

    let secret = keys::secret(Path::new(&data));
    let vapid_public = std::env::var("VAPID_PUBLIC_KEY").ok().unwrap_or_else(|| keys::vapid_public(Path::new(&data)));
    let ctx = Arc::new(render::Ctx { signer: Signer::new(&secret), assets: asset_urls(Path::new(&assets)), vapid_public });

    let db_path = Path::new(&data).join("app.sqlite").to_string_lossy().into_owned();
    let w = db::open(&db_path, true).expect("open the database");
    let r = db::open(&db_path, false).expect("open the database to read");
    let users = state::load_users(&r).expect("users");
    let sessions = state::load_sessions(&r).expect("sessions");
    let snap = Arc::new(ArcSwap::from_pointee(state::Snap {
        version: 0,
        host: host.clone(),
        sessions,
        users,
        rooms: HashMap::new(),
        sidebars: HashMap::new(),
        end: Arc::new(render::page_end(&ctx)),
    }));
    drop(r);

    let (tx, rx) = std::sync::mpsc::sync_channel(writer::QUEUE);
    let wstats = Arc::new(writer::Stats::default());
    {
        let (ctx, snap, wstats) = (ctx.clone(), snap.clone(), wstats.clone());
        std::thread::Builder::new().name("etch-writer".into()).spawn(move || writer::run(w, ctx, snap, rx, wstats)).unwrap();
    }
    // checkpoints the WAL once a second when there were commits (as crudspec)
    {
        let snap = snap.clone();
        let ck = db::open(&db_path, true).expect("open the database to checkpoint");
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
    let app = Arc::new(serve::App { ctx, snap, writer: tx, db_path, verify, counts: Default::default(), wstats });
    println!("started in {} ms ({workers} workers{})", start.elapsed().as_millis(), if verify { ", verifying" } else { "" });
    let addr = format!("0.0.0.0:{port}").parse().unwrap();
    etch_rt::serve(addr, workers, app).expect("serve");
}

/// Every asset's address with its content's hash, by file name (crudspec's AssetURL).
fn asset_urls(dir: &Path) -> HashMap<String, String> {
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
