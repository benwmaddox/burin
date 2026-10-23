# The mixed benchmark

Many people (117) reading and posting in many rooms (60), at different rates, against etch,
crudspec and the Rust port of Campfire (Rails and Flow are stage 2). Read-heavy, with writes
from various people. Each run is two passes (the second in reverse order) under the shared
bench lock, in its own network namespace and cgroup, the server on cores 8-11 and the load on
12-19.

    wsl -d Ubuntu -u root -- bash /mnt/d/code/etch/bench/mix/drive.sh <tag> etch crudspec rust

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
- **Reading.** etch's page says when the room being read was unread for the viewer, and its
  script marks it read (`/_do/read_room`): the load does the same ("marks/s"). Rails marks a
  room read through its presence channel over ActionCable: that's stage B (live connections).
- **Posting** follows each app's browser flow: etch and crudspec answer the form with a
  redirect, which the browser follows (the post's time includes that page); Rails' composer
  asks for a Turbo Stream and gets no page. The post comes from the room's page (its referer).
- **Static files** aren't asked for: a browser keeps them.

"ms CPU" is all the server's CPU over the page views (sidebars and read marks included).

## Not the same work (read every table with this)

- **Unread.** etch counts posts per room and members' last-seen counts: a post writes the room
  and the poster's membership. Rails, the Rust port and crudspec mark each disconnected
  member's row on every post (a write per member).
- **Presence.** Until stage B, nothing here holds a live connection: Rails' per-visit presence
  writes and its live pushes aren't exercised, and etch has no live updates yet.

## Results

`node bench/mix/table.mjs bench/runs mix-<tag>-`. Runs before s5 asked for the room page only
(no sidebar request at all, which favored the apps with the sidebar in a frame).
