#![expect(incomplete_features, reason = "explicit_tail_calls")]
#![feature(
    const_block_items,
    const_cmp,
    const_trait_impl,
    const_try,
    const_try_residual,
    explicit_tail_calls,
    fn_align,
    signed_bigint_helpers,
    try_blocks
)]

mod elf;
mod histogram;
mod instruction;
mod interpreter;
mod threaded;
mod time_csr;

use crate::elf::{LoadedElf, load_elf};
use crate::instruction::CoremarkInstruction;
use crate::interpreter::{EagerInstructions, GuestMemory};
use crate::time_csr::TimeCsrState;
use ab_riscv_interpreter::basic::{BasicInterpreterState, BasicRegisters};
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use anyhow::Context;
use std::ffi::CStr;
use std::time::{Duration, Instant};

/// Coremark ELF binary compiled by build.rs for the RISC-V guest
const COREMARK_ELF: &[u8] = include_bytes!(env!("COREMARK_ELF"));
/// Guest virtual address of the trap / return sentinel.
///
/// The caller writes this into `ra` before calling `main`; when `main` returns, the interpreter
/// sees PC = 0 and halts.
const TRAP_ADDRESS: u64 = 0x0;
/// Base address at which the PIE ELF is loaded into guest memory.
///
/// Address 0 is safe as a trap sentinel because `set_pc` checks for `TRAP_ADDRESS` before any
/// memory access, so the interpreter halts cleanly without ever dereferencing it.
const MEMORY_BASE_ADDRESS: u64 = 0x0;
/// Total guest memory size.
///
/// Must be large enough to hold the ELF segments, stack, and output buffer.
const MEMORY_SIZE: usize = 512 * 1024;

/// Which way of getting from one guest instruction to the next to run.
///
/// The shipped path is [`Self::Threaded`] and it is what an unset `COREMARK_DISPATCH` selects, so
/// running the binary with no environment at all measures what the interpreter actually does. The
/// rest are here to be compared against it: [`Self::Loop`] is the `match` loop it replaced, and the
/// three prototype back ends are hand-written models of dispatch shapes the emitter does not
/// implement, kept so that a question about one can be answered by measuring rather than arguing.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Dispatch {
    /// Generated per-instruction handlers chained with `become`, the shipped path
    Threaded,
    /// The generic `match` loop in `BasicInterpreterState::execute()`
    Loop,
    /// Prototype: indirect (token) threading, a discriminant in the slot and a handler table
    Token,
    /// Prototype: direct threading with a full handler pointer in a sixteen-byte slot
    Direct,
    /// Prototype: direct threading with a handler offset and packed operands in eight bytes
    Packed,
}

impl Dispatch {
    /// Read the mode out of `COREMARK_DISPATCH`, defaulting to the shipped path
    fn from_env() -> anyhow::Result<Self> {
        let Ok(dispatch) = std::env::var("COREMARK_DISPATCH") else {
            return Ok(Self::Threaded);
        };

        match dispatch.as_str() {
            "threaded" => Ok(Self::Threaded),
            "loop" => Ok(Self::Loop),
            "token" => Ok(Self::Token),
            "direct" => Ok(Self::Direct),
            "packed" => Ok(Self::Packed),
            _ => Err(anyhow::anyhow!(
                "Unknown `COREMARK_DISPATCH` value {dispatch}, expected one of `threaded`, \
                `loop`, `token`, `direct` or `packed`"
            )),
        }
    }
}

/// Read the null-terminated Coremark output string from the output buffer
fn read_output<Memory>(memory: &Memory, addr: u64, size: u32) -> Option<&str>
where
    Memory: VirtualMemory,
{
    let slice = memory.read_slice_up_to(addr, size);
    CStr::from_bytes_until_nul(slice).ok()?.to_str().ok()
}

fn main() -> anyhow::Result<()> {
    if COREMARK_ELF.is_empty() {
        return Err(anyhow::anyhow!(
            "Coremark ELF not found, install `riscv64-unknown-elf-gcc` and/or specify `RISCV_CC` \
            environment variable to specify a different toolchain, use `build-elf-required` \
            feature to make ELF building required"
        ));
    }

    let dispatch = Dispatch::from_env()?;

    // Repeating the whole guest run in-process and reporting the best time makes results usable on
    // noisy machines, where a single run can easily be 10% off
    let repeats = std::env::var("COREMARK_REPEAT")
        .ok()
        .map(|repeats| repeats.parse::<u32>())
        .transpose()
        .context("`COREMARK_REPEAT` is not a number")?
        .unwrap_or(1)
        .max(1);

    let mut best_elapsed = None::<Duration>;

    for _ in 0..repeats {
        let host_elapsed = run_once(dispatch)?;
        best_elapsed = Some(match best_elapsed {
            Some(best) => best.min(host_elapsed),
            None => host_elapsed,
        });
    }

    println!(
        "Host elapsed: {:.3} s",
        best_elapsed
            .expect("At least one repeat; qed")
            .as_secs_f64()
    );

    Ok(())
}

/// Run Coremark once from a pristine guest state, print its output and return how long the
/// interpreter itself took
fn run_once(dispatch: Dispatch) -> anyhow::Result<Duration> {
    let mut memory = GuestMemory::<MEMORY_BASE_ADDRESS, MEMORY_SIZE>::default();
    let LoadedElf {
        entry_point,
        global_pointer,
        text_addr,
        text_data,
        output_buf_addr,
        output_buf_size,
    } = load_elf(COREMARK_ELF, &mut memory)?;

    // argv is a pointer-to-pointer: write output_buf_addr as a `u64` into guest memory, then pass
    // its address in a1. Stack pointer sits below that, 16-byte aligned per psABI.
    let stack_top = (MEMORY_BASE_ADDRESS + MEMORY_SIZE as u64) & !0xF;
    let argv_addr = stack_top - 8;
    let stack_pointer = argv_addr - 8;

    memory
        .write::<u64>(argv_addr, output_buf_addr)
        .context("argv slot does not fit in guest memory")?;

    let mut regs = BasicRegisters::<_, true>::default();
    regs.write(Reg::Ra, TRAP_ADDRESS);
    regs.write(Reg::Sp, stack_pointer);
    regs.write(Reg::Gp, global_pointer);
    regs.write(Reg::A0, 1);
    regs.write(Reg::A1, argv_addr);

    // SAFETY: ELF was produced by a trusted compiler
    let instructions = unsafe { EagerInstructions::decode(text_data, TRAP_ADDRESS, text_addr) };
    // SAFETY: `entry_point` is valid and aligned
    let instruction_fetcher = unsafe { instructions.fetcher(entry_point) };

    if std::env::var("COREMARK_HISTOGRAM").is_ok() {
        histogram::histogram(&mut BasicInterpreterState {
            regs,
            env: TimeCsrState::default(),
            memory,
            instruction_fetcher,
        })?;

        return Ok(Duration::ZERO);
    }

    let host_elapsed = match dispatch {
        Dispatch::Threaded => {
            let host_start = Instant::now();

            let ThreadedExecutionResult {
                outcome,
                program_counter: _,
            } = CoremarkInstruction::execute_threaded(
                instruction_fetcher,
                &mut regs,
                &mut TimeCsrState::default(),
                &mut memory,
            );

            outcome.context("Coremark execution failed")?;

            host_start.elapsed()
        }
        Dispatch::Loop => {
            let mut state = BasicInterpreterState {
                regs,
                env: TimeCsrState::default(),
                memory,
                instruction_fetcher,
            };

            let host_start = Instant::now();
            state.execute().context("Coremark execution failed")?;
            let host_elapsed = host_start.elapsed();

            memory = state.memory;

            host_elapsed
        }
        Dispatch::Token | Dispatch::Direct | Dispatch::Packed => {
            run_prototype::<threaded::ZeroStoreRegisters, _>(
                dispatch,
                instructions.decoded(),
                text_addr,
                entry_point,
                stack_pointer,
                global_pointer,
                argv_addr,
                &mut memory,
            )?
        }
    };

    let output = read_output(&memory, output_buf_addr, output_buf_size)
        .context("Coremark output not found in guest memory")?;
    print!("{output}");

    Ok(host_elapsed)
}

/// Run one of the prototype back ends over the same decoded instructions the shipped path walks.
///
/// Still generic over the register file even though only one is implemented, because that is the
/// design conclusion rather than an accident: the handlers do not know which one they got.
#[expect(
    clippy::too_many_arguments,
    reason = "guest setup, which the prototypes do not share with the interpreter"
)]
fn run_prototype<Regs, Memory>(
    dispatch: Dispatch,
    instructions: &[CoremarkInstruction],
    text_addr: u64,
    entry_point: u64,
    stack_pointer: u64,
    global_pointer: u64,
    argv_addr: u64,
    memory: &mut Memory,
) -> anyhow::Result<Duration>
where
    Regs: RegisterFile<Reg<u64>> + Default,
    Memory: VirtualMemory,
{
    let mut regs = Regs::default();
    regs.write(Reg::Ra, TRAP_ADDRESS);
    regs.write(Reg::Sp, stack_pointer);
    regs.write(Reg::Gp, global_pointer);
    regs.write(Reg::A0, 1);
    regs.write(Reg::A1, argv_addr);

    // Building a direct-threaded stream is setup rather than execution, but it is inside the timed
    // region on purpose: it is the cost token threading does not have, and a fair comparison pays
    // it. It is a single pass over the decoded stream and does not measurably move the score.
    let host_start = Instant::now();
    let stop = match dispatch {
        Dispatch::Packed => threaded::run_packed_threaded(
            instructions,
            text_addr,
            TRAP_ADDRESS,
            entry_point,
            &mut regs,
            memory,
        ),
        Dispatch::Direct => threaded::run_direct_threaded(
            instructions,
            text_addr,
            TRAP_ADDRESS,
            entry_point,
            &mut regs,
            memory,
        ),
        _ => threaded::run_threaded(
            instructions,
            text_addr,
            TRAP_ADDRESS,
            entry_point,
            &mut regs,
            memory,
        ),
    };
    let host_elapsed = host_start.elapsed();

    if stop != threaded::Stop::Done {
        return Err(anyhow::anyhow!("Threaded execution failed: {stop:?}"));
    }

    Ok(host_elapsed)
}
