#!/bin/bash
# The page's script applied to the last golden run's pages (scripts/golden.sh), the sidebar frame
# put where the browser puts it: must equal the old pages built per viewer on the server.
#   bash /mnt/d/code/burin/scripts/viewer-js-check.sh
cd /mnt/d/code/burin
for p in "david room" "jason room-jason"; do
  set -- $p
  python3 scripts/golden-viewer-js.py /tmp/burin-golden/$1-1.html /tmp/burin-golden/$1-sidebar.html data/golden-pages/$2-server-rendered.html
done
