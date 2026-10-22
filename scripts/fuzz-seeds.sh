#!/bin/bash
# Several fuzz seeds: bash scripts/fuzz-seeds.sh <steps> <seed>…
STEPS=$1; shift
for s in "$@"; do
  bash /mnt/d/code/etch/scripts/fuzz.sh $STEPS $s 2>&1 | grep -E '"seed"|PASS|FAIL' | cut -c1-160
done
