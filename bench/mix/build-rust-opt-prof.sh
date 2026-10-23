#!/bin/bash
# The Rust port variant with frame pointers (for perf call stacks; not for measuring), into
# ~/linuxbench/rust-opt/bin/campfire-prof.
#   bash /mnt/d/code/etch/bench/mix/build-rust-opt-prof.sh
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
cd /mnt/d/code/campfire-rust-opt
RUSTFLAGS="-C force-frame-pointers=yes" CARGO_TARGET_DIR=$HOME/rust-opt-target-prof taskset -c 0-7 cargo +1.98.1 build -q --release -p campfire 2>&1 | grep -E '^error' -A5 | head -20 || true
cp $HOME/rust-opt-target-prof/release/campfire $HOME/linuxbench/rust-opt/bin/campfire-prof
echo "built $(git rev-parse --short HEAD) with frame pointers"
