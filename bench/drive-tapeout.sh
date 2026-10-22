#!/bin/bash
# The optimizer stages of the same hand-placed app: release (thin LTO) -> fat LTO + native ->
# + PGO -> + BOLT. Reads (fair lifetime protocol) and reads with a writer in the same room.
# Two passes, the second in reverse order; holds the shared bench lock throughout.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/etch/bench/drive-tapeout.sh <tag>
set -euo pipefail
TAG=$1
B=/mnt/d/code/etch/bench
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"
flock -x 9
echo "$(date -u +%T) have the lock"
export BENCH_LOCKED=1
builds=(etch-1a etch-lto etch-pgo etch-bolt)
one() { # pass, build
  local r1="$TAG-p$1-reads-$2" r2="$TAG-p$1-same-$2"
  echo "$(date -u +%T) $2"
  GOBIN=/home/ben/hw/$2 bash $B/lifetime.sh "$r1" 20 32 64 128 > /dev/null 2>&1 || echo "  failed: $r1"
  sed 's/^/  /' $B/runs/$r1/summary.jsonl 2>/dev/null | cut -c1-200
  GOBIN=/home/ben/hw/$2 bash $B/writes.sh "$r2" 10 0 5 20 > /dev/null 2>&1 || echo "  failed: $r2"
  sed 's/^/  /' $B/runs/$r2/summary.jsonl 2>/dev/null | cut -c1-200
}
for b in "${builds[@]}"; do one 1 $b; done
for ((i=${#builds[@]}-1; i>=0; i--)); do one 2 ${builds[$i]}; done
echo "$(date -u +%T) done"
