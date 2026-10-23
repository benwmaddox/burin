#!/bin/bash
# The live updates' checks only (a quicker loop than check-all while working on them).
#   bash /mnt/d/code/burin/scripts/live-all.sh
cd /mnt/d/code/burin
export BURIN_BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
for s in live-check revoke-check signals-check browser-live; do
  echo "$s: $(bash scripts/$s.sh 2>&1 | grep -E '^(PASS|FAIL)' | tail -1)"
done
for seed in 1 2; do
  echo "live-fuzz $seed: $(bash scripts/live-fuzz.sh 400 $seed 2>&1 | grep -E '^(PASS|FAIL)$|DIFFERS|STREAM' | head -1)"
done
