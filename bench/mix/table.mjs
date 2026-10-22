// The mixed benchmark's table: per run and phase, reads/s, CPU per read, read p50/p99, posts and
// their p50/p99, page bytes, memory.  node bench/mix/table.mjs <runs dir> <prefix>
import { readdirSync, readFileSync, existsSync } from "node:fs";
const [dir, prefix] = process.argv.slice(2);
const n = (x, d = 0) => (x == null ? "-" : Number(x).toFixed(d));
console.log(["run".padEnd(24), "phase".padEnd(10), "reads/s".padStart(9), "ms CPU".padStart(7), "p50".padStart(6), "p99".padStart(7), "posts".padStart(6), "post p50".padStart(9), "post p99".padStart(9), "KB".padStart(5), "rooms".padStart(5), "PSS MB".padStart(7)].join(" "));
for (const run of readdirSync(dir).filter((d) => d.startsWith(prefix)).sort()) {
  const f = `${dir}/${run}/summary.jsonl`;
  if (!existsSync(f)) continue;
  const pss = existsSync(`${dir}/${run}/pss.txt`) ? readFileSync(`${dir}/${run}/pss.txt`, "utf8").match(/TOTAL_PSS_MB=([\d.]+)/)?.[1] : null;
  for (const line of readFileSync(f, "utf8").trim().split("\n")) {
    const r = JSON.parse(line);
    console.log([run.slice(prefix.length).padEnd(24), r.phase.padEnd(10), n(r.reads_per_s).padStart(9), n(r.cpu_ms_per_read, 3).padStart(7), n(r.read_p50_ms, 2).padStart(6),
      n(r.read_p99_ms, 1).padStart(7), `${r.posts_ok}/${r.posts_sent}`.padStart(6), n(r.post_p50_ms, 1).padStart(9), n(r.post_p99_ms, 1).padStart(9),
      n(r.avg_bytes / 1024).padStart(5), String(r.rooms_read).padStart(5), (pss ?? "-").padStart(7)].join(" "));
  }
}
