#!/usr/bin/env python3
# One-off: the language's rename, etch -> Burin (etch-lang.org is another language). Whole words
# only (`fetch`, `sketch` stay), in the repository's own text; vendored assets, past benchmark
# runs, data copies and exported patches are left alone. Paths are renamed with `git mv`.
#   python3 scripts/rename-to-burin.py        (from the repository's root)
import os, re, subprocess

SKIP = re.compile(r'^(bench/runs/|apps/campfire/assets/|data/campfire|bench/rust-opt/patches/|tools/browser/node_modules/)')
BINARY = re.compile(r'\.(png|gif|webp|mp3|jpg|ico|woff2?|sqlite|lock)$')

# in order: compound names first, then prefixes, then the word itself
RULES = [
    (r'libetchapp', 'libburinapp'),
    (r'libetchext', 'libburinext'),
    (r'\betchapp\b', 'burinapp'),
    (r'\betchext\b', 'burinext'),
    (r'\betchmix\b', 'burinmix'),
    (r'\betchReading', 'burinReading'),
    (r'\betchc\b', 'burinc'),
    (r'\bETCH_', 'BURIN_'),
    (r'\betch_', 'burin_'),
    (r'(?<=_)etch\b', 'burin'),
    (r'\bETCH\b', 'BURIN'),
    (r'\bEtch\b', 'Burin'),
    (r'\betch\b', 'burin'),
]

def renamed(text):
    for a, b in RULES:
        text = re.sub(a, b, text)
    return text

files = subprocess.run(['git', 'ls-files'], capture_output=True, text=True, check=True).stdout.splitlines()
changed = 0
for f in files:
    if SKIP.match(f) or BINARY.search(f) or not os.path.isfile(f):
        continue
    try:
        s = open(f, encoding='utf-8', newline='').read()
    except UnicodeDecodeError:
        continue
    t = renamed(s)
    if t != s:
        open(f, 'w', encoding='utf-8', newline='').write(t)
        changed += 1
# Cargo.lock: the two crates' names
s = open('Cargo.lock', encoding='utf-8', newline='').read()
open('Cargo.lock', 'w', encoding='utf-8', newline='').write(s.replace('"etch-rt"', '"burin-rt"').replace('"etchc"', '"burinc"'))
print('files changed:', changed)

# paths: directories first (deepest last so parents move once)
moves = {}
for f in files:
    if f.startswith('bench/runs/') or f.startswith('tools/browser/node_modules/'):
        continue
    n = renamed(f)
    if n != f:
        moves[f] = n
dirs = sorted({(os.path.dirname(a), os.path.dirname(b)) for a, b in moves.items() if os.path.dirname(a) != os.path.dirname(b)})
done_dirs = set()
for a, b in dirs:
    # the topmost directory that changes name
    pa, pb = a.split('/'), b.split('/')
    for i in range(len(pa)):
        if pa[i] != pb[i]:
            top_a, top_b = '/'.join(pa[:i + 1]), '/'.join(pb[:i + 1])
            break
    if top_a not in done_dirs and os.path.isdir(top_a):
        subprocess.run(['git', 'mv', top_a, top_b], check=True)
        done_dirs.add(top_a)
        print('moved', top_a, '->', top_b)
for a, b in moves.items():
    # a file whose own name changes (its directory may have moved already)
    cur = a
    for d in done_dirs:
        if cur.startswith(d + '/'):
            cur = renamed(d) + cur[len(d):]
    if cur != b and os.path.exists(cur):
        os.makedirs(os.path.dirname(b) or '.', exist_ok=True)
        subprocess.run(['git', 'mv', cur, b], check=True)
        print('moved', cur, '->', b)
