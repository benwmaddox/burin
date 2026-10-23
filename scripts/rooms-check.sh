#!/bin/bash
# Rooms, with --verify: making open, closed and direct rooms; their settings pages; reopening,
# closing and their members; involvement; deleting a room with its messages. Every fast page must
# equal a fresh render; no write may clear the screens.
#   bash /mnt/d/code/etch/scripts/rooms-check.sh
set -e
BIN=${ETCH_BIN:-$HOME/hw/etch-gen-release}
D=/tmp/etch-rc; rm -rf $D; mkdir -p $D; cp -a /mnt/d/code/etch/data/campfire-golden $D/data; rm -f $D/data/app.lock
PORT=13793
q() { node /mnt/d/code/etch/scripts/q.js $D/data/app.sqlite "$1"; }
DAVID=$(q "select token from Session where id = 481019661"); DU=$(q "select user from Session where token = '$DAVID'")
JASON=jasontokenjasontokenjasontoken00; JU=$(q "select user from Session where token = '$JASON'")
KU=$(q "select id from User where email = 'kevin@37signals.com'")
TZ=America/New_York taskset -c 0-3 "$BIN" --data $D/data --assets /mnt/d/code/etch/apps/campfire/assets --port $PORT --workers 2 --verify > $D/server.log 2>&1 &
PID=$!
trap "kill $PID 2>/dev/null" EXIT
for i in $(seq 1 100); do curl -fsS -o /dev/null http://127.0.0.1:$PORT/up 2>/dev/null && break; sleep 0.05; done
U=http://127.0.0.1:$PORT
stat() { curl -sS $U/_etch/stats | sed "s/.*\"$1\":\([0-9]*\).*/\1/"; }
# an action: its outcome and where it went (the room's id is the Location's last part)
act() { local who=$1 name=$2; shift 2; local a=(); for f in "$@"; do a+=(--data-urlencode "$f"); done
  curl -sS -o /dev/null -D - -H "Cookie: session_token=$who" "${a[@]}" $U/_do/$name | tr -d '\r' | awk '/^[Xx]-[Ee]tch-[Oo]utcome/{o=$2} /^[Ll]ocation/{l=$2} END{printf "%s -> %s", o, l}'; }
get() { curl -sS -o $D/page.html -w "%{http_code}" -H "Cookie: session_token=$1" $U$2; }
room_of() { echo "${1##*/rooms/}"; }
members() { q "select group_concat(u.name, ', ') from (select u.name from Membership m join User u on u.id = m.user where m.room = $1 order by u.name) u"; }
twice() { get $1 /rooms/$2 >/dev/null; sleep 0.25; local f0=$(stat fast); local c=$(get $1 /rooms/$2); echo "$c (fast: $(( $(stat fast) - f0 )))"; }

echo "new room page: $(get $DAVID /rooms/opens/new), its forms: $(grep -c 'create_open_room\|create_closed_room' $D/page.html); direct page: $(get $DAVID /rooms/directs/new)"
r=$(act $DAVID create_open_room "name=Lounge & <b>bar</b>"); OPEN=$(room_of "$r"); echo "open room:    $r"
echo "  members: $(q "select count(*) from Membership where room = $OPEN") of $(q "select count(*) from User where status = 'active'") active users; david reads it: $(twice $DAVID $OPEN); kevin-free title: $(grep -o '<title>[^<]*' $D/page.html | sed 's/<title>//')"
echo "blank name:   $(act $DAVID create_open_room "name=  ")"
r=$(act $JASON create_closed_room "name=Secret" "users=$KU"); CLOSED=$(room_of "$r"); echo "closed room:  $r"
echo "  members: $(members $CLOSED); david reads it: $(get $DAVID /rooms/$CLOSED); jason: $(twice $JASON $CLOSED)"
r=$(act $DAVID create_direct_room "__arg_users=@$JU"); echo "direct with jason: $r (the existing one: 186869642)"
r=$(act $DAVID create_direct_room "users=$JU" "users=$KU"); NEWD=$(room_of "$r"); echo "direct with jason and kevin: $r, members: $(members $NEWD)"
echo "  again: $(act $DAVID create_direct_room "users=$KU" "users=$JU") (the same room)"
echo "  jason's title: $(twice $JASON $NEWD), \"$(grep -o '<title>[^<]*' $D/page.html | sed 's/<title>//')\""
echo "settings page, jason: $(get $JASON /rooms/$CLOSED/edit), kevin checked: $(grep -c "value=\"$KU\" checked" $D/page.html); david (not in it): $(get $DAVID /rooms/$CLOSED/edit)"
echo "settings of a direct room: $(get $JASON /rooms/$NEWD/edit)"
echo "david updates jason's room: $(act $DAVID update_closed_room "room=$CLOSED" "name=Mine now" "users=$KU")"
echo "jason adds david:   $(act $JASON update_closed_room "room=$CLOSED" "name=Secret 2" "users=$KU" "users=$DU"); members: $(members $CLOSED); david reads it: $(twice $DAVID $CLOSED)"
echo "jason drops kevin and david: $(act $JASON update_closed_room "room=$CLOSED" "name=Secret 3"); members: $(members $CLOSED); david: $(get $DAVID /rooms/$CLOSED)"
echo "jason opens it:     $(act $JASON update_open_room "room=$CLOSED" "name=Open now"); members: $(q "select count(*) from Membership where room = $CLOSED"); david: $(twice $DAVID $CLOSED)"
echo "david's sidebar lists the lounge: $(get $DAVID /users/me/sidebar >/dev/null; grep -c "href=\"/rooms/$OPEN\"" $D/page.html || true)"
echo "david hides the lounge: $(act $DAVID set_involvement "room=$OPEN" "involvement=invisible"); in his sidebar: $(sleep 0.25; get $DAVID /users/me/sidebar >/dev/null; grep -c "href=\"/rooms/$OPEN\"" $D/page.html || true)"
echo "  a made-up involvement: $(act $DAVID set_involvement "room=$OPEN" "involvement=loud")"
curl -sS -o /dev/null -H "Cookie: session_token=$JASON" --data-urlencode "room=$CLOSED" --data-urlencode "body=to be deleted" $U/_do/post_message
echo "jason deletes his room: $(act $JASON delete_room "room=$CLOSED"); left: rooms $(q "select count(*) from Room where id = $CLOSED"), messages $(q "select count(*) from Message where room = $CLOSED"), memberships $(q "select count(*) from Membership where room = $CLOSED")"
echo "  jason reads it now: $(get $JASON /rooms/$CLOSED)"
sleep 0.3
for r in 486777696 $OPEN $NEWD; do get $DAVID /rooms/$r >/dev/null; get $JASON /rooms/$r > /dev/null; done
curl -sS $U/_etch/stats; echo
grep -v "^started" $D/server.log | head -10 || true
