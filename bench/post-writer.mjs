// Posts chat messages at a steady rate, for the write benchmark (bench/crudspec-writes.sh).
//   node bench/post-writer.mjs <base> <cookie> <room> <posts-per-second> <seconds>
// Prints {posted, ok, p50_ms, p99_ms, slow}: each post's time to its answer (a redirect when it
// worked), and the slow ones (over 20 ms) with when they were sent.
const [base, cookie, room, rate, secs] = process.argv.slice(2);
const per = 1000 / Number(rate), end = Date.now() + Number(secs) * 1000;
const times = [], slow = []; let posted = 0, ok = 0, n = 0;
const start = performance.now();
const post = async () => {
  const t0 = performance.now(), n0 = n + 1;
  const r = await fetch(`${base}/_do/post_message`, {
    method: "POST", redirect: "manual",
    headers: { Cookie: cookie, "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ room, body: `benchmark message ${++n}`, client_id: `bench-${process.pid}-${n}` }),
  }).catch(() => null);
  const ms = performance.now() - t0;
  times.push(ms); posted++;
  if (ms > 20) slow.push([n0, +(t0 - start).toFixed(0), +ms.toFixed(1)]);
  // burin's outcome header, or the reference baseline's
  if (r && (r.headers.get("X-Burin-Outcome") ?? r.headers.get("X-Crudspec-Outcome")) === "ok") ok++;
};
// one post first, untimed: the client's own warm-up (its first connection, its JIT) isn't the
// server's latency (it showed as every server's p99 at low rates)
if (Number(rate) > 0) {
  await fetch(`${base}/_do/post_message`, { method: "POST", redirect: "manual", headers: { Cookie: cookie, "Content-Type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ room, body: "warm-up" }) }).catch(() => null);
}
const pending = [];
if (Number(rate) > 0) {
  for (let next = Date.now(); next < end; next += per) {
    const wait = next - Date.now();
    if (wait > 0) await new Promise((res) => setTimeout(res, wait));
    pending.push(post());
  }
} else await new Promise((res) => setTimeout(res, Number(secs) * 1000));
await Promise.all(pending);
times.sort((a, b) => a - b);
const pct = (p) => (times.length ? +times[Math.min(times.length - 1, Math.floor(times.length * p))].toFixed(1) : null);
// slow: [post number, ms from the start, ms taken] for each post over 20 ms
console.log(JSON.stringify({ posted, ok, p50_ms: pct(0.5), p99_ms: pct(0.99), slow: slow.slice(0, 20) }));
