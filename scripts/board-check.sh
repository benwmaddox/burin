#!/bin/bash
# The second app, end to end: etch creates its schema in an empty folder; a seed adds people,
# sessions, boards, memberships and posts; with --verify, two viewers read, post, and read again.
#   bash /mnt/d/code/etch/scripts/board-check.sh
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-board-release}
D=/tmp/etch-board; rm -rf $D; mkdir -p $D/data
PORT=13798
# the schema, made by the app on its first start
"$BIN" --data $D/data --port $PORT --workers 1 --exit-after 1 > $D/first.log 2>&1 || true
python3 - <<'EOF'
import sqlite3, time
db = sqlite3.connect('/tmp/etch-board/data/app.sqlite')
now = int(time.time() * 1000) - 3_600_000
seq = 0
def row(table, cols):
    global seq, now
    seq += 1; now += 1000
    keys = ['seq', 'created', 'updated'] + list(cols)
    db.execute(f'INSERT INTO "{table}" ({", ".join(chr(34)+k+chr(34) for k in keys)}) VALUES ({", ".join("?" for _ in keys)})', [seq, now, now] + list(cols.values()))
    return db.execute('SELECT last_insert_rowid()').fetchone()[0]
ada = row('User', {'name': 'Ada <admin>'}); bob = row('User', {'name': 'Bob'}); cy = row('User', {'name': 'Cy'})
row('Session', {'user': ada, 'token': 'ada-token'}); row('Session', {'user': bob, 'token': 'bob-token'})
b1 = row('Board', {'title': 'General & "news"'}); b2 = row('Board', {'title': 'Ada only'})
for u in (ada, bob, cy):
    row('Member', {'board': b1, 'user': u})
row('Member', {'board': b2, 'user': ada})
for i in range(25):
    row('Post', {'board': b1, 'author': (ada, bob, cy)[i % 3], 'text': f'post {i} <b>bold?</b> & more'})
row('Post', {'board': b2, 'author': ada, 'text': 'private note'})
db.commit()
print('seeded: boards', b1, b2)
EOF
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
get() { curl -sS -o $D/$3.html -w "%{http_code} " -H "Cookie: sid=$1" $4 http://127.0.0.1:$PORT/boards/$2; }
post() { curl -sS -o /dev/null -D - -H "Cookie: sid=$1" --data-urlencode "board=$2" --data-urlencode "text=$3" http://127.0.0.1:$PORT/_do/add_post | grep -i x-etch-outcome | tr -d '\r'; }
echo "reads (cold, then fast): $(get ada-token 1 a1) $(get bob-token 1 b1) $(sleep 0.3; get ada-token 1 a2) $(get bob-token 1 b2) $(get bob-token 1 b3 '--compressed')"
echo "bob on Ada's board: $(get bob-token 2 x) (a redirect)"
echo "post by bob: $(post bob-token 1 'hello from bob <script>x</script>')"
echo "blank post: $(post bob-token 1 '   ')"
echo "post on a board bob isn't in: $(post bob-token 2 'sneaky')"
sleep 0.3
echo "reads after: $(get ada-token 1 a4) $(get bob-token 1 b4) $(get bob-token 1 b5 '--compressed')"
curl -sS http://127.0.0.1:$PORT/_etch/stats; echo
grep -v '^started' $D/server.log | head -5 || true
python3 - <<'EOF'
import re
D = '/tmp/etch-board/'
a, b = open(D + 'a4.html').read(), open(D + 'b4.html').read()
posts = re.findall(r'<li class="post" id="post-(\d+)" data-author-id="(\d+)"><b>([^<]*)</b> <span>([^<]*)</span>', b)
me = re.search(r'<aside id="me" data-user-id="(\d+)"', b).group(1)
print('posts shown to bob:', len(posts), '| first:', (posts[0][0],) + posts[0][2:], '| last:', (posts[-1][0],) + posts[-1][2:])
# "mine" is marked by the page's script, from the post's author and the viewer's id
print('bob's own posts the script marks mine:', sum(1 for p in posts if p[1] == me), 'of', sum(1 for p in posts if p[2] == 'Bob'))
print('newest first:', [int(p[0]) for p in posts] == sorted([int(p[0]) for p in posts], reverse=True))
print("bob's panel:", re.search(r'Signed in as <b>([^<]*)</b>', b).group(1), '|', len(re.findall(r'<ul class="mine">(.*?)</ul>', b)[0].split('<li>')) - 1, 'of his posts')
print("ada's panel:", re.search(r'Signed in as <b>([^<]*)</b>', a).group(1))
print('escaped:', '&lt;script&gt;' in b and '<script>x' not in b)
EOF
# audiences kept current: bob joins Ada's board (then served fast), leaves the first (then refused)
act() { curl -sS -o /dev/null -D - -H "Cookie: sid=$1" --data-urlencode "board=$2" http://127.0.0.1:$PORT/_do/$3 | grep -i x-etch-outcome | tr -d '\r'; }
fast() { curl -sS http://127.0.0.1:$PORT/_etch/stats | sed 's/.*"fast":\([0-9]*\).*/\1/'; }
echo "bob joins board 2: $(act bob-token 2 join); joins again: $(act bob-token 2 join)"
sleep 0.2; f0=$(fast)
echo "bob reads board 2: $(get bob-token 2 j1) $(get bob-token 2 j2 --compressed) (fast: $(( $(fast) - f0 )) of 2); ada: $(get ada-token 2 j3)"
grep -q 'Signed in as <b>Bob' $D/j1.html && echo "  bob sees board 2 as himself"
echo "bob leaves board 1: $(act bob-token 1 leave); leaves again: $(act bob-token 1 leave)"
sleep 0.2; f0=$(fast)
echo "bob reads board 1: $(get bob-token 1 l1) (fast: $(( $(fast) - f0 ))); ada: $(get ada-token 1 l2) $(get cy-token 1 l3 2>/dev/null || true)"
curl -sS http://127.0.0.1:$PORT/_etch/stats; echo
