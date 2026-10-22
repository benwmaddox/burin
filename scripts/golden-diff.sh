#!/bin/bash
# What differs between the last golden run's pages (scripts/golden.sh) and the expected ones, tag
# by tag (the pages are one long line).
#   bash /mnt/d/code/etch/scripts/golden-diff.sh
for n in david:room jason:room-jason; do
  a=${n%%:*}; b=${n#*:}
  echo "== $a"
  diff <(sed 's/>/>\n/g' /mnt/d/code/etch/data/golden-pages/$b.html) <(sed 's/>/>\n/g' /tmp/etch-golden/$a-2.html) | cut -c1-300
done
