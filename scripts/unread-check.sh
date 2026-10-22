#!/bin/bash
# Unread as counts (Room.posted, Membership.seen): a post marks the room unread for the others
# (one write to the room, none per member), never for the poster; reading the room clears it; the
# room being viewed is never shown unread. Golden data, as David and Jason.
#   bash /mnt/d/code/etch/scripts/unread-check.sh
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-gen-release}
D=/tmp/etch-un; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/etch/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13795; U=http://127.0.0.1:$PORT
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null $U/up 2>/dev/null && break; sleep 0.05; done
DAVID=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
JASON=jasontokenjasontokenjasontoken00
ROOM=486777696          # both are in it
OTHER=$(node -e "const {DatabaseSync}=require('node:sqlite');const d=new DatabaseSync('$D/data/app.sqlite');console.log(d.prepare(\"select m.room from Membership m join Room r on r.id = m.room join Membership j on j.room = m.room and j.user = (select user from Session where token = '$JASON') where m.user = 127326141 and r.type != 'direct' and m.room != $ROOM limit 1\").get().room)" 2>/dev/null)
fail=0
page() { curl -sS -H "Cookie: session_token=$1" $U/rooms/$2; }
# is room $3 shown unread on $1's page of room $2?
unread() { page "$1" "$2" | grep -c "<a href=\"/rooms/$3\" class=\"align-center gap room btn txt-nowrap unread\">" || true; }
want() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got $2, want $3"; fail=1; fi; }

want "Jason: room $OTHER read at first" "$(unread $JASON $ROOM $OTHER)" 0
curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data "room=$OTHER&body=unread+check" $U/_do/post_message
sleep 0.2
want "Jason: unread after David posts" "$(unread $JASON $ROOM $OTHER)" 1
want "David: his own post isn't unread for him" "$(unread $DAVID $ROOM $OTHER)" 0
want "Jason viewing it: not shown unread" "$(page $JASON $OTHER | grep -c "data-read-room=\"$OTHER\"")" 1
curl -sS -o /dev/null -H "Cookie: session_token=$JASON" --data "room=$OTHER" $U/_do/read_room
sleep 0.2
want "Jason: read after read_room" "$(unread $JASON $ROOM $OTHER)" 0
want "Jason viewing it again: nothing to mark" "$(page $JASON $OTHER | grep -c 'data-read-room=')" 0
want "the post wrote the room, not each member" "$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/data/app.sqlite').prepare('select posted from Room where id = $OTHER').get().posted)" 2>/dev/null)" 2
curl -sS $U/_etch/stats | grep -oE '"mismatched":[0-9]+'
grep -iE "panic|error" $D/server.log && fail=1
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
