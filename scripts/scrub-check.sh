#!/bin/bash
# The scrubber: a message changed behind burin's back (straight in SQLite, as a missed edge would
# leave it) is found by the idle writer, logged and rebuilt; readers then get the right page.
#   bash /mnt/d/code/burin/scripts/scrub-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-sc; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13792
q() { node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "$1"; }
DAVID=$(q "select token from Session where id = 481019661")
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --scrub-ms 20 > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
U=http://127.0.0.1:$PORT
get() { curl -sS -o $D/page.html -w "%{http_code}" -H "Cookie: session_token=$DAVID" $U/rooms/486777696; }
get >/dev/null; sleep 0.5; get >/dev/null
echo "before: $(curl -sS $U/_burin/stats)"
M=$(q "select id from Message where room = 486777696 order by created desc limit 1")
node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "update Message set body = 'changed behind burin''s back' where id = $M"
echo "served right after the change: $(get), shows the change: $(grep -c "changed behind burin" $D/page.html || true)"
sleep 2
echo "after two idle seconds: $(get), shows the change: $(grep -c "changed behind burin" $D/page.html || true)"
echo "after: $(curl -sS $U/_burin/stats)"
grep "^scrub" $D/server.log | head -3
