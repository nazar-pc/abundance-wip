#!/bin/bash
# Builds one binary per dispatch configuration so that their handlers do not share a text section.
#
# The configurations are the cross product of the axes still worth measuring: the register file the
# handlers are instantiated with, and the calling convention they are given. Each binary also
# contains the generic interpreter loop, which is the baseline: run it by leaving COREMARK_DISPATCH
# unset.
#
# Usage:
#   COREMARK_ITERATIONS=3000 ./build-dispatch-bins.sh [out-dir]
#
# COREMARK_ITERATIONS is baked into the guest at build time, so every binary here gets the same
# value and run-dispatch-bins.sh refuses to compare binaries that disagree about it.
set -euo pipefail

OUT=${1:-./dispatch-bins}
ITER=${COREMARK_ITERATIONS:-300}

mkdir -p "$OUT"

# Ask cargo where it put the binary rather than assuming `target/release/`, which is wrong whenever
# CARGO_TARGET_DIR, `build.target-dir` or a `--target` triple is in play. `executable` is null for
# every artifact that is not a binary, and null does not match the quoted pattern.
build() {
    local features=$1 path log status
    log=$(mktemp)

    path=$(COREMARK_ITERATIONS=$ITER cargo build -p ab-riscv-coremark-runner --release \
        --no-default-features --features "$features" --message-format=json-render-diagnostics \
        2>"$log" | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1) && status=0 || status=$?

    if [ "$status" -ne 0 ] || [ -z "$path" ]; then
        echo "build failed for features: $features" >&2
        cat "$log" >&2
        rm -f "$log"
        exit 1
    fi

    rm -f "$log"
    echo "$path"
}

for regs in basic branchless zerostore; do
    for abi in rust preserve-none; do
        features="dispatch-$regs"
        name="$regs"
        if [ "$abi" = preserve-none ]; then
            features="$features,dispatch-preserve-none"
            name="$name-pn"
        fi

        cp "$(build "$features")" "$OUT/$name"
        echo "built $OUT/$name"
    done
done
