#!/bin/bash
# Signing in and out, with --verify: the sign-in page; wrong passwords, unknown and forged
# attempts refused; a good one sets the cookie, "/" goes to a room, and the room is then served
# fast (the new session is in the snapshot); signing out clears it and the old cookie is refused.
#   bash /mnt/d/code/burin/scripts/signin-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-si; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13794
q() { node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "$1"; }
# Kevin (a member) gets a password; he's added to the bench room
KU=$(q "select id from User where email = 'kevin@37signals.com'")
HASH=$("$BIN" --hash-password 'kevins pass')
node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "update User set password = '$HASH' where id = $KU; insert into Membership (id, seq, created, updated, room, user, involvement) values (900000002, 2, 0, 0, 486777696, $KU, 'everything')"
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
U=http://127.0.0.1:$PORT
stat() { curl -sS $U/_burin/stats | sed "s/.*\"$1\":\([0-9]*\).*/\1/"; }
# a sign-in attempt: its outcome, where it goes, its cookie, how long it took
signin() { curl -sS -o /dev/null -D $D/h -w "%{time_total}" -H "Referer: $U/session/new" "$@" $U/_do/sign_in > $D/t
  tr -d '\r' < $D/h | awk -v t=$(cat $D/t) '/^[Xx]-[Ee]tch-[Oo]utcome/{o=$2} /^[Ll]ocation/{l=$2} /^[Ss]et-[Cc]ookie/{c=$2} END{printf "%s -> %s %s (%.0f ms)", o, l, (c ? "cookie " substr(c, 1, 22) "…" : "no cookie"), t * 1000}'; }
echo "sign-in page: $(curl -sS -o $D/page.html -w '%{http_code}' $U/session/new), its form: $(grep -c 'action="/_do/sign_in"' $D/page.html), title: $(grep -o '<h1[^>]*>[^<]*' $D/page.html | sed 's/.*>//')"
echo "/ signed out:          $(curl -sS -o /dev/null -w '%{http_code} -> %{redirect_url}' $U/)"
echo "wrong password:        $(signin --data-urlencode 'email=kevin@37signals.com' --data-urlencode 'password=nope')"
echo "unknown email:         $(signin --data-urlencode 'email=nobody@example.com' --data-urlencode 'password=nope')"
echo "forged verified field: $(signin --data-urlencode 'email=kevin@37signals.com' --data-urlencode 'password=nope' --data-urlencode "__verified_0=$HASH")"
echo "right password:        $(signin --data-urlencode 'email=kevin@37signals.com' --data-urlencode 'password=kevins pass')"
TOKEN=$(tr -d '\r' < $D/h | sed -n 's/^[Ss]et-[Cc]ookie: session_token=\([^;]*\).*/\1/p')
echo "  the cookie: $(tr -d '\r' < $D/h | grep -i '^set-cookie' | sed 's/session_token=[^;]*/session_token=…/')"
echo "  sessions for kevin: $(q "select count(*) from Session where user = $KU")"
HOME_TO=$(curl -sS -o /dev/null -w '%{redirect_url}' -H "Cookie: session_token=$TOKEN" $U/)
echo "/ signed in:           -> $HOME_TO"
R=${HOME_TO##*/}
curl -sS -o /dev/null -H "Cookie: session_token=$TOKEN" $U/rooms/$R; sleep 0.3
f0=$(stat fast)
for i in 1 2 3; do curl -sS -o /dev/null -w '%{http_code} ' -H "Cookie: session_token=$TOKEN" $U/rooms/$R; done
curl -sS -o /dev/null -w '%{http_code} ' --compressed -H "Cookie: session_token=$TOKEN" $U/rooms/$R
echo "(served fast: $(( $(stat fast) - f0 )) of 4)"
curl -sS -o $D/k.html -H "Cookie: session_token=$TOKEN" -H "Turbo-Frame: user_sidebar" $U/users/me/sidebar
echo "kevin's sidebar is his: $(grep -c "/avatars/User/$KU\" width=\"48\" height=\"48\"><span class=\"for-screen-reader\">My Settings" $D/k.html)"
echo "sign out:              $(curl -sS -o /dev/null -D - -H "Cookie: session_token=$TOKEN" -X POST $U/_do/sign_out | tr -d '\r' | awk '/^[Xx]-[Ee]tch-[Oo]utcome/{o=$2} /^[Ll]ocation/{l=$2} /^[Ss]et-[Cc]ookie/{c=$2} END{printf "%s -> %s %s", o, l, c}')"
sleep 0.2; f0=$(stat fast)
echo "the old cookie:        $(curl -sS -o /dev/null -w '%{http_code} -> %{redirect_url}' -H "Cookie: session_token=$TOKEN" $U/rooms/$R) (served fast: $(( $(stat fast) - f0 )))"
echo "  sessions for kevin: $(q "select count(*) from Session where user = $KU")"
curl -sS $U/_burin/stats; echo
grep -v "^started" $D/server.log | head -10 || true
