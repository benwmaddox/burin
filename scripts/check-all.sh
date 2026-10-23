#!/bin/bash
# Every correctness check, on the release builds: golden pages, writes, outcomes, edits and
# boosts, the board app, numbers and time (apps/numbers), external functions (apps/notes), and the
# generated fast path's allocations.
#   bash /mnt/d/code/burin/scripts/check-all.sh
cd /mnt/d/code/burin
export BURIN_BIN=$HOME/hw/burin-gen-release
echo "== golden";        bash scripts/golden.sh 2>&1 | tail -4
echo "== the viewer script"; bash scripts/viewer-js-check.sh
echo "== writes";        bash scripts/writes-verify.sh 2>&1 | tail -2
echo "== outcomes";      bash scripts/outcomes.sh 2>&1 | tail -4
echo "== edits";         bash scripts/edits-verify.sh 2>&1 | tail -1
echo "== sign in";     bash scripts/signin-check.sh 2>&1 | tail -8
echo "== rooms";       bash scripts/rooms-check.sh 2>&1 | tail -3
echo "== people";      bash scripts/people-check.sh 2>&1 | tail -1
echo "== scrub";       bash scripts/scrub-check.sh 2>&1 | tail -2
echo "== fuzz";        bash scripts/fuzz-seeds.sh 600 1 2 3 | grep -E "PASS|FAIL"
echo "== unread";      bash scripts/unread-check.sh 2>&1 | grep -E "FAIL|PASS"
echo "== direct";      bash scripts/direct-check.sh 2>&1 | tail -3
echo "== board";         BURIN_BIN=$HOME/hw/burin-board-release bash scripts/board-check.sh 2>&1 | tail -16
echo "== externs";     BURIN_BIN=$HOME/hw/burin-notes-release bash scripts/notes-check.sh 2>&1 | grep -E "FAIL|PASS"
echo "== numbers";       BURIN_BIN=$HOME/hw/burin-numbers-release bash scripts/numbers-check.sh 2>&1 | grep -E "MISMATCH|PASS|FAIL"
echo "== live";        bash scripts/live-check.sh 2>&1 | grep -E "FAIL|PASS"
echo "== browser";     bash scripts/browser-smoke.sh 2>&1 | grep -E "FAIL|PASS"
echo "== browser live"; bash scripts/browser-live.sh 2>&1 | grep -E "FAIL|PASS"
echo "== hot path";      bash scripts/hot-path-test.sh 2>&1 | tail -4
