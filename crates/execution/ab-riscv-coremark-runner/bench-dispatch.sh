#!/bin/bash
# Builds the Coremark runner and compares its dispatch modes against each other. All of them live
# in the same binary, selected by COREMARK_DISPATCH, so what differs between two columns is
# dispatch and nothing else.
#
# The modes are interleaved round-robin rather than run all-of-one-then-the-other, and the reported
# figure is best-of, because that is the ordering that survives a machine with background load. The
# spread column is as much the point as the timings are: an effect smaller than the spread has not
# been measured.
#
# Usage:
#   ./bench-dispatch.sh
#   MODES="threaded token packed" ./bench-dispatch.sh
#
# Modes, in the order they are reported by default. The first is the baseline the rest are
# expressed against:
#   loop      the generic `match` loop, `BasicInterpreterState::execute()`
#   threaded  generated per-instruction handlers chained with `become` — the shipped path
#   token     prototype: indirect threading, discriminant in the slot plus a handler table
#   direct    prototype: direct threading, full handler pointer, sixteen-byte slot
#   packed    prototype: direct threading, handler offset plus packed operands, eight-byte slot
#
# Environment:
#   COREMARK_ITERATIONS=3000  guest work, baked in at build time
#   MODES="loop threaded ..." which modes to compare, first one is the baseline
#   ROUNDS=5                  interleaved rounds
#   CORE=1                    pin to this core; unset means unpinned, which adds noise
#   COREMARK_REPEAT=1         runs per invocation, in-process, best kept
set -euo pipefail

cd "$(dirname "$0")/../../.."

ITER=${COREMARK_ITERATIONS:-3000}
MODES=${MODES:-"loop threaded token direct packed"}
ROUNDS=${ROUNDS:-5}
CORE=${CORE:-}
REPEAT=${COREMARK_REPEAT:-1}

# CoreMark's known-good per-algorithm CRCs for this workload size. These are the correctness signal;
# "Correct operation validated" is not, because CoreMark only prints it when the run also happens to
# exceed ten seconds, which benchmarking iteration counts do not.
EXPECT="0xe714 0x1fd7 0x8e3a"

# Ask cargo where it put the binary rather than assuming `target/release/`, which is wrong whenever
# CARGO_TARGET_DIR, `build.target-dir` or a `--target` triple is in play.
build() {
    local log path status
    log=$(mktemp)
    path=$(COREMARK_ITERATIONS=$ITER cargo build -p ab-riscv-coremark-runner --release \
        --features build-elf-required --message-format=json-render-diagnostics 2>"$log" \
        | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1) && status=0 || status=$?

    if [ "$status" -ne 0 ] || [ -z "$path" ]; then
        echo "build failed" >&2
        cat "$log" >&2
        rm -f "$log"
        exit 1
    fi

    rm -f "$log"
    echo "$path"
}

BIN=$(build)

if [ -n "$CORE" ]; then
    PIN=(taskset -c "$CORE")
else
    PIN=()
    echo "warning: not pinned to a core, set CORE=<n> to reduce noise" >&2
fi

# Run one mode once, aborting with the guest's own output if it fails
run_one() {
    local mode=$1 out status
    out=$(COREMARK_DISPATCH="$mode" COREMARK_REPEAT="$REPEAT" ${PIN[@]+"${PIN[@]}"} "$BIN" 2>&1) \
        && status=0 || status=$?

    if [ "$status" -ne 0 ]; then
        echo "$mode: exited $status" >&2
        echo "$out" | sed 's/^/  /' >&2
        exit 1
    fi

    echo "$out"
}

field() { echo "$1" | grep -E "^\\[0\\]$2" | awk '{print $NF}' || true; }
elapsed() { echo "$1" | grep "^Host elapsed" | awk '{print $3}' || true; }

echo "cpu:        $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')"
echo "cores:      $(nproc)$([ -n "$CORE" ] && echo ", pinned to $CORE" || echo ", unpinned")"
echo "iterations: $ITER"
echo "rounds:     $ROUNDS x $REPEAT repeat"
echo "modes:      $MODES"
echo

# Validation pass, which doubles as a warm-up
for mode in $MODES; do
    out=$(run_one "$mode")
    got="$(field "$out" crclist) $(field "$out" crcmatrix) $(field "$out" crcstate)"
    if [ "$got" != "$EXPECT" ]; then
        echo "$mode: WRONG RESULTS, got CRCs [$got], expected [$EXPECT]" >&2
        exit 1
    fi
    echo "$mode: CRCs ok"
done

declare -A samples
for mode in $MODES; do
    samples[$mode]=""
done

for round in $(seq 1 "$ROUNDS"); do
    for mode in $MODES; do
        seconds=$(elapsed "$(run_one "$mode")")
        if [ -z "$seconds" ]; then
            echo "$mode: no timing in output" >&2
            exit 1
        fi
        samples[$mode]="${samples[$mode]} $seconds"
    done
    echo "round $round/$ROUNDS" >&2
done

stats() {
    tr ' ' '\n' | grep -v '^$' | sort -n | awk '
        { a[NR] = $1 }
        END {
            median = (NR % 2) ? a[(NR + 1) / 2] : (a[NR / 2] + a[NR / 2 + 1]) / 2
            printf "%.3f %.3f %.3f %.1f", a[1], median, a[NR], (a[NR] - a[1]) / a[1] * 100
        }'
}

echo
printf "%-12s %8s %8s %8s %8s %8s\n" mode best median worst "spread%" "vs base"
BASE=""
WORST_SPREAD=0
for mode in $MODES; do
    read -r min median max spread <<<"$(echo "${samples[$mode]}" | stats)"
    [ -z "$BASE" ] && BASE=$min
    ratio=$(awk -v b="$BASE" -v m="$min" 'BEGIN { printf "%.2f", b / m }')
    printf "%-12s %8s %8s %8s %8s %8s\n" "$mode" "$min" "$median" "$max" "$spread" "${ratio}x"
    WORST_SPREAD=$(awk -v a="$WORST_SPREAD" -v b="$spread" 'BEGIN { print (b > a) ? b : a }')
done

if awk -v s="$WORST_SPREAD" 'BEGIN { exit !(s > 8) }'; then
    echo
    echo "warning: worst spread is ${WORST_SPREAD}%, too wide to resolve small differences."
    echo "         Raise COREMARK_ITERATIONS or ROUNDS, and pin to an otherwise idle core."
fi
