#!/bin/bash
# The mixed load's reads/s with one worker and with four, then the four-worker profile's top.
for w in 1 4; do
  echo "workers $w: $(WORKERS=$w bash /mnt/d/code/burin/bench/mix/profile.sh 6 2>&1 | head -1 | grep -o '"per_s":[0-9.]*')"
done
WORKERS=4 bash /mnt/d/code/burin/bench/mix/profile.sh 6 2>&1 | grep -E '^ +[0-9]' | head -12 | cut -c1-80
