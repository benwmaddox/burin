#!/bin/bash
# Live updates in real browsers (tools/browser/live.mjs): David and Jason on the same room (and
# a second one, for the sidebar's signals), on a copy of the golden data, --verify.
#   bash /mnt/d/code/burin/scripts/browser-live.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-brl; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13805
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
DAVID=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
JU=$(node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "select user from Session where token = 'jasontokenjasontokenjasontoken00'")
# another room both are in, on Jason's sidebar
B=$(node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "select m.room from Membership m join Room r on r.id = m.room where m.user = $JU and m.involvement != 'invisible' and r.type != 'direct' and m.room != 486777696 and m.room in (select room from Membership where user = 127326141) order by m.room limit 1")
cd /mnt/d/code/burin/tools/browser
taskset -c 4-7 node live.mjs http://127.0.0.1:$PORT "$DAVID" jasontokenjasontokenjasontoken00 486777696 $B
curl -sS http://127.0.0.1:$PORT/_burin/stats | grep -oE '"mismatched":[0-9]+|"live":[0-9]+' | tr '\n' ' '; echo
