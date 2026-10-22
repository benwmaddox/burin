APP=board CRATE=board-app bash /mnt/d/code/etch/scripts/build-etch.sh release 2>&1 | grep -E '^(error|emit|spec)'
bash /mnt/d/code/etch/scripts/etchc-test.sh 1
bash /mnt/d/code/etch/scripts/check-all.sh 2>&1 | grep -E '^==|IDENTICAL|differ|PASS|FAIL|0 allocations|test result: ok. 1|served fast|old cookie|"mismatched":[1-9]|"cleared":[1-9]|scrub:'
