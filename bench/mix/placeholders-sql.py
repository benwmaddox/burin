#!/usr/bin/env python3
# The sidebar's "start a ping with" query on the mixed data: as etch lowers the spec today, and
# with the room-type condition inside the viewer's own memberships (their direct rooms first, as
# Rails does), timed for every viewer. Same rows required.
#   python3 /mnt/d/code/etch/bench/mix/placeholders-sql.py
import sqlite3, time

c = sqlite3.connect("file:/home/ben/etch-mix/etch/app.sqlite?mode=ro", uri=True)
users = [u for (u,) in c.execute('SELECT id FROM "User" WHERE status = \'active\'')]
now = '''SELECT t0.id, t0."name" FROM "User" t0 WHERE (((t0."status" IS 'active') AND (t0.id IS NOT :v)) AND NOT ((t0.id IN (SELECT s4."user" FROM "Membership" s4 WHERE s4."user" IS NOT NULL AND ((SELECT x."type" FROM "Room" x WHERE x.id = s4."room") IS 'direct') AND (s4."room" IN (SELECT s6."room" FROM "Membership" s6 WHERE s6."room" IS NOT NULL AND (s6."user" IS :v))))))) ORDER BY t0."created", t0."seq" LIMIT max(0, (20 - (SELECT count(DISTINCT s7."user") FROM "Membership" s7 WHERE (((SELECT x."type" FROM "Room" x WHERE x.id = s7."room") IS 'direct') AND (s7."room" IN (SELECT s9."room" FROM "Membership" s9 WHERE s9."room" IS NOT NULL AND (s9."user" IS :v)))))))'''
inner = '''SELECT t0.id, t0."name" FROM "User" t0 WHERE (((t0."status" IS 'active') AND (t0.id IS NOT :v)) AND NOT ((t0.id IN (SELECT s4."user" FROM "Membership" s4 WHERE s4."user" IS NOT NULL AND (s4."room" IN (SELECT s6."room" FROM "Membership" s6 WHERE s6."room" IS NOT NULL AND (s6."user" IS :v) AND ((SELECT x."type" FROM "Room" x WHERE x.id = s6."room") IS 'direct'))))))) ORDER BY t0."created", t0."seq" LIMIT max(0, (20 - (SELECT count(DISTINCT s7."user") FROM "Membership" s7 WHERE (s7."room" IN (SELECT s9."room" FROM "Membership" s9 WHERE s9."room" IS NOT NULL AND (s9."user" IS :v) AND ((SELECT x."type" FROM "Room" x WHERE x.id = s9."room") IS 'direct'))))))'''
for name, q in (("as lowered now", now), ("type inside the viewer's memberships", inner)):
    rows = {u: c.execute(q, {"v": u}).fetchall() for u in users}
    t0 = time.perf_counter(); k = 0
    while time.perf_counter() - t0 < 0.5:
        for u in users:
            c.execute(q, {"v": u}).fetchall(); k += 1
    print(f"{(time.perf_counter() - t0) / k * 1e6:8.1f} us  {name}")
    if name.startswith("as"):
        base = rows
    else:
        print("same rows for every viewer:", rows == base)
