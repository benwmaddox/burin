#!/bin/bash
# Builds Campfire from its spec: burinc emits the registers' LLVM IR; rustc's own LLVM tools verify
# and assemble it to bitcode; the app crate links it under cross-language LTO (rust-lld), so the
# runtime's helpers inline into the generated code.
#   bash /mnt/d/code/burin/scripts/build-burin.sh [profile: release|tapeout] [extra RUSTFLAGS]
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
PROFILE=${1:-release}; EXTRA=${2:-}
cd /mnt/d/code/burin
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
GEN=$HOME/burin-gen-${APP:-campfire}; mkdir -p $GEN
t0=$(date +%s.%N)
# burinc is only the compiler: built with a newer toolchain (stable 1.93 crashes on it: a rustc bug)
CARGO_TARGET_DIR=$HOME/burin-target-burinc taskset -c 0-7 cargo +1.99.0 build -q --release -p burinc
# rustc's datalayout and triple, so the module links with the runtime's bitcode
echo 'pub fn f() {}' > $GEN/dl.rs; rustc --crate-type=lib --emit=llvm-ir -o $GEN/dl.ll $GEN/dl.rs
export BURIN_DATALAYOUT=$(grep '^target datalayout' $GEN/dl.ll | sed 's/.*= "\(.*\)"/\1/')
export BURIN_TRIPLE=$(grep '^target triple' $GEN/dl.ll | sed 's/.*= "\(.*\)"/\1/')
$HOME/burin-target-burinc/release/burinc emit apps/${APP:-campfire} $GEN
$BIN/opt -passes=verify -disable-output $GEN/app.ll
$BIN/llvm-as $GEN/app.ll -o $GEN/app.bc
rm -f $GEN/libburinapp.a; $BIN/llvm-ar rcs $GEN/libburinapp.a $GEN/app.bc
# the app's external functions (apps/<app>/ext/): C sources into libburinext.a; ext.rs, Rust
# functions, into the app crate (an empty module when there is none)
EXT=apps/${APP:-campfire}/ext
rm -rf $GEN/ext; mkdir -p $GEN/ext; rm -f $GEN/libburinext.a
for c in $EXT/*.c; do
  [ -e "$c" ] || continue
  cc -O2 -fPIC -Wall -Werror -Iinclude -c "$c" -o $GEN/ext/$(basename "$c" .c).o
done
ls $GEN/ext/*.o > /dev/null 2>&1 && ar rcs $GEN/libburinext.a $GEN/ext/*.o
if [ -f $EXT/ext.rs ]; then export BURIN_EXT_RS=/mnt/d/code/burin/$EXT/ext.rs; else : > $GEN/ext_none.rs; export BURIN_EXT_RS=$GEN/ext_none.rs; fi
t1=$(date +%s.%N)
FLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld $EXTRA"
BURIN_GEN_DIR=$GEN RUSTFLAGS="$FLAGS" CARGO_TARGET_DIR=$HOME/burin-target-gen taskset -c 0-7 cargo build -q --profile $PROFILE -p ${CRATE:-campfire-app}
t2=$(date +%s.%N)
DIR=$PROFILE; [ "$PROFILE" = dev ] && DIR=debug
cp $HOME/burin-target-gen/$DIR/${CRATE:-campfire-app} ~/hw/burin-${APP:-gen}-$PROFILE
echo "spec -> IR -> bitcode: $(echo "$t1 - $t0" | bc | cut -c1-5) s; link with LTO: $(echo "$t2 - $t1" | bc | cut -c1-5) s -> ~/hw/burin-${APP:-gen}-$PROFILE"
echo "generated functions left in the binary (inlined when absent): $($BIN/llvm-nm ~/hw/burin-${APP:-gen}-$PROFILE | grep -cE ' burin_(build|bits)_')"
echo "runtime helpers left as symbols: $($BIN/llvm-nm ~/hw/burin-${APP:-gen}-$PROFILE | grep -cE ' T burin_(put|esc|int|json|sign|time|str_eq)$')"
