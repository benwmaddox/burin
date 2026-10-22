#!/bin/bash
# The sidebar loader's queries' plans, on the mixed data (EXPLAIN QUERY PLAN).
D=$HOME/etch-mix/etch/app.sqlite
bash /mnt/d/code/etch/bench/mix/sidebar-sql.sh 100000 | while IFS= read -r q; do
  echo "== ${q:0:110}"
  sqlite3 "$D" "EXPLAIN QUERY PLAN $q" 2>&1 | head -14
done
