#!/bin/bash
# The Rust port's (Rails-faithful) room page and sidebar frame for one person on the mixed data:
# the shape burin's pages copy. Cores 0-7, not a measurement. Writes /tmp/rust-shape/.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/capture-rust.sh
set -euo pipefail
U=ben; UH=/home/$U; M=$UH/burin-mix; L=$UH/linuxbench
FIX=/mnt/d/code/AgentLang/.agentlang/once-campfire-verification/fixtures/default
O=/tmp/rust-shape; W=/tmp/rust-shape-w; rm -rf $O $W; mkdir -p $O $W/storage/db $W/storage/logs
cp "$M/rails.sqlite3" $W/storage/db/production.sqlite3; cp -r "$FIX/storage" $W/storage/files; chown -R $U $W
ENVV=(HOME=$UH TZ=America/New_York LANG=C.UTF-8 USER=$U RAILS_ENV=production PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin)
while IFS= read -r line || [ -n "$line" ]; do case "$line" in ''|'#'*) continue;; esac; ENVV+=("$line"); done < "$FIX/reference.env"
ENVV+=(HTTP_PORT=13784 TARGET_PORT=13785)
( cd $W; exec taskset -c 0-3 setpriv --reuid=$U --regid=$U --init-groups env -i "${ENVV[@]}" $L/rust/bin/campfire server ) > $W/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null; pkill -u $U -f 'campfire server' 2>/dev/null || true" EXIT
for i in $(seq 1 600); do curl -fsS -o /dev/null http://127.0.0.1:13784/up 2>/dev/null && break; sleep 0.1; done
node /mnt/d/code/burin/bench/mix/rails-cookie.mjs make "$FIX/reference.env" "$M/people.json" $W/storage/db/production.sqlite3 sha256 1000 64 sha1 2>/dev/null > $W/signed.json
read -r ID ROOM < <(node -e 'const p=require(process.argv[1]); const x=p.people[0]; console.log(x.id, x.rooms[0])' "$M/people.json")
C=$(node -e 'const s=require(process.argv[1]); process.stdout.write(s[process.argv[2]].cookie)' $W/signed.json "$ID")
curl -sS -D $O/room.headers -o $O/room.html -H "Cookie: $C" http://127.0.0.1:13784/rooms/$ROOM
curl -sS -D $O/sidebar.headers -o $O/sidebar.html -H "Cookie: $C" -H "Turbo-Frame: user_sidebar" http://127.0.0.1:13784/users/me/sidebar
curl -sS -o $O/sidebar-noframe.html -H "Cookie: $C" http://127.0.0.1:13784/users/me/sidebar
grep -viE 'set-cookie' $O/room.headers > $O/room.h; mv $O/room.h $O/room.headers
grep -viE 'set-cookie' $O/sidebar.headers > $O/s.h; mv $O/s.h $O/sidebar.headers
wc -c $O/*.html
