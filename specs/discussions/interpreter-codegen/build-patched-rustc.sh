#!/usr/bin/env bash
#
# Build a rustc whose LLVM carries the interpreter-targeted RISC-V tuning, so the contract
# workloads - not just CoreMark - can be compiled with `generic-interpreter-rv64`.
#
# Run it in an empty directory; everything it needs is created underneath. It is resumable - each
# step is skipped if its output is already there.
#
#   mkdir /tmp/rustc-interp && cd /tmp/rustc-interp && /path/to/build-patched-rustc.sh
#
# Needs: git, curl, cmake, ninja, python3, a C++ compiler, rustup. Roughly 40 GB of disk, and
# several hours on a 4-core machine - it builds LLVM and then rustc twice. `JOBS` is worth setting.
#
# Environment overrides:
#   CHANNEL      which nightly to reproduce (default: the one this repository pins)
#   LLVM_REPO / LLVM_REF   where the interpreter-targeted LLVM comes from
#   AB_REPO   / AB_REF     where the interpreter and the benchmarks come from
#   TOOLCHAIN    name to register with rustup (default riscv-interp)
#   LLVM_COMMITS how many commits at the tip of LLVM_REF make up the change (default 2: one adds
#                the CPUs, the next tunes them)
#   JOBS         build parallelism (default: nproc)
#
set -euo pipefail

CHANNEL=${CHANNEL:-nightly-2026-08-25}
LLVM_REPO=${LLVM_REPO:-https://github.com/nazar-pc/llvm-project}
LLVM_REF=${LLVM_REF:-claude/riscv-interpreter-target}
AB_REPO=${AB_REPO:-https://github.com/nazar-pc/abundance-wip}
AB_REF=${AB_REF:-claude/interpreter-codegen-on-main}
TOOLCHAIN=${TOOLCHAIN:-riscv-interp}
LLVM_COMMITS=${LLVM_COMMITS:-2}
JOBS=${JOBS:-$(nproc)}

ROOT=$(pwd)
say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

missing=()
for tool in git curl cmake ninja python3 c++ rustup; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}" >&2
  exit 1
fi

# ------------------------------------------------------ which rustc commit to reproduce
say "resolving $CHANNEL"
rustup toolchain install --profile minimal "$CHANNEL"
RUSTC_COMMIT=$(rustc "+$CHANNEL" -vV | sed -n 's/^commit-hash: //p')
LLVM_VERSION=$(rustc "+$CHANNEL" -vV | sed -n 's/^LLVM version: //p')
echo "rustc commit $RUSTC_COMMIT, LLVM $LLVM_VERSION"

# ------------------------------------------------------------------ rust checkout
if [ ! -d "$ROOT/rust/.git" ]; then
  git init -q "$ROOT/rust"
  git -C "$ROOT/rust" remote add origin https://github.com/rust-lang/rust
fi
# `--is-ancestor` rather than an equality check: re-running finds HEAD one commit ahead, on the
# commit that pins the patched LLVM below, and re-checking-out would throw that away.
if ! git -C "$ROOT/rust" merge-base --is-ancestor "$RUSTC_COMMIT" HEAD 2>/dev/null; then
  say "fetching rust-lang/rust at $RUSTC_COMMIT"
  git -C "$ROOT/rust" fetch -q --depth 1 origin "$RUSTC_COMMIT"
  git -C "$ROOT/rust" checkout -q --force --detach "$RUSTC_COMMIT"
fi

# rustc pins its own LLVM fork, which is the one that has to be patched.
if [ ! -d "$ROOT/rust/src/llvm-project/llvm" ]; then
  say "fetching rustc's LLVM fork"
  git -C "$ROOT/rust" submodule update --init --depth 1 src/llvm-project
fi

# ------------------------------------------------------------------ the patch
#
# Taken from the LLVM branch rather than from a file, so that what is applied is exactly what was
# measured. rustc's fork is a few months behind upstream, so a hunk may need adjusting; the failure
# below tells you which one.
#
# The source commit is recorded in the trailer of the commit this makes, so that re-running after
# the branch moves re-applies the new version instead of leaving the old one in place.
LLVMDIR=$ROOT/rust/src/llvm-project
BASEFILE=$ROOT/.llvm-pinned-commit
MARKER='interpreter-target-source: '

marker_of() { git -C "$LLVMDIR" log -1 --format=%B | sed -n "s/^$MARKER//p"; }

# The commit rustc actually pins, remembered before anything is applied on top of it, so that a
# later re-apply has something to reset to. The submodule is cloned shallow, so `HEAD^` is not
# available for this.
if [ ! -f "$BASEFILE" ]; then
  if [ -n "$(marker_of)" ]; then
    echo "$LLVMDIR already carries a patch but $BASEFILE is missing;" >&2
    echo "re-run 'git -C $ROOT/rust submodule update --force --checkout src/llvm-project' first" >&2
    exit 1
  fi
  git -C "$LLVMDIR" rev-parse HEAD > "$BASEFILE"
fi
LLVM_BASE=$(cat "$BASEFILE")

git -C "$LLVMDIR" remote add interp "$LLVM_REPO" 2>/dev/null || \
  git -C "$LLVMDIR" remote set-url interp "$LLVM_REPO"
# One commit deeper than the change itself, because the diff needs something to start from, and a
# shallow clone that stops exactly at the oldest wanted commit would silently produce a diff of the
# entire LLVM tree instead.
git -C "$LLVMDIR" fetch -q --depth $((LLVM_COMMITS + 1)) interp "$LLVM_REF"
LLVM_FROM=FETCH_HEAD~$LLVM_COMMITS
if ! git -C "$LLVMDIR" rev-parse -q --verify "$LLVM_FROM" >/dev/null; then
  echo "$LLVM_REF has fewer than $LLVM_COMMITS commits to diff; set LLVM_COMMITS" >&2
  exit 1
fi

# Only the backend files matter for a compiler build. The change also touches tests and
# llvm/docs/RISCVUsage.md, which upstream renamed from .rst after the release rustc pins, so
# applying those would fail for no reason.
#
# The scheduling model is left out too. `RISCVSchedInterpreter.td` is a copy of
# `RISCVSchedSpacemitX60.td` from LLVM main, and names scheduling classes rustc's older LLVM does
# not have, so tablegen stops on the first of them. It is a copy with the names changed and nothing
# else, so the CPUs point at the release's own `SpacemitX60Model` instead, which is also what
# `generic-rv64` uses there.
git -C "$LLVMDIR" diff "$LLVM_FROM" FETCH_HEAD -- 'llvm/lib/Target/RISCV/*' \
  ':!llvm/lib/Target/RISCV/RISCV.td' ':!llvm/lib/Target/RISCV/RISCVSchedInterpreter.td' |
  sed 's/\bInterpreterModel\b/SpacemitX60Model/g' > "$ROOT/interpreter-target.patch"
# A diff of the whole backend means the starting point was wrong; applying it would be a mess.
if [ "$(wc -c < "$ROOT/interpreter-target.patch")" -gt 1000000 ]; then
  echo "the generated patch is implausibly large; LLVM_COMMITS is probably too large" >&2
  exit 1
fi

# The source commit and the patch made from it, so that a change to either - including to how this
# script makes the patch - re-applies it instead of leaving the old one in place.
WANT="$(git -C "$LLVMDIR" rev-parse FETCH_HEAD) $(git hash-object "$ROOT/interpreter-target.patch")"

if [ "$(marker_of)" != "$WANT" ]; then
  say "applying the interpreter-target patch to rustc's LLVM"
  # Unconditional, so that a moved branch replaces the old patch instead of stacking on it.
  git -C "$LLVMDIR" reset -q --hard "$LLVM_BASE"
  git -C "$LLVMDIR" clean -qdf
  if ! git -C "$LLVMDIR" apply --index "$ROOT/interpreter-target.patch"; then
    cat >&2 <<EOF

The patch did not apply to rustc's LLVM $LLVM_VERSION unchanged. The backend part is only:
  llvm/lib/Target/RISCV/RISCVFeatures.td       (one tuning feature)
  llvm/lib/Target/RISCV/RISCVProcessors.td     (three CPUs)
  llvm/lib/Target/RISCV/RISCVSubtarget.h       (isJumpExpensive)
  llvm/lib/Target/RISCV/RISCVTargetTransformInfo.{h,cpp}
Reapply by hand with
  cd $LLVMDIR && git apply --reject $ROOT/interpreter-target.patch
and fix the .rej files, then re-run this script.
EOF
    exit 1
  fi
  git -C "$LLVMDIR" -c user.email=none -c user.name=none commit -q \
    -m 'RISCV: generic-interpreter CPUs' -m "$MARKER$WANT"
  # Nothing to invalidate by hand. Bootstrap hashes the submodule's git state into
  # `build/<host>/llvm/.llvm-stamp` and rebuilds when it stops matching, and the commit made just
  # above changes it. An earlier version of this deleted the whole LLVM build directory here, which
  # threw away every object file and turned a one-line change to a .td file into a full LLVM build;
  # ninja rebuilds only what tablegen invalidated. To force a rebuild anyway, delete that stamp.
else
  say "the patch is already current, so LLVM will not be rebuilt"
fi

# Bootstrap manages `src/llvm-project` itself: it compares the gitlink recorded in the superproject
# against the submodule's HEAD and, on a mismatch, runs `submodule update` followed by
# `reset --hard` and `clean -qdfx`. That silently throws the patch away and builds a stock LLVM, so
# move the gitlink onto the patched commit and the comparison finds nothing to do.
git -C "$ROOT/rust" add src/llvm-project
if ! git -C "$ROOT/rust" diff --cached --quiet; then
  git -C "$ROOT/rust" -c user.email=none -c user.name=none commit -q \
    -m 'pin the interpreter-targeted LLVM'
fi
PINNED=$(git -C "$ROOT/rust" ls-tree HEAD src/llvm-project | awk '{print $3}')
if [ "$PINNED" != "$(git -C "$LLVMDIR" rev-parse HEAD)" ]; then
  echo "the gitlink still does not point at the patched LLVM; bootstrap would reset it" >&2
  exit 1
fi

# ------------------------------------------------------------------ build
#
# Building only the host and RISC-V LLVM backends. download-ci-llvm must be off or bootstrap would
# fetch a prebuilt LLVM and ignore the patch entirely, which fails silently.
CONF=$ROOT/rust/bootstrap.toml
[ -f "$ROOT/rust/config.toml" ] && CONF=$ROOT/rust/config.toml
if [ -f "$CONF" ] && ! grep -q '^change-id' "$CONF"; then
  printf 'change-id = "ignore"\n' | cat - "$CONF" > "$CONF.new" && mv "$CONF.new" "$CONF"
fi
if [ ! -f "$CONF" ]; then
  say "configuring"
  cat > "$CONF" <<EOF
change-id = "ignore"

[llvm]
download-ci-llvm = false
targets = "RISCV;X86"
assertions = false
optimize = true

[rust]
channel = "nightly"
debug-assertions = false
incremental = false

[build]
extended = false
docs = false
EOF
fi

# Unconditional: bootstrap is incremental, and skipping it on the existence of the stage 2 binary
# would keep an older LLVM in place after the branch moved.
say "building rustc stage 2 (hours on a first run)"
( cd "$ROOT/rust" && ./x build --stage 2 -j "$JOBS" library )

# `-Zbuild-std=core`, which the contract build uses, wants the standard library source where the
# rust-src component would have put it.
SRCDIR=$ROOT/rust/build/host/stage2/lib/rustlib/src/rust
if [ ! -e "$SRCDIR" ]; then
  mkdir -p "$(dirname "$SRCDIR")"
  ln -s "$ROOT/rust" "$SRCDIR"
fi

say "registering the toolchain as '$TOOLCHAIN'"
rustup toolchain uninstall "$TOOLCHAIN" >/dev/null 2>&1 || true
rustup toolchain link "$TOOLCHAIN" "$ROOT/rust/build/host/stage2"
rustc "+$TOOLCHAIN" -vV

# ------------------------------------------------------------------ check it took
say "checking that the new CPU is visible to rustc"
if rustc "+$TOOLCHAIN" --print target-cpus --target riscv64gc-unknown-none-elf 2>/dev/null |
     grep -q generic-interpreter; then
  echo "generic-interpreter is available"
else
  echo "generic-interpreter NOT found." >&2
  if [ "$(marker_of)" != "$WANT" ]; then
    echo "The patch is no longer in $LLVMDIR - something reset the submodule during the build." >&2
  elif [ -d "$ROOT/rust/build/host/ci-llvm" ]; then
    echo "Bootstrap used the prebuilt $ROOT/rust/build/host/ci-llvm instead of building one." >&2
    echo "Set download-ci-llvm = false in $CONF, remove that directory and re-run." >&2
  else
    echo "The patch is still applied and LLVM was built from source, so the build did not pick" >&2
    echo "up the new .td files. Remove $ROOT/rust/build/host/llvm and re-run." >&2
  fi
  exit 1
fi

# ------------------------------------------------------------------ measure
# Re-fetched on every run rather than cloned once, so that re-running after the branch moves, or
# with a different AB_REF, measures what is asked for. A clone left at an older ref and then
# half-updated by hand is how a tree ends up with one branch's interpreter and another's macros,
# which fails in the build script with nothing to suggest the checkout is the problem.
if [ ! -d "$ROOT/abundance-wip/.git" ]; then
  say "cloning $AB_REPO"
  git init -q "$ROOT/abundance-wip"
  git -C "$ROOT/abundance-wip" remote add origin "$AB_REPO"
fi
if [ -n "$(git -C "$ROOT/abundance-wip" status --porcelain 2>/dev/null)" ]; then
  echo "$ROOT/abundance-wip has local changes; commit, stash or remove them" >&2
  exit 1
fi
say "fetching $AB_REF"
git -C "$ROOT/abundance-wip" fetch -q --depth 1 origin "$AB_REF"
git -C "$ROOT/abundance-wip" checkout -q --force --detach FETCH_HEAD
git -C "$ROOT/abundance-wip" clean -qdff

cd "$ROOT/abundance-wip"

# The host stays on the toolchain `rust-toolchain.toml` pins and only the guest is built with the
# patched compiler, named by absolute path.
#
# `RUSTUP_TOOLCHAIN` would not do: Cargo hands build scripts `RUSTC` as an absolute path into the
# host toolchain rather than as the `rustup` shim, and the nested Cargo honors that variable, so the
# override is inherited straight past and the guest is built by the host compiler. The stage 2
# sysroot has no `cargo` of its own either, so the shim would fall back to the host's anyway.
GUEST_RUSTC=$ROOT/rust/build/host/stage2/bin/rustc

# The dispatch counter lives on the branch that carries the interpreter's fusion pass, so on a
# branch without it there is nothing quick to measure and the toolchain is simply reported as ready.
# Either way this is a convenience, not the deliverable: the toolchain above is already usable.
if [ -f crates/execution/ab-riscv-benchmarks/examples/dispatch_count.rs ]; then
  set +e
  say "measuring - the toolchain above is already usable if this part fails"

  say "baseline: generic-rv64"
  AB_GUEST_TOOLCHAIN=$GUEST_RUSTC \
    cargo run --release -q -p ab-riscv-benchmarks --example dispatch_count | tail -n 3

  say "generic-interpreter-rv64"
  AB_GUEST_TOOLCHAIN=$GUEST_RUSTC AB_GUEST_CPU=generic-interpreter-rv64 \
    cargo run --release -q -p ab-riscv-benchmarks --example dispatch_count | tail -n 3
  set -e

  cat <<EOF

Both rows above are dispatch counts for the same two workloads; they do not move between runs.
EOF
else
  say "$AB_REF has no dispatch_count example, so nothing was measured"
fi

cat <<EOF

To keep using it, set the two variables on any Cargo command in this checkout:
  AB_GUEST_TOOLCHAIN=$GUEST_RUSTC   # which compiler builds the guest
  AB_GUEST_CPU=generic-interpreter-rv64              # which LLVM CPU it targets

An absolute path is taken as the compiler itself; anything else is taken as a \`rustup\` toolchain
name (\`$TOOLCHAIN\` is linked, but has no \`cargo\`, so the path is the one to use). Leaving
\`AB_GUEST_CPU\` unset means the checked-in \`generic-rv64\`.

Wall clock for the same workloads, which needs more than a few percent of difference to be readable:
  AB_GUEST_TOOLCHAIN=$GUEST_RUSTC AB_GUEST_CPU=generic-interpreter-rv64 \\
    cargo bench -p ab-riscv-benchmarks --bench riscv -- 'interpreter/threaded/eager'
EOF
