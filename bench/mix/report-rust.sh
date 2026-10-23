#!/bin/bash
# The last Rust port profile (bench/mix/profile-rust.sh) by inclusive time: what each function
# and everything it calls cost (needs the frame-pointer build, build-rust-opt-prof.sh).
#   bash /mnt/d/code/burin/bench/mix/report-rust.sh [filter regex] [limit %]
P=$(ls /usr/lib/linux-tools-*/perf | head -1)
$P report -i /tmp/rust-prof/perf.data --children -g none --percent-limit ${2:-1.5} --sort symbol --stdio 2>/dev/null \
  | grep -vE '^#|^$' | grep -E "${1:-.}" | head -80 | cut -c1-200
