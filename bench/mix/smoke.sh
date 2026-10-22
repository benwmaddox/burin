#!/bin/bash
# A quick check of the mixed load against etch or crudspec on the mixed data (cores 0-7, not a
# measurement): every read valid, the posts accepted, and etch's fast pages equal fresh builds.
#   bash /mnt/d/code/etch/bench/mix/smoke.sh <etch|crudspec> [binary]
set -euo pipefail
APP=$1
BIN=${2:-$([ "$APP" = etch ] && echo $HOME/hw/etch-gen-release || echo $HOME/hw/campfire-pk)}
M=$HOME/etch-mix; D=/tmp/etch-mix-smoke; rm -rf $D; mkdir -p $D; cp -a "$M/$APP" $D/data; rm -f $D/data/app.lock
PORT=13787
EXTRA=(); [ "$APP" = etch ] && EXTRA=(--verify --workers 2)
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT "${EXTRA[@]}" > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 200); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
taskset -c 4-7 $HOME/etch-target-mixload/release/mixload --base http://127.0.0.1:$PORT --people $M/people.json --app $APP --conc 8 --duration 5 --post-rate 10 --check-every 1
[ "$APP" = etch ] && curl -sS http://127.0.0.1:$PORT/_etch/stats && echo
grep -vE "^started|^slow batch" $D/server.log | head -5 || true
