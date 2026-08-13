#!/bin/bash
# Runs every dispatch configuration built by build-dispatch-bins.sh and reports a comparison.
#
# Configurations are interleaved round-robin rather than run all-of-one-then-all-of-the-next, and
# the reported number is the best of all rounds. That ordering is not a nicety: the same two
# binaries have measured base 77 us / new 65 us in one batch and base 44 us / new 47 us in another,
# with the ordering reversed, so a batched A-then-B comparison can invert the result.
#
# The spread column is the point of the table as much as the timings are. An effect smaller than the
# spread has not been measured, it has been guessed at.
#
# Usage:
#   ./run-dispatch-bins.sh [bins-dir]          # defaults to ./dispatch-bins
#
# Environment:
#   ROUNDS=5           how many interleaved rounds to run
#   CORE=3             pin to this core; unset means unpinned, which adds noise
#   COREMARK_REPEAT=1  runs per invocation, in-process, best kept (multiplies with ROUNDS)
#   ONLY="a b"         restrict to these configurations
#   SKIP_BASELINE=1    skip the generic interpreter loop
set -euo pipefail

BINS=${1:-./dispatch-bins}
ROUNDS=${ROUNDS:-5}
CORE=${CORE:-}
REPEAT=${COREMARK_REPEAT:-1}
ONLY=${ONLY:-}
SKIP_BASELINE=${SKIP_BASELINE:-}

# CoreMark's known-good per-algorithm CRCs for this workload size. These are the correctness signal;
# "Correct operation validated" is not, because CoreMark only prints it when the run also happens to
# exceed ten seconds, which benchmarking iteration counts do not.
EXPECT="0xe714 0x1fd7 0x8e3a"

ALL="basic basic-pn branchless branchless-pn zerostore zerostore-pn"

if [ ! -d "$BINS" ]; then
    echo "No such directory: $BINS" >&2
    echo "Build the binaries first: COREMARK_ITERATIONS=3000 $(dirname "$0")/build-dispatch-bins.sh" >&2
    exit 1
fi

CFGS=""
for cfg in $ALL; do
    if [ -n "$ONLY" ] && [[ " $ONLY " != *" $cfg "* ]]; then
        continue
    fi
    if [ -x "$BINS/$cfg" ]; then
        CFGS="$CFGS $cfg"
    fi
done

if [ -z "$CFGS" ]; then
    echo "No dispatch binaries found in $BINS" >&2
    exit 1
fi

if [ -n "$CORE" ]; then
    PIN=(taskset -c "$CORE")
else
    PIN=()
    echo "warning: not pinned to a core, set CORE=<n> to reduce noise" >&2
fi

# Run one configuration once, aborting with the guest's own output if it fails. Empty dispatch means
# the generic interpreter loop, which is the baseline every speedup is relative to.
run_one() {
    local bin=$1 dispatch=$2 out status
    if [ -n "$dispatch" ]; then
        out=$(COREMARK_DISPATCH="$dispatch" COREMARK_REPEAT="$REPEAT" \
            ${PIN[@]+"${PIN[@]}"} "$BINS/$bin" 2>&1) && status=0 || status=$?
    else
        out=$(env -u COREMARK_DISPATCH COREMARK_REPEAT="$REPEAT" \
            ${PIN[@]+"${PIN[@]}"} "$BINS/$bin" 2>&1) && status=0 || status=$?
    fi

    if [ "$status" -ne 0 ]; then
        echo "$bin: exited $status" >&2
        echo "$out" | sed 's/^/  /' >&2
        exit 1
    fi

    echo "$out"
}

field() { echo "$1" | grep -E "^\\[0\\]$2" | awk '{print $NF}' || true; }
elapsed() { echo "$1" | grep "^Host elapsed" | awk '{print $3}' || true; }

echo "cpu:      $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')"
echo "cores:    $(nproc)$([ -n "$CORE" ] && echo ", pinned to $CORE" || echo ", unpinned")"
echo "rounds:   $ROUNDS x $REPEAT repeat"
echo

# Validation pass. Doubles as a warm-up, and catches the two mistakes that silently invalidate a
# comparison: a miscompiled configuration, and binaries built with different iteration counts
# (COREMARK_ITERATIONS is baked in at build time, so a partial rebuild leaves them incomparable).
ITERATIONS=""
BASELINE_BIN=""
for cfg in $CFGS; do
    out=$(run_one "$cfg" "${cfg%-pn}")
    got="$(field "$out" crclist) $(field "$out" crcmatrix) $(field "$out" crcstate)"
    if [ "$got" != "$EXPECT" ]; then
        echo "$cfg: WRONG RESULTS, got CRCs [$got], expected [$EXPECT]" >&2
        exit 1
    fi

    iterations=$(echo "$out" | grep "^Iterations " | awk '{print $NF}' || true)
    if [ -z "$ITERATIONS" ]; then
        ITERATIONS=$iterations
        BASELINE_BIN=$cfg
    elif [ "$iterations" != "$ITERATIONS" ]; then
        echo "$cfg ran $iterations iterations but $BASELINE_BIN ran $ITERATIONS" >&2
        echo "Rebuild them all with the same COREMARK_ITERATIONS; these are not comparable." >&2
        exit 1
    fi
    echo "$cfg: CRCs ok"
done
echo "iterations: $ITERATIONS (baked in at build time)"

RUNS=$CFGS
if [ -z "$SKIP_BASELINE" ]; then
    RUNS="generic $CFGS"
fi

declare -A samples
for cfg in $RUNS; do samples[$cfg]=""; done

for round in $(seq 1 "$ROUNDS"); do
    for cfg in $RUNS; do
        if [ "$cfg" = generic ]; then
            out=$(run_one "$BASELINE_BIN" "")
        else
            out=$(run_one "$cfg" "${cfg%-pn}")
        fi

        seconds=$(elapsed "$out")
        if [ -z "$seconds" ]; then
            echo "$cfg: no timing in output" >&2
            echo "$out" | sed 's/^/  /' >&2
            exit 1
        fi
        samples[$cfg]="${samples[$cfg]} $seconds"
    done
    echo "round $round/$ROUNDS" >&2
done

# min, median, max, spread as a percentage of min
stats() {
    tr ' ' '\n' | grep -v '^$' | sort -n | awk '
        { a[NR] = $1 }
        END {
            median = (NR % 2) ? a[(NR + 1) / 2] : (a[NR / 2] + a[NR / 2 + 1]) / 2
            printf "%.3f %.3f %.3f %.1f", a[1], median, a[NR], (a[NR] - a[1]) / a[1] * 100
        }'
}

echo
printf "%-16s %8s %8s %8s %8s %8s\n" configuration best median worst "spread%" "vs base"
BASE=""
WORST_SPREAD=0
for cfg in $RUNS; do
    read -r min median max spread <<<"$(echo "${samples[$cfg]}" | stats)"
    if [ -z "$BASE" ]; then
        BASE=$min
    fi
    ratio=$(awk -v b="$BASE" -v m="$min" 'BEGIN { printf "%.2f", b / m }')
    printf "%-16s %8s %8s %8s %8s %8s\n" "$cfg" "$min" "$median" "$max" "$spread" "${ratio}x"
    WORST_SPREAD=$(awk -v a="$WORST_SPREAD" -v b="$spread" 'BEGIN { print (b > a) ? b : a }')
done

echo
echo "Speedups are against the first row. Any difference smaller than the spread column is"
echo "unmeasured rather than absent."

# A short workload is dominated by process start-up and ELF decoding rather than by the interpreter,
# so the spread balloons and nothing smaller than it can be resolved. Iteration count is baked in at
# build time, which makes this easy to leave wrong for a whole session.
if awk -v s="$WORST_SPREAD" 'BEGIN { exit !(s > 8) }'; then
    echo
    echo "warning: worst spread is ${WORST_SPREAD}%, which is too wide to compare these"
    echo "         configurations against each other. Rebuild with a larger COREMARK_ITERATIONS"
    echo "         (3000 gives roughly a second per run, and a 3-7% spread), raise ROUNDS, and"
    echo "         pin to an otherwise idle core."
fi
