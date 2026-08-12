//! Dynamic instruction histogram, enabled with `COREMARK_HISTOGRAM=1`.
//!
//! Runs the program through the same generic loop `BasicInterpreterState::execute()` uses, but
//! counts how often each instruction is executed. Useful for deciding which instructions are worth
//! specializing.

use crate::instruction::CoremarkInstruction;
use crate::interpreter::{EagerInstructionFetcher, GuestMemory};
use crate::time_csr::TimeCsrState;
use ab_riscv_interpreter::basic::{
    BasicInterpreterState, BasicRegisters, IllegalEcallSystemInstructionHandler,
};
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use core::ops::ControlFlow;

const N: usize = 1024;

type State<const BASE: u64, const SIZE: usize> = BasicInterpreterState<
    BasicRegisters<Reg<u64>>,
    TimeCsrState,
    GuestMemory<BASE, SIZE>,
    EagerInstructionFetcher,
    IllegalEcallSystemInstructionHandler,
>;

#[inline(never)]
pub(crate) fn histogram<const BASE: u64, const SIZE: usize>(
    state: &mut State<BASE, SIZE>,
) -> anyhow::Result<()> {
    let BasicInterpreterState {
        regs,
        ext_state,
        memory,
        instruction_fetcher,
        system_instruction_handler,
    } = state;

    let mut counts = vec![0u64; N];
    let mut examples = vec![None::<CoremarkInstruction>; N];

    loop {
        let instruction = match instruction_fetcher.fetch_instruction(memory) {
            FetchInstructionResult::Instruction(instruction) => instruction,
            FetchInstructionResult::Continue => continue,
            FetchInstructionResult::Break => break,
            FetchInstructionResult::Err(error) => return Err(anyhow::anyhow!("{error}")),
        };

        // SAFETY: instrumentation only; the enum was observed to carry a 2-byte discriminant at
        // offset 0
        let discriminant = usize::from(unsafe { (&raw const instruction).cast::<u16>().read() });
        counts[discriminant] += 1;
        examples[discriminant] = Some(instruction);

        let Rs1Rs2Operands { rs1, rs2 } = instruction.get_rs1_rs2_operands();
        let rs1rs2_values = Rs1Rs2OperandValues {
            rs1_value: regs.read(rs1),
            rs2_value: regs.read(rs2),
        };

        let outcome = instruction.execute(
            rs1rs2_values,
            regs,
            ext_state,
            memory,
            &mut *instruction_fetcher,
            system_instruction_handler,
        );

        // Same shape as `BasicInterpreterState::execute()`: the instruction says where execution
        // goes next, and applying that is the loop's job
        let control_flow = match outcome {
            ExecutionResult::Continue { rd, value } => {
                regs.write(rd, value);
                continue;
            }
            ExecutionResult::Branch { offset } => {
                instruction_fetcher.set_pc_relative(memory, instruction.size(), offset)
            }
            ExecutionResult::Jump { target } => instruction_fetcher.set_pc(memory, target),
            ExecutionResult::Break => break,
            ExecutionResult::Err(error) => return Err(anyhow::anyhow!("{error}")),
        };

        match control_flow {
            Ok(ControlFlow::Continue(())) => {}
            Ok(ControlFlow::Break(())) => break,
            Err(error) => return Err(anyhow::anyhow!("{error}")),
        }
    }

    let total = counts.iter().sum::<u64>();
    let mut sorted = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .collect::<Vec<_>>();
    sorted.sort_by_key(|(_, count)| core::cmp::Reverse(**count));

    println!("HISTOGRAM total={total} distinct={}", sorted.len());
    let mut cumulative = 0u64;
    for (discriminant, count) in sorted {
        let count = *count;
        cumulative += count;
        let example = examples[discriminant].expect("Counted; qed");
        let name = format!("{example}");
        let name = name.split_whitespace().next().unwrap_or("?").to_string();
        println!(
            "HIST {discriminant:>4} {name:<16} {count:>12} {:>6.2}% cum {:>6.2}%",
            count as f64 / total as f64 * 100.0,
            cumulative as f64 / total as f64 * 100.0
        );
    }

    Ok(())
}
