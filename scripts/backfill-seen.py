#!/usr/bin/env python3
# One-off, for a Campfire database made before unread became counts (Room.posted,
# Membership.seen) instead of a time set on every member's row by each post: the same rooms stay
# unread. Every room gets posted = 1; a member is caught up (seen = 1) unless the old
# unread_since is set. Does nothing to a database already moved.
#   python3 scripts/backfill-seen.py <app.sqlite>...
import sqlite3, sys

for path in sys.argv[1:]:
    c = sqlite3.connect(path)
    cols = lambda t: {r[1] for r in c.execute(f'PRAGMA table_info("{t}")')}
    if "posted" not in cols("Room"):
        c.execute('ALTER TABLE "Room" ADD COLUMN "posted" INTEGER NOT NULL DEFAULT 0')
    if "seen" not in cols("Membership"):
        c.execute('ALTER TABLE "Membership" ADD COLUMN "seen" INTEGER NOT NULL DEFAULT 0')
    if c.execute('SELECT count(*) FROM "Room" WHERE posted > 0').fetchone()[0] or "unread_since" not in cols("Membership"):
        print(f"{path}: already moved")
    else:
        c.execute('UPDATE "Room" SET posted = 1')
        c.execute('UPDATE "Membership" SET seen = CASE WHEN unread_since IS NULL THEN 1 ELSE 0 END')
        n = c.execute('SELECT count(*) FROM "Membership" WHERE seen = 0').fetchone()[0]
        print(f"{path}: moved ({n} memberships unread)")
    c.commit()
