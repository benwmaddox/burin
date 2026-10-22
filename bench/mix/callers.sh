#!/bin/bash
# Who calls a symbol, in the last profile (bench/mix/profile.sh): its call chains.
#   bash /mnt/d/code/etch/bench/mix/callers.sh <symbol>
P=$(ls /usr/lib/linux-tools-*/perf | head -1)
$P report -i /tmp/etch-mix-prof/perf.data --no-children --symbols "$1" -G --stdio 2>/dev/null | grep -vE '^$|^#' | head -50 | cut -c1-160
