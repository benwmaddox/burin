#!/bin/bash
# The load generator's CPU during a run's last phase (is it the bottleneck?): runs run.sh with
# the given settings and samples mixload's CPU over 8 seconds of the phase it waits for.
# Run as root:
#   [LIVE=1000] bash /mnt/d/code/burin/bench/mix/lg-cpu.sh <app> <run> <phase-file-before-it> [run.sh args after run…]
set -euo pipefail
APP=$1; RUN=$2; BEFORE=$3; shift 3
OUT=/mnt/d/code/burin/bench/runs/mix-$RUN
bash /mnt/d/code/burin/bench/mix/run.sh "$APP" "$RUN" "$@" > /tmp/lg-cpu-run.log 2>&1 &
RUNPID=$!
# the phase before the one sampled is written when the sampled one starts
until [ -s "$OUT/$BEFORE.json" ]; do sleep 0.5; done
sleep 4
P=$(pgrep -n mixload)
HZ=$(getconf CLK_TCK)
t0=$(awk '{print $14+$15}' /proc/$P/stat); s0=$(date +%s.%N)
sleep 8
t1=$(awk '{print $14+$15}' /proc/$P/stat); s1=$(date +%s.%N)
echo "mixload cores busy: $(echo "($t1 - $t0) / $HZ / ($s1 - $s0)" | bc -l | cut -c1-5) (of $(taskset -cp $P | sed 's/.*: //'))"
wait $RUNPID
