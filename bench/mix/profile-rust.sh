#!/bin/bash
# Where a Rust port build spends its time under the mixed browser-model load: the port on the
# mixed data (cores 0-3), mixload (cores 4-7) warming then measuring with perf on the server.
# Not a measurement (use drive.sh for that).
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/profile-rust.sh [binary] [secs] [posts/s]
set -euo pipefail
BIN=${1:-/home/ben/linuxbench/rust/bin/campfire}; SECS=${2:-8}; RATE=${3:-10}
U=ben; UH=/home/$U; M=$UH/burin-mix
FIX=/mnt/d/code/AgentLang/.agentlang/once-campfire-verification/fixtures/default
D=/tmp/rust-prof; rm -rf $D; mkdir -p $D/w/storage/db $D/w/storage/logs
cp "$M/rails.sqlite3" $D/w/storage/db/production.sqlite3; cp -r "$FIX/storage" $D/w/storage/files; chown -R $U $D
ENVV=(HOME=$UH TZ=America/New_York LANG=C.UTF-8 USER=$U RAILS_ENV=production PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin)
while IFS= read -r line || [ -n "$line" ]; do case "$line" in ''|'#'*) continue;; esac; ENVV+=("$line"); done < "$FIX/reference.env"
ENVV+=(HTTP_PORT=13787 TARGET_PORT=13789 ${EXTRA_ENV:-})
( cd $D/w; exec taskset -c 0-3 setpriv --reuid=$U --regid=$U --init-groups env -i "${ENVV[@]}" "$BIN" server ) > $D/server.log 2>&1 &
trap "pkill -u $U -f '$BIN server' 2>/dev/null || true" EXIT
for i in $(seq 1 600); do curl -fsS -o /dev/null http://127.0.0.1:13787/up 2>/dev/null && break; sleep 0.1; done
PID=$(pgrep -u $U -f "$BIN server" | head -1)
# everyone's session cookie, signed as Rails signs it
node /mnt/d/code/burin/bench/mix/rails-cookie.mjs make "$FIX/reference.env" "$M/people.json" $D/w/storage/db/production.sqlite3 sha256 1000 64 sha1 2>/dev/null > $D/signed.json
node -e 'const s=require(process.argv[1]); const o={}; for (const [id, v] of Object.entries(s)) o[id]={cookie: v.cookie, csrf: ""}; console.log(JSON.stringify(o))' $D/signed.json > $D/cookies.json
chmod 600 $D/signed.json $D/cookies.json
ML=$UH/burin-target-mixload/release/mixload
taskset -c 4-7 $ML --base http://127.0.0.1:13787 --people $M/people.json --cookies $D/cookies.json --app rust --conc 16 --duration 4 > /dev/null
P=$(ls /usr/lib/linux-tools-*/perf | head -1)
$P record -q -g -p $PID -o $D/perf.data -- sleep $SECS > /dev/null 2>&1 &
PERF=$!
taskset -c 4-7 $ML --base http://127.0.0.1:13787 --people $M/people.json --cookies $D/cookies.json --app rust --conc 16 --duration $SECS --post-rate $RATE | head -c 900; echo
wait $PERF || true
$P report -i $D/perf.data --no-children --sort symbol --stdio 2>/dev/null | grep -E '^ +[0-9]' | head -40 | cut -c1-150
