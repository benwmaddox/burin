# The page's viewer script (apps/campfire/scripts/viewer.js.html), applied here: the new page with
# what it shows made visible, and what stays hidden removed, must equal the old golden page (built
# on the server per viewer). Proves the move to the client changed nothing a viewer sees.
#   python3 scripts/golden-viewer-js.py <new page> <old golden page>
import re, sys
new, old = open(sys.argv[1], encoding='utf-8').read(), open(sys.argv[2], encoding='utf-8').read()
me = re.search(r'<aside id="sidebar" data-user-id="(\d+)" data-role="([^"]*)"(?: data-read-room="\d+")?>', new)
uid, admin = me.group(1), me.group(2) == 'administrator'
page = new.replace(me.group(0), '<aside id="sidebar">')
page = page.replace(open('/mnt/d/code/etch/apps/campfire/scripts/viewer.js.html', encoding='utf-8').read(), '')

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
if page == old:
    print('equal after the script: identical to the old page (%d bytes)' % len(old.encode()))
else:
    i = next((j for j in range(min(len(page), len(old))) if page[j] != old[j]), min(len(page), len(old)))
    print('DIFFERS at', i, '\n got: ', repr(page[max(0, i - 120):i + 120]), '\n want:', repr(old[max(0, i - 120):i + 120]))
