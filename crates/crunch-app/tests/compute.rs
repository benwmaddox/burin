//! The helpers burinc compiled (apps/crunch), called directly and timed: DEFLATE and CRC-32 on
//! the file BURIN_CRUNCH_INPUT. Writes the compressed bytes to BURIN_CRUNCH_OUT (bench/crunch
//! checks they inflate back, and equal C's). Prints one JSON line. Skipped without the input.

use std::time::Instant;

// (linked for its generated module, where the helpers are)
extern crate crunch_app;

extern "C" {
    fn burin_fn_deflate(p: *const u8, n: i64, out: *mut Vec<u8>);
    fn burin_fn_crc32(p: *const u8, n: i64) -> i64;
}

/// The median of `runs` timings of `f`, in seconds.
fn median(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed().as_secs_f64()
        })
        .collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    t[runs / 2]
}

#[test]
fn deflate_and_crc32_timed() {
    let Ok(path) = std::env::var("BURIN_CRUNCH_INPUT") else {
        eprintln!("skipped: BURIN_CRUNCH_INPUT isn't set");
        return;
    };
    let input = std::fs::read(&path).unwrap();
    let runs: usize = std::env::var("BURIN_CRUNCH_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let mut out = Vec::with_capacity(input.len());
    let deflate = median(runs, || {
        out.clear();
        unsafe { burin_fn_deflate(input.as_ptr(), input.len() as i64, &mut out) };
    });
    assert!(!out.is_empty(), "deflate failed (none)");
    if let Some(o) = std::env::var("BURIN_CRUNCH_OUT").ok().filter(|o| !o.is_empty()) {
        std::fs::write(o, &out).unwrap();
    }
    let mut crc = 0;
    let crc_s = median(runs, || crc = unsafe { burin_fn_crc32(input.as_ptr(), input.len() as i64) });
    let mb = input.len() as f64 / 1e6;
    println!(
        "{{\"impl\":\"burin\",\"bytes\":{},\"deflate_mb_s\":{:.1},\"compressed\":{},\"crc32_mb_s\":{:.1},\"crc32\":{crc}}}",
        input.len(),
        mb / deflate,
        out.len(),
        mb / crc_s
    );
}
