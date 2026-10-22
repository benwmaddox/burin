// Posts chat messages at a steady rate, for the write benchmark (bench/crudspec-writes.sh).
//   node bench/post-writer.mjs <base> <cookie> <room> <posts-per-second> <seconds>
// Prints {posted, ok, p50_ms, p99_ms}: each post's time to its answer (a redirect when it worked).
const [base, cookie, room, rate, secs] = process.argv.slice(2);
const per = 1000 / Number(rate), end = Date.now() + Number(secs) * 1000;
const times = []; let posted = 0, ok = 0, n = 0;
const post = async () => {
  const t0 = performance.now();
  const r = await fetch(`${base}/_do/post_message`, {
    method: "POST", redirect: "manual",
    headers: { Cookie: cookie, "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ room, body: `benchmark message ${++n}`, client_id: `bench-${process.pid}-${n}` }),
  }).catch(() => null);
  times.push(performance.now() - t0); posted++;
  if (r && r.headers.get("X-Crudspec-Outcome") === "ok") ok++;
};
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
console.log(JSON.stringify({ posted, ok, p50_ms: pct(0.5), p99_ms: pct(0.99) }));
