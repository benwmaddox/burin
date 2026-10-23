#!/bin/bash
# Random actions by every active person (each given a session), with --verify and the scrubber on;
# passes when no fast page differed from a fresh build, no write cleared the screens and the
# scrubber found nothing to fix.
#   bash /mnt/d/code/burin/scripts/fuzz.sh [steps] [seed]
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
STEPS=${1:-400}; SEED=${2:-1}
D=/tmp/burin-fz; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13789
# a session for each active person
node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "insert into Session (id, seq, created, updated, user, token) select 910000000 + u.id % 1000000, 1, 0, 0, u.id, 'fuzz' || u.id || 'tokentokentokentoken' from User u where u.status = 'active' and not exists (select 1 from Session s where s.user = u.id)"
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify --scrub-ms 10 > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
node /mnt/d/code/burin/scripts/fuzz.mjs http://127.0.0.1:$PORT $D/data/app.sqlite $STEPS $SEED
# the scrubber's turn over every kept page
sleep 3
S=$(curl -sS http://127.0.0.1:$PORT/_burin/stats); echo "$S"
grep -E "^(verify|scrub)" $D/server.log | head -8 || true
node -e "const s=$S; const bad=s.mismatched+s.cleared+s.scrub_fixed; console.log(bad ? 'FAIL: mismatched '+s.mismatched+', cleared '+s.cleared+', scrub_fixed '+s.scrub_fixed : 'PASS ('+s.verified+' fast pages verified, '+s.scrubbed+' scrubbed)'); process.exit(bad ? 1 : 0)"
