#!/bin/bash
# A token-cookie app's (etch or crudspec) room page and /users/me/sidebar for one person on the
# mixed data: what each serves where Rails has its sidebar frame. Cores 0-3, not a measurement.
#   bash /mnt/d/code/etch/bench/mix/capture-token-app.sh <binary> <out dir>
set -euo pipefail
BIN=$1; O=$2; M=$HOME/etch-mix; D=/tmp/cap-data-$$
rm -rf $O $D; mkdir -p $O; cp -a $M/crudspec $D; rm -f $D/app.lock
taskset -c 0-3 $BIN --data $D --assets /mnt/d/code/etch/apps/campfire/assets --port 13788 > $O/server.log 2>&1 &
PID=$!; trap "kill $PID 2>/dev/null; rm -rf $D" EXIT
for i in $(seq 1 200); do curl -fsS -o /dev/null http://127.0.0.1:13788/up 2>/dev/null && break; sleep 0.05; done
read -r TOK ROOM < <(node -e 'const p=require(process.argv[1]); const x=p.people[0]; console.log(x.token, x.rooms[0])' $M/people.json)
curl -sS -o $O/room.html -w "room %{http_code} %{size_download}\n" -H "Cookie: session_token=$TOK" http://127.0.0.1:13788/rooms/$ROOM
curl -sS -o $O/sidebar.html -w "sidebar %{http_code} %{size_download}\n" -H "Cookie: session_token=$TOK" -H "Turbo-Frame: user_sidebar" http://127.0.0.1:13788/users/me/sidebar
