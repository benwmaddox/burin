#!/bin/bash
# Makes the last golden run's pages (scripts/golden.sh) the expected ones: only after reading
# scripts/golden-diff.sh and the viewer-script check (scripts/viewer-js-check.sh).
#   bash /mnt/d/code/etch/scripts/golden-accept.sh
set -e
G=/mnt/d/code/etch/data/golden-pages; O=/tmp/etch-golden
cp $O/david-2.html $G/room.html; cp $O/jason-2.html $G/room-jason.html
cp $O/david-sidebar.html $G/sidebar.html; cp $O/jason-sidebar.html $G/sidebar-jason.html
ls -l $G
