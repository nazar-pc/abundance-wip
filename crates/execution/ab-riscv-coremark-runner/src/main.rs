#![expect(incomplete_features, reason = "explicit_tail_calls")]
#![feature(
    const_cmp,
    explicit_tail_calls,
    const_trait_impl,
    const_try,
    const_try_residual,
    explicit_tail_calls,
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
use crate::interpreter::{EagerInstructionFetcher, GuestMemory};
use crate::time_csr::TimeCsrState;
use ab_riscv_interpreter::basic::{
    BasicInterpreterState, BasicRegisters, IllegalEcallSystemInstructionHandler,
};
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use anyhow::Context;
use std::ffi::CStr;

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
    // Repeating the whole guest run in-process and reporting the best time makes results usable on
    // noisy machines, where a single run can easily be 10% off
    let repeats = std::env::var("COREMARK_REPEAT")
        .ok()
        .map(|repeats| repeats.parse::<u32>())
        .transpose()
        .context("`COREMARK_REPEAT` is not a number")?
        .unwrap_or(1)
        .max(1);

    let mut best_elapsed = None::<std::time::Duration>;

    for _ in 0..repeats {
        let host_elapsed = run_once()?;
        best_elapsed = Some(match best_elapsed {
            Some(best) => best.min(host_elapsed),
            None => host_elapsed,
        });
    }

    println!(
        "Host elapsed: {:.3} s",
        best_elapsed.expect("At least one repeat; qed").as_secs_f64()
    );

    Ok(())
}

/// Run Coremark once from a pristine guest state, print its output and return how long the
/// interpreter itself took
fn run_once() -> anyhow::Result<std::time::Duration> {
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

    let mut regs = BasicRegisters::default();
    regs.write(Reg::Ra, TRAP_ADDRESS);
    regs.write(Reg::Sp, stack_pointer);
    regs.write(Reg::Gp, global_pointer);
    regs.write(Reg::A0, 1);
    regs.write(Reg::A1, argv_addr);

    // SAFETY: entry_point is valid and aligned; ELF was produced by a trusted compiler
    let instruction_fetcher =
        unsafe { EagerInstructionFetcher::new(text_data, TRAP_ADDRESS, text_addr, entry_point) };

    let mut state = BasicInterpreterState {
        regs,
        ext_state: TimeCsrState::default(),
        memory,
        instruction_fetcher,
        system_instruction_handler: IllegalEcallSystemInstructionHandler,
    };

    if std::env::var("COREMARK_HISTOGRAM").is_ok() {
        crate::histogram::histogram(&mut state)?;
        return Ok(std::time::Duration::ZERO);
    }

    if let Ok(dispatch) = std::env::var("COREMARK_DISPATCH") {
        let dispatch = threaded::Dispatch::parse(&dispatch)
            .with_context(|| format!("Unknown `COREMARK_DISPATCH` value {dispatch}"))?;

        // The safe back end runs against exactly the same `BasicRegisters` and `GuestMemory` the
        // generic loop uses, borrowed out of the state that already owns them
        if dispatch == threaded::Dispatch::SafeBranchless {
            let BasicInterpreterState {
                memory,
                instruction_fetcher,
                ..
            } = &mut state;

            let mut regs = threaded::BranchlessRegisters::default();
            regs.write(Reg::Ra, TRAP_ADDRESS);
            regs.write(Reg::Sp, stack_pointer);
            regs.write(Reg::Gp, global_pointer);
            regs.write(Reg::A0, 1);
            regs.write(Reg::A1, argv_addr);

            let host_start = std::time::Instant::now();
            let stop = threaded::run_safe(
                instruction_fetcher.instructions(),
                text_addr,
                TRAP_ADDRESS,
                entry_point,
                &mut regs,
                memory,
            );
            let host_elapsed = host_start.elapsed();

            if stop != threaded::Stop::Done {
                return Err(anyhow::anyhow!("Threaded execution failed: {stop:?}"));
            }

            let output = read_output(&state.memory, output_buf_addr, output_buf_size)
                .context("Coremark output not found in guest memory")?;
            print!("{output}");

            return Ok(host_elapsed);
        }

        if dispatch == threaded::Dispatch::Safe {
            let BasicInterpreterState {
                regs,
                memory,
                instruction_fetcher,
                ..
            } = &mut state;

            let host_start = std::time::Instant::now();
            let stop = threaded::run_safe(
                instruction_fetcher.instructions(),
                text_addr,
                TRAP_ADDRESS,
                entry_point,
                regs,
                memory,
            );
            let host_elapsed = host_start.elapsed();

            if stop != threaded::Stop::Done {
                return Err(anyhow::anyhow!("Threaded execution failed: {stop:?}"));
            }

            let output = read_output(&state.memory, output_buf_addr, output_buf_size)
                .context("Coremark output not found in guest memory")?;
            print!("{output}");

            return Ok(host_elapsed);
        }

        let mut ctx = threaded::Ctx::new(
            state.instruction_fetcher.instructions(),
            text_addr,
            TRAP_ADDRESS,
            dispatch,
        );

        load_elf(COREMARK_ELF, ctx.as_mut())?;
        ctx.write_u64(argv_addr, output_buf_addr);
        ctx.set_reg(Reg::Ra, TRAP_ADDRESS);
        ctx.set_reg(Reg::Sp, stack_pointer);
        ctx.set_reg(Reg::Gp, global_pointer);
        ctx.set_reg(Reg::A0, 1);
        ctx.set_reg(Reg::A1, argv_addr);

        let host_start = std::time::Instant::now();
        let stop = ctx.run(dispatch, entry_point);
        let host_elapsed = host_start.elapsed();

        if stop != threaded::Stop::Done {
            return Err(anyhow::anyhow!("Threaded execution failed: {stop:?}"));
        }

        let output = read_output(ctx.as_ref(), output_buf_addr, output_buf_size)
            .context("Coremark output not found in guest memory")?;
        print!("{output}");

        return Ok(host_elapsed);
    }

    let host_start = std::time::Instant::now();

    state.execute().context("Coremark execution failed")?;

    let host_elapsed = host_start.elapsed();

    let output = read_output(&state.memory, output_buf_addr, output_buf_size)
        .context("Coremark output not found in guest memory")?;
    print!("{output}");

    Ok(host_elapsed)
}
