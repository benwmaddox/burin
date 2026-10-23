#!/bin/bash
# The mixed benchmark for several implementations: two passes, the second in reverse order,
# holding the shared bench lock throughout.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/drive.sh <tag> <app>…
set -euo pipefail
TAG=$1; shift; APPS=("$@")
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"; flock -x 9; echo "$(date -u +%T) have the lock"
export BENCH_LOCKED=1
one() { echo "$(date -u +%T) $2 pass $1"; bash /mnt/d/code/burin/bench/mix/run.sh "$2" "$TAG-p$1-$2" "${SECS:-15}" ${RATES:-0 10 50} > /dev/null 2>&1 || echo "  failed: $2"; }
for a in "${APPS[@]}"; do one 1 "$a"; done
for ((i=${#APPS[@]}-1; i>=0; i--)); do one 2 "${APPS[$i]}"; done
node /mnt/d/code/burin/bench/mix/table.mjs /mnt/d/code/burin/bench/runs "mix-$TAG-"
echo "$(date -u +%T) done"
