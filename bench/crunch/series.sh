#!/bin/bash
# The table research: a series of 10 texts (1 KB to 512 KB) compressed by deflate (2^15-entry
# tables, zeroed each call) and deflate_sized (tables sized to the text), in burin warm and cold
# (its helper arena kept, or given back before each call) and in C (gcc and clang -O2, static
# tables: always warm). Checks first: for each text and design, burin's bytes equal C's and
# inflate back. Under the shared bench lock, pinned to one core; the JSON lines go to
# bench/runs/crunch-series-<tag>-{burin,gcc,clang}.jsonl.
#   bash /mnt/d/code/burin/bench/crunch/series.sh <input> <tag> [runs]
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
IN=$1; TAG=$2; RUNS=${3:-15}; CORE=${CORE:-12}
B=/mnt/d/code/burin; W=/tmp/burin-crunch-series; rm -rf $W; mkdir -p $W/dump
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
cd $B
OUT=bench/runs/crunch-series-$TAG
gcc -O2 -o $W/c-gcc bench/crunch/deflate.c -lz -ldeflate
clang -O2 -o $W/c-clang bench/crunch/deflate.c -lz -ldeflate
APP=crunch CRATE=crunch-app bash scripts/build-burin.sh release > /dev/null 2>&1
export BURIN_GEN_DIR=$HOME/burin-gen-crunch RUSTFLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld" CARGO_TARGET_DIR=$HOME/burin-target-gen
taskset -c 0-7 cargo test -q --release -p crunch-app --test compute --no-run 2>&1 | grep -E '^error' -A5 || true
# the checks (on 0-7: one run each, its bytes kept)
BURIN_CRUNCH_INPUT=$IN BURIN_CRUNCH_ARENA=1 BURIN_CRUNCH_RUNS=1 BURIN_CRUNCH_DUMP=$W/dump taskset -c 0-7 cargo test -q --release -p crunch-app --test compute arena_warm_and_cold -- --nocapture > /dev/null 2>&1
taskset -c 0-7 $W/c-clang "$IN" --series $W/dump 1 > /dev/null
python3 - "$IN" $W/dump <<'PY'
import sys, zlib
src, d = open(sys.argv[1], 'rb').read(), sys.argv[2]
n = len(src)
for s in (0, 1):
    for k in range(10):
        ln = min(1024 << k, n); at = (k * 997331) % (n - ln + 1)
        b, c = open(f"{d}/burin-{s}-{k}.raw", 'rb').read(), open(f"{d}/c-{s}-{k}.raw", 'rb').read()
        if b != c or zlib.decompress(b, -15) != src[at:at + ln]:
            sys.exit(f"FAIL: design {s}, text {k}: burin's bytes differ from C's, or don't inflate back")
print("checked: both designs, all 10 texts: burin's bytes equal C's and inflate back")
PY
# the timings, one core, the lock held
exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9
BURIN_CRUNCH_INPUT=$IN BURIN_CRUNCH_ARENA=1 BURIN_CRUNCH_RUNS=$RUNS taskset -c $CORE cargo test -q --release -p crunch-app --test compute arena_warm_and_cold -- --nocapture 2>&1 | grep '^{' > $OUT-burin.jsonl
taskset -c $CORE $W/c-gcc "$IN" --series "" $RUNS > $OUT-gcc.jsonl
taskset -c $CORE $W/c-clang "$IN" --series "" $RUNS > $OUT-clang.jsonl
flock -u 9
node -e '
const read = (f) => require("fs").readFileSync(f, "utf8").trim().split("\n").map(JSON.parse);
const [b, g, c] = [read(process.argv[1] + "-burin.jsonl"), read(process.argv[1] + "-gcc.jsonl"), read(process.argv[1] + "-clang.jsonl")];
const p = (x, w = 9) => String(x).padStart(w);
console.log("               ------------- deflate (2^15 tables) -------------   --------------- sized tables ----------------");
console.log("text    bytes  burin warm  burin cold   gcc      clang            burin warm  burin cold   gcc      clang");
for (const r of b.filter(r => r.text !== undefined)) {
  const [gr, cr] = [g[r.text], c[r.text]];
  console.log(p(r.text, 4), p(r.bytes), p(r.deflate_warm_us, 11), p(r.deflate_cold_us, 11), p(gr.deflate_us), p(cr.deflate_us), "       ", p(r.sized_warm_us, 11), p(r.sized_cold_us, 11), p(gr.sized_us), p(cr.sized_us));
}
const s = b.find(r => r.series), sum = (rows, k) => rows.reduce((a, r) => a + r[k], 0).toFixed(1);
console.log(`the series of 10 (us): deflate burin warm ${s.deflate_warm_us}, cold ${s.deflate_cold_us}, gcc ${sum(g, "deflate_us")}, clang ${sum(c, "deflate_us")}; sized burin warm ${s.sized_warm_us}, cold ${s.sized_cold_us}, gcc ${sum(g, "sized_us")}, clang ${sum(c, "sized_us")}`);
' $OUT
