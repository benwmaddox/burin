#!/bin/bash
# The one-off unread backfill (scripts/backfill-seen.py) on this machine's Campfire databases,
# each backed up first (in WSL home, never in the repository).
#   bash /mnt/d/code/etch/scripts/backfill-all.sh
set -euo pipefail
B=$HOME/etch-backup-$(date +%Y-%m-%d)
mkdir -p "$B"
for d in /mnt/d/code/etch/data/campfire /mnt/d/code/etch/data/campfire-golden $HOME/etch-mix/etch; do
  [ -d "$B/$(basename "$d")" ] || cp -a "$d" "$B/$(basename "$d")"
  python3 /mnt/d/code/etch/scripts/backfill-seen.py "$d/app.sqlite"
done
du -sh "$B"
