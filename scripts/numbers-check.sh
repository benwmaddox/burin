#!/bin/bash
# Numbers and time, end to end (apps/numbers): every value a page computes in its fragment's code
# must equal what an action computed in SQL and stored, and what Python computes here, for
# planets made to hit the edges: a divisor of 0, none, negatives, and overflow at 64 bits.
#   APP=numbers CRATE=numbers-app bash scripts/build-burin.sh release
#   bash /mnt/d/code/burin/scripts/numbers-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-numbers-release}
D=/tmp/burin-numbers; rm -rf $D; mkdir -p $D/data
PORT=13799
"$BIN" --data $D/data --port $PORT --workers 1 --exit-after 1 > $D/first.log 2>&1 || true
python3 - <<'EOF'
import sqlite3, time
db = sqlite3.connect('/tmp/burin-numbers/data/app.sqlite')
now = int(time.time() * 1000) - 3_600_000
seq = 0
def row(table, cols):
    global seq, now
    seq += 1; now += 1000
    keys = ['seq', 'created', 'updated'] + list(cols)
    db.execute(f'INSERT INTO "{table}" ({", ".join(chr(34)+k+chr(34) for k in keys)}) VALUES ({", ".join("?" for _ in keys)})', [seq, now, now] + list(cols.values()))
    return db.execute('SELECT last_insert_rowid()').fetchone()[0]
u = row('User', {'name': 'Ada'})
row('Session', {'user': u, 'token': 'ada-token'})
t = 1772304240000
for name, metal, rate, level, bonus, took in [
    ('plain', 12345, 37, 4, 5, 5_400_000),
    ('zero level', -7, 2, 0, None, 59_999),
    ('overflow', 2**62, 4, -3, -1, 3 * 86_400_000 + 1),
    ('negatives', -17, -5, 5, 0, -90_000),
    ('edge', 2**63 - 1, -1, 1, None, 0),
]:
    row('Planet', {'owner': u, 'name': name, 'metal': metal, 'rate': rate, 'level': level, 'bonus': bonus, 'started': t, 'took': took})
db.commit()
EOF
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
get() { curl -sS -o $D/$2.html -w "%{http_code}" -H "Cookie: sid=ada-token" http://127.0.0.1:$PORT/planets/$1; }
act() { curl -sS -o /dev/null -D - -H "Cookie: sid=ada-token" --data-urlencode "p=$1" http://127.0.0.1:$PORT/_do/$2 | grep -i x-burin-outcome | tr -d '\r' | sed 's/.*: //'; }
for p in 1 2 3 4 5; do
  echo "planet $p: read $(get $p cold$p), snapshot $(act $p snapshot), read $(sleep 0.2; get $p snap$p) $(get $p fast$p)"
done
for p in 1 2 3 4 5; do
  echo "$(act $p probe_a) $(act $p probe_b) $(act $p probe_c)" > $D/probes$p
done
for p in 1 2; do
  echo "planet $p: build $(act $p build), read $(sleep 0.2; get $p built$p)"
done
echo "planet 1: requeue $(act 1 requeue), read $(sleep 0.2; get 1 requeued1)"
grep -iE 'mismatch|panic|error' $D/server.log | head -5 || true
python3 - <<'EOF'
import re, datetime
D = '/tmp/burin-numbers/'
MIN, MAX = -2**63, 2**63 - 1
def fit(x):
    # none for none in, a divisor of 0, or outside 64 bits (none is i64::MIN itself)
    return None if x is None or not (MIN < x <= MAX) else x
def op(f, *xs):
    if any(x is None for x in xs):
        return None
    try:
        return fit(f(*xs))
    except ZeroDivisionError:
        return None
def tdiv(a, b):
    q = abs(a) // abs(b)
    return q if (a < 0) == (b < 0) else -q
def tmod(a, b):
    return a - b * tdiv(a, b)
def span(ms):
    if ms is None: return ''
    sign = '-' if ms < 0 else ''
    left = abs(ms) // 1000
    parts = []
    for unit, size in (('d', 86400), ('h', 3600), ('m', 60), ('s', 1)):
        n, left = left // size, left % size
        if n > 0 or parts: parts.append(f'{n}{unit}')
        if len(parts) == 2: break
    if len(parts) == 2 and parts[1].startswith('0'): parts.pop()
    return sign + ' '.join(parts) if parts else '0s'
def iso(ms):
    if ms is None: return ''
    return datetime.datetime.fromtimestamp(ms / 1000, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%S.') + f'{ms % 1000:03d}Z'
def num(x): return '' if x is None else str(x)
def expect(metal, rate, level, bonus, started, took):
    neg = op(lambda a: 0 - a, metal)
    return {
        'prod': num(op(lambda a, b: a * b, metal, rate)),
        'per': num(op(tdiv, metal, level)),
        'rem': num(op(tmod, metal, level)),
        'neg': num(neg),
        'bonus1': num(op(lambda a: a + 1, bonus)),
        'lo': num(min(metal, rate)), 'hi': num(max(metal, rate)),
        'cl': num(max(0, min(metal, 1000))),
        'ab': num(op(abs, neg)),
        # (metal * 2 + rate) - (metal / 3 % 7), each step in 64 bits
        'mix': num(op(lambda a, b: a - b, op(lambda a, b: a + b, op(lambda a: a * 2, metal), rate), op(tmod, op(tdiv, metal, 3), 7))),
        'ends': iso(op(lambda a, b: a + b, started, took)),
        'half': span(op(tdiv, took, 2)),
        'mins': num(op(tdiv, took, 60_000)),
        'left': span(op(tmod, took, 3_600_000)),
        'back': iso(started - 2 * 86_400_000 + 90_000),
        'longest': span(op(lambda a: a * 3, max(took, 3_600_000))),
    }
planets = [(12345, 37, 4, 5, 5_400_000), (-7, 2, 0, None, 59_999), (2**62, 4, -3, -1, 3 * 86_400_000 + 1), (-17, -5, 5, 0, -90_000), (2**63 - 1, -1, 1, None, 0)]
bad = 0
for i, (metal, rate, level, bonus, took) in enumerate(planets, 1):
    want = expect(metal, rate, level, bonus, 1772304240000, took)
    for name in (f'snap{i}', f'fast{i}'):
        h = open(D + name + '.html').read()
        page = dict(re.findall(r'<li data-k="(\w+)">([^<]*)</li>', re.search(r'<ul id="page">(.*?)</ul>', h, re.S).group(1)))
        sql = dict(re.findall(r'<li data-k="(\w+)">([^<]*)</li>', re.search(r'<ul id="sql">(.*?)</ul>', h, re.S).group(1)))
        for k, w in want.items():
            if not (page.get(k) == sql.get(k) == w):
                bad += 1
                print(f'  MISMATCH planet {i} {name} {k}: page {page.get(k)!r} sql {sql.get(k)!r} want {w!r}')
    h = open(D + f'snap{i}.html').read()
    print(f'planet {i}:', ' '.join(f'{k}={v}' for k, v in re.findall(r'<li data-k="(\w+)">([^<]*)</li>', re.search(r'<ul id="page">(.*?)</ul>', h, re.S).group(1)) if k in ('prod', 'per', 'rem', 'mix', 'half', 'left')), '|', re.search(r'<p id="metal" title="([^"]*)">([^<]*)', h).groups())
# a build: its outcome as the page predicted, its time as the page's "next"
for i in (1, 2):
    before, after = open(D + f'snap{i}.html').read(), open(D + f'built{i}.html').read()
    can = re.search(r'<p id="metal"[^>]*>([^<]*)', before).group(1) == 'can build'
    builds = re.findall(r'<p class="build" data-level="(\d+)">([^<]*)</p>', after)
    nxt = re.search(r'<p id="next">([^<]*)', before).group(1)
    ok = (bool(builds) == can) and (not builds or builds[0][1] == nxt)
    bad += not ok
    print(f'build on planet {i}: page said {"can" if can else "cannot"}; builds {builds}; next was {nxt!r}:', 'ok' if ok else 'MISMATCH')
# an order on none is false in a page and in a require; `not` of it is true in both
for i in range(1, 6):
    flags = re.search(r'<p id="flags">([^<]*)</p>', open(D + f'snap{i}.html').read()).group(1)
    outcomes = open(D + f'probes{i}').read().split()
    want = ['ok' if f in flags else 'invalid' for f in 'abc']
    ok = outcomes == want
    bad += not ok
    print(f'planet {i}: page flags {flags!r}, requires {outcomes}:', 'ok' if ok else 'MISMATCH')
# requeue: each build again, ten levels up (arithmetic on a for each's row)
levels = re.findall(r'<p class="build" data-level="(\d+)">', open(D + 'requeued1.html').read())
ok = levels == ['15', '5']
bad += not ok
print('requeued builds on planet 1:', levels, 'ok' if ok else 'MISMATCH')
print('PASS' if bad == 0 else f'FAIL: {bad}')
EOF
