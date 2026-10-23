#!/bin/bash
# Spike: a hand-written LLVM IR function (as burin will generate) linked into a Rust binary with
# cross-language LTO (rustc's LLVM 21 tools, rust-lld), calling a Rust runtime helper that should
# be inlined into it. Prints whether the helper survives as a call.
set -e
source ~/.cargo/env 2>/dev/null || true
BIN=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
S=/tmp/burin-spike; rm -rf $S; mkdir -p $S/src; cd $S
# the datalayout and triple rustc uses
echo 'pub fn f() {}' > dl.rs; rustc --crate-type=lib --emit=llvm-ir -o dl.ll dl.rs
DL=$(grep '^target datalayout' dl.ll); TT=$(grep '^target triple' dl.ll)
cat > gen.ll <<EOF
$DL
$TT
declare i64 @rt_mix(i64, i64)
define i64 @burin_gen_f(i64 %x) {
  %a = call i64 @rt_mix(i64 %x, i64 7)
  %b = call i64 @rt_mix(i64 %a, i64 %x)
  ret i64 %b
}
EOF
$BIN/opt -passes=verify gen.ll -disable-output
$BIN/llvm-as gen.ll -o gen.bc
$BIN/llvm-ar rcs libgen.a gen.bc
cat > Cargo.toml <<'EOF'
[package]
name = "spike"
version = "0.1.0"
edition = "2021"
[profile.release]
lto = "fat"
codegen-units = 1
EOF
cat > build.rs <<'EOF'
fn main() {
    println!("cargo:rustc-link-search=native=/tmp/burin-spike");
    println!("cargo:rustc-link-lib=static=gen");
}
EOF
cat > src/main.rs <<'EOF'
#[no_mangle]
pub extern "C" fn rt_mix(a: i64, b: i64) -> i64 {
    a.wrapping_mul(31) ^ b
}
extern "C" {
    fn burin_gen_f(x: i64) -> i64;
}
#[inline(never)]
fn run(x: i64) -> i64 {
    unsafe { burin_gen_f(x) }
}
fn main() {
    let x: i64 = std::env::args().count() as i64;
    println!("{}", run(x));
}
EOF
RUSTFLAGS="-Clinker-plugin-lto -Clink-arg=-fuse-ld=lld -Clink-arg=-B$BIN/gcc-ld" cargo build --release -q 2>&1 | tail -5
./target/release/spike
echo "calls to rt_mix left in run(): $($BIN/llvm-objdump -d --no-show-raw-insn target/release/spike | awk '/<_ZN5spike3run/,/ret/' | grep -c 'call.*rt_mix' || true)"
$BIN/llvm-objdump -d --no-show-raw-insn target/release/spike | awk '/<_ZN5spike3run/,/ret/' | head -12
SYM=$($BIN/llvm-nm target/release/spike | grep -oE "_ZN5spike3run[A-Za-z0-9_]*" | head -1)
echo "--- run():"
$BIN/llvm-objdump -d --no-show-raw-insn --disassemble-symbols=$SYM target/release/spike | tail -n +7
echo "--- symbols left: $($BIN/llvm-nm target/release/spike | grep -cE 'rt_mix|burin_gen_f')"
