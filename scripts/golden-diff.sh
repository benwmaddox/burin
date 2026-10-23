#!/bin/bash
# What differs between the last golden run's pages (scripts/golden.sh) and the expected ones, tag
# by tag (the pages are one long line).
#   bash /mnt/d/code/burin/scripts/golden-diff.sh
G=/mnt/d/code/burin/data/golden-pages; O=/tmp/burin-golden
for n in david-2:room jason-2:room-jason david-sidebar:sidebar jason-sidebar:sidebar-jason; do
  a=${n%%:*}; b=${n#*:}
  echo "== $a"
  [ -f $G/$b.html ] || { echo "(no expected page yet)"; continue; }
  diff <(sed 's/>/>\n/g' $G/$b.html) <(sed 's/>/>\n/g' $O/$a.html) | cut -c1-300
done
