#!/bin/bash
# People and the account, with --verify: avatars, the profile (a rename shows everywhere; a new
# password signs in), someone's page, the account page, roles (variants follow a viewer's role)
# and deactivating someone. No fast page may differ from a fresh build.
#   bash /mnt/d/code/burin/scripts/people-check.sh
set -e
BIN=${BURIN_BIN:-$HOME/hw/burin-gen-release}
D=/tmp/burin-pc; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/burin/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13788
q() { node /mnt/d/code/burin/scripts/q.js $D/data/app.sqlite "$1"; }
DAVID=$(q "select token from Session where id = 481019661"); DU=$(q "select user from Session where token = '$DAVID'")
JASON=jasontokenjasontokenjasontoken00; JU=$(q "select user from Session where token = '$JASON'")
KU=$(q "select id from User where email = 'kevin@37signals.com'"); KEVIN=kevintokenkevintokenkevintoken00
ZU=$(q "select id from User where name = 'JZ'"); JZ=jztokenjztokenjztokenjztokenjz00
BOT=$(q "select id from User where role = 'bot' limit 1")
R=486777696
node /mnt/d/code/burin/scripts/x.js $D/data/app.sqlite "insert into Session (id, seq, created, updated, user, token) values (900000001, 1, 0, 0, $KU, '$KEVIN'), (900000003, 1, 0, 0, $ZU, '$JZ'); insert into Membership (id, seq, created, updated, room, user, involvement) select 900000002, 2, 0, 0, $R, $KU, 'everything' where not exists (select 1 from Membership where room = $R and user = $KU); insert into Membership (id, seq, created, updated, room, user, involvement) select 900000004, 2, 0, 0, $R, $ZU, 'everything' where not exists (select 1 from Membership where room = $R and user = $ZU)"
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/burin/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
U=http://127.0.0.1:$PORT
get() { curl -sS -o $D/page.html -w "%{http_code}" -H "Cookie: session_token=$1" $U$2; }
act() { local who=$1 name=$2; shift 2; local a=(); for f in "$@"; do a+=(--data-urlencode "$f"); done
  curl -sS -o /dev/null -D - -H "Cookie: session_token=$who" "${a[@]}" $U/_do/$name | tr -d '\r' | awk '/^[Xx]-[Ee]tch-[Oo]utcome/{o=$2} /^[Ll]ocation/{l=$2} END{printf "%s -> %s", o, l}'; }
room() { get $1 /rooms/$R >/dev/null; sleep 0.25; get $1 /rooms/$R; }

echo "david's avatar: $(curl -sS -o $D/a.svg -w '%{http_code} %{content_type}' -H "Cookie: session_token=$DAVID" $U/avatars/User/$DU), $(grep -o 'fill="#[0-9A-F]*"' $D/a.svg | head -1) $(grep -o '>[A-Z]*</text>' $D/a.svg), cache: $(curl -sS -D - -o /dev/null -H "Cookie: session_token=$DAVID" $U/avatars/User/$DU | tr -d '\r' | grep -i '^cache-control' | cut -d' ' -f2-)"
echo "rita's: $(curl -sS -H "Cookie: session_token=$DAVID" $U/avatars/User/$(q "select id from User where name = 'Rita Lopez'") | grep -o '>[A-Z]*</text>'); a bot's: $(curl -sS -H "Cookie: session_token=$DAVID" $U/avatars/User/$BOT | head -c 40)…"
echo "profile page: $(get $DAVID /users/me/profile), its forms: $(grep -c 'update_profile\|sign_out' $D/page.html)"
echo "kevin's page, seen by david: $(get $DAVID /users/$KU), bio: $(grep -c Programmer $D/page.html), ping button: $(grep -c 'create_direct_room' $D/page.html); by kevin: $(get $KEVIN /users/$KU), ping button: $(grep -c 'create_direct_room' $D/page.html || true)"
room $DAVID >/dev/null; room $KEVIN >/dev/null; room $JASON >/dev/null
echo "david renames himself: $(act $DAVID update_profile 'name=David H' 'email=david@example.com' 'bio=Makes things' 'password=new pass 1')"
echo "  the room as jason: $(room $JASON), 'David H' on it: $(grep -c 'title="David H"' $D/page.html) times; the old name: $(grep -c 'title="David"' $D/page.html)"
echo "  signing in with the new password: $(curl -sS -o /dev/null -D - --data-urlencode 'email=david@example.com' --data-urlencode 'password=new pass 1' $U/_do/sign_in | tr -d '\r' | grep -i x-burin-outcome | cut -d' ' -f2); the old one: $(curl -sS -o /dev/null -D - --data-urlencode 'email=david@example.com' --data-urlencode 'password=nope' $U/_do/sign_in | tr -d '\r' | grep -i x-burin-outcome | cut -d' ' -f2)"
echo "  a blank name: $(act $DAVID update_profile 'name= ' 'email=' 'bio=' 'password=')"
echo "account page, david: $(get $DAVID /account/edit), forms: $(grep -c 'update_account\|change_role' $D/page.html); kevin: $(get $KEVIN /account/edit), forms: $(grep -c 'update_account\|change_role' $D/page.html || true)"
echo "kevin's edit buttons before: $(room $KEVIN) $(grep -c 'edit_message' $D/page.html || true)"
echo "kevin made an administrator: $(act $DAVID change_role "user=$KU" 'role=administrator'); by kevin before? $(act $JZ change_role "user=$ZU" 'role=administrator')"
echo "  kevin's edit buttons now: $(room $KEVIN) $(grep -c 'edit_message' $D/page.html)"
echo "a made-up role: $(act $DAVID change_role "user=$KU" 'role=owner')"
echo "demote jason: $(act $DAVID change_role "user=$JU" 'role=member'); kevin: $(act $DAVID change_role "user=$KU" 'role=member'); then david demotes himself (the last one): $(act $DAVID change_role "user=$DU" 'role=member')"
echo "deactivate jz: $(act $DAVID deactivate_user "user=$ZU"); his rooms left: $(q "select count(*) from Membership m join Room r on r.id = m.room where m.user = $ZU and r.type != 'direct'"), direct: $(q "select count(*) from Membership m join Room r on r.id = m.room where m.user = $ZU and r.type = 'direct'"), sessions: $(q "select count(*) from Session where user = $ZU")"
echo "  jz reads the room: $(get $JZ /rooms/$R); again: $(act $DAVID deactivate_user "user=$ZU")"
echo "account renamed: $(act $DAVID update_account 'name=Burin Co' 'restrict=on'); kevin may make rooms: $(get $KEVIN /rooms/opens/new) ; sign-in page: $(curl -sS $U/session/new | grep -o '<h1[^>]*>[^<]*' | sed 's/.*>//')"
echo "  and back: $(act $DAVID update_account 'name=Burin Co' 'restrict=')"
sleep 0.3
for t in $DAVID $JASON $KEVIN; do room $t >/dev/null; done
curl -sS $U/_burin/stats; echo
grep -E "^(verify|scrub)" $D/server.log | head -5 || true
