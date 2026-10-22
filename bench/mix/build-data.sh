#!/bin/bash
# The mixed benchmark's data, in WSL home (never in the repository): the Rails-format database
# (Rails, the Rust port, Flow), crudspec's import of it (crudspec, and etch's copy), and
# people.json: everyone who signs in, their session token, the rooms they're in, how active they
# are, and how popular each room is.
#   bash /mnt/d/code/etch/bench/mix/build-data.sh [seed]
set -euo pipefail
SEED=${1:-1}
M=$HOME/etch-mix; mkdir -p "$M"; chmod 700 "$M"
FIX=/mnt/d/code/AgentLang/.agentlang/once-campfire-verification/fixtures/default
# the bench password (synthetic; everyone added uses it), hashed with the system's bcrypt
PASS="etch mix bench"
DIGEST=$(python3 -W ignore -c "import crypt; print(crypt.crypt('$PASS', crypt.mksalt(crypt.METHOD_BLOWFISH, rounds=16)))")
rm -f "$M/rails.sqlite3"
node /mnt/d/code/etch/bench/mix/gen-rails.mjs "$FIX/db/production.sqlite3" "$M/rails.sqlite3" "$DIGEST" "$SEED"
# crudspec's import (it keeps every id and the session tokens); etch reads the same database
rm -rf "$M/crudspec" "$M/etch"
(cd /mnt/d/code/crudspec && node bench/import-rails.ts "$M/rails.sqlite3" "$M/crudspec" > /dev/null)
cp -a "$M/crudspec" "$M/etch"; rm -f "$M/etch/secret" "$M/etch/vapid.json"
python3 /mnt/d/code/etch/scripts/backfill-seen.py "$M/etch/app.sqlite"
node /mnt/d/code/etch/bench/mix/people.mjs "$M/rails.sqlite3" "$PASS" > "$M/people.json"
node -e "const p=require('$M/people.json'); console.log('people', p.people.length, 'rooms', Object.keys(p.rooms).length, 'memberships', p.people.reduce((s,x)=>s+x.rooms.length,0))"
