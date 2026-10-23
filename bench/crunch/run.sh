#!/bin/bash
# Compute in burin against C on the same algorithm: DEFLATE (greedy LZ77, fixed Huffman codes)
# and CRC-32, apps/crunch's helpers against bench/crunch/deflate.c, on one input. Checks first:
# burin's compressed bytes equal C's and inflate back to the input. Then times, under the shared
# bench lock, pinned to one core: burin (with its loops' step budget and without), C from gcc
# and clang (plain, and with overflow and bounds checks that trap), and zlib and libdeflate for
# context. Prints a table; the JSON lines go to bench/runs/crunch-<tag>.jsonl.
#   bash /mnt/d/code/burin/bench/crunch/run.sh <input> <tag> [runs]
set -euo pipefail
IN=$1; TAG=$2; RUNS=${3:-9}
CORE=${CORE:-12}
B=/mnt/d/code/burin; W=/tmp/burin-crunch-bench; mkdir -p $W
OUT=$B/bench/runs/crunch-$TAG.jsonl; : > $OUT
cd $B
# builds (on 0-7, outside the lock)
CHK="-fsanitize=signed-integer-overflow,bounds -fsanitize-trap=all"
gcc -O2 -o $W/c-gcc bench/crunch/deflate.c -lz -ldeflate
clang -O2 -o $W/c-clang bench/crunch/deflate.c -lz -ldeflate
gcc -O2 $CHK -o $W/c-gcc-checked bench/crunch/deflate.c -lz -ldeflate
clang -O2 $CHK -o $W/c-clang-checked bench/crunch/deflate.c -lz -ldeflate
build() { APP=crunch CRATE=crunch-app BURIN_NO_STEPS=$1 bash scripts/build-burin.sh release > /dev/null 2>&1; }
# the checks
build 0
PIN="taskset -c 0-7" bash bench/crunch/burin.sh "$IN" $W/burin.raw 1 > /dev/null
$W/c-clang "$IN" $W/c.raw 1 > /dev/null
cmp -s $W/burin.raw $W/c.raw || { echo "FAIL: burin's bytes differ from C's"; exit 1; }
python3 -c "import zlib,sys; sys.exit(0 if zlib.decompress(open('$W/burin.raw','rb').read(), -15) == open('$IN','rb').read() else 1)" || { echo "FAIL: doesn't inflate back"; exit 1; }
echo "checked: burin's $(stat -c %s $W/burin.raw) bytes equal C's and inflate back to the input"
# the timings, one core, the lock held
exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9
run() { echo "{\"run\":\"$1\",$("${@:2}" | sed 's/^{//')" >> $OUT; }
run burin env PIN="taskset -c $CORE" bash bench/crunch/burin.sh "$IN" "" $RUNS
for c in gcc clang gcc-checked clang-checked; do run "c-$c" taskset -c $CORE $W/c-$c "$IN" "" $RUNS; done
flock -u 9
build 1
flock -x 9
run burin-no-steps env PIN="taskset -c $CORE" bash bench/crunch/burin.sh "$IN" "" $RUNS
flock -u 9
build 0
node -e '
const rows = require("fs").readFileSync(process.argv[1], "utf8").trim().split("\n").map(JSON.parse);
const c = rows.find(r => r.run === "c-clang");
console.log("run              deflate MB/s   vs clang   crc32 MB/s   vs clang   crc32 64 B ns   deflate 1 KB ns");
for (const r of rows) console.log(r.run.padEnd(16), String(r.deflate_mb_s).padStart(12), (r.deflate_mb_s / c.deflate_mb_s).toFixed(2).padStart(10), String(r.crc32_mb_s).padStart(12), (r.crc32_mb_s / c.crc32_mb_s).toFixed(2).padStart(10), String(r.crc32_64b_ns).padStart(15), String(r.deflate_1k_ns).padStart(17));
const g = rows.find(r => r.run === "c-gcc");
console.log(`context (other algorithms): zlib 1 ${g.zlib1_mb_s} MB/s (${g.zlib1_compressed} bytes), zlib 6 ${g.zlib6_mb_s} (${g.zlib6_compressed}), libdeflate 1 ${g.libdeflate1_mb_s} (${g.libdeflate1_compressed}), libdeflate 6 ${g.libdeflate6_mb_s} (${g.libdeflate6_compressed}); fixed-code greedy: ${g.compressed} bytes; zlib crc32 ${g.zlib_crc32_mb_s} MB/s`);
' $OUT
