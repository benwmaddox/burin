#!/bin/bash
# A stream ends when its viewer leaves the room's audience: Jason makes a closed room with Kevin
# and David, both hold its stream, and Jason drops David. David's stream must close (and he can't
# reopen it); Kevin's stays open and still gets Jason's next post; David gets nothing after he left.
#   bash /mnt/d/code/burin/scripts/revoke-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-rv; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13807; U=http://127.0.0.1:$PORT
q() { node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "$1"; }
DAVID=$(q "select token from Session where id = 481019661"); DU=$(q "select user from Session where token = '$DAVID'")
JASON=jasontokenjasontokenjasontoken00
KU=$(q "select id from User where email = 'kevin@37signals.com'")
KEVIN=kevin${KU}tokentokentokentoken
node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "insert into Session (id, seq, created, updated, user, token) values (919999999, 1, 0, 0, $KU, '$KEVIN')"
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null; kill %2 %3 2>/dev/null || true" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null $U/up 2>/dev/null && break; sleep 0.05; done
fail=0
want() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got $2, want $3"; fail=1; fi; }
act() { local who=$1 name=$2; shift 2; local a=(); for f in "$@"; do a+=(--data-urlencode "$f"); done
  curl -sS -o /dev/null -D - -H "Cookie: session_token=$who" "${a[@]}" $U/_do/$name | tr -d '\r' | awk '/^[Ll]ocation/{print $2}'; }
src() { curl -sS -H "Cookie: session_token=$1" $U/rooms/$2 | grep -o '<turbo-stream-source src="[^"]*"' | sed 's/.*src="//; s/"$//; s/&amp;/\&/g'; }
loc=$(act $JASON create_closed_room "name=Revoked" "users=$KU" "users=$DU"); ROOM=${loc##*/rooms/}
want "the room has three members" "$(q "select count(*) from Membership where room = $ROOM")" 3
DSRC=$(src $DAVID $ROOM); KSRC=$(src $KEVIN $ROOM)
want "both pages have their streams" "$(echo "$DSRC$KSRC" | grep -o '/_burin/live' | wc -l)" 2
curl -sSN -H "Cookie: session_token=$DAVID" "$U$DSRC" > $D/david.txt &
DPID=$!
curl -sSN -H "Cookie: session_token=$KEVIN" "$U$KSRC" > $D/kevin.txt &
sleep 0.4
act $JASON post_message "room=$ROOM" "body=while all three" > /dev/null
sleep 0.3
act $JASON update_closed_room "room=$ROOM" "name=Revoked" "users=$KU" > /dev/null
want "David is out" "$(q "select count(*) from Membership where room = $ROOM and user = $DU")" 0
gone=no; for i in $(seq 1 30); do kill -0 $DPID 2>/dev/null || { gone=yes; break; }; sleep 0.1; done
want "David's stream closed" "$gone" yes
act $JASON post_message "room=$ROOM" "body=after david left" > /dev/null
sleep 0.4
want "both got the post before" "$(grep -c 'data: .*while all three' $D/david.txt) $(grep -c 'data: .*while all three' $D/kevin.txt)" "1 1"
want "Kevin's stream gets the post after" "$(grep -c 'data: .*after david left' $D/kevin.txt)" 1
want "David's doesn't" "$(grep -c 'after david left' $D/david.txt)" 0
want "David can't reopen it" "$(curl -sS -m 1 -o /dev/null -w '%{http_code}' -H "Cookie: session_token=$DAVID" "$U$DSRC" || true)" 404
curl -sS $U/_burin/stats | grep -oE '"mismatched":[0-9]+|"live":[0-9]+' | tr '\n' ' '; echo
grep -iE "panic" $D/server.log && fail=1
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
