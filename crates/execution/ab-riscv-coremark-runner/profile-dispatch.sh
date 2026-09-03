#!/bin/bash
# Profiles the Coremark runner's dispatch modes against each other with `perf`, and normalises
# every counter per *guest* instruction, which is the only unit in which they are comparable and
# the only one in which the numbers mean anything.
#
# The modes are the same ones bench-dispatch.sh reports; see its header. `loop` is the generic
# `match` loop, `threaded` is the shipped generated path, and `token`, `direct` and `packed` are
# the prototype back ends.
#
# The guest instruction count is not guessed: COREMARK_HISTOGRAM counts the dynamic instruction mix
# exactly, so `cycles / guest instructions` is a real cycles-per-guest-instruction figure.
#
# What the output is for:
#
#   cycles/guest-insn      The headline. The generic loop measured ~10.2 on Zen 4 historically.
#   ind-mispred/guest-insn Every handler ends in one indirect jump through the dispatch table, so
#                          this is at most 1.0. Above ~0.05 and misprediction is the bottleneck,
#                          which would make an inline handler pointer (removing the table lookup)
#                          or handler replication worth trying. Below ~0.02 and prediction is fine,
#                          so dispatch is not what to work on next.
#   L1-icache misses       ~10 KB of handlers should fit Zen 4's 32 KB L1i. If this is high anyway,
#                          the layout is wrong rather than the size.
#   L1-dcache misses       Guest memory and the decoded stream. High here points at the workload's
#                          working set rather than at dispatch.
#   host-insn/guest-insn   Falls when handlers get leaner. Compare against cycles/guest-insn: if
#                          instructions fall but cycles do not, the loop is stalled, not
#                          throughput-bound.
#   IPC                    A diagnostic, never a goal. The big `match` reached IPC above 5 and was
#                          still slower, because it executed four times as many instructions per
#                          guest instruction; most of them were an operand preamble the instruction
#                          did not need, and being useless they were also independent, which is
#                          exactly what inflates IPC. Low IPC on a 6-wide core means the loop is
#                          waiting on a dependency chain and has issue width to spare.
#   loads, stores, stlf    Per guest instruction. The guest register file lives in memory, so a
#                          guest-level dependency becomes a store followed by a load of the same
#                          slot, and `stlf` counts exactly those store-to-load forwards. If it is
#                          near the number of guest instructions with a register dependency, the
#                          critical path is the register file round trip rather than dispatch.
#                          `stlf-fail` is the expensive case: a load that could not be forwarded and
#                          had to wait for the store to reach the cache, which costs far more than a
#                          successful forward. It should be near zero, because every register-file
#                          access is naturally aligned and the same width.
#
# Usage:
#   ./profile-dispatch.sh
#
# Environment:
#   COREMARK_ITERATIONS=3000  guest work, baked in at build time
#   MODES="loop threaded"     which modes to profile
#   RECORD_MODE=threaded      which mode the per-handler sampling pass profiles
#   CORE=1                    pin to this core
#   REPEATS=3                 perf stat repetitions
#   SKIP_RECORD=1             skip the per-handler sampling pass
set -euo pipefail

cd "$(dirname "$0")/../../.."

ITER=${COREMARK_ITERATIONS:-3000}
MODES=${MODES:-"loop threaded"}
RECORD_MODE=${RECORD_MODE:-threaded}
CORE=${CORE:-1}
REPEATS=${REPEATS:-3}
SKIP_RECORD=${SKIP_RECORD:-}

# `perf annotate` will otherwise try to fetch debuginfo over the network for every object it sees
# and hang there indefinitely. Everything needed is already in the binary.
export DEBUGINFOD_URLS=""

if ! command -v perf >/dev/null; then
    echo "perf not found; install linux-tools for your kernel" >&2
    exit 1
fi

paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 2)
if [ "$paranoid" -gt 2 ]; then
    echo "warning: perf_event_paranoid is $paranoid, counters may be unavailable." >&2
    echo "         sudo sysctl kernel.perf_event_paranoid=1" >&2
fi

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

echo "cpu:        $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')"
echo "binary:     $BIN"
echo "iterations: $ITER, pinned to core $CORE"
echo

# Exact dynamic guest instruction count for this build's iteration count
echo "counting guest instructions..." >&2
GUEST=$(COREMARK_HISTOGRAM=1 taskset -c "$CORE" "$BIN" \
    | sed -n 's/^HISTOGRAM total=\([0-9]*\).*/\1/p')

if [ -z "$GUEST" ]; then
    echo "could not read guest instruction count from COREMARK_HISTOGRAM output" >&2
    exit 1
fi
echo "guest instructions: $GUEST"
echo

# Only ask for events this machine actually has. `perf stat` exits non-zero on an unknown event, so
# probing is just running it against `true`. The AMD-specific ones are the interesting part: generic
# `branch-misses` lumps conditional and indirect branches together, and only the indirect ones are
# dispatch.
EVENTS="cycles,instructions,branches,branch-misses"
for event in \
    L1-dcache-loads L1-dcache-load-misses L1-icache-load-misses \
    iTLB-load-misses dTLB-load-misses \
    ex_ret_brn_ind_misp ex_ret_ind_brch_instr \
    ls_dispatch.ld_dispatch ls_dispatch.store_dispatch ls_stlf ls_bad_status2.stli_other
do
    if perf stat -e "$event" true >/dev/null 2>&1; then
        EVENTS="$EVENTS,$event"
    else
        echo "note: no such event on this machine, skipping: $event" >&2
    fi
done
echo

stat_mode() {
    local mode=$1 out
    out=$(mktemp)

    COREMARK_DISPATCH="$mode" perf stat -e "$EVENTS" -r "$REPEATS" -x, -o "$out" \
        taskset -c "$CORE" "$BIN" >/dev/null 2>&1

    # `perf stat -x,` writes `value,unit,event,run,pct[,noise,...]` with a comment header
    awk -F, -v mode="$mode" -v guest="$GUEST" '
        /^#/ || NF < 3 { next }
        { gsub(/^ +| +$/, "", $1); gsub(/^ +| +$/, "", $3); v[$3] = $1 + 0 }
        END {
            printf "%-10s", mode
            printf " %10.2f", v["cycles"] / guest
            printf " %10.2f", v["instructions"] / guest
            printf " %8.2f", (v["cycles"] > 0) ? v["instructions"] / v["cycles"] : 0
            printf " %10.4f", v["branch-misses"] / guest
            ind = ("ex_ret_brn_ind_misp" in v) ? v["ex_ret_brn_ind_misp"] : -1
            if (ind >= 0) { printf " %12.4f", ind / guest } else { printf " %12s", "n/a" }
            printf " %10.4f", ("L1-icache-load-misses" in v) ? v["L1-icache-load-misses"] / guest : 0
            printf " %10.4f", ("L1-dcache-load-misses" in v) ? v["L1-dcache-load-misses"] / guest : 0
            loads = ("ls_dispatch.ld_dispatch" in v) ? v["ls_dispatch.ld_dispatch"] \
                  : (("L1-dcache-loads" in v) ? v["L1-dcache-loads"] : -1)
            if (loads >= 0) { printf " %8.2f", loads / guest } else { printf " %8s", "n/a" }
            st = ("ls_dispatch.store_dispatch" in v) ? v["ls_dispatch.store_dispatch"] : -1
            if (st >= 0) { printf " %8.2f", st / guest } else { printf " %8s", "n/a" }
            stlf = ("ls_stlf" in v) ? v["ls_stlf"] : -1
            if (stlf >= 0) { printf " %8.2f", stlf / guest } else { printf " %8s", "n/a" }
            bad = ("ls_bad_status2.stli_other" in v) ? v["ls_bad_status2.stli_other"] : -1
            if (bad >= 0) { printf " %9.4f", bad / guest } else { printf " %9s", "n/a" }
            printf "\n"
        }' "$out"

    rm -f "$out"
}

echo "All columns are per guest instruction, except IPC."
echo
printf "%-10s %10s %10s %8s %10s %12s %10s %10s %8s %8s %8s %9s\n" \
    mode cycles host-insn IPC br-miss ind-mispred i\$-miss d\$-miss loads stores stlf stlf-fail
for mode in $MODES; do
    stat_mode "$mode"
done

if [ -z "$SKIP_RECORD" ]; then
    echo
    echo "Hottest handlers ($RECORD_MODE). Tail calls mean there are no stack frames, so these are"
    echo "flat self-costs, which is exactly what is wanted here."
    echo
    data=$(mktemp)

    # Plain `cycles` samples land a variable distance past the instruction that actually stalled,
    # which is useless inside a twelve-instruction handler. `cycles:pp` maps to IBS on AMD and to
    # PEBS on Intel and attributes precisely, so prefer it and say which was used.
    probe=$(mktemp)
    if perf record -e cycles:pp -o "$probe" --quiet true >/dev/null 2>&1; then
        SAMPLE_EVENT=cycles:pp
    else
        SAMPLE_EVENT=cycles
    fi
    rm -f "$probe" "$probe.old" 2>/dev/null || true

    COREMARK_DISPATCH="$RECORD_MODE" perf record -e "$SAMPLE_EVENT" -F 4000 -o "$data" --quiet \
        taskset -c "$CORE" "$BIN" >/dev/null 2>&1 || true
    perf report -i "$data" --stdio --no-children --percent-limit 0.8 2>/dev/null \
        | grep -E "^ +[0-9]" \
        | sed -e 's/ab_riscv_coremark_runner::threaded::safe_handlers:://' \
              -e 's/execute_coremark_instruction_//' -e 's/_threaded\b//' \
              -e 's/::<.*>//' \
        | head -25

    echo
    echo "Cycle attribution inside the hottest handlers, sampled with $SAMPLE_EVENT."
    if [ "$SAMPLE_EVENT" = cycles ]; then
        echo "Not a precise event, so read it as 'the stall is around here', not 'on this line'."
    fi
    echo
    # No symbol argument on purpose: perf wants the full mangled-and-demangled name including the
    # generic arguments, and passing a shortened one silently annotates nothing. Letting it walk
    # every symbol above the limit cannot miss, and the hottest comes out first.
    perf annotate --stdio -i "$data" --percent-limit 2 2>/dev/null \
        | sed -e 's/ab_riscv_coremark_runner::threaded::safe_handlers:://' \
              -e 's/execute_coremark_instruction_//' -e 's/_threaded\b//' \
              -e 's/::<.*>//' \
        | head -70 || true

    rm -f "$data" "$data.old" 2>/dev/null || true
fi
