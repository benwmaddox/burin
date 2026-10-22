#!/bin/bash
# Runs the send experiment for every method, raw and gzip-sized pages.
set -e
source ~/.cargo/env 2>/dev/null || true
cd /mnt/d/code/etch
exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9
CARGO_TARGET_DIR=$HOME/etch-target taskset -c 0-7 cargo build -q --release -p sendlab
B=$HOME/etch-target/release/sendlab
for size in raw gz; do
  for m in ${METHODS:-writev zerocopy sendfile sendfilec sendfile5 sendfile1 splice}; do
    $B $m $size ${1:-20000} || echo "$m $size failed"
  done
done
