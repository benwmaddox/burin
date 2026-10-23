# Burin

*Burin was called etch until 23 Oct 2026; it was renamed because [etch-lang.org](https://etch-lang.org/) is another
language. A burin is the engraver's steel tool. Benchmark runs before the rename keep the old name.*

An experimental web app language designed like hardware. The compiler (Rust) reads a whole app's
spec, turns it into a dataflow netlist, and emits LLVM IR that is linked with a small runtime under
cross-language LTO. Pages are precomputed when data changes: a read looks up bytes already built
(and already gzipped) and writes them to the socket, with no allocation on that path; writes are
fixed rules run by one writer that update exactly the pieces they affect.

- [docs/DESIGN.md](docs/DESIGN.md): the principles, the efficiency rules the checker enforces,
  measurements, and when to stop.
- [docs/IR.md](docs/IR.md): the netlist and its lowering to LLVM.

## Layout

- `crates/burinc`: the compiler (`check`, `report`, `emit`).
- `crates/burin-rt`: the runtime every generated app links (HTTP fast path, snapshot, writer).
- `apps/campfire`, `apps/board`: two apps written in burin; `crates/campfire-app`,
  `crates/board-app` are their 3-line binaries.
- `scripts/`: building (`build-burin.sh`) and every correctness check (`check-all.sh`).
- `bench/`: benchmarks.

## Building and checking (Linux or WSL)

```
bash scripts/build-burin.sh release                              # Campfire
APP=board CRATE=board-app bash scripts/build-burin.sh release    # the board app
bash scripts/check-all.sh
```

The compiler is built with Rust 1.99 (`cargo +1.99.0`); the apps with stable. The checks need a
copy of a Campfire database in `data/campfire-golden/` (not in the repository).

## License

Apache License 2.0; see [LICENSE](LICENSE) and [NOTICE](NOTICE). The Campfire app reimplements
ONCE Campfire by 37signals (MIT).
