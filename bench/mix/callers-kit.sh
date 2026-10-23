#!/bin/bash
# Which of the port's own functions (campfire crates) appear on the call chains into a function
# (substring), in the last profile: their mangled names' readable parts, by how often.
#   bash /mnt/d/code/burin/bench/mix/callers-kit.sh <symbol substring>
P=$(ls /usr/lib/linux-tools-*/perf | head -1)
$P report -i /tmp/rust-prof/perf.data --no-children --symbol-filter="$1" -g caller,0.5,callee,function,percent --stdio 2>/dev/null \
  | grep -oE "campfire_[a-z]+[A-Za-z0-9_]{0,160}" | sed -E 's/[0-9]+([a-z_]+)/ \1/g' | sort | uniq -c | sort -rn | head -20
