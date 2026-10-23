#!/bin/bash
# A real browser (headless Chromium) on burin's Campfire, on a copy of the golden data, as David:
# tools/browser/smoke.mjs (Turbo: the sidebar loaded once and kept, visits without reloads,
# posting from the composer).
#   bash /mnt/d/code/burin/scripts/browser-smoke.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-br; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13796
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
DAVID=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
cd /mnt/d/code/burin/tools/browser
taskset -c 4-7 node smoke.mjs http://127.0.0.1:$PORT "$DAVID" 486777696
curl -sS http://127.0.0.1:$PORT/_burin/stats | grep -oE '"mismatched":[0-9]+'
