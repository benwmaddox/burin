# Adapts crudspec's bench scripts for burin's runs (kept to show what changed). Run once.
for f in ['lifetime.sh', 'writes.sh']:
    s = open(f, encoding='utf-8').read()
    # the shared bench lock around the whole run; results in burin's repo
    s = s.replace('set -euo pipefail\n', 'set -euo pipefail\nexec 9>>/home/ben/linuxbench/bench.lock; flock -x 9   # the bench lock the sessions share\n', 1)
    s = s.replace('OUT=$REPO/bench/runs/$RUN', 'OUT=/mnt/d/code/burin/bench/runs/$RUN')
    s = s.replace('${GOGC:+GOGC=$GOGC}', '${GOGC:+GOGC=$GOGC} ${PAGE_KEYS:+CRUDSPEC_PAGE_KEYS=1}')
    s = s.replace('CRUDSPEC_PPROF=6060 \\', 'CRUDSPEC_PPROF=6060 ${PAGE_KEYS:+CRUDSPEC_PAGE_KEYS=1} \\')
    open(f, 'w', encoding='utf-8', newline='\n').write(s)
s = open('writes.sh', encoding='utf-8').read()
s = s.replace('SNAPDATA=/mnt/d/code/crudspec-run/bench-snap-nc\n', 'SNAPDATA=${SNAPDATA:-/mnt/d/code/crudspec-run/bench-snap-nc5}\n')
s = s.replace('ROOM=486777696\n', 'ROOM=486777696\nWROOM=${WROOM:-$ROOM}   # the room the writer posts into (another room the reader is in: 654632876)\n')
s = s.replace('"$COOKIE" "$ROOM" "$R" "$SECS"', '"$COOKIE" "$WROOM" "$R" "$SECS"')
s = s.replace('cache() { inns curl -fsS http://127.0.0.1:6060/debug/cache; }',
              'cache() { inns curl -fsS http://127.0.0.1:6060/debug/cache 2>/dev/null || echo \'{"hits":0,"misses":0}\'; }')
s = s.replace('node "$REPO/bench/post-writer.mjs"', 'node /mnt/d/code/burin/bench/post-writer.mjs')
s = s.replace('note "ready run=$RUN conns=$CONNS', 'note "ready run=$RUN gobin=$GOBIN page_keys=${PAGE_KEYS:-} wroom=$WROOM conns=$CONNS')
open('writes.sh', 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
