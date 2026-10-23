#!/bin/bash
# Spec to binary after a one-line change in the spec (one word in a template), twice.
set -e
cd /mnt/d/code/burin
F=apps/campfire/room.burin
cp $F /tmp/room.burin.bak
trap "cp /tmp/room.burin.bak $F" EXIT
for w in Reply1 Reply2; do
  sed -i "s/<span class=\"for-screen-reader\">Reply[0-9]*<\/span>/<span class=\"for-screen-reader\">$w<\/span>/" $F
  s=$(date +%s.%N); bash scripts/build-burin.sh ${1:-release} > /tmp/spec-edit.log 2>&1; e=$(date +%s.%N)
  echo "one-line spec change -> binary: $(echo "$e - $s" | bc | cut -c1-5) s ($(grep -E '^spec' /tmp/spec-edit.log))"
done
