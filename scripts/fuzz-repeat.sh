#!/bin/bash
# One fuzz seed several times; for a failing run, its server's mismatch reports.
#   bash /mnt/d/code/burin/scripts/fuzz-repeat.sh <steps> <seed> <times>
for i in $(seq 1 $3); do
  r=$(bash /mnt/d/code/burin/scripts/fuzz.sh $1 $2 2>&1 | tail -1)
  echo "run $i: $r"
  case "$r" in FAIL*) grep -A2 "^verify" /tmp/burin-fz/server.log | head -12 | cut -c1-240; break;; esac
done
