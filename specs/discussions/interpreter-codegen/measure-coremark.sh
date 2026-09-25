#!/usr/bin/env bash
#
# Build an interpreter-targeted LLVM and measure what it is worth, by interpreting CoreMark and
# counting the exact number of guest instructions dispatched.
#
# Run it in an empty directory; everything it needs is created underneath. It is resumable - each
# step is skipped if its output is already there - so re-running after a failure is cheap.
#
#   mkdir /tmp/interp && cd /tmp/interp && /path/to/measure-coremark.sh
#
# Needs: git, curl, cmake, ninja, clang, a C++ compiler, rustup. Roughly 12 GB of disk and, on a
# 4-core machine, about an hour, nearly all of it building LLVM.
#
# Environment overrides:
#   LLVM_REPO / LLVM_REF   where the interpreter-targeted LLVM comes from
#   AB_REPO   / AB_REF     where the interpreter and the CoreMark runner come from
#   ITERATIONS             CoreMark iterations, fixed so every build does the same work (default 1000)
#   JOBS                   build parallelism (default: nproc)
#   TIMED_RUNS             wall-clock repeats per configuration (default 3, 0 to skip)
#   OPT_LEVEL              optimization level handed to `opt` and `llc` (default O3)
#
set -euo pipefail

LLVM_REPO=${LLVM_REPO:-https://github.com/nazar-pc/llvm-project}
LLVM_REF=${LLVM_REF:-claude/riscv-interpreter-target}
AB_REPO=${AB_REPO:-https://github.com/nazar-pc/abundance-wip}
AB_REF=${AB_REF:-claude/interpreter-targeted-codegen-results}
ITERATIONS=${ITERATIONS:-1000}
JOBS=${JOBS:-$(nproc)}
TIMED_RUNS=${TIMED_RUNS:-3}
OPT_LEVEL=${OPT_LEVEL:-O3}

ROOT=$(pwd)
GCC_PREFIX=$ROOT/gcc14/usr
LLVM_BIN=$ROOT/llvm-build/bin

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# ---------------------------------------------------------------- prerequisites
missing=()
for tool in git curl cmake ninja clang c++ rustup; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}" >&2
  echo "on Ubuntu: apt-get install git curl cmake ninja-build clang g++ && \\" >&2
  echo "           curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
fi

# ------------------------------------------------- RISC-V GCC, for `as`, `ld` and the reference row
#
# Used for three things: assembling and linking what LLVM emits, supplying the soft-float helpers
# behind CoreMark's `%f` reporting, and as the reference compiler to compare against.
if [ ! -x "$GCC_PREFIX/bin/riscv64-unknown-elf-gcc" ]; then
  say "fetching riscv64-unknown-elf GCC 14.2 and binutils 2.43.1"
  mkdir -p "$ROOT/debs" "$ROOT/gcc14"
  base=http://archive.ubuntu.com/ubuntu/pool/universe
  ( cd "$ROOT/debs"
    curl -fsSLO "$base/g/gcc-riscv64-unknown-elf/gcc-riscv64-unknown-elf_14.2.0+19_amd64.deb"
    curl -fsSLO "$base/b/binutils-riscv64-unknown-elf/binutils-riscv64-unknown-elf_2.43.1-4ubuntu1+7_amd64.deb" )
  for deb in "$ROOT"/debs/*.deb; do dpkg -x "$deb" "$ROOT/gcc14"; done
fi
"$GCC_PREFIX/bin/riscv64-unknown-elf-gcc" --version | head -1

# ------------------------------------------------------------------ LLVM under test
# Fetch the named ref every time rather than only on the first run, so that re-running after the
# branch moves measures the branch rather than whatever was cloned once.
sync_to() { # dir repo ref
  local dir=$1 repo=$2 ref=$3
  if [ ! -d "$dir/.git" ]; then
    say "cloning $repo ($ref)"
    git init -q "$dir"
    git -C "$dir" remote add origin "$repo"
  else
    git -C "$dir" remote set-url origin "$repo"
  fi
  local before after
  before=$(git -C "$dir" rev-parse --verify -q HEAD || true)
  git -C "$dir" fetch -q --depth 1 origin "$ref"
  git -C "$dir" checkout -q --force --detach FETCH_HEAD
  after=$(git -C "$dir" rev-parse HEAD)
  if [ "$before" != "$after" ]; then
    say "$(basename "$dir") now at $(git -C "$dir" log -1 --format='%h %s')"
  fi
}

sync_to "$ROOT/llvm-project" "$LLVM_REPO" "$LLVM_REF"

# Only `opt` and `llc` are needed: clang is used as a frontend only, and the system one will do,
# because -Xclang -disable-llvm-passes means it makes no optimization decisions.
if [ ! -f "$ROOT/llvm-build/build.ninja" ]; then
  say "configuring LLVM"
  cmake -S "$ROOT/llvm-project/llvm" -B "$ROOT/llvm-build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DLLVM_TARGETS_TO_BUILD=RISCV \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_BUILD_LLVM_DYLIB=ON -DLLVM_LINK_LLVM_DYLIB=ON \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF
fi
# Unconditional: ninja is a no-op when nothing changed, and skipping it on the existence of `llc`
# would silently keep measuring an older build of the branch.
say "building llc and opt (the long part on a first run)"
ninja -C "$ROOT/llvm-build" -j "$JOBS" llc opt
"$LLVM_BIN/llc" --version | sed -n 's/^ *LLVM version/LLVM version/p'

# --------------------------------------------------------- interpreter and CoreMark runner
sync_to "$ROOT/abundance-wip" "$AB_REPO" "$AB_REF"

# ------------------------------------------------------------------ compiler wrappers
#
# The CoreMark build script invokes $RISCV_CC and reruns only when that variable changes, so every
# configuration needs its own wrapper path or the ELF is silently reused.
mkdir -p "$ROOT/cc"

cat > "$ROOT/cc/_llvm-common.sh" <<EOF
#!/usr/bin/env bash
# clang is the frontend only - -disable-llvm-passes means it makes no optimization decisions, so
# both the mid-level pipeline and codegen come from the LLVM under test.
set -euo pipefail
BIN=$LLVM_BIN
GCC=$GCC_PREFIX/bin/riscv64-unknown-elf-gcc
CPU=\${CM_CPU:-generic-rv64}
OPT_LEVEL=$OPT_LEVEL
ATTR=\${CM_ATTR:-+m,+c,+zba,+zbb,+zbs}
read -r -a OPT_EXTRA <<< "\${CM_OPT_EXTRA:-}"
read -r -a LLC_EXTRA <<< "\${CM_LLC_EXTRA:-}"
WORK=\$(mktemp -d); trap 'rm -rf "\$WORK"' EXIT

SOURCES=(); FRONTEND=(); OUT=a.out
while [ \$# -gt 0 ]; do
  case "\$1" in
    *.c) SOURCES+=("\$1");;
    -o) OUT="\$2"; shift;;
    -I*|-D*|-O*|-march=*|-mabi=*|-ffreestanding|-Werror) FRONTEND+=("\$1");;
  esac
  shift
done

ASM=()
for src in "\${SOURCES[@]}"; do
  name=\$(basename "\$src" .c)
  clang --target=riscv64-unknown-elf "\${FRONTEND[@]}" -Xclang -disable-llvm-passes \\
    -S -emit-llvm -o "\$WORK/\$name.0.ll" "\$src"
  # Let -mcpu/-mattr decide rather than whatever clang's own -march put on each function
  sed -i 's/"target-features"="[^"]*"//g; s/"target-cpu"="[^"]*"//g' "\$WORK/\$name.0.ll"
  "\$BIN/opt" -mtriple=riscv64-unknown-elf -mcpu="\$CPU" -mattr="\$ATTR" -\$OPT_LEVEL \\
    \${OPT_EXTRA[@]+"\${OPT_EXTRA[@]}"} -S -o "\$WORK/\$name.ll" "\$WORK/\$name.0.ll"
  "\$BIN/llc" -mtriple=riscv64-unknown-elf -mcpu="\$CPU" -mattr="\$ATTR" -\$OPT_LEVEL \\
    \${LLC_EXTRA[@]+"\${LLC_EXTRA[@]}"} -relocation-model=pic -o "\$WORK/\$name.s" "\$WORK/\$name.ll"
  ASM+=("\$WORK/\$name.s")
done

"\$GCC" -march=rv64imc_zba_zbb_zbs -mabi=lp64 \\
  -ffreestanding -nostdlib -nostartfiles -static-pie -Wl,--entry=main \\
  "\${ASM[@]}" -lgcc -o "\$OUT"
[ -n "\${CM_KEEP:-}" ] && { mkdir -p "\$CM_KEEP"; cp "\$OUT" "\$CM_KEEP/coremark.elf"; }
exit 0
EOF
chmod +x "$ROOT/cc/_llvm-common.sh"

# One row per configuration: a name, a CPU, a -mattr string, and any extra `opt` flags. A CPU of
# "-" means GCC rather than LLVM; "-" in the other two fields means the default. Add rows to sweep.
#   name                       cpu                        -mattr                      extra opt flags
CONFIGS=${CONFIGS:-"
gcc-14.2                       -                          -                           -
llvm-generic-rv64              generic-rv64               -                           -
llvm-generic-interpreter-rv64  generic-interpreter-rv64   -                           -
"}

make_wrapper() { # name cpu attr extra
  local name=$1 cpu=$2 attr=$3 extra=$4 w=$ROOT/cc/$1.sh
  if [ "$cpu" = "-" ]; then
    cat > "$w" <<EOF
#!/usr/bin/env bash
set -euo pipefail
PATH=$GCC_PREFIX/bin:\$PATH $GCC_PREFIX/bin/riscv64-unknown-elf-gcc "\$@"
out=a.out; prev=
for a in "\$@"; do [ "\$prev" = "-o" ] && out=\$a; prev=\$a; done
mkdir -p $ROOT/elf/$name && cp "\$out" $ROOT/elf/$name/coremark.elf
EOF
  else
    cat > "$w" <<EOF
#!/usr/bin/env bash
export CM_CPU=$cpu
export CM_KEEP=$ROOT/elf/$name
$( [ "$attr" = "-" ] || echo "export CM_ATTR='$attr'" )
$( [ "$extra" = "-" ] || echo "export CM_OPT_EXTRA='$extra'
export CM_LLC_EXTRA='$extra'" )
exec $ROOT/cc/_llvm-common.sh "\$@"
EOF
  fi
  chmod +x "$w"
  echo "$w"
}

# -------------------------------------------------------------------------- measure
mkdir -p "$ROOT/elf"
cd "$ROOT/abundance-wip"

EXPECTED_CRC=${COREMARK_CRC:-}
bad=0

say "measuring"
printf '%-30s %14s %14s %12s %13s %12s %15s\n' \
  config 'dispatches' 'disp. w/fusion' 'static insts' 'fusable pairs' 'Iter/s' 'Iter/s w/fusion'

while read -r name cpu attr extra; do
  [ -z "${name:-}" ] && continue
  cc=$(make_wrapper "$name" "$cpu" "${attr:--}" "${extra:--}")

  # `|| true`, because a build failure here is a row that should say FAIL next to the rows that
  # worked, not a `set -e` abort that kills the sweep with no output at all.
  out=$(RISCV_CC=$cc COREMARK_DISPATCH_COUNT=1 COREMARK_ITERATIONS=$ITERATIONS \
        cargo run --release -q -p ab-riscv-coremark-runner 2>&1 || true)
  unfused=$(sed -n 's/^Dispatches: //p' <<<"$out")
  pairs=$(sed -n 's/^Instructions: \([0-9]*\), of which fused pairs: \([0-9]*\).*/\2/p' <<<"$out")
  insts=$(sed -n 's/^Instructions: \([0-9]*\),.*/\1/p' <<<"$out")
  crc=$(grep -o 'crcfinal *: *0x[0-9a-f]*' <<<"$out" | grep -o '0x[0-9a-f]*' | head -1)

  out=$(RISCV_CC=$cc COREMARK_FUSION=1 COREMARK_DISPATCH_COUNT=1 COREMARK_ITERATIONS=$ITERATIONS \
        cargo run --release -q -p ab-riscv-coremark-runner 2>&1 || true)
  fused=$(sed -n 's/^Dispatches: //p' <<<"$out")

  # Median of TIMED_RUNS, with the interpreter's pair fusion off and then on, because which of the
  # two dispatch counts a deployment cares about depends on whether it fuses.
  median_score() { # COREMARK_FUSION=0|1
    local scores=()
    for _ in $(seq "$TIMED_RUNS"); do
      scores+=("$(env "$1" RISCV_CC="$cc" COREMARK_ITERATIONS="$ITERATIONS" \
        cargo run --release -q -p ab-riscv-coremark-runner 2>&1 |
        sed -n 's/^Iterations\/Sec *: *//p' || true)")
    done
    printf '%s\n' "${scores[@]}" | sort -n | awk 'NR==int((NR+1)/2){print}' | head -1
  }

  score=- score_fused=-
  if [ "$TIMED_RUNS" -gt 0 ]; then
    score=$(median_score COREMARK_FUSION=0)
    score_fused=$(median_score COREMARK_FUSION=1)
  fi

  printf '%-30s %14s %14s %12s %13s %12s %15s\n' \
    "$name" "${unfused:-FAIL}" "${fused:-FAIL}" "${insts:-?}" "${pairs:-?}" \
    "${score:--}" "${score_fused:--}"

  # Every correct build computes the same checksum, so the only thing worth doing with it is
  # comparing, and the only interesting outcome is a mismatch - which means the row above is
  # measuring something that is not CoreMark.
  if [ -z "$crc" ]; then
    echo "  !! $name produced no checksum: the build or the run failed" >&2
    bad=1
  elif [ -z "$EXPECTED_CRC" ]; then
    EXPECTED_CRC=$crc
  elif [ "$crc" != "$EXPECTED_CRC" ]; then
    echo "  !! $name computed $crc, everything else computed $EXPECTED_CRC" >&2
    bad=1
  fi
done <<< "$CONFIGS"

cat <<EOF

The \`w/fusion\` columns are with the interpreter's adjacent-pair fusion enabled and the other two
without it; \`static insts\` and \`fusable pairs\` count the guest binary, not the execution.

Read the dispatch columns, not the scores: dispatch counts reproduce to five significant figures
(CoreMark prints its own elapsed time, so formatting a different number of digits costs a different
number of dispatches - expect the last few digits to move by around a hundred), while the
run-to-run spread of a score on identical binaries is around 3%. To resolve a difference smaller
than that, run the two builds alternately and take the per-round difference, rather than comparing
medians of separate batches.

Every row's CoreMark checksum was checked against the others; a mismatch would have been reported
above and is the only way this says anything about correctness.

Disassemble what was built:  riscv64-unknown-elf-objdump -d $ROOT/elf/<config>/coremark.elf
EOF

exit $bad
