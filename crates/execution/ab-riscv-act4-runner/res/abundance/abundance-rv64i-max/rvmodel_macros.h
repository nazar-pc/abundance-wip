// rvmodel_macros.h - abundance RV64 cores (ACT4 self-checking framework)
//
// Shared by all abundance-rv64i-* cores (symlinked from other core directories).
//
// Halt mechanism: HTIF tohost write (Sail's exit protocol).
//   Writing (exit_code << 1) | 1 to tohost signals exit.
//   Code 0 = pass: write 1. Code != 0 = fail: write (code<<1)|1.
//
// No console, no timer, no interrupts implemented.

// ---------------------------------------------------------------------------
// These cores are M-mode only (no S/U), but do implement a real, writable
// mtvec and dispatch traps (ecall, illegal instruction, ...) through it -
// see TestEnv::take_trap() in the runner. Defining this lets the framework
// install and use its own real trap handler, instead of leaving tests that
// rely on it (e.g. any RVTEST_GOTO_MMODE user, or extensions - like Zkr -
// whose tests are registered under priv/) with no working trap handler at
// all. RVMODEL_BOOT_TO_MMODE is left undefined, the default boot works.
// ---------------------------------------------------------------------------
#define STANDARD_SM_SUPPORTED

// ---------------------------------------------------------------------------
// HTIF tohost helper: writes VALUE to tohost then spins.
// Clobbers t0, t1.
// ---------------------------------------------------------------------------
#define _RVMODEL_HTIF_EXIT(value)       \
    .option push;                       \
    .option norvc;                      \
    la   t0, tohost;                    \
    li   t1, value;                     \
    sd   t1, 0(t0);                     \
    1: j 1b;                            \
    .option pop

// ---------------------------------------------------------------------------
// Required: halt with pass (exit code 0 -> tohost = 1)
// ---------------------------------------------------------------------------
#define RVMODEL_HALT_PASS  _RVMODEL_HTIF_EXIT(1)

// ---------------------------------------------------------------------------
// Required: halt with fail (exit code 1 -> tohost = 3, i.e. (1<<1)|1)
// ---------------------------------------------------------------------------
#define RVMODEL_HALT_FAIL  _RVMODEL_HTIF_EXIT(3)

# Perform boot operations. Can be empty or left undefined unless needed for
# DUT-specific behavior such as turning on a memory controller or
# initializing custom state.
//#define RVMODEL_BOOT

// ---------------------------------------------------------------------------
// HTIF tohost/fromhost placement.
// ---------------------------------------------------------------------------
#define RVMODEL_DATA_SECTION \
    .pushsection .tohost,"aw",@progbits;                \
    .balign 8; .global tohost; tohost: .dword 0;        \
    .balign 8; .global fromhost; fromhost: .dword 0;    \
    .popsection

// ---------------------------------------------------------------------------
// Console I/O: no console available, leave blank.
// The macro takes register arguments (_R1, _R2, _R3) and a string pointer.
// ---------------------------------------------------------------------------
#define RVMODEL_IO_WRITE_STR(_R1, _R2, _R3, _STR_PTR)

// ---------------------------------------------------------------------------
// Access fault address: below RAM base, guaranteed to fault on load/store.
// ---------------------------------------------------------------------------
#define RVMODEL_ACCESS_FAULT_ADDRESS 0x00000000

// ---------------------------------------------------------------------------
// Interrupts: there is no CLINT, interrupt controller or timer accessible
// from test code, and MEI_INTR_IMPL/MSI_INTR_IMPL/MTI_INTR_IMPL are all false
// in the UDB config, so no interrupt is ever raised. RVMODEL_MTIME_ADDRESS,
// RVMODEL_MTIMECMP_ADDRESS, RVMODEL_MSIP_ADDRESS and the
// RVMODEL_{SET,CLR}_*_INT macros are intentionally left undefined, letting
// the framework's trap handler abort on any unexpected interrupt. The CLINT
// address in sail.json is for Sail's own reference-model use only.
//
// The two below are still required under STANDARD_SM_SUPPORTED, but are
// never used with no interrupts implemented.
// ---------------------------------------------------------------------------
#define RVMODEL_INTERRUPT_LATENCY 1
#define RVMODEL_TIMER_INT_SOON_DELAY 1
