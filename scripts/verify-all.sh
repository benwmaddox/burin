#!/bin/bash
# Extra fuzz seeds, then every check, summarized.
bash /mnt/d/code/etch/scripts/fuzz-seeds.sh 800 4 5 6 7 8 2>&1 | grep -E 'PASS|FAIL'
bash /mnt/d/code/etch/scripts/run-all-quiet.sh 2>&1 | grep -E '^==|IDENTICAL|DIFFERS|PASS|FAIL|0 allocations|test result: ok. 1|"mismatched":[1-9]|"cleared":[1-9]|panicked' | cut -c1-160
