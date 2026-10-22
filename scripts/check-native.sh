#!/bin/bash
# Counts instructions only a target-cpu=native build (AVX2/BMI2 here) would contain.
OD=$(~/.cargo/bin/rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-objdump
for b in etch-1a etch-lto etch-pgo etch-bolt; do
  n=$($OD -d --no-show-raw-insn ~/hw/$b 2>/dev/null | grep -cE '\b(shlx|shrx|sarx|andn|bzhi|vpbroadcast[bwdq]|vpermq)\b')
  echo "$b: $n BMI2/AVX2 instructions"
done
ls -la --time-style=full-iso ~/etch-target-lto/tapeout/campfire-hand ~/hw/etch-lto | cut -c30-
