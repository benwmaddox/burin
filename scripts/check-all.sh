#!/bin/bash
# Every correctness check, on the release builds: golden pages, writes, outcomes, edits and
# boosts, the board app, numbers and time (apps/numbers), and the generated fast path's allocations.
#   bash /mnt/d/code/etch/scripts/check-all.sh
cd /mnt/d/code/etch
export ETCH_BIN=$HOME/hw/etch-gen-release
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
echo "== board";         ETCH_BIN=$HOME/hw/etch-board-release bash scripts/board-check.sh 2>&1 | tail -16
echo "== numbers";       ETCH_BIN=$HOME/hw/etch-numbers-release bash scripts/numbers-check.sh 2>&1 | grep -E "MISMATCH|PASS|FAIL"
echo "== browser";     bash scripts/browser-smoke.sh 2>&1 | grep -E "FAIL|PASS"
echo "== hot path";      bash scripts/hot-path-test.sh 2>&1 | tail -4
