#!/bin/bash
# Two builds of the Rust port's optimized variant, compared on the bench cores: runs in the order
# A B B A, short phases, under the shared bench lock.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/ab-rustopt-bin.sh <tag> <binary A> <binary B> [secs] [rates]
set -euo pipefail
TAG=$1; A=$2; B=$3; SECS=${4:-8}; RATES=${5:-0 50}
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"; flock -x 9; echo "$(date -u +%T) have the lock"
export BENCH_LOCKED=1
n=0
for side in A B B A; do
  n=$((n+1))
  if [ $side = A ]; then bin=$A; else bin=$B; fi
  echo "$(date -u +%T) $side ($bin)"
  RUSTOPT_BIN="$bin" bash /mnt/d/code/burin/bench/mix/run.sh rustopt "ab-$TAG-$n$side" "$SECS" $RATES > /dev/null 2>&1 || echo "  failed"
done
node /mnt/d/code/burin/bench/mix/table.mjs /mnt/d/code/burin/bench/runs "mix-ab-$TAG-"
