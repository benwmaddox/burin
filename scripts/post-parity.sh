#!/bin/bash
# Posts the same messages to crudspec and to etch (each on its own copy of the golden data) and
# compares the room pages, with the post times and the random client ids masked.
#   bash /mnt/d/code/etch/scripts/post-parity.sh
set -e
CS=$HOME/hw/campfire-pk
ET=${ETCH_BIN:-$HOME/hw/etch-gen-release}
D=/tmp/etch-pp; rm -rf $D; mkdir -p $D
cp -a /mnt/d/code/etch/data/campfire-golden $D/cs; cp -a /mnt/d/code/etch/data/campfire-golden $D/et; rm -f $D/cs/app.lock $D/et/app.lock
cd /mnt/d/code/crudspec
TZ=America/New_York CRUDSPEC_NO_CACHE=1 taskset -c 0-3 "$CS" --data $D/cs --assets /mnt/d/code/crudspec-assets/campfire --port 13792 > $D/cs.log 2>&1 &
P1=$!
trap "kill $P1 2>/dev/null; true" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:13792/up 2>/dev/null && break; sleep 0.05; done
DAVID=$(node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$D/cs/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
msgs=("plain words" "<b>bold</b> & 'quotes' \"double\"" "see https://example.com/x?y=1&z=2 now" "line one
line two")
run() { # port
  for m in "${msgs[@]}"; do curl -sS -o /dev/null -H "Cookie: session_token=$DAVID" --data-urlencode "room=486777696" --data-urlencode "body=$m" http://127.0.0.1:$1/_do/post_message; done
  curl -sS -o $D/page-$1.html -H "Cookie: session_token=$DAVID" http://127.0.0.1:$1/rooms/486777696
}
run 13792; kill $P1; sleep 0.5
TZ=America/New_York taskset -c 0-3 "$ET" --data $D/et --assets /mnt/d/code/crudspec-assets/campfire --port 13793 --workers 2 > $D/et.log 2>&1 &
P2=$!
trap "kill $P2 2>/dev/null; true" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:13793/up 2>/dev/null && break; sleep 0.05; done
run 13793
python3 - <<EOF
import re
def norm(t):
    t = re.sub(r'<!--/?f:[^>]*-->', '', t)
    t = re.sub(r'\n<script id="crudspec-page".*?</script><script src="/_live\.[0-9a-f]+\.js" defer></script>', '\n', t, flags=re.S)
    t = t.replace('127.0.0.1:13792', 'HOST').replace('127.0.0.1:13793', 'HOST')
    t = re.sub(r'datetime="[^"]*"', 'datetime="T"', t)
    t = re.sub(r'>\d{1,2}:\d\d [AP]M<', '>CLOCK<', t)
    t = re.sub(r'>\w{3} \w{3} \d\d \d{4}<', '>DATE<', t)
    t = re.sub(r'id="message_[^"]*"', 'id="message_X"', t)
    return t
a = norm(open('$D/page-13792.html', encoding='utf-8').read())
b = norm(open('$D/page-13793.html', encoding='utf-8').read())
if a == b:
    print('posted pages identical (masked):', len(a))
else:
    i = next((j for j in range(min(len(a), len(b))) if a[j] != b[j]), min(len(a), len(b)))
    print('DIFFER at', i)
    print(' crudspec:', repr(a[max(0,i-150):i+150]))
    print(' etch:    ', repr(b[max(0,i-150):i+150]))
EOF
