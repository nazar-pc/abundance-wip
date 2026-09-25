#!/usr/bin/env bash
#
# Take `generic-interpreter-rv64` apart on the contract workloads: build the guest once per
# combination of the pieces that make it up, and report the dispatch counts of `blake3_hash_chunk`
# and `ed25519_verify` for each, optionally with the pinned wall-clock benchmarks too.
#
# Run it from the repository root, with the rustc from `build-patched-rustc.sh`:
#
#   AB_GUEST_TOOLCHAIN=~/rustc-interp/rust/build/host/stage2/bin/rustc \
#     specs/discussions/interpreter-codegen/ablate-rust.sh
#
# The pieces, each switched by an option upstream LLVM already has, except C and D which exist only
# behind `+interpreter-target`. The guest target specification already enables the fusion
# features that pay off for contracts, so all of these are on top of those:
#
#   J  jumps are expensive, so compound conditions are computed   -jump-is-expensive
#   S  SelectOptimize off, which the interpreter CPU drops by       -enable-select-opt
#      replacing the generic tune feature list
#   A  RISC-V's own unroll preferences instead of the generic ones  +no-default-unroll
#   B  partial and runtime unrolling capped at one                  -unroll-max-count=1
#      (A2 and A3 cap it at two and three instead: the nearest upstream approximation of C)
#   C  unroll a small loop to the period of a shift chain           +interpreter-target only
#   D  predictable-branch threshold of one                          +interpreter-target only
#
# Environment overrides:
#   AB_GUEST_TOOLCHAIN  the patched rustc (required)
#   CONFIGS             which rows to run, by name (default: all)
#   BENCH               1 to also run the threaded eager benchmarks for each row (slow)
#   PIN_CPU             core to pin benchmarks to (default 2)
#   BENCH_ARGS          extra arguments for Criterion, e.g. `--save-baseline base` on one row and
#                       `--baseline base` on the rest to see each against it
#
set -euo pipefail

: "${AB_GUEST_TOOLCHAIN:?point it at the patched rustc from build-patched-rustc.sh}"
export AB_GUEST_TOOLCHAIN
BENCH=${BENCH:-0}
PIN_CPU=${PIN_CPU:-2}

# name | CPU (- for the specification's) | features appended to the specification's | llvm-args
ROWS="
base      | - | -                                                 | -
J         | - | -                                                 | -jump-is-expensive
S         | - | -enable-select-opt                                | -
A         | - | +no-default-unroll                                | -
AB        | - | +no-default-unroll                                | -unroll-max-count=1
A2        | - | +no-default-unroll                                | -unroll-max-count=2
A3        | - | +no-default-unroll                                | -unroll-max-count=3
JS        | - | -enable-select-opt                                | -jump-is-expensive
ABS       | - | +no-default-unroll,-enable-select-opt             | -unroll-max-count=1
ABJS      | - | +no-default-unroll,-enable-select-opt             | -unroll-max-count=1 -jump-is-expensive
I         | - | +interpreter-target                               | -
IS        | - | +interpreter-target,-enable-select-opt            | -
IS-noC    | - | +interpreter-target,-enable-select-opt            | -riscv-interpreter-max-shift-chain-unroll-count=0
IS-noJ    | - | +interpreter-target,-enable-select-opt            | -jump-is-expensive=false
IS-noCJ   | - | +interpreter-target,-enable-select-opt            | -riscv-interpreter-max-shift-chain-unroll-count=0 -jump-is-expensive=false
IS-noB    | - | +interpreter-target,-enable-select-opt            | -riscv-interpreter-max-unroll-count=4294967295
oldcpu    | generic-interpreter-rv64 | -                          | -
"

trim() { sed 's/^ *//; s/ *$//' <<<"$1"; }

while IFS='|' read -r name cpu features args; do
  name=$(trim "${name:-}")
  [ -z "$name" ] && continue
  if [ -n "${CONFIGS:-}" ] && ! grep -qw -- "$name" <<<"$CONFIGS"; then continue; fi
  cpu=$(trim "$cpu"); features=$(trim "$features"); args=$(trim "$args")

  unset AB_GUEST_CPU AB_GUEST_FEATURES AB_EXTRA_RUSTFLAGS
  [ "$cpu" = "-" ] || export AB_GUEST_CPU=$cpu
  [ "$features" = "-" ] || export AB_GUEST_FEATURES=$features
  if [ "$args" != "-" ]; then
    rustflags=
    for arg in $args; do rustflags+=" -C llvm-args=$arg"; done
    export AB_EXTRA_RUSTFLAGS=$rustflags
  fi

  echo "== $name  cpu=$cpu features=$features llvm-args=$args"
  if ! out=$(cargo run --release -q -p ab-riscv-benchmarks --example dispatch_count 2>&1); then
    echo "$out" >&2
    echo "building or running row $name failed, see above" >&2
    exit 1
  fi
  grep -E '^(blake3_hash_chunk|ed25519_verify):|instructions, ' <<<"$out" | sed 's/^/   /'

  if [ "$BENCH" = 1 ]; then
    # Compile with every core, then run pinned: after `--no-run` there is nothing left for the
    # pinned Cargo to build, so the benchmark is all that runs on that core.
    cargo bench -q -p ab-riscv-benchmarks --bench riscv --no-run
    taskset -c "$PIN_CPU" cargo bench -q -p ab-riscv-benchmarks --bench riscv -- \
      'interpreter/threaded/eager' ${BENCH_ARGS:-}
  fi
done <<<"$ROWS"
