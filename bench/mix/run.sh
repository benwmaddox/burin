#!/bin/bash
# The mixed benchmark for one implementation: many people reading many rooms at different rates
# while some post (tools/mixload), on the mixed data (bench/mix/build-data.sh). The server runs
# alone in its own network namespace and cgroup, pinned to SRV_CPUS (4 vCPUs); the load on
# LG_CPUS. Phases: a cold start (the first seconds after start), then steady reads at each post
# rate. Reports reads/s, CPU per read, read and post latency, memory, and etch's or crudspec's
# cache figures. Holds the shared bench lock.
# Run as root:
#   wsl -d Ubuntu -u root -- bash /mnt/d/code/etch/bench/mix/run.sh <etch|crudspec|rust> <run> [secs] [rates…]
set -euo pipefail
[ -n "${BENCH_LOCKED:-}" ] || { exec 9>>/home/ben/linuxbench/bench.lock; flock -x 9; }
APP=$1; RUN=$2; SECS=${3:-15}; shift 2; [ $# -gt 0 ] && shift; RATES=("$@"); [ ${#RATES[@]} -eq 0 ] && RATES=(0 10 50)
CONNS=${CONNS:-64}
U=ben; UH=/home/$U; L=$UH/linuxbench; M=$UH/etch-mix
FIX=/mnt/d/code/AgentLang/.agentlang/once-campfire-verification/fixtures/default
OUT=/mnt/d/code/etch/bench/runs/mix-$RUN; W=$UH/etch-bench/mix-$RUN
NS=etchmix; CG=/sys/fs/cgroup/etchmix-srv
SRV_CPUS=${SRV_CPUS:-8-11}; LG_CPUS=${LG_CPUS:-12-19}
MIX=$UH/etch-target-mixload/release/mixload
LGBIN=$UH/cm258/loadgen-linux/release/loadgen
case "$APP" in etch|crudspec|rust) ;; *) echo "app: etch, crudspec or rust"; exit 64;; esac
test ! -e "$OUT" || { echo "run $OUT exists"; exit 2; }
test ! -e "$CG" || { echo "cgroup $CG exists"; exit 2; }
mkdir -p "$OUT"
note() { echo "$(date -u +%FT%T.%3NZ) $*" | tee -a "$OUT/driver.log"; }
asu() { setpriv --reuid=$U --regid=$U --init-groups "$@"; }
inns() { nsenter --net=/run/netns/$NS "$@"; }
LG() { inns taskset -c "$LG_CPUS" setpriv --reuid=$U --regid=$U --init-groups "$@"; }

# a fresh copy of the data
asu rm -rf "$W"; asu mkdir -p -m 700 "$W"
ENVV=(HOME=$UH TZ=America/New_York)
case "$APP" in
  etch) asu cp -a "$M/etch" "$W/data"; CMD=(${ETCH:-$UH/hw/etch-gen-release} --data "$W/data" --assets /mnt/d/code/etch/apps/campfire/assets --port 13774) ;;
  crudspec) asu cp -a "$M/crudspec" "$W/data"; CMD=(${CRUDSPEC:-$UH/hw/campfire-pk} --data "$W/data" --assets /mnt/d/code/etch/apps/campfire/assets --port 13774) ;;
  rust)
    asu mkdir -p "$W/storage/db" "$W/storage/logs"; asu cp "$M/rails.sqlite3" "$W/storage/db/production.sqlite3"; asu cp -r "$FIX/storage" "$W/storage/files"
    ENVV+=(LANG=C.UTF-8 USER=$U RAILS_ENV=production PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin)
    # the fixture's environment (its secrets stay in the process: never printed)
    while IFS= read -r line || [ -n "$line" ]; do case "$line" in ''|'#'*) continue;; esac; ENVV+=("$line"); done < "$FIX/reference.env"
    ENVV+=(HTTP_PORT=13774 TARGET_PORT=13775)
    CMD=($L/rust/bin/campfire server) ;;
esac

cleanup() {
  if [ -e "$CG" ]; then
    [ -n "${BOOT:-}" ] && kill -TERM "$BOOT" 2>/dev/null || true
    sleep 1; [ -s "$CG/cgroup.procs" ] && echo 1 > "$CG/cgroup.kill"
    for i in $(seq 1 50); do grep -q "populated 0" "$CG/cgroup.events" && break; sleep 0.1; done
    rmdir "$CG" 2>/dev/null || true
  fi
  ip netns del $NS 2>/dev/null || true
  [ -f "$W/server.log" ] && grep -viE 'session_token|cookie|secret|password' "$W/server.log" | tail -200 > "$OUT/server.log" || true
}
trap cleanup EXIT
ip netns add $NS; ip -n $NS link set lo up
mkdir "$CG"
( cd "$W"; echo $BASHPID > "$CG/cgroup.procs"
  exec nsenter --net=/run/netns/$NS taskset -c "$SRV_CPUS" setpriv --reuid=$U --regid=$U --init-groups env -i "${ENVV[@]}" "${CMD[@]}" ) \
  > "$W/server.log" 2>&1 &
BOOT=$!
for i in $(seq 1 1200); do inns curl -fsS -o /dev/null http://127.0.0.1:13774/up 2>/dev/null && break; sleep 0.1; done
inns curl -fsS -o /dev/null http://127.0.0.1:13774/up || { note "not ready"; tail -20 "$W/server.log"; exit 3; }
note "ready app=$APP run=$RUN conns=$CONNS secs=$SECS rates=${RATES[*]} server_cpus=$SRV_CPUS loadgen_cpus=$LG_CPUS"

# Rails-style apps: everyone signs in (a cookie each) and gets a CSRF token for posting
COOKIES=()
if [ "$APP" = rust ]; then
  asu node -e '
    const p = require(process.argv[1]);
    for (const x of p.people) console.log([x.id, x.email, x.password, x.rooms[0]].join("\t"));
  ' "$M/people.json" > "$W/signins.tsv"
  # one JSON field of a loadgen answer ("" when there's none)
  field() { node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{process.stdout.write(JSON.parse(s)[process.argv[1]]||"")}catch{}})' "$1"; }
  # each person's session cookie, signed as Rails signs it (their session is in the data; signing
  # in through the app is limited to 10 people per 3 minutes from one address)
  asu node /mnt/d/code/etch/bench/mix/rails-cookie.mjs make "$FIX/reference.env" "$M/people.json" "$W/storage/db/production.sqlite3" sha256 1000 64 sha1 2>/dev/null > "$W/signed.json"
  echo "{" > "$W/cookies.json"; first=1; n=0; failed=
  while IFS=$'\t' read -r id email pass room; do
    c=$(node -e 'const s=require(process.argv[1]); process.stdout.write((s[process.argv[2]]||{}).cookie||"")' "$W/signed.json" "$id")
    [ -n "$c" ] || continue
    t=$( (LG "$LGBIN" scrape --base http://127.0.0.1:13774 --cookie "$c" --room "$room" 2>/dev/null || true) | field csrf || true)
    [ $first = 1 ] || echo "," >> "$W/cookies.json"; first=0; n=$((n+1))
    node -e 'process.stdout.write(JSON.stringify(process.argv[1]) + ":" + JSON.stringify({cookie: process.argv[2], csrf: process.argv[3]}))' "$id" "$c" "$t" >> "$W/cookies.json"
  done < "$W/signins.tsv"
  echo "}" >> "$W/cookies.json"; chown $U "$W/cookies.json"; chmod 600 "$W/cookies.json"
  COOKIES=(--cookies "$W/cookies.json")
  note "signed in: $n people"
fi

cg_usec() { awk '$1=="usage_usec"{print $2}' "$CG/cpu.stat"; }
stats() { case "$APP" in etch) inns curl -fsS http://127.0.0.1:13774/_etch/stats 2>/dev/null || echo '{}';; *) echo '{}';; esac; }
NCPU=$(taskset -c "$SRV_CPUS" nproc)
phase() { # name rate secs
  local S0 C0 S1 C1
  S0=$(cg_usec); C0=$(stats)
  LG "$MIX" --base http://127.0.0.1:13774 --people "$M/people.json" "${COOKIES[@]}" --app "$APP" --conc "$CONNS" --duration "$3" --post-rate "$2" --seed "${SEED:-1}" > "$OUT/$1.json"
  S1=$(cg_usec); C1=$(stats)
  node -e '
    const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const s0 = JSON.parse(process.argv[2] || "{}"), s1 = JSON.parse(process.argv[3] || "{}");
    const cpu = (Number(process.argv[4]) - Number(process.argv[5])) / 1e6, secs = r.secs, ok = r.reads.ok;
    const d = (k) => (s1[k] ?? 0) - (s0[k] ?? 0);
    const etch = s1.fast !== undefined ? { fast_share: +(d("fast") / Math.max(1, d("fast") + d("cold"))).toFixed(3), write_reads: d("write_reads"), cleared: d("cleared") } : {};
    console.log(JSON.stringify({ phase: process.argv[6], posts_per_s: Number(process.argv[7]), reads_per_s: r.reads.per_s, reads_invalid: r.reads.invalid, read_errors: r.reads.errors,
      cpu_ms_per_read: +(cpu * 1000 / Math.max(1, ok)).toFixed(3), server_util: +(cpu / (secs * Number(process.argv[8]))).toFixed(2),
      read_p50_ms: r.reads.p50_ms, read_p99_ms: r.reads.p99_ms, avg_bytes: r.reads.avg_bytes, rooms_read: r.reads.rooms_read, people_reading: r.reads.people_reading,
      posts_ok: r.posts.ok, posts_sent: r.posts.sent, post_p50_ms: r.posts.p50_ms, post_p99_ms: r.posts.p99_ms, ...etch }));
  ' "$OUT/$1.json" "$C0" "$C1" "$S1" "$S0" "$1" "$2" "$NCPU" | tee -a "$OUT/summary.jsonl"
}
phase cold 0 "$SECS"
for R in "${RATES[@]}"; do phase "steady-$R" "$R" "$SECS"; done
for pid in $(cat "$CG/cgroup.procs"); do awk '/^Pss:/{print $2}' /proc/$pid/smaps_rollup 2>/dev/null; done | awk '{t+=$1} END{printf "TOTAL_PSS_MB=%.1f\n", t/1024}' | tee -a "$OUT/pss.txt"
note "done"
