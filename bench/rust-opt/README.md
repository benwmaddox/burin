# An optimized variant of the Rust port of Campfire

The mixed benchmark (`bench/mix`) compares burin with [once-campfire-rust], Basecamp's Rust port of
Campfire. burin was leaner, mostly from how it caches, not from the language. This variant takes
those ideas where they fit idiomatic Rust, to compare burin with a tuned Rust app as well as the
published one.

`patches/` are the variant's commits, on top of once-campfire-rust `6dae2fd` (MIT, 37signals;
`MIT-LICENSE`). The working copy is `D:\code\campfire-rust-opt`, branch `optimized`:

    git clone https://github.com/basecamp/once-campfire-rust campfire-rust-opt && cd campfire-rust-opt
    git checkout -b optimized 6dae2fd && git am /path/to/burin/bench/rust-opt/patches/*.patch
    # reference/: the submodule (or a copy of it); then, in WSL:
    bash /mnt/d/code/burin/bench/mix/build-rust-opt.sh    # -> ~/linuxbench/rust-opt/bin/campfire

The benchmark runs it as `rustopt` (`bench/mix/run.sh`, `drive.sh`).

## What it changes, and what each change measured

Mixed load (117 people, 60 rooms, the browser model), reads/s on the bench cores, A/B runs in
the order A B B A (`bench/mix/ab-rustopt.sh`, `ab-rustopt-bin.sh`):

| Change | No writes | 50 posts/s |
| --- | --- | --- |
| The published port | ~29k | ~1.6k |
| Caches versioned by what each write changes (SQLite's update hook, `crates/db/src/changes.rs`), not by `PRAGMA data_version` | ~33k | ~15k |
| Unread as counts: a post writes the room and its poster, not every member's row (as burin) | same | same (post p50 3.9 -> 3.3 ms) |
| A page's first message compressed on its own (it was a different gzip piece for every viewer) | +5% | +18% |
| The layout's import map and stylesheet links as shared fragments | same | +0-5% (noisy) |
| A session refresh no longer drops cached pages | same | ~16.5k -> ~19k |

Tried and not kept: bigger response or gzip-piece budgets (more memory, no gain), compressing the
layout texts on their own (no gain, larger pages), a lower log level (no difference).

## Correctness

The port's own tests pass (`cargo test --workspace --exclude html5ever`; the integration tests
that need Docker's seed data skip). `bench/mix/rustopt-verify.sh` runs the variant against itself
with its caches off, on one database: random posts, edits, deletes, boosts, renames, involvement
and profile changes go to the cached server, and after each the same pages (room pages, sidebars)
from both servers must be the same bytes. It passes (3 seeds, ~4,200 pages), and it fails a
deliberately broken build (`build-rust-opt-mutant.sh`: a message that doesn't move its room's
version).

[once-campfire-rust]: https://github.com/basecamp/once-campfire-rust
