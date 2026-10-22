#!/bin/bash
# The generated action's refusals: a blank body is invalid; a room the viewer isn't in is not_found;
# a good post is ok. Prints each outcome header.
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-gen-release}
D=/tmp/etch-oc; rm -rf $D; mkdir -p $D; cp -a ~/hw/golden-data $D/data; rm -f $D/data/app.lock
PORT=13797
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/crudspec-assets/campfire --port $PORT --workers 2 > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
JASON=jasontokenjasontokenjasontoken00
NOTIN=$(node -e "const {DatabaseSync}=require('node:sqlite');const db=new DatabaseSync('$D/data/app.sqlite');console.log(db.prepare('select id from Room where id not in (select room from Membership where user = 149087659) limit 1').get()?.id ?? 1)" 2>/dev/null)
post() { curl -sS -o /dev/null -D - -H "Cookie: session_token=$JASON" --data-urlencode "room=$1" --data-urlencode "body=$2" http://127.0.0.1:$PORT/_do/post_message | grep -i outcome | tr -d '\r'; }
echo "blank body:        $(post 486777696 '   ')"
echo "room not a member: $(post $NOTIN 'hello')   (room $NOTIN)"
echo "good post:         $(post 486777696 'hello')"
