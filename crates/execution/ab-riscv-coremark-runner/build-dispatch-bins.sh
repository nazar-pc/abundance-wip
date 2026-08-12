#!/bin/bash
# Builds one binary per dispatch configuration so that their handlers do not share a text section.
#
# The configurations are the cross product of the two axes still worth measuring: the register file
# the handlers are instantiated with, and the calling convention they are given. Each binary also
# contains the generic interpreter loop, which is the baseline: run it by leaving COREMARK_DISPATCH
# unset.
set -e
OUT=${1:-./dispatch-bins}
ITER=${COREMARK_ITERATIONS:-300}
mkdir -p "$OUT"
for regs in basic branchless zerostore; do
  for abi in rust preserve-none; do
    features="dispatch-$regs"
    name="$regs"
    if [ "$abi" = preserve-none ]; then
      features="$features,dispatch-preserve-none"
      name="$name-pn"
    fi
    COREMARK_ITERATIONS=$ITER cargo build -p ab-riscv-coremark-runner --release \
      --no-default-features --features "$features" >/dev/null 2>&1
    cp target/release/ab-riscv-coremark-runner "$OUT/$name"
    echo "built $OUT/$name"
  done
done
