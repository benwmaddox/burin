# The room page under reads and writes: the fair-benchmark setup of lifetime.sh (the
# server in its own network namespace and cgroup, pinned to SRV_CPUS; the clients on LG_CPUS),
# with readers loading the room page at CONNS connections while a writer posts messages to the
# same room at each rate. Pages change as messages arrive, so they are checked for status, not
# against the read-only contract. Reports reads/s, CPU per read, the writes' latency, and the
# fragment cache's hit rate.
# Run as root:
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/burin/bench/writes.sh <run-name> [secs] [rates...]
set -euo pipefail
[ -n "${BENCH_LOCKED:-}" ] || { exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9; }   # the bench lock the sessions share (a driver may hold it already)
RUN=$1; SECS=${2:-10}; shift; [ $# -gt 0 ] && shift; RATES=("$@"); [ ${#RATES[@]} -eq 0 ] && RATES=(0 1 5 20 50)
CONNS=${CONNS:-32}
U=ben; UH=/home/$U
REPO=/mnt/d/code/burin
SNAPDATA=${SNAPDATA:-/mnt/d/code/burin/data/campfire}
ROOM=486777696
WROOM=${WROOM:-$ROOM}   # the room the writer posts into (another room the reader is in: 654632876)
OUT=/mnt/d/code/burin/bench/runs/$RUN; W=$UH/burin-bench/$RUN
NS=burinbench; CG=/sys/fs/cgroup/burinbench-srv
SRV_CPUS=${SRV_CPUS:-8-11}; LG_CPUS=${LG_CPUS:-12-19}
LGBIN=/mnt/d/code/burin/tools/loadgen
test -n "${GOBIN:-}" || { echo "GOBIN=<the compiled app> is required"; exit 64; }
test ! -e "$OUT" || { echo "run $RUN exists"; exit 2; }
test ! -e "$CG" || { echo "cgroup $CG exists"; exit 2; }
mkdir -p "$OUT"
note() { echo "$(date -u +%FT%T.%3NZ) $*" | tee -a "$OUT/driver.log"; }
asu() { setpriv --reuid=$U --regid=$U --init-groups "$@"; }
inns() { nsenter --net=/run/netns/$NS "$@"; }

asu rm -rf "$W"; asu mkdir -p "$W"; asu cp -a "$SNAPDATA" "$W/data"; rm -f "$W/data/app.lock"
TOKEN=$(asu node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$W/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
COOKIE="session_token=$TOKEN"

cleanup() {
  if [ -e "$CG" ]; then
    [ -n "${BOOT:-}" ] && kill -TERM "$BOOT" 2>/dev/null || true
    sleep 1; [ -s "$CG/cgroup.procs" ] && echo 1 > "$CG/cgroup.kill"
    for i in $(seq 1 50); do grep -q "populated 0" "$CG/cgroup.events" && break; sleep 0.1; done
    rmdir "$CG" 2>/dev/null || true
  fi
  ip netns del $NS 2>/dev/null || true
  [ -f "$W/server.log" ] && grep -v 'session_token' "$W/server.log" > "$OUT/server.log" || true
}
trap cleanup EXIT

ip netns add $NS; ip -n $NS link set lo up
mkdir "$CG"
( cd "$REPO"; echo $BASHPID > "$CG/cgroup.procs"
  exec nsenter --net=/run/netns/$NS taskset -c "$SRV_CPUS" setpriv --reuid=$U --regid=$U --init-groups env -i HOME=$UH TZ=America/New_York CRUDSPEC_PPROF=6060 ${PAGE_KEYS:+CRUDSPEC_PAGE_KEYS=1} \
    "$GOBIN" --data "$W/data" --assets /mnt/d/code/burin/apps/campfire/assets --port 13774 ) \
  > "$W/server.log" 2>&1 &
BOOT=$!
for i in $(seq 1 600); do inns curl -fsS -o /dev/null http://127.0.0.1:13774/up 2>/dev/null && break; sleep 0.1; done
inns curl -fsS -o /dev/null http://127.0.0.1:13774/up || { note "not ready"; tail -20 "$W/server.log"; exit 3; }
note "ready run=$RUN gobin=$GOBIN page_keys=${PAGE_KEYS:-} wroom=$WROOM conns=$CONNS server_cpus=$SRV_CPUS loadgen_cpus=$LG_CPUS"
cg_usec() { awk '$1=="usage_usec"{print $2}' "$CG/cpu.stat"; }
cache() { inns curl -fsS http://127.0.0.1:6060/debug/cache 2>/dev/null || echo '{"hits":0,"misses":0}'; }
inns taskset -c "$LG_CPUS" setpriv --reuid=$U --regid=$U --init-groups "$LGBIN" http --base http://127.0.0.1:13774 --cookie "$COOKIE" --path "/rooms/$ROOM" --conc 4 --duration 3 --gzip ${GZIP:-0} > "$OUT/warmup.json"
NCPU=$(taskset -c "$SRV_CPUS" nproc)
for R in "${RATES[@]}"; do
  S0=$(cg_usec); C0=$(cache)
  inns taskset -c "$LG_CPUS" setpriv --reuid=$U --regid=$U --init-groups node /mnt/d/code/burin/bench/post-writer.mjs http://127.0.0.1:13774 "$COOKIE" "$WROOM" "$R" "$SECS" > "$W/writer-$R.json" &
  WP=$!
  inns taskset -c "$LG_CPUS" setpriv --reuid=$U --regid=$U --init-groups "$LGBIN" http --base http://127.0.0.1:13774 --cookie "$COOKIE" --path "/rooms/$ROOM" --conc "$CONNS" --duration "$SECS" --gzip ${GZIP:-0} > "$OUT/cell-$R.json"
  wait $WP; S1=$(cg_usec); C1=$(cache)
  cp "$W/writer-$R.json" "$OUT/writer-$R.json"
  node -e '
    const fs = require("fs");
    const r = JSON.parse(fs.readFileSync(process.argv[1], "utf8")), wr = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
    const c0 = JSON.parse(process.argv[3]), c1 = JSON.parse(process.argv[4]);
    const cpu = (Number(process.argv[5]) - Number(process.argv[6])) / 1e6, secs = Number(process.argv[7]);
    const ok = r.ok ?? 0, hits = c1.hits - c0.hits, misses = c1.misses - c0.misses;
    console.log(JSON.stringify({ posts_per_s: Number(process.argv[8]), reads_per_s: +(ok / secs).toFixed(1), read_errors: r.errors ?? null, statuses: r.statuses,
      cpu_ms_per_read: +(cpu * 1000 / Math.max(1, ok)).toFixed(3), server_util: +(cpu / (secs * Number(process.argv[9]))).toFixed(2),
      read_p50_ms: r.latency?.p50_ms ?? null, read_p99_ms: r.latency?.p99_ms ?? null,
      posts: wr.posted, posts_ok: wr.ok, post_p50_ms: wr.p50_ms, post_p99_ms: wr.p99_ms,
      cache_hit_rate: +(hits / Math.max(1, hits + misses)).toFixed(3), avg_page_bytes: r.avg_bytes ?? null }));
  ' "$OUT/cell-$R.json" "$OUT/writer-$R.json" "$C0" "$C1" "$S1" "$S0" "$SECS" "$R" "$NCPU" | tee -a "$OUT/summary.jsonl"
done
for pid in $(cat "$CG/cgroup.procs"); do awk '/^Pss:/{print $2}' /proc/$pid/smaps_rollup 2>/dev/null; done | awk '{t+=$1} END{printf "TOTAL_PSS_MB=%.1f\n", t/1024}' | tee -a "$OUT/pss.txt"
note "done"
