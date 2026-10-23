# The mixed benchmark

Many people (117) reading and posting in many rooms (60), at different rates, against burin,
crudspec, the Rust port of Campfire as published (`rust`) and its optimized variant (`rustopt`,
`bench/rust-opt`: burin's caching ideas applied in idiomatic Rust). Rails and Flow are stage 2. Read-heavy, with writes
from various people. Each run is two passes (the second in reverse order) under the shared
bench lock, in its own network namespace and cgroup, the server on cores 8-11 and the load on
12-19.

    wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/mix/drive.sh <tag> burin crudspec rust

Data: `build-data.sh` (in WSL home, never in the repository). The load: `tools/mixload`.

## The browser model (since s5, 23 Oct 2026)

Every app gets the same load, shaped like browsers using Rails Campfire with Turbo:

- **Sessions.** A reader is one person's session at a time (chosen by how active they are):
  it opens the app at one of their rooms (chosen by popularity), then visits about 20 rooms
  (a geometric number) and ends; the next session is someone else.
- **Opening the app** asks for the room's page, then the sidebar's frame where the app has one
  (`/users/me/sidebar` with `Turbo-Frame: user_sidebar`, as Turbo asks for it). crudspec builds
  its sidebar into every page, so it has no frame request.
- **A visit** is a Turbo Drive visit: the page only. The frame is permanent, so it's kept; it is
  asked for again after a visit with probability 0.05 (Rails reloads it when the tab is shown
  again).
- **Reading.** burin's page says when the room being read was unread for the viewer; its stream
  marks it read when it opens (`watch read_room`). Readers here don't open streams, so they mark
  it read with `/_do/read_room` instead ("marks/s"). Rails marks a room read through its
  presence channel over Action Cable (live clients, below).
- **Posting** follows each app's browser flow: burin and crudspec answer the form with a
  redirect, which the browser follows (the post's time includes that page); Rails' composer
  asks for a Turbo Stream and gets no page. The post comes from the room's page (its referer).
- **Static files** aren't asked for: a browser keeps them.

"ms CPU" is all the server's CPU over the page views (sidebars and read marks included).

## Live clients (since s7, 23 Oct 2026; burin only so far)

`LIVE=N` adds N people with a room open, apart from the readers. Each opens one of their rooms,
the page and then its stream, and keeps it for an exponential time (`LIVE_HOLD`, 30 s on
average). Then it closes it and opens another, so there is one stream per visit, as a browser's
navigation makes. Opening and closing a stream reads the room (`watch read_room`).
- **Delivery** is measured from when the poster sent the post to when the client had its
  `append`. Only posts sent after that stream opened count, not replays from before it.
- **The delivered share:** a post was due to every stream on its room that opened before the post
  was sent and stayed open until a second after the server answered it. The share is how many of
  those got it, matched by the post's number in its text, the one key all three apps' markup
  carries.

Rails and the Rust ports get the same clients over Action Cable: still to build.

## Not the same work (read every table with this)

- **Unread.** burin and `rustopt` count posts per room and members' last-seen counts: a post
  writes the room and the poster's membership. Rails, the published Rust port and crudspec
  mark each disconnected member's row on every post (a write per member).
- **Presence.** Without `LIVE`, nothing holds a live connection: Rails' per-visit presence
  writes and its live pushes aren't exercised, and burin's readers stand in for its stream's
  read with `/_do/read_room`.
- **Live pushes.** Rails sends `unread` once per member, to each member's own stream. burin
  sends one event, which each worker filters by its streams' viewers: fewer server operations
  per post. Room changes push the sidebar's row in Rails; burin's sidebar reloads its frame. The
  mixed load doesn't change rooms.

## Results

`node bench/mix/table.mjs bench/runs mix-<tag>-`. Runs before s5 asked for the room page only
(no sidebar request at all, which favored the apps with the sidebar in a frame).
