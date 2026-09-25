#!/usr/bin/env bash
#
# Measure `generic-interpreter-rv64` on libriscv, a consumer that dispatches RISC-V directly
# rather than lowering it to its own bytecode first - which is what PolkaVM does, and why PolkaVM
# shows nothing.
#
# The workload is CoreMark again, but a different build of it: libriscv wants a hosted binary, so
# this one is linked against picolibc with a small ecall-based host interface, and validated by
# CoreMark's own CRCs. It reports guest instructions dispatched (exact, from libriscv's counter)
# and host wall-clock time.
#
# Run it in an empty directory, or in the same directory as `measure-coremark.sh`, in which case
# it reuses that run's LLVM build and GCC instead of repeating them.
#
#   mkdir /tmp/interp && cd /tmp/interp && /path/to/bench-libriscv.sh
#
# Needs: git, curl, cmake, ninja, clang, a C++ compiler, dpkg. Every step is skipped if its output
# is already there, so re-running after a failure is cheap.
#
# Environment overrides:
#   LLVM_REPO / LLVM_REF   where the interpreter-targeted LLVM comes from
#   LR_REF                 libriscv commit to measure (default: the one this was written against)
#   ITERATIONS             CoreMark iterations (default 40000; shorter runs are lost in host noise)
#   TIMED_RUNS             wall-clock repeats per configuration (default 9)
#   PIN_CPU                core to pin the runs to (default 2; empty to not pin)
#   JOBS                   build parallelism (default: nproc)
#
set -euo pipefail

LLVM_REPO=${LLVM_REPO:-https://github.com/nazar-pc/llvm-project}
LLVM_REF=${LLVM_REF:-claude/riscv-interpreter-target}
LR_REPO=${LR_REPO:-https://github.com/libriscv/libriscv}
LR_REF=${LR_REF:-e121610a7b88bb902e9d2a33b4cdfc14847003e5}
CM_REPO=${CM_REPO:-https://github.com/eembc/coremark}
CM_REF=${CM_REF:-main}
ITERATIONS=${ITERATIONS:-40000}
TIMED_RUNS=${TIMED_RUNS:-9}
PIN_CPU=${PIN_CPU-2}
JOBS=${JOBS:-$(nproc)}

ROOT=$(pwd)
GCC_PREFIX=$ROOT/gcc14/usr
PICO=$ROOT/pico/usr/riscv64-unknown-elf
LLVM_BIN=$ROOT/llvm-build/bin

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

missing=()
for tool in git curl cmake ninja clang c++ dpkg; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}" >&2
  echo "on Ubuntu: apt-get install git curl cmake ninja-build clang g++ dpkg" >&2
  exit 1
fi

sync_to() { # dir repo ref
  local dir=$1 repo=$2 ref=$3 before after
  if [ ! -d "$dir/.git" ]; then
    say "cloning $repo ($ref)"
    git init -q "$dir"
    git -C "$dir" remote add origin "$repo"
  else
    git -C "$dir" remote set-url origin "$repo"
  fi
  before=$(git -C "$dir" rev-parse --verify -q HEAD || true)
  git -C "$dir" fetch -q --depth 1 origin "$ref"
  git -C "$dir" checkout -q --force --detach FETCH_HEAD
  after=$(git -C "$dir" rev-parse HEAD)
  [ "$before" = "$after" ] || say "$(basename "$dir") now at $(git -C "$dir" log -1 --format='%h %s')"
}

# --------------------------------------------- RISC-V GCC, used only to assemble and link
if [ ! -x "$GCC_PREFIX/bin/riscv64-unknown-elf-gcc" ]; then
  say "fetching riscv64-unknown-elf GCC 14.2 and binutils 2.43.1"
  mkdir -p "$ROOT/debs" "$ROOT/gcc14"
  base=http://archive.ubuntu.com/ubuntu/pool/universe
  ( cd "$ROOT/debs"
    curl -fsSLO "$base/g/gcc-riscv64-unknown-elf/gcc-riscv64-unknown-elf_14.2.0+19_amd64.deb"
    curl -fsSLO "$base/b/binutils-riscv64-unknown-elf/binutils-riscv64-unknown-elf_2.43.1-4ubuntu1+7_amd64.deb" )
  for deb in "$ROOT"/debs/*.deb; do dpkg -x "$deb" "$ROOT/gcc14"; done
fi

# --------------------------------------------- picolibc, so CoreMark's stdio and %f work
if [ ! -f "$PICO/lib/rv64i/lp64/libc.a" ]; then
  say "fetching picolibc for riscv64-unknown-elf"
  mkdir -p "$ROOT/debs" "$ROOT/pico"
  curl -fsSL -o "$ROOT/debs/picolibc.deb" \
    http://archive.ubuntu.com/ubuntu/pool/universe/p/picolibc/picolibc-riscv64-unknown-elf_1.8.12-1_all.deb
  dpkg -x "$ROOT/debs/picolibc.deb" "$ROOT/pico"
fi

# picolibc's linker script packs .text and .rodata into one page. libriscv maps each segment with
# its own permissions, so a page that is both the tail of an executable segment and the head of a
# read-only one is rejected before the program starts. One ALIGN fixes it and changes nothing else.
if [ ! -f "$ROOT/picolibc-aligned.ld" ]; then
  sed -e 's/^\t\.rodata : {/\t.rodata : ALIGN(4096) {/' "$PICO/lib/picolibc.ld" > "$ROOT/picolibc-aligned.ld"
  grep -q 'rodata : ALIGN' "$ROOT/picolibc-aligned.ld" || { echo "linker script patch did not apply" >&2; exit 1; }
fi

# --------------------------------------------- LLVM under test
sync_to "$ROOT/llvm-project" "$LLVM_REPO" "$LLVM_REF"
if [ ! -f "$ROOT/llvm-build/build.ninja" ]; then
  say "configuring LLVM"
  cmake -S "$ROOT/llvm-project/llvm" -B "$ROOT/llvm-build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DLLVM_TARGETS_TO_BUILD=RISCV \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_BUILD_LLVM_DYLIB=ON -DLLVM_LINK_LLVM_DYLIB=ON \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF
fi
say "building llc and opt (the long part on a first run)"
ninja -C "$ROOT/llvm-build" -j "$JOBS" llc opt
"$LLVM_BIN/llc" --version | sed -n 's/^ *LLVM version/LLVM version/p'

sync_to "$ROOT/coremark" "$CM_REPO" "$CM_REF"
sync_to "$ROOT/libriscv" "$LR_REPO" "$LR_REF"

# --------------------------------------------- the guest's host interface
mkdir -p "$ROOT/guest"
cat > "$ROOT/guest/guest_support.c" <<'EOF'
/* Minimal host interface for running CoreMark under a RISC-V interpreter: Linux-style ecalls for
   write and exit, and a monotonic clock from the cycle counter so CoreMark's own timing works. */
#include <stddef.h>
#include <stdio.h>
#include <time.h>

static long syscall3(long n, long a, long b, long c) {
    register long a7 __asm__("a7") = n;
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return a0;
}

int _write(int fd, const char *buf, int len) { return (int)syscall3(64, fd, (long)buf, len); }
void _exit(int code) { syscall3(93, code, 0, 0); __builtin_unreachable(); }

/* CoreMark divides by the elapsed time, so this has to move. */
clock_t clock(void) {
    unsigned long c;
    __asm__ volatile("rdcycle %0" : "=r"(c));
    return (clock_t)c;
}

int _close(int fd) { (void)fd; return -1; }
int _fstat(int fd, void *st) { (void)fd; (void)st; return -1; }
int _isatty(int fd) { (void)fd; return 1; }
long _lseek(int fd, long off, int whence) { (void)fd; (void)off; (void)whence; return -1; }
int _read(int fd, char *buf, int len) { (void)fd; (void)buf; (void)len; return -1; }
int _kill(int pid, int sig) { (void)pid; (void)sig; return -1; }
int _getpid(void) { return 1; }

/* picolibc's tinystdio leaves `stdout` to the application. */
static int stdout_putc(char c, FILE *f) {
    (void)f;
    return _write(1, &c, 1) == 1 ? (unsigned char)c : -1;
}
static FILE __stdout = FDEV_SETUP_STREAM(stdout_putc, NULL, NULL, _FDEV_SETUP_WRITE);
FILE *const stdout = &__stdout;
FILE *const stderr = &__stdout;

/* Entry point. The interpreter loads the segments and sets the stack pointer, so all that is
   left is to establish `gp` for gp-relative addressing and call main. No crt0: .data comes from
   the file and .bss is zeroed by the loader. */
extern int main(int, char **);
__attribute__((naked, used, section(".text.init"))) void _start(void) {
    __asm__ volatile(
        ".option push\n"
        ".option norelax\n"
        "lla gp, __global_pointer$\n"
        ".option pop\n"
        "li a0, 0\n"
        "li a1, 0\n"
        "call main\n"
        "call _exit\n");
}
EOF

# --------------------------------------------- build CoreMark twice
#
# clang is the frontend only: -disable-llvm-passes means it makes no optimization decisions, so
# both the mid-level pipeline and codegen come from the LLVM under test. The CPU under test is the
# only thing that differs between the two builds.
build_guest() { # cpu out
  local cpu=$1 out=$2 work n src objs=()
  work=$(mktemp -d); trap 'rm -rf "$work"' RETURN
  for src in core_list_join core_matrix core_state core_util core_main posix/core_portme; do
    n=$(basename "$src")
    clang --target=riscv64-unknown-elf -isystem "$PICO/include" \
      -I"$ROOT/coremark" -I"$ROOT/coremark/posix" \
      -DITERATIONS="$ITERATIONS" -DPERFORMANCE_RUN=1 -DUSE_CLOCK=1 \
      -DSEED_METHOD=SEED_VOLATILE -DMAIN_HAS_NOARGC=1 -DFLAGS_STR='"-O3"' \
      -w -O3 -Xclang -disable-llvm-passes -S -emit-llvm -o "$work/$n.0.ll" "$ROOT/coremark/$src.c"
    # clang stamps its own host defaults into every function; strip them so -mcpu decides.
    sed -i 's/"target-features"="[^"]*"//g; s/"target-cpu"="[^"]*"//g' "$work/$n.0.ll"
    "$LLVM_BIN/opt" -mtriple=riscv64-unknown-elf -mcpu="$cpu" -mattr=+m,+c,+zba,+zbb,+zbs \
      -O3 -S -o "$work/$n.ll" "$work/$n.0.ll"
    "$LLVM_BIN/llc" -mtriple=riscv64-unknown-elf -mcpu="$cpu" -mattr=+m,+c,+zba,+zbb,+zbs \
      -O3 -o "$work/$n.s" "$work/$n.ll"
    objs+=("$work/$n.s")
  done
  clang --target=riscv64-unknown-elf -isystem "$PICO/include" -I"$ROOT/coremark" \
    -w -O2 -fno-addrsig -S -o "$work/guest_support.s" "$ROOT/guest/guest_support.c"
  objs+=("$work/guest_support.s")
  "$GCC_PREFIX/bin/riscv64-unknown-elf-gcc" -march=rv64imc_zba_zbb_zbs -mabi=lp64 \
    -nostdlib -nostartfiles -Wl,--entry=_start -T "$ROOT/picolibc-aligned.ld" \
    -Wl,--defsym=__flash=0x10000 -Wl,--defsym=__flash_size=0x800000 \
    -Wl,--defsym=__ram=0x810000 -Wl,--defsym=__ram_size=0x300000 \
    -Wl,--defsym=__stack_size=0x80000 \
    "${objs[@]}" -L"$PICO/lib/rv64i/lp64" -lc -lm -lgcc -o "$out"
}

say "building CoreMark for generic-rv64 and generic-interpreter-rv64"
build_guest generic-rv64 "$ROOT/guest/generic-rv64.elf"
build_guest generic-interpreter-rv64 "$ROOT/guest/generic-interpreter-rv64.elf"

# --------------------------------------------- the libriscv harness
mkdir -p "$ROOT/harness"
cat > "$ROOT/harness/CMakeLists.txt" <<'EOF'
cmake_minimum_required(VERSION 3.14)
project(lrbench LANGUAGES CXX)
set(CMAKE_INTERPROCEDURAL_OPTIMIZATION TRUE)
option(RISCV_64I "" ON)
option(RISCV_EXT_C "" ON)
add_subdirectory(${LIBRISCV_DIR}/lib libriscv)
add_executable(lrbench main.cpp)
target_link_libraries(lrbench riscv)
EOF
cat > "$ROOT/harness/main.cpp" <<'EOF'
// Run a bare-metal RV64 ELF under libriscv and report how long the host took.
//
// No `setup_linux`: the guest is a freestanding picolibc binary with its own entry point, so the
// Linux stack and auxv that `setup_linux` builds are neither wanted nor compatible with its
// memory layout. All it needs is a stack pointer and somewhere for `write` and `exit` to go.
#include <chrono>
#include <cstdio>
#include <fstream>
#include <iostream>
#include <vector>
#include <libriscv/machine.hpp>
using namespace riscv;

int main(int argc, char** argv) {
    if (argc < 2) { std::cerr << "usage: " << argv[0] << " <elf>\n"; return 2; }
    std::ifstream stream(argv[1], std::ios::in | std::ios::binary);
    if (!stream) { std::cerr << argv[1] << ": not found\n"; return 2; }
    const std::vector<uint8_t> binary((std::istreambuf_iterator<char>(stream)),
                                      std::istreambuf_iterator<char>());
    try {
        Machine<RISCV64> machine{binary, {.memory_max = 512UL << 20}};

        machine.install_syscall_handler(64, [](Machine<RISCV64>& m) {   // write
            const auto [fd, buf, len] = m.sysargs<int, address_type<RISCV64>, size_t>();
            std::vector<uint8_t> data(len);
            m.copy_from_guest(data.data(), buf, len);
            std::fwrite(data.data(), 1, len, fd == 2 ? stderr : stdout);
            m.set_result(len);
        });
        machine.install_syscall_handler(93, [](Machine<RISCV64>& m) {   // exit
            m.stop();
        });

        machine.cpu.reg(REG_SP) = 0x20000000;

        const auto t0 = std::chrono::steady_clock::now();
        machine.simulate(1'000'000'000'000ull);
        const auto t1 = std::chrono::steady_clock::now();
        std::fflush(stdout);
        std::fprintf(stderr, "GUEST_INSTRUCTIONS %llu\n", (unsigned long long)machine.instruction_counter());
        std::fprintf(stderr, "HOST_ELAPSED_MS %.3f\n",
                     std::chrono::duration<double, std::milli>(t1 - t0).count());
        return 0;
    } catch (const MachineException& e) {
        std::cerr << "machine-exception: " << e.what() << " type=" << e.type()
                  << " data=0x" << std::hex << e.data() << "\n";
        return 1;
    } catch (const std::exception& e) {
        std::cerr << "failed: " << e.what() << "\n";
        return 1;
    }
}
EOF
if [ ! -f "$ROOT/harness/build/build.ninja" ]; then
  cmake -S "$ROOT/harness" -B "$ROOT/harness/build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DLIBRISCV_DIR="$ROOT/libriscv"
fi
ninja -C "$ROOT/harness/build" -j "$JOBS"

# --------------------------------------------- run
say "running"
run_one() {
  if [ -n "$PIN_CPU" ]; then taskset -c "$PIN_CPU" "$ROOT/harness/build/lrbench" "$1"
  else "$ROOT/harness/build/lrbench" "$1"; fi
}
for cpu in generic-rv64 generic-interpreter-rv64; do
  elf=$ROOT/guest/$cpu.elf
  out=$(run_one "$elf" 2>&1 >/dev/null)
  first=$(run_one "$elf" 2>/dev/null | grep -E 'Correct operation|ERROR')
  printf '%-28s text=%s bytes  instructions=%s\n  %s\n' "$cpu" \
    "$("$GCC_PREFIX/bin/riscv64-unknown-elf-size" -A "$elf" | awk '/^\.text/{print $2}')" \
    "$(sed -n 's/^GUEST_INSTRUCTIONS //p' <<< "$out")" "$first"
  printf '  wall-clock ms:'
  for _ in $(seq 1 "$TIMED_RUNS"); do
    printf ' %s' "$(run_one "$elf" 2>&1 >/dev/null | sed -n 's/^HOST_ELAPSED_MS //p')"
  done
  printf '\n'
done
