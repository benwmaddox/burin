#!/bin/bash
# A deliberately broken build of the Rust port variant, to prove rustopt-verify.sh catches stale
# pages: crates/db/src/changes.rs mutated while it builds (then put back),
# into ~/linuxbench/rust-opt/bin/campfire-mutant.
#   bash /mnt/d/code/burin/bench/mix/build-rust-opt-mutant.sh
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
cd /mnt/d/code/campfire-rust-opt
F=crates/db/src/changes.rs
cp $F /tmp/changes.rs.good
trap "cp /tmp/changes.rs.good $F" EXIT
# the mutation: a new or changed message no longer moves its room's version
python3 - "$F" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
a = """                Some(room) => {
                    rooms.insert(room);
                    activity = true;
                }"""
assert s.count(a) == 1, "the mutation's target moved"
open(p, "w").write(s.replace(a, """                Some(_room) => {
                    activity = true;
                }"""))
PY
CARGO_TARGET_DIR=$HOME/rust-opt-target-mutant taskset -c 0-7 cargo +1.98.1 build -q --release -p campfire 2>&1 | grep -E '^error' -A5 | head -20 || true
cp $HOME/rust-opt-target-mutant/release/campfire $HOME/linuxbench/rust-opt/bin/campfire-mutant
echo "built the mutant"
