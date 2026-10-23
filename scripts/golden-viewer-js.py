# What a browser shows, rebuilt here from the room page as served (a shell, as Rails Campfire's)
# and its sidebar frame (/users/me/sidebar, which the browser puts into the shell's empty frame),
# with the page's script applied (apps/campfire/scripts/viewer.js.html): must equal the old page,
# built per viewer on the server. Proves the page's new shape changed nothing a viewer sees.
#   python3 scripts/golden-viewer-js.py <room page> <sidebar frame> <old page>
#
# Declared differences (removed before comparing): Turbo's script and the meta tags in the head
# (what the script reads), the live stream's tag and the live list's id, the frame's wrapper, and the hidden __h fields the old sidebar's
# "start a ping" forms carried (burin never read them).
import re, sys
new, side, old = (open(p, encoding='utf-8').read() for p in sys.argv[1:4])
meta = lambda n: (re.search(r'<meta name="%s" content="([^"]*)">' % n, new) or [None, None])[1]
uid, admin, room = meta('current-user-id'), meta('current-user-role') == 'administrator', meta('current-room-id')
assert uid and room, 'no current-user-id or current-room-id meta'

page = new.replace(open('/mnt/d/code/burin/apps/campfire/scripts/viewer.js.html', encoding='utf-8').read(), '')
page = re.sub(r'<script type="module" src="/assets/turbo\.min\.js\?v=[0-9a-f]+"></script>', '', page)
page = re.sub(r'<meta name="(current-room-id|current-user-id|current-user-name|current-user-role|current-room-unread)" content="[^"]*">', '', page)
# live updates: the page's stream and the live list's id
page = re.sub(r'<turbo-stream-source src="/_burin/live\?[^"]*"></turbo-stream-source>', '', page)
page = page.replace(' id="live_room_messages"', '')

# the frame filled with what /users/me/sidebar answered
m = re.fullmatch(r'<turbo-frame id="user_sidebar">(.*)</turbo-frame>\n', side, re.S)
assert m, 'the sidebar answer is not one user_sidebar frame'
inner = m.group(1)
# the script: the room being read is never shown unread
inner = inner.replace('<a href="/rooms/%s" class="align-center gap room btn txt-nowrap unread">' % room, '<a href="/rooms/%s" class="align-center gap room btn txt-nowrap">' % room)
inner = inner.replace('<a href="/rooms/%s" class="direct unread">' % room, '<a href="/rooms/%s" class="direct">' % room)
frame = '<aside id="sidebar"><turbo-frame id="user_sidebar" src="/users/me/sidebar" target="_top" data-turbo-permanent="true"></turbo-frame></aside>\n'
assert page.count(frame) == 1, 'no sidebar frame in the page'
page = page.replace(frame, '<aside id="sidebar">' + inner + '</aside>\n')

# each message: its author, then its edit controls shown or removed, and "mine"
def message(m):
    body = m.group(0)
    author = re.search(r'data-user-id="(\d+)"', body).group(1)
    mine = author == uid
    if mine:
        body = body.replace('class="message message--formatted"', 'class="message message--formatted message--me"', 1)
    edit = re.search(r'<details class="position-relative message__edit" hidden>.*?</form></div></details>', body, re.S)
    if edit:
        body = body.replace(edit.group(0), edit.group(0).replace('<details class="position-relative message__edit" hidden>', '<details class="position-relative">', 1) if (mine or admin) else '', 1)
    return body
page = re.sub(r'<div id="message_[^"]*" class="message message--formatted".*?(?=<div id="message_|</div><button type="button" class="message-area__return)', message, page, flags=re.S)
# the viewer's own boosts' remove buttons shown; the others removed
def boost(m):
    return m.group(0).replace(' boost__remove" data-booster-id="%s" hidden>' % uid, '" >') if 'data-booster-id="%s"' % uid in m.group(0) else ''
page = re.sub(r'<form method="post" action="/_do/unboost" class="flex-inline boost__remove" data-booster-id="\d+" hidden>.*?</button></form>', boost, page, flags=re.S)

# the old page, less what's declared above
old = re.sub(r'(<form method="post" action="/_do/create_direct_room" class="flex-inline" ><input type="hidden" name="__arg_users" value="@\d+">)<input type="hidden" name="__h" value="[^"]*">', r'\1', old)
if page == old:
    print('equal after the script: identical to the old page (%d bytes)' % len(old.encode()))
else:
    i = next((j for j in range(min(len(page), len(old))) if page[j] != old[j]), min(len(page), len(old)))
    print('DIFFERS at', i, '\n got: ', repr(page[max(0, i - 120):i + 120]), '\n want:', repr(old[max(0, i - 120):i + 120]))
