#!/bin/bash
# The tape-out builds, each step on top of the last, copied to ~/hw for benchmarking:
#   etch-lto   fat LTO, one codegen unit, this CPU's instructions (-C target-cpu=native)
#   etch-pgo   + profile-guided optimization (LLVM's instrumented profile, from scripts/train.sh)
#   etch-bolt  + BOLT: the binary's code laid out by a profile of the same load
# Builds and training run on cores 0-7, away from the bench cores.
#   bash /mnt/d/code/etch/scripts/tapeout.sh
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
cd /mnt/d/code/etch
TOOLS=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
BOLT=/usr/lib/llvm-19/bin/llvm-bolt
P=/tmp/etch-pgo; rm -rf $P; mkdir -p $P
B="taskset -c 0-7 cargo build --profile tapeout -p campfire-hand -q"
NATIVE="-C target-cpu=native"
t() { local s=$(date +%s.%N); "$@"; echo "  $(echo "$(date +%s.%N) - $s" | bc | cut -c1-5) s"; }

echo "== lto (fat LTO, one codegen unit, native)"
t env CARGO_TARGET_DIR=$HOME/etch-target-lto RUSTFLAGS="$NATIVE" $B
cp $HOME/etch-target-lto/tapeout/campfire-hand ~/hw/etch-lto

echo "== pgo: instrumented build, training, merge"
t env CARGO_TARGET_DIR=$HOME/etch-target-pgogen RUSTFLAGS="$NATIVE -Cprofile-generate=$P" $B
LLVM_PROFILE_FILE="$P/etch-%p-%m.profraw" bash scripts/train.sh $HOME/etch-target-pgogen/tapeout/campfire-hand 20
$TOOLS/llvm-profdata merge -o $P/merged.profdata $P/*.profraw
ls -la $P/merged.profdata
echo "== pgo: optimized build (with relocations kept, for BOLT)"
t env CARGO_TARGET_DIR=$HOME/etch-target-pgo RUSTFLAGS="$NATIVE -Cprofile-use=$P/merged.profdata -C link-args=-Wl,--emit-relocs" $B
cp $HOME/etch-target-pgo/tapeout/campfire-hand ~/hw/etch-pgo

echo "== bolt: instrument, train, lay out"
rm -f /tmp/etch-bolt.fdata
$BOLT ~/hw/etch-pgo -instrument -instrumentation-file=/tmp/etch-bolt.fdata -o /tmp/etch-pgo.inst > /tmp/etch-bolt-inst.log 2>&1
bash scripts/train.sh /tmp/etch-pgo.inst 20
ls -la /tmp/etch-bolt.fdata
t $BOLT ~/hw/etch-pgo -o ~/hw/etch-bolt -data=/tmp/etch-bolt.fdata -reorder-blocks=ext-tsp -reorder-functions=cdsort \
  -split-functions -split-all-cold -split-eh -icf=1 -use-gnu-stack -dyno-stats > /tmp/etch-bolt.log 2>&1
grep -iE "BOLT-INFO: (basic block reordering|.*profile|.*functions)|taken branches|executed instructions" /tmp/etch-bolt.log | head -12
ls -la ~/hw/etch-1a ~/hw/etch-lto ~/hw/etch-pgo ~/hw/etch-bolt
