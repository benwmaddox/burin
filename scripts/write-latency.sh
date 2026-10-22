#!/bin/bash
# Post latency at a write rate, with readers, on cores 0-7 (server 0-3), and a perf profile of
# the writer thread.
#   bash /mnt/d/code/etch/scripts/write-latency.sh [binary] [posts/s] [secs]
set -e
BIN=${1:-$HOME/hw/etch-gen-release}; RATE=${2:-200}; SECS=${3:-10}
D=/tmp/etch-wl; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/etch/data/campfire $D/data; rm -f $D/data/app.lock
PORT=13799
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT --workers 3 > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
TOKEN=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
C="session_token=$TOKEN"
curl -sS -o /dev/null -H "Cookie: $C" http://127.0.0.1:$PORT/rooms/486777696; sleep 0.3
WT=$(ls /proc/$PID/task | while read t; do [ "$(cat /proc/$PID/task/$t/comm)" = etch-writer ] && echo $t || true; done)
perf record -q -g -t $WT -o $D/perf.data -- sleep $SECS > /dev/null 2>&1 &
PERF=$!
taskset -c 6-7 /mnt/d/code/etch/tools/loadgen http --base http://127.0.0.1:$PORT --cookie "$C" --path /rooms/486777696 --conc 8 --duration $SECS --gzip 1 > $D/reads.json &
LGP=$!
taskset -c 4-5 node /mnt/d/code/etch/bench/post-writer.mjs http://127.0.0.1:$PORT "$C" 486777696 $RATE $SECS
wait $PERF 2>/dev/null || true
wait $LGP
echo "reads: $(node -e "const r=require('$D/reads.json');console.log(Math.round(r.ok/$SECS),'per s')")"
perf report -i $D/perf.data --no-children --sort symbol --stdio 2>/dev/null | grep -E '^ +[0-9]' | head -25
