// A compact table of bench runs: node bench/table.js <runs dir> <tag prefix>
// Per run and write rate: reads/s, CPU ms per read, read p99, post p50/p99.
const fs = require('fs');
const [dir, tag] = process.argv.slice(2);
for (const run of fs.readdirSync(dir).filter((d) => d.startsWith(tag)).sort()) {
  const f = `${dir}/${run}/summary.jsonl`;
  if (!fs.existsSync(f)) continue;
  for (const line of fs.readFileSync(f, 'utf8').trim().split('\n')) {
    const r = JSON.parse(line);
    const n = (x, d = 0) => (x == null ? '-' : Number(x).toFixed(d));
    console.log([run.padEnd(18), String(r.posts_per_s).padStart(4), n(r.reads_per_s).padStart(8), n(r.cpu_ms_per_read, 3).padStart(6),
      n(r.read_p99_ms, 2).padStart(6), n(r.post_p50_ms, 1).padStart(6), n(r.post_p99_ms, 1).padStart(7)].join('  '));
  }
}
