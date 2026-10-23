#!/bin/bash
# Campfire under the mixed load (bench/mix/profile.sh), its samples grouped: generated code,
# runtime, SQLite, compression, libc. Not a measurement (cores 0-7).
#   [POST_RATE=10] bash /mnt/d/code/burin/bench/mix/profile-groups.sh
cd /mnt/d/code/burin
POST_RATE=${POST_RATE:-10} CONC=64 TOP=100000 bash bench/mix/profile.sh 8 > /tmp/prof-all.txt 2>&1
grep -E '^ +[0-9.]+%' /tmp/prof-all.txt | awk '
{ p = $1; sub("%", "", p); s = $0; sub(/^ +[0-9.]+% +\[[^]]*\] +/, "", s)
  g = "other runtime (burin_rt)"
  if (s ~ /burin_build_|burin_load_|burin_act_|burin_bits_|Glue>::(build|load|act|bits)|gen::Spec/) g = "generated code"
  else if (s ~ /^sqlite3|^vdbe|^btree|^pcache|^wal|^getCell|^moveTo|^freeSpace|^balance|^getPage|^pager|^unixRead|^accessPayload|^insertCell|^fillInCell|^clearCell|^dropCell|^allocateSpace|^defragment|^rebuild|^releasePage/) g = "SQLite"
  else if (s ~ /miniz|deflate|inflate|crc32|adler/) g = "compression"
  else if (s ~ /^_int_|^malloc|^free|^cfree|^realloc|^__mem|^__strlen|^pthread_|^__libc|^__GI|^memcpy|^memmove|^memset|^__lll/) g = "libc (alloc, copies, locks)"
  else if (s ~ /^\[|^0x/) g = "unknown"
  t[g] += p }
END { for (g in t) printf "%6.2f%%  %s\n", t[g], g }' | sort -rn
echo "-- the generated code's hottest:"
grep -E '^ +[0-9.]+%' /tmp/prof-all.txt | grep -E 'burin_build_|burin_load_|burin_act_|Glue>::' | head -8
