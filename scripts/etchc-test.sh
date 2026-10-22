#!/bin/bash
# The compiler's tests (its refusals), on cores 0-7.
set -e
source ~/.cargo/env 2>/dev/null || true
cd /mnt/d/code/etch
CARGO_TARGET_DIR=$HOME/etch-target taskset -c 0-7 cargo +1.99.0 test -q -p etchc 2>&1 | tail -${1:-30}
