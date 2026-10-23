#!/bin/bash
export EXTRA_ENV="$1"
shift
bash /mnt/d/code/burin/bench/mix/profile-rust.sh "$@"
