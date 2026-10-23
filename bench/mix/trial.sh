#!/bin/bash
# Short trial runs (5-second phases, 10 posts/s) of the given apps: run.sh works for each.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/trial.sh burin crudspec rust
for a in "$@"; do
  rm -rf /mnt/d/code/burin/bench/runs/mix-trial-$a
  bash /mnt/d/code/burin/bench/mix/run.sh $a trial-$a 5 10 2>&1 | tail -4 | cut -c1-460
done
