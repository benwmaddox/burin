#!/bin/bash
# A periodic check: the current build against an earlier etch build and crudspec, gzip on, under
# writes into the read room (0, 5, 50, 200 posts/s). Two passes, the second in reverse order;
# holds the bench lock.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/etch/bench/drive-now.sh <tag> <current binary> [earlier binary]
set -euo pipefail
TAG=$1; CUR=$2; EARLIER=${3:-/home/ben/hw/etch-fwd}
B=/mnt/d/code/etch/bench
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"; flock -x 9; echo "$(date -u +%T) have the lock"
export BENCH_LOCKED=1
runs=(
  "now     GOBIN=$CUR"
  "earlier GOBIN=$EARLIER"
  "cs      GOBIN=/home/ben/hw/campfire-pk"
)
one() {
  read -r name envs <<< "$2"
  local run="$TAG-p$1-$name"
  echo "$(date -u +%T) $run"
  env $envs GZIP=1 bash $B/writes.sh "$run" 10 0 5 50 200 > /dev/null 2>&1 || echo "  failed: $run"
  sed 's/^/  /' $B/runs/$run/summary.jsonl 2>/dev/null | cut -c1-230
}
for s in "${runs[@]}"; do one 1 "$s"; done
for ((i=${#runs[@]}-1; i>=0; i--)); do one 2 "${runs[$i]}"; done
echo "$(date -u +%T) done"
