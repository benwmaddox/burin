#!/bin/bash
# Helpers (apps/crunch), end to end: CRC-32 written in burin (a loop, a stack table, bit
# operations), called by a cached fragment and by an action, must equal zlib's on every text,
# including empty, multi-byte and long ones.
#   APP=crunch CRATE=crunch-app bash scripts/build-burin.sh release
#   bash /mnt/d/code/burin/scripts/crunch-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-crunch-release}
D=/tmp/burin-crunch; rm -rf $D; mkdir -p $D/data
PORT=13810; U=http://127.0.0.1:$PORT
"$BIN" --data $D/data --port $PORT --workers 1 --exit-after 1 > $D/first.log 2>&1 || true
python3 - <<'EOF'
import sqlite3, time
db = sqlite3.connect('/tmp/burin-crunch/data/app.sqlite')
now = int(time.time() * 1000)
def row(table, cols, seq=[0]):
    seq[0] += 1
    keys = ['seq', 'created', 'updated'] + list(cols)
    db.execute(f'INSERT INTO "{table}" ({", ".join(chr(34)+k+chr(34) for k in keys)}) VALUES ({", ".join("?" for _ in keys)})', [seq[0], now, now] + list(cols.values()))
    return db.execute('SELECT last_insert_rowid()').fetchone()[0]
u = row('User', {'name': 'Ada'})
row('Session', {'user': u, 'token': 'ada-token'})
row('Grid', {'owner': u, 'seed': 7})
db.commit()
EOF
"$BIN" --data $D/data --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null $U/up 2>/dev/null && break; sleep 0.05; done
G=$(python3 -c "import sqlite3; print(sqlite3.connect('$D/data/app.sqlite').execute('select id from Grid').fetchone()[0])")
python3 - "$U" "$G" <<'EOF'
import sys, urllib.request, urllib.parse, zlib, re, sqlite3
u, g = sys.argv[1], sys.argv[2]
texts = ["", "a", "hello, world", "héllo wörld ✓ 漢字 🎉", "x" * 50_000 + "y", "The quick brown fox jumps over the lazy dog"]
for t in texts:
    req = urllib.request.Request(f"{u}/_do/add_note", data=urllib.parse.urlencode({"g": g, "body": t}).encode(), headers={"Cookie": "sid=ada-token"})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a): return None
    try:
        urllib.request.build_opener(NoRedirect).open(req)
    except urllib.error.HTTPError as e:
        if e.code not in (302, 303):
            raise
page = urllib.request.urlopen(urllib.request.Request(f"{u}/notes/{g}", headers={"Cookie": "sid=ada-token"})).read().decode()
rows = re.findall(r'<td class="page">([^<]*)</td><td class="stored">([^<]*)</td>', page)
db = sqlite3.connect('/tmp/burin-crunch/data/app.sqlite')
bodies = [r[0] for r in db.execute('select body from Note order by seq')]
ok = len(rows) == len(texts) and bodies == texts
for (page_crc, stored), t in zip(rows, texts):
    want = zlib.crc32(t.encode())
    good = page_crc == str(want) and stored == str(want)
    ok &= good
    print(f"{'ok  ' if good else 'FAIL'} {len(t.encode()):>6} bytes: page {page_crc}, action {stored}, zlib {want}")
print("PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
EOF
curl -sS $U/_burin/stats | grep -oE '"mismatched":[0-9]+' ; grep -iE "panic" $D/server.log && exit 1 || true
