#!/bin/bash
# burin's helpers timed (crates/crunch-app/tests/compute.rs): DEFLATE and CRC-32 on a file, the
# generated module linked under the same LTO as the app. Prints one JSON line.
#   bash /mnt/d/code/burin/bench/crunch/burin.sh <input> [compressed output] [runs]
set -e
source ~/.cargo/env 2>/dev/null || true
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
cd /mnt/d/code/burin
BURIN_CRUNCH_INPUT=$1 BURIN_CRUNCH_OUT=${2:-} BURIN_CRUNCH_RUNS=${3:-9} BURIN_GEN_DIR=$HOME/burin-gen-crunch RUSTFLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld" CARGO_TARGET_DIR=$HOME/burin-target-gen ${PIN:-taskset -c 0-7} cargo test -q --release -p crunch-app --test compute -- --nocapture 2>&1 | grep -E '^\{|panicked|error'
