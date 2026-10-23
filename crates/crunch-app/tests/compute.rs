//! The helpers burinc compiled (apps/crunch), called directly and timed: DEFLATE and CRC-32 on
//! the file BURIN_CRUNCH_INPUT. Writes the compressed bytes to BURIN_CRUNCH_OUT (bench/crunch
//! checks they inflate back, and equal C's). Prints one JSON line. Skipped without the input.

use std::time::Instant;

// (linked for its generated module, where the helpers are)
extern crate crunch_app;

extern "C" {
    fn burin_fn_deflate(p: *const u8, n: i64, out: *mut Vec<u8>);
    fn burin_fn_deflate_sized(p: *const u8, n: i64, out: *mut Vec<u8>);
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

/// A series of 10 texts (slices of the input from different places, 1 KB to 512 KB), each
/// compressed warm (the thread's helper arena kept from the calls before, as a worker's or the
/// writer's is) and cold (the arena given back before each call: its memory fresh from the
/// system), by deflate and by deflate_sized (its tables sized to the text). CRC-32, which makes
/// no arena array, is the control. One JSON line per text, then the series' totals; with
/// BURIN_CRUNCH_DUMP=dir, each one's bytes as dir/burin-<sized>-<text>.raw. Only with
/// BURIN_CRUNCH_ARENA=1.
#[test]
fn arena_warm_and_cold() {
    let (Ok(path), Ok(_)) = (std::env::var("BURIN_CRUNCH_INPUT"), std::env::var("BURIN_CRUNCH_ARENA")) else {
        eprintln!("skipped: BURIN_CRUNCH_INPUT or BURIN_CRUNCH_ARENA isn't set");
        return;
    };
    let input = std::fs::read(&path).unwrap();
    let runs: usize = std::env::var("BURIN_CRUNCH_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(15);
    let dump = std::env::var("BURIN_CRUNCH_DUMP").ok().filter(|d| !d.is_empty());
    let texts: Vec<&[u8]> = (0..10)
        .map(|k| {
            let len = (1024usize << k).min(input.len());
            let at = (k * 997_331) % (input.len() - len + 1);
            &input[at..at + len]
        })
        .collect();
    type Helper = unsafe extern "C" fn(*const u8, i64, *mut Vec<u8>);
    let helpers: [Helper; 2] = [burin_fn_deflate, burin_fn_deflate_sized];
    let mut out = Vec::with_capacity(1 << 20);
    fn run(f: Helper, t: &[u8], out: &mut Vec<u8>) {
        out.clear();
        unsafe { f(t.as_ptr(), t.len() as i64, out) };
        assert!(!out.is_empty());
    }
    // one call's time: the median of `runs`, each `before` it (untimed)
    let time = |t: &[u8], before: &dyn Fn(), f: &mut dyn FnMut(&[u8])| {
        let mut ts: Vec<f64> = (0..runs)
            .map(|_| {
                before();
                let t0 = Instant::now();
                f(t);
                t0.elapsed().as_secs_f64() * 1e9
            })
            .collect();
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ts[runs / 2]
    };
    // warm the arena: the whole series once, by each
    for (s, h) in helpers.iter().enumerate() {
        for (k, t) in texts.iter().enumerate() {
            run(*h, t, &mut out);
            if let Some(d) = &dump {
                std::fs::write(format!("{d}/burin-{s}-{k}.raw"), &out).unwrap();
            }
        }
    }
    let mut totals = [[0.0f64; 2]; 2];
    for (k, t) in texts.iter().enumerate() {
        let mut line = format!("{{\"text\":{k},\"bytes\":{}", t.len());
        for (s, h) in helpers.iter().enumerate() {
            let warm = time(t, &|| {}, &mut |t| run(*h, t, &mut out));
            let cold = time(t, &|| burin_rt::abi::helper_arena_drop(), &mut |t| run(*h, t, &mut out));
            totals[s][0] += warm;
            totals[s][1] += cold;
            let name = ["deflate", "sized"][s];
            line += &format!(",\"{name}_warm_us\":{:.1},\"{name}_cold_us\":{:.1}", warm / 1e3, cold / 1e3);
        }
        let mut sink = 0;
        let crc_warm = time(t, &|| {}, &mut |t| sink += unsafe { burin_fn_crc32(t.as_ptr(), t.len() as i64) });
        let crc_cold = time(t, &|| burin_rt::abi::helper_arena_drop(), &mut |t| sink += unsafe { burin_fn_crc32(t.as_ptr(), t.len() as i64) });
        std::hint::black_box(sink);
        println!("{line},\"crc32_warm_us\":{:.2},\"crc32_cold_us\":{:.2}}}", crc_warm / 1e3, crc_cold / 1e3);
    }
    println!(
        "{{\"series\":10,\"deflate_warm_us\":{:.1},\"deflate_cold_us\":{:.1},\"sized_warm_us\":{:.1},\"sized_cold_us\":{:.1}}}",
        totals[0][0] / 1e3,
        totals[0][1] / 1e3,
        totals[1][0] / 1e3,
        totals[1][1] / 1e3
    );
}


#[test]
fn deflate_and_crc32_timed() {
    if std::env::var("BURIN_CRUNCH_ARENA").is_ok() {
        return;
    }
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
    // small inputs, burin's usual (a row's text in a cached piece): a call's own cost shows
    let (small, k) = (&input[..64.min(input.len())], 100_000);
    let mut sink = 0i64;
    let crc64_ns = median(runs, || {
        for _ in 0..k {
            sink = sink.wrapping_add(unsafe { burin_fn_crc32(small.as_ptr(), small.len() as i64) });
        }
    }) * 1e9 / k as f64;
    let (kb, k2) = (&input[..1024.min(input.len())], 2_000);
    let deflate1k_ns = median(runs, || {
        for _ in 0..k2 {
            out.clear();
            unsafe { burin_fn_deflate(kb.as_ptr(), kb.len() as i64, &mut out) };
        }
    }) * 1e9 / k2 as f64;
    std::hint::black_box(sink);
    let mb = input.len() as f64 / 1e6;
    println!(
        "{{\"impl\":\"burin\",\"bytes\":{},\"deflate_mb_s\":{:.1},\"compressed\":{},\"crc32_mb_s\":{:.1},\"crc32\":{crc},\"crc32_64b_ns\":{:.0},\"deflate_1k_ns\":{:.0}}}",
        input.len(),
        mb / deflate,
        out.len(),
        mb / crc_s,
        crc64_ns,
        deflate1k_ns
    );
}
