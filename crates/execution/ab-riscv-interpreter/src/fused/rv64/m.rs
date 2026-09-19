//! Fused instructions of the RISC-V RV64 M extension

#[cfg(test)]
mod tests;

use crate::fused::FusedInstruction;
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

/// A multiply followed by an add of the product, which is LLVM's `mul-add` macro fusion: `mul` +
/// `add` and `mulw` + `addw`.
///
/// The register added to the product is read after the multiply would have written its
/// destination, so it must not be that destination - a fused instruction would read the value it
/// had before the multiply.
///
/// See [module-level documentation](super::super) for what fusion is and when a pair may be fused.
#[instruction(inherit = [Rv64Instruction, Rv64MInstruction])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rv64MFusedInstruction<Reg> {
    #[instruction(if = [Mul, Add])]
    FusedMulAdd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Mulw, Addw])]
    FusedMulwAddw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
}

const _: () = {
    // Fused instructions must not make an instruction set larger than it is without them, see
    // `fused_helpers::upper_immediate_fits()` for what that budget looks like
    assert!(
        size_of::<Rv64MFusedInstruction<Reg<u64>>>() == size_of::<Rv64Instruction<Reg<u64>>>(),
        "Fused instructions must fit into an instruction of the instruction set they extend"
    );
};

#[instruction]
const impl<Reg> Instruction for Rv64MFusedInstruction<Reg>
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
const impl<Reg> FusedInstruction for Rv64MFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn fuse(prev: Self, next: Self) -> (Self, Self) {
        match (prev, next) {
            (
                Self::Mul {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Add {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedMulAdd { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Mulw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Addw {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedMulwAddw { rd, rs1, rs2, rs3 }, next)
            }
        }
    }
}

#[instruction]
impl<Reg> fmt::Display for Rv64MFusedInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    /// A fused instruction prints as the first instruction of the pair it replaced, see
    /// [module-level documentation](super::super)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FusedMulAdd {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "mul {rd}, {rs1}, {rs2}"),
            Self::FusedMulwAddw {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "mulw {rd}, {rs1}, {rs2}"),
        }
    }
}

#[instruction_execution]
const impl<Reg> ExecutableInstructionOperands for Rv64MFusedInstruction<Reg> where
    Reg: Register<Type = u64>
{
}

#[instruction_execution]
const impl<Reg, Env> ExecutableInstructionCsr<Env> for Rv64MFusedInstruction<Reg> where
    Reg: Register<Type = u64>
{
}

#[instruction_execution]
const impl<Reg, Regs, Env, Memory, PC> ExecutableInstruction<Regs, Env, Memory, PC>
    for Rv64MFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
    Regs: [const] RegisterFile<Reg>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn execute(
        self,
        Rs1Rs2OperandValues {
            rs1_value,
            rs2_value,
        }: Rs1Rs2OperandValues<<Self::Reg as Register>::Type>,
        regs: &mut Regs,
        _env: &mut Env,
        _memory: &mut Memory,
        _program_counter: &mut PC,
    ) -> ExecutionResult<Self::Reg> {
        match self {
            Self::FusedMulAdd {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = rs1_value
                    .wrapping_mul(rs2_value)
                    .wrapping_add(regs.read(rs3));
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedMulwAddw {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let product = (rs1_value as i32).wrapping_mul(rs2_value as i32);
                let sum = product.wrapping_add(regs.read(rs3) as i32);
                ExecutionResult::Continue {
                    rd,
                    value: i64::from(sum).cast_unsigned(),
                }
            }
        }
    }
}
