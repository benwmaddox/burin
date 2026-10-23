#!/bin/bash
# The generated Campfire's hot-path test (tests/hot_path.rs), on a fresh copy of the golden data.
set -e
source ~/.cargo/env 2>/dev/null || true
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
D=/tmp/burin-hp; rm -rf $D; cp -a /mnt/d/code/burin/data/campfire-golden $D; rm -f $D/app.lock
cd /mnt/d/code/burin
TZ=America/New_York BURIN_TRACE_ALLOCS=${TRACE:-} BURIN_TEST_DATA=$D BURIN_GEN_DIR=$HOME/burin-gen-campfire RUSTFLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld" CARGO_TARGET_DIR=$HOME/burin-target-gen taskset -c 0-7 cargo test -q --release -p campfire-app -- --nocapture 2>&1 | grep -E "the request|fast page|panicked|assert|test result|error|alloc from|^ +[0-9]+: |at ./" 
