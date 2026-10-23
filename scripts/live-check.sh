#!/bin/bash
# Live updates on Campfire's golden data: Jason holds the room's stream (as the page's
# <turbo-stream-source> does) while David posts, edits, boosts and deletes; each change must
# arrive as its Turbo Stream, in order. Opening the stream marks the room read (watch read_room);
# a reconnect with Last-Event-ID gets what it missed; a stream for a room the viewer isn't in is
# refused.
#   bash /mnt/d/code/burin/scripts/live-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-live; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13804; U=http://127.0.0.1:$PORT
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null; kill %2 %3 2>/dev/null || true" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null $U/up 2>/dev/null && break; sleep 0.05; done
sql() { node -e "const {DatabaseSync}=require('node:sqlite');const d=new DatabaseSync('$D/data/app.sqlite');const r=d.prepare(process.argv[1]).get();console.log(r?Object.values(r)[0]:'')" "$1" 2>/dev/null; }
DAVID=$(sql 'select token from Session where id = 481019661')
JASON=jasontokenjasontokenjasontoken00
JU=$(sql "select user from Session where token = '$JASON'")
ROOM=$(sql "select m.room from Membership m join Room r on r.id = m.room where m.user = 127326141 and r.type != 'direct' and m.room in (select room from Membership where user = $JU) and m.room != 486777696 limit 1")
fail=0
want() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got $2, want $3"; fail=1; fi; }
# David posts once first, so Jason has something unread
curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data "room=$ROOM&body=before+the+stream" $U/_do/post_message
sleep 0.3
want "Jason has the room unread" "$(sql "select (r.posted > m.seen) from Membership m join Room r on r.id = m.room where m.room = $ROOM and m.user = $JU")" 1
# Jason opens the page: its stream's address
curl -sS -H "Cookie: session_token=$JASON" $U/rooms/$ROOM > $D/page.html
SRC=$(grep -o '<turbo-stream-source src="[^"]*"' $D/page.html | sed 's/.*src="//; s/"$//; s/&amp;/\&/g')
want "the page has its stream" "$(echo "$SRC" | grep -c '^/_burin/live?s=')" 1
want "the list's element has its live id" "$(grep -c 'id="live_room_messages"' $D/page.html)" 1
curl -sSN -H "Cookie: session_token=$JASON" "$U$SRC" > $D/stream.txt &
sleep 0.4
want "opening the stream read the room (watch read_room)" "$(sql "select (r.posted > m.seen) from Membership m join Room r on r.id = m.room where m.room = $ROOM and m.user = $JU")" 0
# David: a post, an edit of it, a boost, a delete
curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data "room=$ROOM&body=live+hello" $U/_do/post_message
sleep 0.3
MSG=$(sql "select id from Message where body = 'live hello'")
CID=$(sql "select client_id from Message where id = $MSG")
curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data "__arg_message=%23$MSG&body=live+edited" $U/_do/edit_message
sleep 0.3
curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data "__arg_message=%23$MSG&__arg_content=%F0%9F%91%8D" $U/_do/boost
sleep 0.3
curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data "__arg_message=%23$MSG" $U/_do/delete_message
sleep 0.5
kill %2 2>/dev/null || true
sleep 0.2
actions=$(grep -o 'data: <turbo-stream action="[a-z]*" target="[^"]*"' $D/stream.txt | sed 's/.*action="//; s/" target="/ /; s/"$//' | tr '\n' ',')
want "the stream's events (the list's, and the sidebar's signals)" "$actions" "read $ROOM,append live_room_messages,unread $ROOM,replace message_$CID,replace message_$CID,remove message_$CID,"
want "the append carries the message" "$(grep -c 'data: .*live hello' $D/stream.txt)" 1
want "the edit's replace carries the new text" "$(grep -c 'data: .*live edited' $D/stream.txt)" 2
want "events have ids" "$(grep -c '^id: [0-9]' $D/stream.txt)" 6
# a reconnect from the post gets what it missed after it (and nothing earlier): the first stream's
# closing read the room (read: it had posts since it opened)
POST=$(grep -B2 'data: <turbo-stream action="append"' $D/stream.txt | grep -m1 '^id: ' | sed 's/id: //')
curl -sSN -m 1 -H "Cookie: session_token=$JASON" -H "Last-Event-ID: $POST" "$U$SRC" > $D/replay.txt || true
want "a reconnect gets the events after its Last-Event-ID" "$(grep -o 'data: <turbo-stream action="[a-z]*"' $D/replay.txt | sed 's/.*action="//; s/"$//' | tr '\n' ',')" "unread,replace,replace,remove,read,"
# not a member: refused
OUT=$(sql "select r.id from Room r where r.type = 'closed' and r.id not in (select room from Membership where user = $JU) limit 1")
if [ -n "$OUT" ]; then
  want "a stream for a room Jason isn't in is refused" "$(curl -sS -m 1 -o /dev/null -w '%{http_code}' -H "Cookie: session_token=$JASON" "$U/_burin/live?s=0&k=$OUT&since=0" || true)" 404
fi
curl -sS $U/_burin/stats | grep -oE '"mismatched":[0-9]+|"live":[0-9]+' | tr '\n' ' '; echo
grep -iE "panic" $D/server.log && fail=1
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
