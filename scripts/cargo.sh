#!/bin/bash
# Runs cargo in WSL with the target directory in WSL's home (D: is short of space).
#   bash /mnt/d/code/etch/scripts/cargo.sh <cargo args...>
set -e
source ~/.cargo/env 2>/dev/null || true
export CARGO_TARGET_DIR=$HOME/etch-target
cd /mnt/d/code/etch
cargo "$@"
