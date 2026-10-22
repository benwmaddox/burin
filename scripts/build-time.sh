#!/bin/bash
# Time to rebuild the release binary after a one-line change in the app crate (the code the
# compiler will generate), pinned away from the bench cores.
#   bash /mnt/d/code/etch/scripts/build-time.sh
set -e
source ~/.cargo/env 2>/dev/null || true
export CARGO_TARGET_DIR=$HOME/etch-target
cd /mnt/d/code/etch
F=crates/campfire-hand/src/render.rs
taskset -c 0-7 cargo build --release -p campfire-hand -q 2>/dev/null
cp $F /tmp/render.rs.bak
trap "cp /tmp/render.rs.bak $F" EXIT
for i in 1 2 3; do
  sed -i "s/Jump to newest message/Jump to newest message$i/" $F
  s=$(date +%s.%N); taskset -c 0-7 cargo build --release -p campfire-hand -q 2>/dev/null; e=$(date +%s.%N)
  echo "rebuild after a one-line change: $(echo "$e - $s" | bc) s"
  cp /tmp/render.rs.bak $F
done
