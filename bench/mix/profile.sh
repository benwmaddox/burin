#!/bin/bash
# Where an app's request workers spend their time under the mixed load (cores 0-7, not a
# measurement): etch on the mixed data, mixload for some seconds, perf on the server.
#   bash /mnt/d/code/etch/bench/mix/profile.sh [secs] [binary]
set -euo pipefail
SECS=${1:-8}; BIN=${2:-/home/ben/hw/etch-gen-release}
M=/home/ben/etch-mix; D=/tmp/etch-mix-prof; rm -rf $D; mkdir -p $D; cp -a "$M/etch" $D/data; rm -f $D/data/app.lock
PORT=13786
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT --workers ${WORKERS:-4} > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 200); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
# warm (every room), then profile
taskset -c 4-7 /home/ben/etch-target-mixload/release/mixload --base http://127.0.0.1:$PORT --people $M/people.json --app etch --conc 16 --duration 4 > /dev/null
P=$(ls /usr/lib/linux-tools-*/perf | head -1)
$P record -q -g -p $PID -o $D/perf.data -- sleep $SECS > /dev/null 2>&1 &
PERF=$!
taskset -c 4-7 /home/ben/etch-target-mixload/release/mixload --base http://127.0.0.1:$PORT --people $M/people.json --app etch --conc 16 --duration $SECS --post-rate 10 | head -c 600; echo
wait $PERF || true
curl -sS http://127.0.0.1:$PORT/_etch/stats; echo
$P report -i $D/perf.data --no-children --sort symbol --stdio 2>/dev/null | grep -E '^ +[0-9]' | head -25
