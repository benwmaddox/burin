#!/bin/bash
export EXTRA_ENV="$1"
shift
bash /mnt/d/code/etch/bench/mix/profile-rust.sh "$@"
