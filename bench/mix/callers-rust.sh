#!/bin/bash
# The call chains into a function (a substring of its mangled name) in the last Rust port
# profile (bench/mix/profile-rust.sh with the frame-pointer build), callers last.
#   bash /mnt/d/code/burin/bench/mix/callers-rust.sh <symbol substring> [lines]
P=$(ls /usr/lib/linux-tools-*/perf | head -1)
$P report -i /tmp/rust-prof/perf.data --no-children --symbol-filter="$1" -g caller,0.5,callee,function,percent --stdio 2>/dev/null \
  | grep -vE '^$|^#' | sed -E 's/_R[A-Za-z0-9_]*(campfire_[a-z]+|zlib_rs|flate2|hyper|axum|tokio)[A-Za-z0-9_]*/[\1…]/g' | head -${2:-60} | cut -c1-200
