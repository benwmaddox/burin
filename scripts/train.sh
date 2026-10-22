#!/bin/bash
# A training load for profile-guided optimization and BOLT: the binary serves a fresh copy of
# the bench snapshot (cores 0-3) while the load generator reads the room page and a writer posts
# (cores 4-7), then the binary exits cleanly so it writes its profile.
#   bash /mnt/d/code/etch/scripts/train.sh <binary> [secs]
set -e
BIN=$1; SECS=${2:-20}
D=/tmp/etch-train; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/crudspec-run/bench-snap-nc5 $D/data; rm -f $D/data/app.lock
PORT=13795
LG=$HOME/cm258/loadgen-linux/release/loadgen
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/crudspec-assets/campfire --port $PORT --workers 4 --exit-after $((SECS + 8)) > $D/server.log 2>&1 &
PID=$!
for i in $(seq 1 200); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
TOKEN=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
C="session_token=$TOKEN"
# the cold path and the warm-up, then reads with a writer posting into the room and into another
for r in 486777696 654632876 486777696; do curl -sS -o /dev/null -H "Cookie: $C" http://127.0.0.1:$PORT/rooms/$r; sleep 0.1; done
taskset -c 4-7 node /mnt/d/code/etch/bench/post-writer.mjs http://127.0.0.1:$PORT "$C" 486777696 20 $SECS > $D/writer.json &
taskset -c 4-7 $LG http --base http://127.0.0.1:$PORT --cookie "$C" --path /rooms/486777696 --conc 32 --duration $SECS --gzip 0 > $D/reads.json
wait %2 2>/dev/null || true
wait $PID || true
echo "trained: $(head -c 200 $D/reads.json) writer $(cat $D/writer.json)"
