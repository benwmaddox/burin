#!/bin/bash
# The writer under load, for each binary given: post latency at 200 posts/s with readers, and a
# profile of the writer thread (scripts/write-latency.sh). Holds the bench lock.
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/drive-wl.sh <binary>…
exec 9>>/home/ben/linuxbench/bench.lock
echo "$(date -u +%T) waiting for the bench lock"; flock -x 9; echo "$(date -u +%T) have the lock"
for b in "$@"; do
  echo "== $b"
  bash /mnt/d/code/burin/scripts/write-latency.sh "$b" 200 10 2>&1 | grep -E "p50|p99|reads|^ +[0-9]" | head -22
done
