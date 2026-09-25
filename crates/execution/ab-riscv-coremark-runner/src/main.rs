#![expect(incomplete_features, reason = "explicit_tail_calls")]
#![feature(
    const_trait_impl,
    const_try,
    const_try_residual,
    explicit_tail_calls,
    fn_align,
    signed_bigint_helpers,
    try_blocks
)]

mod elf;
mod instruction;
mod time_csr;

use crate::elf::{LoadedElf, load_elf};
use crate::instruction::CoremarkInstruction;
use crate::time_csr::TimeCsrState;
use ab_riscv_interpreter::basic::{
    BasicEagerInstructions, BasicInterpreterState, BasicMemory, BasicRegisters,
    CountingInstructionFetcher,
};
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use anyhow::Context;
use std::collections::HashMap;
use std::env;
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

/// Report how many pairs of the program's instructions fuse, which is what decides whether fusion
/// can do anything for it at all
fn print_fusion_stats(text_data: &[u8]) {
    let mut instructions = Vec::<CoremarkInstruction>::new();
    let mut offset = 0;
    while offset < text_data.len() {
        let word = match text_data.get(offset..) {
            Some([byte_0, byte_1, byte_2, byte_3, ..]) => {
                u32::from_le_bytes([*byte_0, *byte_1, *byte_2, *byte_3])
            }
            Some([byte_0, byte_1, ..]) => u32::from_le_bytes([*byte_0, *byte_1, 0, 0]),
            _ => break,
        };
        match <CoremarkInstruction as Instruction>::try_decode(word) {
            Some(instruction) => {
                instructions.push(instruction);
                offset += usize::from(instruction.size());
            }
            None => {
                offset += usize::from(<CoremarkInstruction as Instruction>::ALIGNMENT);
            }
        }
    }

    let fused = instructions
        .windows(2)
        .filter(|pair| CoremarkInstruction::fuse(pair[0], pair[1]).0 != pair[0])
        .count();

    println!(
        "Instructions: {}, of which fused pairs: {fused} ({:.2}%)",
        instructions.len(),
        fused as f64 * 100.0 / instructions.len() as f64
    );

    if env::var("COREMARK_FUSION_BREAKDOWN").is_ok_and(|value| value != "0") {
        print_fusion_breakdown(&instructions);
    }
}

/// Report which fusion rules fire and how often, which is what says where the win comes from and
/// which of them a compiler is failing to set up
fn print_fusion_breakdown(instructions: &[CoremarkInstruction]) {
    // The `Debug` name of a variant up to its payload identifies the instruction, and for the
    // result of a fusion it names the rule that fired.
    fn variant(instruction: CoremarkInstruction) -> String {
        let debug = format!("{instruction:?}");
        debug
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    let mut counts = HashMap::<String, usize>::new();
    for pair in instructions.windows(2) {
        let (fused, _) = CoremarkInstruction::fuse(pair[0], pair[1]);
        if fused == pair[0] {
            continue;
        }
        let key = format!(
            "{} + {} -> {}",
            variant(pair[0]),
            variant(pair[1]),
            variant(fused)
        );
        *counts.entry(key).or_default() += 1;
    }

    let mut counts = counts.into_iter().collect::<Vec<_>>();
    // Descending by count, then by name so that two runs of the same binary print the same order.
    counts.sort_unstable_by(|(left_key, left), (right_key, right)| {
        right.cmp(left).then_with(|| left_key.cmp(right_key))
    });
    println!("Fused pair breakdown:");
    for (key, count) in counts {
        println!("  {count:>5}  {key}");
    }
}

fn main() -> anyhow::Result<()> {
    let fusion = env::var("COREMARK_FUSION").is_ok_and(|value| value != "0");
    // Iterations/Sec is quoted to six digits but moves several percent between runs of the same
    // binary, which is more than a change to the compiler or the fusion rules usually is. Counting
    // what the interpreter dispatches instead is stable to five significant figures, at the cost of
    // running the `match` loop rather than threaded dispatch. Not to more than that: Coremark
    // reports its own elapsed time, and formatting a different number of digits dispatches a
    // different number of instructions, which moves the total by around a hundred in 300 million.
    let count_dispatches = env::var("COREMARK_DISPATCH_COUNT").is_ok_and(|value| value != "0");

    if COREMARK_ELF.is_empty() {
        return Err(anyhow::anyhow!(
            "Coremark ELF not found, install `riscv64-unknown-elf-gcc` and/or specify `RISCV_CC` \
            environment variable to specify a different toolchain, use `build-elf-required` \
            feature to make ELF building required"
        ));
    }

    let mut memory = BasicMemory::<MEMORY_BASE_ADDRESS, MEMORY_SIZE>::default();
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

    println!("Instruction fusion: {}", if fusion { "on" } else { "off" });
    print_fusion_stats(text_data);

    let host_start = std::time::Instant::now();

    let mut regs = BasicRegisters::<_, true>::default();
    regs.write(Reg::Ra, TRAP_ADDRESS);
    regs.write(Reg::Sp, stack_pointer);
    regs.write(Reg::Gp, global_pointer);
    regs.write(Reg::A0, 1);
    regs.write(Reg::A1, argv_addr);

    let fallback = CoremarkInstruction::Unimp {
        rs1: Reg::ZERO,
        rs2: Reg::ZERO,
    };
    // SAFETY: ELF was produced by a trusted compiler, `.text` section is loaded at `text_addr`
    // and ends with a jump, and the trap address is outside of it
    let instructions = unsafe {
        if fusion {
            BasicEagerInstructions::decode_fused(text_data, fallback, TRAP_ADDRESS, text_addr)
        } else {
            BasicEagerInstructions::decode(text_data, fallback, TRAP_ADDRESS, text_addr)
        }
    };
    // SAFETY: `entry_point` is valid and aligned
    let instruction_fetcher = unsafe { instructions.fetcher(entry_point) };

    let dispatches = if count_dispatches {
        let mut state = BasicInterpreterState {
            regs,
            env: TimeCsrState::default(),
            memory,
            instruction_fetcher: CountingInstructionFetcher::new(instruction_fetcher),
        };
        state.execute().context("Coremark execution failed")?;

        let dispatches = state.instruction_fetcher.dispatches();
        memory = state.memory;
        Some(dispatches)
    } else {
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

        None
    };

    let host_elapsed = host_start.elapsed();

    let output = read_output(&memory, output_buf_addr, output_buf_size)
        .context("Coremark output not found in guest memory")?;
    print!("{output}");

    println!("Host elapsed: {:.3} s", host_elapsed.as_secs_f64());
    if let Some(dispatches) = dispatches {
        println!("Dispatches: {dispatches}");
    }

    Ok(())
}
