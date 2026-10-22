#!/bin/bash
# Builds Campfire from its spec: etchc emits the registers' LLVM IR; rustc's own LLVM tools verify
# and assemble it to bitcode; the app crate links it under cross-language LTO (rust-lld), so the
# runtime's helpers inline into the generated code.
#   bash /mnt/d/code/etch/scripts/build-etch.sh [profile: release|tapeout] [extra RUSTFLAGS]
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
PROFILE=${1:-release}; EXTRA=${2:-}
cd /mnt/d/code/etch
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
GEN=$HOME/etch-gen; mkdir -p $GEN
t0=$(date +%s.%N)
# etchc is only the compiler: built with a newer toolchain (stable 1.93 crashes on it: a rustc bug)
CARGO_TARGET_DIR=$HOME/etch-target-etchc taskset -c 0-7 cargo +1.99.0 build -q --release -p etchc
# rustc's datalayout and triple, so the module links with the runtime's bitcode
echo 'pub fn f() {}' > $GEN/dl.rs; rustc --crate-type=lib --emit=llvm-ir -o $GEN/dl.ll $GEN/dl.rs
export ETCH_DATALAYOUT=$(grep '^target datalayout' $GEN/dl.ll | sed 's/.*= "\(.*\)"/\1/')
export ETCH_TRIPLE=$(grep '^target triple' $GEN/dl.ll | sed 's/.*= "\(.*\)"/\1/')
$HOME/etch-target-etchc/release/etchc emit apps/campfire $GEN message,room_head,room_tail,sidebar
$BIN/opt -passes=verify -disable-output $GEN/app.ll
$BIN/llvm-as $GEN/app.ll -o $GEN/app.bc
rm -f $GEN/libetchapp.a; $BIN/llvm-ar rcs $GEN/libetchapp.a $GEN/app.bc
t1=$(date +%s.%N)
FLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld $EXTRA"
ETCH_GEN_DIR=$GEN RUSTFLAGS="$FLAGS" CARGO_TARGET_DIR=$HOME/etch-target-gen taskset -c 0-7 cargo build -q --profile $PROFILE -p campfire-etch
t2=$(date +%s.%N)
DIR=$PROFILE; [ "$PROFILE" = dev ] && DIR=debug
cp $HOME/etch-target-gen/$DIR/campfire-etch ~/hw/etch-gen-$PROFILE
echo "spec -> IR -> bitcode: $(echo "$t1 - $t0" | bc | cut -c1-5) s; link with LTO: $(echo "$t2 - $t1" | bc | cut -c1-5) s -> ~/hw/etch-gen-$PROFILE"
echo "generated functions left in the binary (inlined when absent): $($BIN/llvm-nm ~/hw/etch-gen-$PROFILE | grep -cE ' etch_(build|bits)_')"
echo "runtime helpers left as symbols: $($BIN/llvm-nm ~/hw/etch-gen-$PROFILE | grep -cE ' T etch_(put|esc|int|json|sign|time|str_eq)$')"
