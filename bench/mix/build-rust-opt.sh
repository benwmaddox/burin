#!/bin/bash
# The optimized variant of the Rust port (D:\code\campfire-rust-opt, branch "optimized"):
# release build (fat LTO, as the port ships) into ~/linuxbench/rust-opt/bin/campfire. Its target
# directory is in WSL home. Cores 0-7.
#   bash /mnt/d/code/burin/bench/mix/build-rust-opt.sh
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
SRC=/mnt/d/code/campfire-rust-opt; OUT=$HOME/linuxbench/rust-opt
mkdir -p $OUT/bin
cd $SRC
echo "building $(git rev-parse --short HEAD) ($(git branch --show-current))"
t0=$(date +%s)
CARGO_TARGET_DIR=$HOME/rust-opt-target taskset -c 0-7 cargo +1.98.1 build -q --release -p campfire 2>&1 | grep -E '^(error|warning: unused)' -A5 | head -40 || true
cp $HOME/rust-opt-target/release/campfire $OUT/bin/campfire
echo "built in $(( $(date +%s) - t0 )) s -> $OUT/bin/campfire ($(git rev-parse --short HEAD))"
git rev-parse HEAD > $OUT/REVISION
