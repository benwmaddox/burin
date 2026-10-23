#!/bin/bash
# The one-off unread backfill (scripts/backfill-seen.py) on this machine's Campfire databases,
# each backed up first (in WSL home, never in the repository).
#   bash /mnt/d/code/burin/scripts/backfill-all.sh
set -euo pipefail
B=$HOME/burin-backup-$(date +%Y-%m-%d)
mkdir -p "$B"
for d in /mnt/d/code/burin/data/campfire /mnt/d/code/burin/data/campfire-golden $HOME/burin-mix/burin; do
  [ -d "$B/$(basename "$d")" ] || cp -a "$d" "$B/$(basename "$d")"
  python3 /mnt/d/code/burin/scripts/backfill-seen.py "$d/app.sqlite"
done
du -sh "$B"
