#!/bin/bash
# Spec to binary after a one-line change in the spec (one word in a template), twice.
set -e
cd /mnt/d/code/etch
F=apps/campfire/room.etch
cp $F /tmp/room.etch.bak
trap "cp /tmp/room.etch.bak $F" EXIT
for w in Reply1 Reply2; do
  sed -i "s/<span class=\"for-screen-reader\">Reply[0-9]*<\/span>/<span class=\"for-screen-reader\">$w<\/span>/" $F
  s=$(date +%s.%N); bash scripts/build-etch.sh ${1:-release} > /tmp/spec-edit.log 2>&1; e=$(date +%s.%N)
  echo "one-line spec change -> binary: $(echo "$e - $s" | bc | cut -c1-5) s ($(grep -E '^spec' /tmp/spec-edit.log))"
done
