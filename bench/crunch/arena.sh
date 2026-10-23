#!/bin/bash
# The helper arena warm and cold (crates/crunch-app/tests/compute.rs, arena_warm_and_cold): a
# series of 10 texts compressed with the thread's arena kept and with it given back before each
# call. Under the shared bench lock, pinned to one core; the JSON lines go to
# bench/runs/crunch-arena-<tag>.jsonl.
#   bash /mnt/d/code/burin/bench/crunch/arena.sh <input> <tag> [runs]
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
IN=$1; TAG=$2; RUNS=${3:-15}; CORE=${CORE:-12}
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
cd /mnt/d/code/burin
OUT=bench/runs/crunch-arena-$TAG.jsonl
export BURIN_GEN_DIR=$HOME/burin-gen-crunch RUSTFLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld" CARGO_TARGET_DIR=$HOME/burin-target-gen
taskset -c 0-7 cargo test -q --release -p crunch-app --test compute --no-run 2>&1 | grep -E '^error' -A5 || true
exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9
BURIN_CRUNCH_INPUT=$IN BURIN_CRUNCH_ARENA=1 BURIN_CRUNCH_RUNS=$RUNS taskset -c $CORE cargo test -q --release -p crunch-app --test compute arena_warm_and_cold -- --nocapture 2>&1 | grep -o '{.*}' > $OUT
flock -u 9
node -e '
const rows = require("fs").readFileSync(process.argv[1], "utf8").trim().split("\n").map(JSON.parse);
console.log("text  bytes     deflate warm us  cold us  cold/warm   crc32 warm us  cold us");
for (const r of rows.filter(r => r.text !== undefined)) console.log(String(r.text).padStart(4), String(r.bytes).padStart(8), String(r.deflate_warm_us).padStart(16), String(r.deflate_cold_us).padStart(8), String(r.cold_over_warm).padStart(10), String(r.crc32_warm_us).padStart(15), String(r.crc32_cold_us).padStart(8));
const s = rows.find(r => r.series);
console.log(`the series of 10: warm ${s.deflate_warm_us} us, cold ${s.deflate_cold_us} us (cold/warm ${s.cold_over_warm})`);
' $OUT
