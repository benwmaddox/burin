#!/bin/bash
# Phase 1a's measurements, holding the shared bench lock throughout: reads (the fair lifetime
# protocol) and reads under writes, for etch, crudspec (default caches), crudspec with only its
# fragment cache, and crudspec with CRUDSPEC_PAGE_KEYS=1. Two passes, the second in reverse order.
# Run as root:
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/etch/bench/drive-1a.sh <tag>
set -euo pipefail
TAG=$1
B=/mnt/d/code/etch/bench
ETCH=/home/ben/hw/etch-1a
CS=/home/ben/hw/campfire-pk
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"
flock -x 9
echo "$(date -u +%T) have the lock"
export BENCH_LOCKED=1
runs=(
  "reads etch       GOBIN=$ETCH"
  "reads cs         GOBIN=$CS"
  "reads csfrag     GOBIN=$CS NO_PAGE_CACHE=1"
  "same etch        GOBIN=$ETCH"
  "same cs          GOBIN=$CS"
  "same cspk        GOBIN=$CS PAGE_KEYS=1"
  "other etch       GOBIN=$ETCH WROOM=654632876"
  "other cs         GOBIN=$CS WROOM=654632876"
  "other cspk       GOBIN=$CS PAGE_KEYS=1 WROOM=654632876"
)
one() { # pass, spec
  read -r kind name envs <<< "$2"
  local run="$TAG-p$1-$kind-$name"
  echo "$(date -u +%T) $run ($envs)"
  if [ "$kind" = reads ]; then
    env $envs bash $B/lifetime.sh "$run" 20 16 32 64 128 > /dev/null 2>&1 || echo "  failed: $run"
  else
    env $envs bash $B/writes.sh "$run" 10 0 1 5 20 50 > /dev/null 2>&1 || echo "  failed: $run"
  fi
  tail -n +1 $B/runs/$run/summary.jsonl 2>/dev/null | sed 's/^/  /' | cut -c1-260
}
for s in "${runs[@]}"; do one 1 "$s"; done
for ((i=${#runs[@]}-1; i>=0; i--)); do one 2 "${runs[$i]}"; done
echo "$(date -u +%T) done"
