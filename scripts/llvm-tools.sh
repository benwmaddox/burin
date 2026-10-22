#!/bin/bash
# What LLVM optimization tools this WSL has: rustc's LLVM version, llvm-profdata (PGO), llvm-bolt.
source ~/.cargo/env 2>/dev/null || true
rustc -vV | grep -E "LLVM|host"
rustup component list --installed 2>/dev/null | grep -i llvm
ls ~/.rustup/toolchains/*/lib/rustlib/*/bin/ 2>/dev/null
which llvm-profdata llvm-bolt perf merge-fdata clang 2>/dev/null
ls /usr/bin/llvm-bolt* /usr/lib/llvm-*/bin/llvm-bolt /usr/lib/llvm-*/bin/llvm-profdata 2>/dev/null
apt-cache policy bolt-18 llvm-18 bolt-19 2>/dev/null | grep -E "^[a-z]|Candidate"
grep -m1 -o "flags.*" /proc/cpuinfo | tr ' ' '\n' | grep -cE "avx2|bmi2"
