#!/bin/bash
# The Rust port's optimized variant against itself with its caches off (rustopt-verify.mjs), on a
# copy of the mixed data: two servers on one database, cores 0-7 (not a measurement).
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/rustopt-verify.sh [steps] [seed] [binary]
set -euo pipefail
STEPS=${1:-200}; SEED=${2:-1}; BIN=${3:-/home/ben/linuxbench/rust-opt/bin/campfire}
U=ben; UH=/home/$U; M=$UH/burin-mix
FIX=/mnt/d/code/AgentLang/.agentlang/once-campfire-verification/fixtures/default
D=/tmp/rustopt-verify; rm -rf $D; mkdir -p $D/w/storage/db $D/w/storage/logs
cp "$M/rails.sqlite3" $D/w/storage/db/production.sqlite3; cp -r "$FIX/storage" $D/w/storage/files; chown -R $U $D
ENVV=(HOME=$UH TZ=America/New_York LANG=C.UTF-8 USER=$U RAILS_ENV=production PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin)
while IFS= read -r line || [ -n "$line" ]; do case "$line" in ''|'#'*) continue;; esac; ENVV+=("$line"); done < "$FIX/reference.env"
start() { # port extra-env... : one server on the shared storage
  local port=$1; shift
  ( cd $D/w; exec taskset -c 0-5 setpriv --reuid=$U --regid=$U --init-groups env -i "${ENVV[@]}" HTTP_PORT=$port TARGET_PORT=$((port+1)) "$@" "$BIN" server ) >> $D/server-$port.log 2>&1 &
  for i in $(seq 1 600); do curl -fsS -o /dev/null http://127.0.0.1:$port/up 2>/dev/null && return; sleep 0.1; done
  echo "server $port didn't start"; tail -5 $D/server-$port.log; exit 3
}
trap "pkill -u $U -f '$BIN server' 2>/dev/null || true" EXIT
# the uncached one first (its boot's writes happen before the cached one starts)
start 13802 CAMPFIRE_RESPONSE_CACHE_MB=0 CAMPFIRE_FRAGMENT_CACHE_MB=0
start 13800
node /mnt/d/code/burin/bench/mix/rails-cookie.mjs make "$FIX/reference.env" "$M/people.json" $D/w/storage/db/production.sqlite3 sha256 1000 64 sha1 2>/dev/null > $D/signed.json
node -e 'const s=require(process.argv[1]); const o={}; for (const [id, v] of Object.entries(s)) o[id]={cookie: v.cookie, csrf: ""}; console.log(JSON.stringify(o))' $D/signed.json > $D/cookies.json
chmod 600 $D/signed.json $D/cookies.json
taskset -c 6-7 node /mnt/d/code/burin/bench/mix/rustopt-verify.mjs http://127.0.0.1:13800 http://127.0.0.1:13802 $D/cookies.json $M/people.json $D/w/storage/db/production.sqlite3 "$STEPS" "$SEED"
