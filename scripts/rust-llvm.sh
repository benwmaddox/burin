#!/bin/bash
# Lists the LLVM tools rustc ships (the same LLVM version as rustc's).
source ~/.cargo/env 2>/dev/null || true
S=$(rustc --print sysroot)
ls $S/lib/rustlib/x86_64-unknown-linux-gnu/bin/ $S/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld 2>/dev/null
$S/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-as --version 2>/dev/null | head -3
