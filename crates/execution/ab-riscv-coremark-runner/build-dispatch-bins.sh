#!/bin/bash
# Builds one binary per register file so that their handlers do not share a text section.
#
# Each binary also contains the generic interpreter loop, which is the baseline: run it by leaving
# COREMARK_DISPATCH unset.
set -e
OUT=${1:-./dispatch-bins}
ITER=${COREMARK_ITERATIONS:-300}
mkdir -p "$OUT"
for regs in basic branchless zerostore; do
  COREMARK_ITERATIONS=$ITER cargo build -p ab-riscv-coremark-runner --release \
    --no-default-features --features "dispatch-$regs" >/dev/null 2>&1
  cp target/release/ab-riscv-coremark-runner "$OUT/$regs"
  echo "built $OUT/$regs"
done
