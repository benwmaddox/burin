#!/bin/bash
# Serves a fresh copy of the golden data, fetches the room page as both golden viewers (cold, fast,
# gzip), and compares them byte for byte with burin's expected pages (data/golden-pages).
#   bash /mnt/d/code/burin/scripts/golden.sh [--verify]
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
G=/mnt/d/code/burin/data/golden-pages
OUT=/tmp/burin-golden; mkdir -p $OUT
PORT=13790
rm -rf /tmp/burin-gd && cp -a /mnt/d/code/burin/data/campfire-golden /tmp/burin-gd && rm -f /tmp/burin-gd/app.lock
TZ=America/New_York taskset -c 0-3 "$BIN" --data /tmp/burin-gd --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 "$@" > $OUT/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
TOKEN=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('/tmp/burin-gd/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
for who in david:$TOKEN jason:jasontokenjasontokenjasontoken00; do
  name=${who%%:*}; tok=${who#*:}
  # twice: the first is a cold render that asks the writer to precompute; the second is fast
  curl -sS -o $OUT/$name-1.html -H "Cookie: session_token=$tok" http://127.0.0.1:$PORT/rooms/486777696
  sleep 0.2
  curl -sS -D $OUT/$name.headers -o $OUT/$name-2.html -H "Cookie: session_token=$tok" http://127.0.0.1:$PORT/rooms/486777696
  # gzip, decoded by curl: must be the same page
  # the sidebar's frame, as the browser asks for it (once; it keeps it across visits)
  curl -sS -o $OUT/$name-sidebar.html -H "Cookie: session_token=$tok" -H "Turbo-Frame: user_sidebar" http://127.0.0.1:$PORT/users/me/sidebar
  curl -sS --compressed -D $OUT/$name-gz.headers -o $OUT/$name-3.html -H "Cookie: session_token=$tok" http://127.0.0.1:$PORT/rooms/486777696
done
curl -sS http://127.0.0.1:$PORT/_burin/stats; echo
cat $OUT/server.log
python3 - <<'EOF'
import re
G='/mnt/d/code/burin/data/golden-pages'; O='/tmp/burin-golden'
def norm(t):
    return t
ok = True
for name, g in [('david', 'room.html'), ('jason', 'room-jason.html'), ('david', 'sidebar.html'), ('jason', 'sidebar-jason.html')]:
    want = norm(open(f'{G}/{g}', encoding='utf-8').read())
    for k in (('sidebar',) if g.startswith('sidebar') else ('1', '2', '3')):
        got = open(f'{O}/{name}-{k}.html', encoding='utf-8').read()
        if got == want:
            print(f'{name}-{k}: identical ({len(got.encode())} bytes)')
        else:
            ok = False
            i = next((j for j in range(min(len(got), len(want))) if got[j] != want[j]), min(len(got), len(want)))
            print(f'{name}-{k}: DIFFERS at char {i} (got {len(got)}, want {len(want)})')
            print('   got: ', repr(got[max(0,i-100):i+120]))
            print('   want:', repr(want[max(0,i-100):i+120]))
print('ALL IDENTICAL' if ok else 'MISMATCH')
EOF
