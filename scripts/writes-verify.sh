#!/bin/bash
# Correctness under writes: on a copy of the golden data, posts messages (as both viewers, into
# the bench room and into another room both are in) between reads, with --verify on: every fast
# page must equal a fresh render from SQLite.
#   bash /mnt/d/code/etch/scripts/writes-verify.sh [posts]
set -e
N=${1:-60}
BIN=$HOME/etch-target/release/campfire-hand
D=/tmp/etch-wv; rm -rf $D; mkdir -p $D; cp -a ~/hw/golden-data $D/data; rm -f $D/data/app.lock
PORT=13791
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/crudspec-assets/campfire --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
DAVID=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
JASON=jasontokenjasontokenjasontoken00
get() { curl -sS -o /dev/null -w "%{http_code} " -H "Cookie: session_token=$1" http://127.0.0.1:$PORT/rooms/$2; }
post() { curl -sS -o /dev/null -w "%{http_code} " -H "Cookie: session_token=$1" --data-urlencode "room=$2" --data-urlencode "body=$3" http://127.0.0.1:$PORT/_do/post_message; }
for r in 486777696 654632876; do get $DAVID $r; get $JASON $r; done; sleep 0.3; echo
for i in $(seq 1 $N); do
  case $((i % 4)) in
    0) post $DAVID 486777696 "david's message $i: <b>bold</b> & https://example.com/x?y=1 'quoted'" ;;
    1) post $JASON 486777696 "jason says $i" ;;
    2) post $JASON 654632876 "in designers $i" ;;
    3) post $DAVID 654632876 "line one $i
line two" ;;
  esac
  get $DAVID 486777696; get $JASON 486777696; get $DAVID 654632876; get $JASON 654632876
done
echo
sleep 0.3
for r in 486777696 654632876; do get $DAVID $r; get $JASON $r; done; echo
curl -sS http://127.0.0.1:$PORT/_etch/stats; echo
grep -v "^started" $D/server.log | head -20 || true
