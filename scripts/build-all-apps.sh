#!/bin/bash
# Builds every app from its spec (release), after a change to the compiler or runtime.
#   bash /mnt/d/code/burin/scripts/build-all-apps.sh
cd /mnt/d/code/burin
for a in "campfire campfire-app" "board board-app" "notes notes-app" "numbers numbers-app" "crunch crunch-app"; do
  set -- $a
  # (Campfire's binary is burin-gen-release: built with APP unset)
  if [ $1 = campfire ]; then out=$(bash scripts/build-burin.sh release 2>&1); else out=$(APP=$1 CRATE=$2 bash scripts/build-burin.sh release 2>&1); fi
  echo "== $1: $(echo "$out" | grep -E '^error|\.burin:|emit:|LLVM ERROR|error:|spec ->' | head -3)"
done
