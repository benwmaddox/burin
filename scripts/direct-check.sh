#!/bin/bash
# Direct rooms: each member sees the room named for the others (variant bits, no per-viewer copy
# of the page); with --verify, every fast page equals a fresh render; a post into one keeps it fast.
#   bash /mnt/d/code/etch/scripts/direct-check.sh
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-gen-release}
D=/tmp/etch-dc; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/etch/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13795
q() { node /mnt/d/code/etch/scripts/q.js $D/data/app.sqlite "$1"; }
DAVID=$(q "select token from Session where id = 481019661")
JASON=jasontokenjasontokenjasontoken00
DU=$(q "select user from Session where token = '$DAVID'"); JU=$(q "select user from Session where token = '$JASON'")
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
get() { curl -sS -o $D/$1-$2.html -w "%{http_code} " $3 -H "Cookie: session_token=$1" http://127.0.0.1:$PORT/rooms/$2; }
title() { grep -o '<title>[^<]*</title>' $D/$1-$2.html | sed 's/<[^>]*>//g'; }
for r in $(q "select id from Room where type = 'direct' order by id"); do
  members=$(q "select group_concat(u.name, ', ') from Membership m join User u on u.id = m.user where m.room = $r")
  echo "direct room $r, members: $members"
  for who in DAVID JASON; do
    t=${!who}
    if [ -n "$(q "select 1 from Membership where room = $r and user = (select user from Session where token = '$t')")" ]; then
      get $t $r; sleep 0.2; get $t $r; get $t $r --compressed
      echo " | $who sees \"$(title $t $r)\""
    fi
  done
done
SHARED=$(q "select room from Membership where user = $DU and room in (select room from Membership where user = $JU) and room in (select id from Room where type = 'direct') limit 1")
if [ -n "$SHARED" ]; then
  echo "a direct room both are in: $SHARED; david posts into it"
  curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data-urlencode "room=$SHARED" --data-urlencode "body=hello from david" http://127.0.0.1:$PORT/_do/post_message
  sleep 0.3; get $DAVID $SHARED; get $JASON $SHARED; get $JASON $SHARED --compressed; echo
  echo "david sees \"$(title $DAVID $SHARED)\", jason sees \"$(title $JASON $SHARED)\""
fi
echo "the open room still: $(get $DAVID 486777696; title $DAVID 486777696)"
curl -sS http://127.0.0.1:$PORT/_etch/stats; echo
grep -v "^started" $D/server.log | head -10 || true
