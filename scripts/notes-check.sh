#!/bin/bash
# External functions end to end (apps/notes, with C and Rust in apps/notes/ext): notes added by an
# action (slug and word count from C, a serial from a Rust effect, stored), then the page (C and
# Rust in its cached and per-viewer parts), every value checked against Python's, with --verify.
#   APP=notes CRATE=notes-app bash scripts/build-etch.sh release
#   bash /mnt/d/code/etch/scripts/notes-check.sh
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-notes-release}
D=/tmp/etch-notes; rm -rf $D; mkdir -p $D/data
PORT=13803; U=http://127.0.0.1:$PORT
"$BIN" --data $D/data --port $PORT --workers 1 --exit-after 1 > $D/first.log 2>&1 || true
python3 - <<'EOF'
import sqlite3, time
db = sqlite3.connect('/tmp/etch-notes/data/app.sqlite')
now = int(time.time() * 1000)
db.execute('INSERT INTO "User" (id, seq, created, updated, name) VALUES (1, 1, ?, ?, ?)', (now, now, 'Ada Lovelace'))
db.execute('INSERT INTO "Session" (id, seq, created, updated, user, token) VALUES (1, 2, ?, ?, 1, ?)', (now, now, 'ada-token'))
db.commit()
EOF
"$BIN" --data $D/data --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null $U/up 2>/dev/null && break; sleep 0.05; done
post() { curl -sS -o /dev/null -w '%{http_code} ' -H "Cookie: sid=ada-token" --data-urlencode "title=$1" --data-urlencode "body=$2" $U/_do/add_note; }
echo "posts: $(post 'Hello, World! <b>bold</b>' 'one two three') $(post 'Ünïcode & spaces   here' 'a b c d e f g h i j k l m n o p q r s t u v w x y z extra words for a long note body')"
sleep 0.3
curl -sS -H "Cookie: sid=ada-token" $U/users/1/notes > $D/page.html
sleep 0.2
curl -sS -H "Cookie: sid=ada-token" $U/users/1/notes > $D/page2.html
python3 - <<'EOF'
import html, re, sys
page = open('/tmp/etch-notes/page.html').read()
assert page == open('/tmp/etch-notes/page2.html').read(), 'the second (fast) page differs from the first'
def slug(t, mx):
    out, dash = '', False
    for c in t.encode():
        ch = chr(c).lower() if 65 <= c <= 90 else chr(c)
        if ch.isascii() and ch.isalnum():
            if dash and out and len(out) < mx - 1: out += '-'
            dash = False
            if len(out) < mx: out += ch
        else:
            dash = True
        if len(out) >= mx: break
    return out
words = lambda t: len(t.split())
notes = [('Hello, World! <b>bold</b>', 'one two three'),
         ('Ünïcode & spaces   here', 'a b c d e f g h i j k l m n o p q r s t u v w x y z extra words for a long note body')]
ok = True
def want(what, got, exp):
    global ok
    good = got == exp
    ok &= good
    print(('ok  ' if good else 'FAIL'), what, '' if good else f': got {got!r}, want {exp!r}')
items = re.findall(r'<li id="note-\d+" data-slug="([^"]*)" data-words="(\d+)" data-serial="(\d+)"><h2>(.*?)</h2><p class="live-slug">(.*?)</p><p class="count">(\d+)</p>(<p class="long">long</p>)?</li>', page)
want('two notes', len(items), 2)
for i, ((title, body), (s, w, serial, h2, live, count, long_)) in enumerate(zip(notes, items)):
    want(f'note {i}: stored slug (C, in the action)', s, html.escape(slug(title, 40)))
    want(f'note {i}: stored word count (C, in the action)', int(w), words(body))
    want(f'note {i}: serial (a Rust effect, in the action)', int(serial), i + 1)
    want(f'note {i}: capitals (Rust, in the cached piece), escaped', h2, html.escape(title.upper(), quote=True))
    want(f'note {i}: slug of title and body (C, in the cached piece)', live, html.escape(slug(title + ' ' + body, 24)))
    want(f'note {i}: word count (C, in the cached piece)', int(count), words(body))
    want(f'note {i}: long (a C flag in an if)', bool(long_), words(body) > 5)
me = re.search(r'<p class="me">(.*?) has (\d+) word\(s\)</p>', page)
want('per viewer: capitals (Rust)', me and me.group(1), 'ADA LOVELACE')
want('per viewer: word count (C)', me and int(me.group(2)), 2)
print('PASS' if ok else 'FAIL'); sys.exit(0 if ok else 1)
EOF
curl -sS $U/_etch/stats | grep -oE '"verified":[0-9]+|"mismatched":[0-9]+' | tr '\n' ' '; echo
grep -iE "panic" $D/server.log && exit 1 || true
