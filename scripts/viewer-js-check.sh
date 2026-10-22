#!/bin/bash
# The page's script applied to the last golden run's pages (scripts/golden.sh), with the on-demand
# part put where the script puts it: must equal the old pages built per viewer on the server.
#   bash /mnt/d/code/etch/scripts/viewer-js-check.sh
cd /mnt/d/code/etch
for p in "david room" "jason room-jason"; do
  set -- $p
  python3 scripts/golden-viewer-js.py /tmp/etch-golden/$1-1.html data/golden-pages/$2-server-rendered.html /tmp/etch-golden/$1-ping.html
done
