#!/bin/bash
# Signals (`signal … to …`), on Campfire's golden data, as Rails' unread and read streams and its
# sidebar broadcasts: a post marks the room unread on its members' open pages (and no one
# else's); a room read on one page is marked read on the viewer's others; a room someone sat in
# while posts came isn't unread on their next sidebar (closing the stream read it); a new open
# room reloads everyone's sidebar, a closed one only its members'.
#   bash /mnt/d/code/burin/scripts/signals-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-sig; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13808; U=http://127.0.0.1:$PORT
q() { node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "$1"; }
DAVID=$(q "select token from Session where id = 481019661"); DU=$(q "select user from Session where token = '$DAVID'")
JASON=jasontokenjasontokenjasontoken00; JU=$(q "select user from Session where token = '$JASON'")
KU=$(q "select id from User where email = 'kevin@37signals.com'")
KEVIN=kevin${KU}tokentokentokentoken
node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "insert into Session (id, seq, created, updated, user, token) values (919999999, 1, 0, 0, $KU, '$KEVIN')"
# rooms David and Jason share (A, B), and one of them Kevin isn't in (B)
A=$(q "select m.room from Membership m join Room r on r.id = m.room where m.user = $DU and r.type != 'direct' and m.room in (select room from Membership where user = $JU) and m.room in (select room from Membership where user = $KU) order by m.room limit 1")
B=$(q "select m.room from Membership m join Room r on r.id = m.room where m.user = $DU and r.type != 'direct' and m.room in (select room from Membership where user = $JU) and m.room not in (select room from Membership where user = $KU) order by m.room limit 1")
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap 'kill $PID $HOLDS 2>/dev/null || true' EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null $U/up 2>/dev/null && break; sleep 0.05; done
fail=0
want() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got $2, want $3"; fail=1; fi; }
act() { local who=$1 name=$2; shift 2; local a=(); for f in "$@"; do a+=(--data-urlencode "$f"); done
  curl -sS -o /dev/null -D - -H "Cookie: session_token=$who" "${a[@]}" $U/_do/$name | tr -d '\r' | awk '/^[Ll]ocation/{print $2}'; }
src() { curl -sS -H "Cookie: session_token=$1" $U/rooms/$2 | grep -o '<turbo-stream-source src="[^"]*"' | sed 's/.*src="//; s/"$//; s/&amp;/\&/g'; }
HOLDS=""
hold() { curl -sSN -H "Cookie: session_token=$1" "$U$(src $1 $2)" > $D/$3.txt & HOLDS="$HOLDS $!"; }
signals() { grep -o 'data: <turbo-stream action="\(unread\|read\|sidebar\|missed\)"[^>]*>' $D/$1.txt | sed 's/data: <turbo-stream action="//; s/" target="/ /; s/"*>$//' | tr '\n' ',' ; }
sidebar_unread() { curl -sS -H "Cookie: session_token=$1" $U/users/me/sidebar | grep -c "href=\"/rooms/$2\" class=\"[^\"]*unread" || true; }
[ -n "$A" ] && [ -n "$B" ] || { echo "FAIL no rooms to use"; exit 1; }

hold $JASON $A jasonA; hold $KEVIN $A kevinA
sleep 0.4
act $DAVID post_message "room=$B" "body=in b" > /dev/null
act $DAVID post_message "room=$A" "body=in a" > /dev/null
sleep 0.4
want "Jason's page (on A): unread for B and A" "$(signals jasonA)" "unread $B,unread $A,"
want "Kevin's (not in B): only A's" "$(signals kevinA)" "unread $A,"
# Jason reads B on another page: his page on A marks it read
hold $JASON $B jasonB
sleep 0.4
want "…and read for B, when he opens B elsewhere" "$(signals jasonA)" "unread $B,unread $A,read $B,"
# Jason leaves both pages; A was posted to while he sat in it
want "while he's in A, its new post still counts (the page's script hides it)" "$(sidebar_unread $JASON $A)" 1
kill $HOLDS 2>/dev/null || true
sleep 0.4
act $DAVID post_message "room=$B" "body=b again" > /dev/null
sleep 0.3
want "his next sidebar: A read (he was in it), B unread (posted after he left)" "$(sidebar_unread $JASON $A) $(sidebar_unread $JASON $B)" "0 1"
# rooms: an open one reloads every sidebar; a closed one its members'
hold $JASON $A jasonA2; hold $KEVIN $A kevinA2
sleep 0.4
act $DAVID create_closed_room "name=Just us" "users=$JU" > /dev/null
sleep 0.3
want "a closed room: its member's sidebar reloads, not others'" "$(signals jasonA2)|$(signals kevinA2)" "sidebar,|"
act $DAVID create_open_room "name=Everyone" > /dev/null
sleep 0.3
want "an open room: every sidebar reloads" "$(signals jasonA2)|$(signals kevinA2)" "sidebar,sidebar,|sidebar,"
# a failed action signals nothing
act $KEVIN post_message "room=$B" "body=not a member" > /dev/null
sleep 0.3
want "a refused post signals no one" "$(signals jasonA2)" "sidebar,sidebar,"
# a long run of reads (each a batch that changes no page, with its read signal) mustn't leave
# new pages behind the signals' ring: the room made unread under the server each time (in
# SQLite, which read_room reads), then read by opening a stream
JM=$(q "select id from Membership where room = $A and user = $JU")
kill $HOLDS 2>/dev/null || true
q0=$(curl -sS $U/_burin/stats | sed 's/.*"quiet":\([0-9]*\).*/\1/')
for i in $(seq 1 270); do
  node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "update Membership set seen = 0 where id = $JM" 2>/dev/null
  curl -sSN -m 0.15 -H "Cookie: session_token=$JASON" "$U$(src $JASON $A)" > /dev/null || true
done
want "270 reads, each quiet" "$(( $(curl -sS $U/_burin/stats | sed 's/.*"quiet":\([0-9]*\).*/\1/') - q0 >= 270 ))" 1
curl -sSN -m 0.5 -H "Cookie: session_token=$KEVIN" "$U$(src $KEVIN $A)" > $D/after.txt || true
want "a page after them doesn't start with missed" "$(grep -c 'action="missed"' $D/after.txt)" 0
want "no stream was sent missed or refresh" "$(curl -sS $U/_burin/stats | grep -oE '"missed":[0-9]+|"refreshed":[0-9]+' | tr '\n' ' ')" '"missed":0 "refreshed":0 '
curl -sS $U/_burin/stats | grep -oE '"mismatched":[0-9]+|"live":[0-9]+' | tr '\n' ' '; echo
grep -iE "panic" $D/server.log && fail=1
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
