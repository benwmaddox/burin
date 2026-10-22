# One server lifetime under the fair-benchmark setup (AgentLang report 282's protocol):
# the server in its own network namespace (loopback only) and cgroup, pinned to SRV_CPUS; the
# load generator pinned to LG_CPUS; every response validated against the canonical room_show
# contract. Sweeps the given connection counts, one measured window each.
# Run as root:
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/etch/bench/lifetime.sh <run-name> [secs] [conns...]
set -euo pipefail
[ -n "${BENCH_LOCKED:-}" ] || { exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9; }   # the bench lock the sessions share (a driver may hold it already)
RUN=$1; SECS=${2:-10}; shift; [ $# -gt 0 ] && shift; CONNS=("$@"); [ ${#CONNS[@]} -eq 0 ] && CONNS=(16 32 64)
U=ben; UH=/home/$U
REPO=/mnt/d/code/etch
CONTRACT=/mnt/d/code/etch/tools/room_show.json
SNAPDATA=${SNAPDATA:-/mnt/d/code/etch/data/campfire}    # made by bench/import-rails.ts; nc4: cleaned of the 6 rows the spec refuses, storage format 4; nc5: nc4 migrated to the schema with app invariants (19 Oct)
ROOM=486777696                                                    # rooms.watercooler in the snapshot
OUT=/mnt/d/code/etch/bench/runs/$RUN; W=$UH/etch-bench/$RUN
NS=etchbench; CG=/sys/fs/cgroup/etchbench-srv
SRV_CPUS=${SRV_CPUS:-8-11}; LG_CPUS=${LG_CPUS:-12-19}
LGBIN=/mnt/d/code/etch/tools/loadgen
test -n "${GOBIN:-}" || { echo "GOBIN=<the compiled app> is required"; exit 64; }
test ! -e "$OUT" || { echo "run $RUN exists"; exit 2; }
test ! -e "$CG" || { echo "cgroup $CG exists"; exit 2; }
mkdir -p "$OUT"
note() { echo "$(date -u +%FT%T.%3NZ) $*" | tee -a "$OUT/driver.log"; }
asu() { setpriv --reuid=$U --regid=$U --init-groups "$@"; }
inns() { nsenter --net=/run/netns/$NS "$@"; }

# a fresh copy of the imported snapshot on ext4
asu rm -rf "$W"; asu mkdir -p "$W"; asu cp -a "$SNAPDATA" "$W/data"; rm -f "$W/data/app.lock"
# another app, or page: PAGE, COOKIE, ASSETS, and NO_VALIDATE=1 (the contract is Campfire's room page)
if [ -z "${COOKIE:-}" ]; then
  TOKEN=$(asu node -e "const {DatabaseSync}=require('node:sqlite');console.log(new DatabaseSync('$W/data/app.sqlite').prepare('select token from Session where id = 481019661').get().token)" 2>/dev/null)
  COOKIE="session_token=$TOKEN"
fi
PAGE=${PAGE:-/rooms/$ROOM}; ASSETS=${ASSETS:-/mnt/d/code/etch/apps/campfire/assets}
VAL=(--validate "$CONTRACT"); [ -n "${NO_VALIDATE:-}" ] && VAL=()

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
mkdir "$CG"; echo "$SRV_CPUS" > "$CG/cpuset.cpus"   # a hard pin: a process that resets its affinity stays on these
( cd "$REPO"; echo $BASHPID > "$CG/cgroup.procs"
  # the compiled app (lang/gogen): GOBIN=<binary>
  exec nsenter --net=/run/netns/$NS taskset -c "$SRV_CPUS" setpriv --reuid=$U --regid=$U --init-groups env -i HOME=$UH TZ=America/New_York ${NO_CACHE:+CRUDSPEC_NO_CACHE=1} ${NO_PAGE_CACHE:+CRUDSPEC_NO_PAGE_CACHE=1} ${X_NORICH:+X_NORICH=1} ${PRUNE:+CRUDSPEC_PRUNE=$PRUNE} ${GOGC:+GOGC=$GOGC} ${PAGE_KEYS:+CRUDSPEC_PAGE_KEYS=1} ${GOMEMLIMIT:+GOMEMLIMIT=$GOMEMLIMIT} \
    "$GOBIN" --data "$W/data" --assets "$ASSETS" --port 13774 ) \
  > "$W/server.log" 2>&1 &
BOOT=$!
for i in $(seq 1 600); do inns curl -fsS -o /dev/null http://127.0.0.1:13774/up 2>/dev/null && break; sleep 0.1; done
inns curl -fsS -o /dev/null http://127.0.0.1:13774/up || { note "not ready"; tail -20 "$W/server.log"; exit 3; }
note "ready run=$RUN no_cache=${NO_CACHE:-} no_page_cache=${NO_PAGE_CACHE:-} prune=${PRUNE:-on} gogc=${GOGC:-} gomemlimit=${GOMEMLIMIT:-} procs=$(wc -l < "$CG/cgroup.procs") server_cpus=$SRV_CPUS loadgen_cpus=$LG_CPUS node=$(node --version)"
LG() { inns taskset -c "$LG_CPUS" setpriv --reuid=$U --regid=$U --init-groups "$LGBIN" "$@"; }
LG fetch --base http://127.0.0.1:13774 --cookie "$COOKIE" --path "$PAGE" --out "$W/body.html" > /dev/null
note "decoded_bytes=$(stat -c %s "$W/body.html")"
# EXPECT_BYTES: the page must be exactly this size (the contract checks the messages, not the rest of the page)
if [ -n "${EXPECT_BYTES:-}" ] && [ "$(stat -c %s "$W/body.html")" != "$EXPECT_BYTES" ]; then note "page is $(stat -c %s "$W/body.html") bytes, expected $EXPECT_BYTES"; exit 4; fi
cg_usec() { awk '$1=="usage_usec"{print $2}' "$CG/cpu.stat"; }
LG http --base http://127.0.0.1:13774 --cookie "$COOKIE" --path "$PAGE" "${VAL[@]}" --conc 4 --duration ${WARMUP:-10} --gzip ${GZIP:-0} > "$OUT/warmup.json"
note "warmup $(cut -c1-300 "$OUT/warmup.json")"
NCPU=$(taskset -c "$SRV_CPUS" nproc); NLG=$(taskset -c "$LG_CPUS" nproc)
for C in "${CONNS[@]}"; do
  S0=$(cg_usec)
  /usr/bin/time -f "%U %S" -o "$W/client-$C.txt" \
    nsenter --net=/run/netns/$NS taskset -c "$LG_CPUS" setpriv --reuid=$U --regid=$U --init-groups "$LGBIN" http --base http://127.0.0.1:13774 --cookie "$COOKIE" --path "$PAGE" "${VAL[@]}" --conc "$C" --duration "$SECS" --gzip ${GZIP:-0} > "$OUT/cell-$C.json"
  S1=$(cg_usec)
  node -e '
    const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const [u, s] = require("fs").readFileSync(process.argv[2], "utf8").trim().split(" ").map(Number);
    const cpu = (Number(process.argv[3]) - Number(process.argv[4])) / 1e6, secs = r.secs ?? Number(process.argv[5]);   // as long as the load generator ran: past the window for queued requests
    const ok = r.ok ?? 0;
    console.log(JSON.stringify({ conns: Number(process.argv[6]), ok, errors: r.errors ?? r.failed ?? null, invalid: r.invalid ?? null, rps: +(ok / secs).toFixed(1),
      cpu_ms_per_req: +(cpu * 1000 / Math.max(1, ok)).toFixed(3), server_util: +(cpu / (secs * Number(process.argv[7]))).toFixed(2),
      client_util: +((u + s) / (secs * Number(process.argv[8]))).toFixed(2), p50: r.p50_ms ?? r.latency?.p50 ?? null, p99: r.p99_ms ?? r.latency?.p99 ?? null }));
  ' "$OUT/cell-$C.json" "$W/client-$C.txt" "$S1" "$S0" "$SECS" "$C" "$NCPU" "$NLG" | tee -a "$OUT/summary.jsonl"
done
for pid in $(cat "$CG/cgroup.procs"); do awk '/^Pss:/{print $2}' /proc/$pid/smaps_rollup 2>/dev/null; done | awk '{t+=$1} END{printf "TOTAL_PSS_MB=%.1f\n", t/1024}' | tee -a "$OUT/pss.txt"
note "done"
