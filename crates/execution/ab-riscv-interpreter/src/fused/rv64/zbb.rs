//! Fused instructions of the RISC-V RV64 Zbb extension

#[cfg(test)]
mod tests;

use crate::fused::FusedInstruction;
use crate::rv64::b::zbb::rv64_zbb_helpers;
use crate::{
    ExecutableInstruction, ExecutableInstructionCsr, ExecutableInstructionOperands, ExecutionError,
    ExecutionResult, FetchInstructionResult, InstructionFetcher, OpaqueThreadedExecutionResult,
    PackedAddress, ProgramCounter, RegisterFile, Rs1Rs2OperandValues, Rs1Rs2Operands,
    SystemInstructionHandler, ThreadedExecutableInstruction, ThreadedExecutionResult,
    VirtualMemory,
};
use ab_riscv_macros::{instruction, instruction_execution};
use ab_riscv_primitives::prelude::*;
use core::fmt;
use core::ops::ControlFlow;

/// Fused instructions that pair `xor` with a rotate by a constant.
///
/// Unlike the rest of the fused instructions, this is not a macro fusion any hardware implements,
/// and LLVM has no feature for it. It is here because it is what the code actually contains: `xor`
/// followed by `rori`/`roriw` is the core of every ARX construction, and in a contract built
/// around hashing it is one of the most frequent pairs where the first result dies immediately.
///
/// See [module-level documentation](super::super) for what fusion is and when a pair may be fused.
#[instruction(inherit = [Rv64Instruction, Rv64ZbbInstruction])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rv64ZbbFusedInstruction<Reg> {
    // `xor` + rotate by a constant
    #[instruction(if = [Xor, Rori])]
    FusedXorRori {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
    },
    #[instruction(if = [Xor, Roriw])]
    FusedXorRoriw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
    },
}

const _: () = {
    // Fused instructions must not make an instruction set larger than it is without them, see
    // `upper_immediate_fits()` for what that budget looks like
    assert!(
        size_of::<Rv64ZbbFusedInstruction<Reg<u64>>>() == size_of::<Rv64Instruction<Reg<u64>>>(),
        "Fused instructions must fit into an instruction of the instruction set they extend"
    );
};

#[instruction]
const impl<Reg> Instruction for Rv64ZbbFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    const ALIGNMENT: u8 = align_of::<u32>() as u8;

    type Reg = Reg;

    /// Fused instructions are not encoded by anything, they only ever come out of
    /// [`FusedInstruction::fuse()`]
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    /// Every fused instruction here replaces a pair of 32-bit instructions
    #[inline(always)]
    fn size(&self) -> u8 {
        2 * size_of::<u32>() as u8
    }
}

#[instruction]
const impl<Reg> FusedInstruction for Rv64ZbbFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn fuse(prev: Self, next: Self) -> (Self, Self) {
        match (prev, next) {
            // `xor` + rotate by a constant
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                    ..
                },
                Self::Rori {
                    rd,
                    rs1: base,
                    shamt,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedXorRori {
                    rd,
                    rs1,
                    rs2,
                    shamt,
                },
                next,
            ),
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                    ..
                },
                Self::Roriw {
                    rd,
                    rs1: base,
                    shamt,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedXorRoriw {
                    rd,
                    rs1,
                    rs2,
                    shamt,
                },
                next,
            ),
        }
    }
}

#[instruction]
impl<Reg> fmt::Display for Rv64ZbbFusedInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    /// A fused instruction prints as the first instruction of the pair it replaced, see
    /// [module-level documentation](super::super)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FusedXorRori {
                rd,
                rs1,
                rs2,
                shamt: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedXorRoriw {
                rd,
                rs1,
                rs2,
                shamt: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
        }
    }
}

#[instruction_execution]
const impl<Reg> ExecutableInstructionOperands for Rv64ZbbFusedInstruction<Reg> where
    Reg: Register<Type = u64>
{
}

#[instruction_execution]
const impl<Reg, Env> ExecutableInstructionCsr<Env> for Rv64ZbbFusedInstruction<Reg> where
    Reg: Register<Type = u64>
{
}

#[instruction_execution]
const impl<Reg, Regs, Env, Memory, PC> ExecutableInstruction<Regs, Env, Memory, PC>
    for Rv64ZbbFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
    Regs: [const] RegisterFile<Reg>,
    Memory: [const] VirtualMemory,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn execute(
        self,
        Rs1Rs2OperandValues {
            rs1_value,
            rs2_value,
        }: Rs1Rs2OperandValues<<Self::Reg as Register>::Type>,
        _regs: &mut Regs,
        _env: &mut Env,
        _memory: &mut Memory,
        _program_counter: &mut PC,
    ) -> ExecutionResult<Self::Reg> {
        match self {
            Self::FusedXorRori {
                rd,
                rs1: _,
                rs2: _,
                shamt,
            } => {
                let value = (rs1_value ^ rs2_value).rotate_right(u32::from(shamt & 0x3f));
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorRoriw {
                rd,
                rs1: _,
                rs2: _,
                shamt,
            } => {
                let value = i64::from(
                    ((rs1_value ^ rs2_value) as u32)
                        .rotate_right(u32::from(shamt & 0x1f))
                        .cast_signed(),
                );
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
        }
    }
}
