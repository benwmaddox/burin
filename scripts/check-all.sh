#!/bin/bash
# Every correctness check, on the release builds: golden pages, writes, outcomes, edits and
# boosts, the board app, and the generated fast path's allocations.
#   bash /mnt/d/code/etch/scripts/check-all.sh
cd /mnt/d/code/etch
export ETCH_BIN=$HOME/hw/etch-gen-release
echo "== golden";        bash scripts/golden.sh 2>&1 | tail -4
echo "== writes";        bash scripts/writes-verify.sh 2>&1 | tail -2
echo "== outcomes";      bash scripts/outcomes.sh 2>&1 | tail -4
echo "== edits";         bash scripts/edits-verify.sh 2>&1 | tail -1
echo "== direct";      bash scripts/direct-check.sh 2>&1 | tail -3
echo "== board";         ETCH_BIN=$HOME/hw/etch-board-release bash scripts/board-check.sh 2>&1 | tail -16
echo "== hot path";      bash scripts/hot-path-test.sh 2>&1 | tail -4
