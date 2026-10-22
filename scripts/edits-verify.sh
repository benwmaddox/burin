#!/bin/bash
# Boosts, unboosts, edits and deletes, with --verify on: every fast page must equal a fresh render
# from SQLite; each refusal's outcome; and no write may clear the snapshot's screens ("cleared").
#   bash /mnt/d/code/etch/scripts/edits-verify.sh
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-gen-release}
D=/tmp/etch-ev; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/etch/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13796
q() { node /mnt/d/code/etch/scripts/q.js $D/data/app.sqlite "$1"; }
# a plain member, added to the room and signed in (the room has two administrators and a bot)
MU=$(q "select id from User where role = 'member' and status = 'active' order by id limit 1")
node /mnt/d/code/etch/scripts/x.js $D/data/app.sqlite "insert into Session (id, seq, created, updated, user, token) values (900000001, 1, 0, 0, $MU, 'membertokenmembertokenmember0000'); insert into Membership (id, seq, created, updated, room, user, involvement) values (900000002, 2, 0, 0, 486777696, $MU, 'everything')"
MEMBER=membertokenmembertokenmember0000
R=486777696
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
DAVID=$(q "select token from Session where id = 481019661")
DU=$(q "select user from Session where id = 481019661")
JASON=jasontokenjasontokenjasontoken00
JU=$(q "select user from Session where token = '$JASON'")
echo "david: $(q "select role from User where id = $DU"), jason: $(q "select role from User where id = $JU"), member $MU: $(q "select role from User where id = $MU")"
get() { if [ "$1" = "$JASON" ]; then Z=(-H "Accept-Encoding: gzip"); else Z=(); fi; curl -sS -o $D/$1.html -w "%{http_code} " "${Z[@]}" -H "Cookie: session_token=$1" http://127.0.0.1:$PORT/rooms/$R; }
both() { get $DAVID; get $JASON; get $MEMBER; sleep 0.05; get $DAVID; get $JASON; get $MEMBER; }
PAGE() { echo $D/$DAVID.html; }
# an action by a viewer with its fields; prints the outcome
act() { local who=$1 name=$2; shift 2; local args=(); for f in "$@"; do args+=(--data-urlencode "$f"); done
  curl -sS -o /dev/null -D - -H "Cookie: session_token=$who" -H "Referer: http://127.0.0.1:$PORT/rooms/$R" "${args[@]}" http://127.0.0.1:$PORT/_do/$name | tr -d '\r' | awk '/^[Xx]-[Ee]tch-[Oo]utcome/{o=$2} /^[Ll]ocation/{l=$2} END{printf "%s -> %s", o, l}'; }
both; sleep 0.3; echo
LAST=$(q "select id from Message where room = $R order by created desc, seq desc limit 1")
MINE_J=$(q "select id from Message where room = $R and creator = $JU order by created desc limit 1")
OTHER=$(q "select id from Message where room = $R and creator != $JU order by created desc limit 1")
NOTIN=$(q "select m.id from Message m where m.room not in (select room from Membership where user = $JU) limit 1")
echo "messages: last $LAST, jason's $MINE_J, not jason's $OTHER, outside jason's rooms ${NOTIN:-none}"

echo "boost (quick form, __arg_ fields):  $(act $JASON boost "__arg_message=#$LAST" "__arg_content=👍")"; both; echo
echo "boost (custom form):                $(act $DAVID boost "__arg_message=#$LAST" "content=nice <b>")"; both; echo
echo "boost, blank:                       $(act $DAVID boost "__arg_message=#$LAST" "content=  ")"
echo "boost, 17 characters:               $(act $DAVID boost "__arg_message=#$LAST" "content=12345678901234567")"
echo "boost, 16 emoji (64 bytes):         $(act $DAVID boost "__arg_message=#$LAST" "content=🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥")"; both; echo
[ -n "$NOTIN" ] && echo "boost, a room jason isn't in:       $(act $JASON boost "__arg_message=#$NOTIN" "content=x")"
JB=$(q "select id from Boost where booster = $JU order by id desc limit 1")
DB=$(q "select id from Boost where booster = $DU order by id desc limit 1")
echo "unboost david's, as jason:          $(act $JASON unboost "__arg_boost=#$DB")"
echo "unboost jason's own:                $(act $JASON unboost "__arg_boost=#$JB")"; both; echo
echo "edit, jason's own:                  $(act $JASON edit_message "__arg_message=#$MINE_J" "body=edited by jason & <i>checked</i>")"; both; echo
grep -c "edited by jason &amp; &lt;i&gt;checked" $(PAGE) >/dev/null && echo "  (the page shows the edit)" || echo "  THE PAGE DOESN'T SHOW THE EDIT"
echo "edit someone else's, as jason:      $(act $JASON edit_message "__arg_message=#$OTHER" "body=hijack")"
echo "edit, blank:                        $(act $JASON edit_message "__arg_message=#$MINE_J" "body= ")"
echo "boost then delete, jason's own:     $(act $DAVID boost "__arg_message=#$MINE_J" "content=🎉") / $(act $JASON delete_message "__arg_message=#$MINE_J")"; both; echo
echo "  boosts left on it: $(q "select count(*) from Boost where message = $MINE_J"), the message: $(q "select count(*) from Message where id = $MINE_J")"
grep -c "message_.*data-message-id=\"$MINE_J\"" $(PAGE) >/dev/null && echo "  THE PAGE STILL SHOWS IT" || echo "  (the page no longer shows it)"
echo "messages on the page: $(grep -o 'data-message-id=' $(PAGE) | wc -l)"
echo "edit someone else's, as a member:   $(act $MEMBER edit_message "__arg_message=#$OTHER" "body=hijack")"
echo "delete someone else's, as a member: $(act $MEMBER delete_message "__arg_message=#$OTHER")"
echo "boost, as a member:                 $(act $MEMBER boost "__arg_message=#$OTHER" "content=👋")"
echo "delete someone else's, as jason:    $(act $JASON delete_message "__arg_message=#$OTHER")"
sleep 0.3; both; echo
curl -sS http://127.0.0.1:$PORT/_etch/stats; echo
grep -v "^started" $D/server.log | head -20 || true
