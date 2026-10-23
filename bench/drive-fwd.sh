#!/bin/bash
# The compiled burin (forwarding) against crudspec, under writes into the
# read room, gzip off and on. Two passes, the second in reverse order; holds the bench lock.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/drive-fwd.sh <tag>
set -euo pipefail
TAG=$1
B=/mnt/d/code/burin/bench
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"; flock -x 9; echo "$(date -u +%T) have the lock"
export BENCH_LOCKED=1
runs=(
  "gen  0 GOBIN=/home/ben/hw/burin-fwd"
  "cs   0 GOBIN=/home/ben/hw/campfire-pk"
  "gen  1 GOBIN=/home/ben/hw/burin-fwd"
  "cs   1 GOBIN=/home/ben/hw/campfire-pk"
)
one() {
  read -r name gz envs <<< "$2"
  local run="$TAG-p$1-gz$gz-$name"
  echo "$(date -u +%T) $run"
  env $envs GZIP=$gz bash $B/writes.sh "$run" 10 0 5 20 50 200 > /dev/null 2>&1 || echo "  failed: $run"
  sed 's/^/  /' $B/runs/$run/summary.jsonl 2>/dev/null | cut -c1-230
}
for s in "${runs[@]}"; do one 1 "$s"; done
for ((i=${#runs[@]}-1; i>=0; i--)); do one 2 "${runs[$i]}"; done
echo "$(date -u +%T) done"
