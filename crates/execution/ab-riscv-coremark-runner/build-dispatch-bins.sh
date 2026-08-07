#!/bin/bash
# Builds one binary per dispatch back end so that their handlers do not share a text section
set -e
OUT=${1:-./dispatch-bins}
ITER=${COREMARK_ITERATIONS:-300}
mkdir -p "$OUT"
for f in match call tail plaintail safe-basic safe-branchless safe-zerostore; do
  COREMARK_ITERATIONS=$ITER cargo build -p ab-riscv-coremark-runner --release \
    --no-default-features --features "dispatch-$f" >/dev/null 2>&1
  cp target/release/ab-riscv-coremark-runner "$OUT/$f"
  echo "built $OUT/$f"
done
