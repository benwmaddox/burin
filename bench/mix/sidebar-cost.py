#!/usr/bin/env python3
# What each of the sidebar loader's statements costs on the mixed data (python's sqlite3, so the
# absolute numbers are higher than the server's; the ranking is what matters).
#   python3 /mnt/d/code/burin/bench/mix/sidebar-cost.py   (after sidebar-sql.sh's build)
import sqlite3, subprocess, time

sql = subprocess.run(["bash", "/mnt/d/code/burin/bench/mix/sidebar-sql.sh", "100000"], capture_output=True, text=True).stdout.splitlines()
c = sqlite3.connect("file:/home/ben/burin-mix/burin/app.sqlite?mode=ro", uri=True)
users = [u for (u,) in c.execute('SELECT "user" FROM "Membership" GROUP BY "user" ORDER BY count(*) DESC')]
direct = c.execute('SELECT m."room", m."user" FROM "Membership" m JOIN "Room" r ON r.id = m."room" WHERE r."type" = \'direct\' LIMIT 1').fetchone()
room = c.execute('SELECT id FROM "Room" LIMIT 1').fetchone()[0]
for q in sql:
    n = q.count("?")
    if q.startswith('SELECT t0.id, (SELECT'):
        args = [room]
    elif '"room" IS ?' in q:
        args = [direct[0], direct[1]]
    else:
        args = [users[len(users) // 2]] * n
    st = c.execute(q, args).fetchall()
    t0 = time.perf_counter(); k = 0
    while time.perf_counter() - t0 < 0.3:
        for u in users:
            if not '"room" IS ?' in q and not q.startswith('SELECT t0.id, (SELECT'):
                args = [u] * n
            c.execute(q, args).fetchall(); k += 1
    us = (time.perf_counter() - t0) / k * 1e6
    print(f"{us:8.1f} us  {len(st):4} rows  {q[:110]}")
