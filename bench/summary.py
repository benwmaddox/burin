"""Tables from a driver's runs: python bench/summary.py <tag>  (e.g. e1)

Reads: each server's best window (by req/s) per pass, with its CPU per request; then the mean of
the passes. Writes: per writer rate, reads/s, CPU per read, the post latency and the cache's hit
rate, as the mean of the passes."""
import glob, json, os, sys
from collections import defaultdict

tag = sys.argv[1]
root = os.path.join(os.path.dirname(__file__), 'runs')

def rows(run):
    p = os.path.join(root, run, 'summary.jsonl')
    if not os.path.exists(p):
        return []
    out = []
    for line in open(p, encoding='utf-8'):
        line = line.strip()
        if line.startswith('{'):
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return out

def pss(run):
    for f in ('pss.txt',):
        p = os.path.join(root, run, f)
        if os.path.exists(p):
            for line in open(p):
                if 'PSS' in line:
                    return line.strip().split('=')[-1]
    return '?'

runs = sorted(os.path.basename(d) for d in glob.glob(os.path.join(root, f'{tag}-p*')))
reads = defaultdict(list)
writes = defaultdict(lambda: defaultdict(list))
for run in runs:
    _, p, kind, name = run.split('-', 3)
    rs = rows(run)
    if kind == 'reads':
        cells = [r for r in rs if 'conns' in r and r.get('ok')]
        if cells:
            best = max(cells, key=lambda r: r['rps'])
            reads[name].append((p, best, pss(run)))
    else:
        for r in rs:
            if 'posts_per_s' in r:
                writes[(kind, name)][r['posts_per_s']].append(r)

print(f'## Reads ({tag}): best window per pass')
print('| server | pass | req/s | CPU ms/req | conns | errors | invalid | PSS |')
print('|---|---|---:|---:|---:|---:|---:|---:|')
for name, lst in sorted(reads.items()):
    for p, b, m in lst:
        print(f"| {name} | {p} | {b['rps']:,.0f} | {b['cpu_ms_per_req']:.3f} | {b['conns']} | {b.get('errors')} | {b.get('invalid')} | {m} |")
print()
for (kind, name), by_rate in sorted(writes.items()):
    print(f'## Reads under writes: {kind} room, {name}')
    print('| posts/s | reads/s | CPU ms/read | read p99 ms | post p50 ms | post p99 ms | posts ok | cache hit |')
    print('|---:|---:|---:|---:|---:|---:|---:|---:|')
    for rate in sorted(by_rate):
        lst = by_rate[rate]
        m = lambda k: sum((r.get(k) or 0) for r in lst) / len(lst)
        print(f"| {rate} | {m('reads_per_s'):,.0f} | {m('cpu_ms_per_read'):.3f} | {m('read_p99_ms'):.1f} | {m('post_p50_ms'):.1f} | {m('post_p99_ms'):.1f} | {sum(r.get('posts_ok') or 0 for r in lst)}/{sum(r.get('posts') or 0 for r in lst)} | {m('cache_hit_rate'):.2f} |")
    print()
