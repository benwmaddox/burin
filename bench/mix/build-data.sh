#!/bin/bash
# The mixed benchmark's data, in WSL home (never in the repository): the Rails-format database
# (Rails, the Rust port, Flow), crudspec's import of it (crudspec, and burin's copy), and
# people.json: everyone who signs in, their session token, the rooms they're in, how active they
# are, and how popular each room is.
#   bash /mnt/d/code/burin/bench/mix/build-data.sh [seed]
set -euo pipefail
SEED=${1:-1}
M=$HOME/burin-mix; mkdir -p "$M"; chmod 700 "$M"
FIX=/mnt/d/code/AgentLang/.agentlang/once-campfire-verification/fixtures/default
# the bench password (synthetic; everyone added uses it), hashed with the system's bcrypt
PASS="burin mix bench"
DIGEST=$(python3 -W ignore -c "import crypt; print(crypt.crypt('$PASS', crypt.mksalt(crypt.METHOD_BLOWFISH, rounds=16)))")
rm -f "$M/rails.sqlite3"
node /mnt/d/code/burin/bench/mix/gen-rails.mjs "$FIX/db/production.sqlite3" "$M/rails.sqlite3" "$DIGEST" "$SEED"
# crudspec's import (it keeps every id and the session tokens); burin reads the same database
rm -rf "$M/crudspec" "$M/burin"
(cd /mnt/d/code/crudspec && node bench/import-rails.ts "$M/rails.sqlite3" "$M/crudspec" > /dev/null)
cp -a "$M/crudspec" "$M/burin"; rm -f "$M/burin/secret" "$M/burin/vapid.json"
python3 /mnt/d/code/burin/scripts/backfill-seen.py "$M/burin/app.sqlite"
node /mnt/d/code/burin/bench/mix/people.mjs "$M/rails.sqlite3" "$PASS" > "$M/people.json"
node -e "const p=require('$M/people.json'); console.log('people', p.people.length, 'rooms', Object.keys(p.rooms).length, 'memberships', p.people.reduce((s,x)=>s+x.rooms.length,0))"
