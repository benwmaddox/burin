#!/usr/bin/env python3
# The query plan of the sidebar statement whose text contains argv[1] (mixed data).
import sqlite3, subprocess, sys
sql = subprocess.run(["bash", "/mnt/d/code/burin/bench/mix/sidebar-sql.sh", "100000"], capture_output=True, text=True).stdout.splitlines()
c = sqlite3.connect("file:/home/ben/burin-mix/burin/app.sqlite?mode=ro", uri=True)
print([r[1] for r in c.execute("SELECT type, sql FROM sqlite_master WHERE type='index'")])
for q in sql:
    if sys.argv[1] in q:
        print(q)
        for r in c.execute("EXPLAIN QUERY PLAN " + q, [1] * q.count("?")):
            print(r)
