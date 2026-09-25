#!/usr/bin/env bash
#
# Measure `generic-interpreter-rv{32,64}` on a second, independent consumer: PolkaVM, which has
# both an interpreter and a recompiler, so it also answers whether the tuning survives a JIT.
#
# Run it in an empty directory. It builds PolkaVM's guest benchmarks twice - once with the target
# specification PolkaVM ships, once with only the `cpu` field changed - and runs its benchmark
# tool against each, printing both tables.
#
#   mkdir /tmp/pvbench && cd /tmp/pvbench && /path/to/bench-polkavm.sh
#
# Needs the patched rustc from `build-patched-rustc.sh`; point RUSTC at its stage 2 binary:
#
#   RUSTC=~/rustc-interp/rust/build/host/stage2/bin/rustc /path/to/bench-polkavm.sh
#
# Environment overrides:
#   RUSTC       the compiler that builds the guests (required)
#   PV_CARGO    rustup toolchain whose Cargo drives the build (default: nightly-2026-08-25)
#   PV_REF      PolkaVM commit to measure (default: the one these instructions were written for)
#   BENCHES     which benchmarks to run (default: pinky prime-sieve)
#   ITERATIONS  benchtool's iteration limit, passed through
#
set -euo pipefail

PV_REPO=${PV_REPO:-https://github.com/paritytech/polkavm}
PV_REF=${PV_REF:-28d460607f46c8609d644d8a84060fb92d94a272}
PV_CARGO=${PV_CARGO:-nightly-2026-08-25}
BENCHES=${BENCHES:-"pinky prime-sieve"}
ROOT=$(pwd)

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

if [ -z "${RUSTC:-}" ]; then
  echo "RUSTC must point at the patched rustc, e.g." >&2
  echo "  RUSTC=~/rustc-interp/rust/build/host/stage2/bin/rustc $0" >&2
  exit 1
fi
RUSTC=$(readlink -f "$RUSTC")
"$RUSTC" --print target-cpus --target riscv64gc-unknown-none-elf 2>/dev/null |
  grep -q generic-interpreter || {
    echo "$RUSTC does not know generic-interpreter; is it the patched build?" >&2
    exit 1
  }

# ------------------------------------------------------------------ checkout
if [ ! -d "$ROOT/polkavm/.git" ]; then
  say "cloning PolkaVM"
  git init -q "$ROOT/polkavm"
  git -C "$ROOT/polkavm" remote add origin "$PV_REPO"
fi
git -C "$ROOT/polkavm" fetch -q --depth 1 origin "$PV_REF"
git -C "$ROOT/polkavm" checkout -q --force --detach FETCH_HEAD

cd "$ROOT/polkavm"
# PolkaVM ships two copies of its target specifications. The `legacy` ones, which its build
# scripts use, spell `target-pointer-width` as a string, which a 2026 rustc rejects outright; the
# `1_91` ones differ in exactly that and are what a compiler of this vintage wants. Point
# everything that builds a guest at those instead.
TARGETS=crates/polkavm-linker/targets
sed -i "s|targets/legacy/|targets/1_91/|g" guest-programs/.cargo/config.toml \
                                           guest-programs/build-benchmarks.sh
SPECS=($TARGETS/1_91/riscv32emac-unknown-none-polkavm.json
       $TARGETS/1_91/riscv64emac-unknown-none-polkavm.json)

# PolkaVM pins a Cargo from early 2025, and a 2026 rustc refuses a `.json` target unless the
# invocation opts in - "custom targets are unstable and require `-Zunstable-options`". The opt-in
# is a Cargo flag, `-Zjson-target-spec`, which that Cargo is too old to know. So the Cargo has to
# come from a toolchain of roughly the same vintage as the patched compiler, while `RUSTC` still
# points at the patched compiler itself.
export RUSTC
export RUSTUP_TOOLCHAIN=$PV_CARGO

# `benchtool` looks for the guest blobs at a fixed path relative to its own manifest,
# `../../guest-programs/target`, while the guest build honours `CARGO_TARGET_DIR`. If one is set
# in the environment the blobs land somewhere benchtool will not look, and it reports nothing
# rather than failing.
unset CARGO_TARGET_DIR
if ! cargo --version >/dev/null 2>&1; then
  echo "toolchain $PV_CARGO is not installed; rustup toolchain install $PV_CARGO" >&2
  exit 1
fi

# Set through the config rather than on each command line. Every edit here is made against a
# fresh checkout, so none of them accumulate across runs.
CFG=guest-programs/.cargo/config.toml
grep -q '^json-target-spec' "$CFG" ||
  sed -i 's/^\[unstable\]$/[unstable]\njson-target-spec = true/' "$CFG"
grep -A3 '^\[unstable\]' "$CFG"

run_variant() { # name
  local name=$1
  say "building guests: $name"
  ( cd guest-programs && ./build-benchmarks.sh )

  say "measuring: $name"
  local filter
  for filter in $BENCHES; do
    # `benchtool` is not a member of the root workspace, so it is run from its own directory.
    # It also asks for `-C target-cpu=native` through its own Cargo config, which is what the
    # host build wants and has nothing to do with the guests.
    # `--no-default-features` leaves only the PolkaVM backend, which is the only one being
    # compared. The defaults drag in wasmtime, wasmer, wasmi and solana_rbpf, which cost several
    # gigabytes of memory to compile and are what the `wild` linker chokes on.
    #
    # The environment is cleared of `RUSTC` and of any ambient `RUSTFLAGS`: this is the harness,
    # the same build serves every variant, and PolkaVM asks for `-C target-cpu=native` through
    # its own Cargo config - which an inherited `RUSTFLAGS` would silently override, since the
    # environment wins over `[build] rustflags`.
    ( cd tools/benchtool &&
      env -u RUSTC -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
        cargo run -q --release --no-default-features -- \
          benchmark ${ITERATIONS:+-i "$ITERATIONS"} "$filter" ) |
      tee "$ROOT/$name-$filter.txt"
  done
}

# ------------------------------------------------------------------ specifications
#
# Three things have to be repaired before a 2026 compiler builds what PolkaVM intends.
#
# The `1_91` copies switched to above fix `target-pointer-width`, which the `legacy` ones spell
# as a string. The `abi` field is missing, and rustc now insists it match `llvm-abiname` for the
# E ABIs. And the three macro-fusion features are spelled the way LLVM spelled them before they
# were renamed to `fusion-*`, so a current LLVM ignores all three with a warning - the tuning
# PolkaVM assembled by hand has silently stopped working. Correcting the names is what makes the
# baseline PolkaVM as designed rather than PolkaVM as it happens to build today.
sed -i 's/+auipc-addi-fusion/+fusion-auipc-addi/; s/+ld-add-fusion/+fusion-ld-add/;
        s/+lui-addi-fusion/+fusion-lui-addi/' "${SPECS[@]}"

# `bench-memset` pins `compiler_builtins` to =0.1.139, which reaches for
# `core::intrinsics::atomic_load_unordered` - gone from a 2026 `core`, and not something to work
# around by bumping a pin, since `-Zbuild-std` supplies its own copy from the sysroot. It is not
# one of the benchmarks measured here and it is built last, so drop it and keep the rest.
sed -i 's/^build_benchmark "bench-memset"/# &/' guest-programs/build-benchmarks.sh

python3 - "${SPECS[@]}" <<'PYEOF'
import json, sys
for path in sys.argv[1:]:
    spec = json.loads(open(path).read())
    abi = spec.get("llvm-abiname")
    if abi in ("ilp32e", "lp64e") and not spec.get("abi"):
        spec["abi"] = abi
        open(path, "w").write(json.dumps(spec, indent=2) + "\n")
        print(f"{path}: added abi = {abi}")
PYEOF
grep -h '"features"' "${SPECS[1]}"

# ------------------------------------------------------------------ variants
say "baseline: PolkaVM's own tuning, with its fusion features actually taking effect"
grep -h '"cpu"' "${SPECS[@]}"
run_variant baseline

say "interpreter: same, with cpu = generic-interpreter-rv{32,64}"
sed -i 's/"cpu": "generic-rv32"/"cpu": "generic-interpreter-rv32"/' "${SPECS[0]}"
sed -i 's/"cpu": "generic-rv64"/"cpu": "generic-interpreter-rv64"/' "${SPECS[1]}"
grep -h '"cpu"' "${SPECS[@]}"
run_variant interpreter

cat <<EOF

One table per variant per benchmark is in $ROOT: <variant>-<benchmark>.txt

The rows to compare are polkavm32_interpreter / polkavm64_interpreter, which dispatch, and
polkavm32_compiler_no_gas / polkavm64_compiler_no_gas, which recompile. The interpreter rows are
where this tuning is meant to help; the compiler rows are the check that it does not hurt a
consumer that JITs.

Both variants have PolkaVM's macro-fusion features working, so what differs is the rest of the
tuning - chiefly the branch pricing - and the fusion features the CPU adds beyond the three
PolkaVM asks for.

Worth reporting upstream regardless of this experiment: as shipped, PolkaVM asks for
\`+auipc-addi-fusion,+ld-add-fusion,+lui-addi-fusion\`, which LLVM renamed to \`fusion-auipc-addi\`,
\`fusion-ld-add\` and \`fusion-lui-addi\`. The old names no longer exist, so all three are being
ignored.
EOF
