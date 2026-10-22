#!/bin/bash
# Where the fast path's CPU goes: user (etch's code) against kernel (the socket's copies), from
# /proc's counters, under 10 s of load. Server on cores 0-3, load generator on 4-7.
#   bash /mnt/d/code/etch/scripts/cpu-split.sh <binary> [gzip 0|1]
set -e
BIN=$1; GZ=${2:-0}
D=/tmp/etch-split; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/crudspec-run/bench-snap-nc5 $D/data; rm -f $D/data/app.lock
PORT=13796
LG=$HOME/cm258/loadgen-linux/release/loadgen
cd /mnt/d/code/crudspec
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/crudspec-assets/campfire --port $PORT > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 200); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
TOKEN=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
taskset -c 4-7 $LG http --base http://127.0.0.1:$PORT --cookie "session_token=$TOKEN" --path /rooms/486777696 --conc 8 --duration 3 --gzip $GZ > /dev/null
read -r u0 s0 < <(awk '{print $14, $15}' /proc/$PID/stat)
taskset -c 4-7 $LG http --base http://127.0.0.1:$PORT --cookie "session_token=$TOKEN" --path /rooms/486777696 --conc 32 --duration 10 --gzip $GZ > $D/r.json
read -r u1 s1 < <(awk '{print $14, $15}' /proc/$PID/stat)
OK=$(node -e "console.log(JSON.parse(require('fs').readFileSync('$D/r.json','utf8')).ok)")
HZ=$(getconf CLK_TCK)
node -e "const u=($u1-$u0)/$HZ, s=($s1-$s0)/$HZ, ok=$OK; console.log(JSON.stringify({bin:'$(basename $BIN)', gzip:$GZ, req_s: Math.round(ok/10), user_ms_per_req:+(u*1000/ok).toFixed(4), kernel_ms_per_req:+(s*1000/ok).toFixed(4), avg_bytes: JSON.parse(require('fs').readFileSync('$D/r.json','utf8')).avg_bytes}))"
