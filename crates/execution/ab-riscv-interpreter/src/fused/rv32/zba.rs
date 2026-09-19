//! Fused instructions of the RISC-V RV32 Zba extension

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

/// Fused instructions that pair a Zba address generation instruction with a load, which is LLVM's
/// `shxadd-load` macro fusion.
///
/// `addi` + `sh[123]add` is neither an LLVM macro fusion nor one any hardware implements. It is
/// here because it is what the code actually contains: indexing a field of an array of structures
/// computes the element offset first and adds the base second, and the offset dies right away.
///
/// The shift amount of `sh[123]add` is a field rather than a variant of its own: an interpreter
/// pays nothing for a variable shift, and three times as many variants would cost instruction
/// cache in every dispatch table they end up in.
///
/// See [module-level documentation](super::super) for what fusion is and when a pair may be fused.
#[instruction(inherit = [Rv32Instruction, Rv32ZbaInstruction])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rv32ZbaFusedInstruction<Reg> {
    // `addi` + `sh[123]add`
    #[instruction(if = [Addi, Sh1add], if = [Addi, Sh2add], if = [Addi, Sh3add])]
    FusedAddiShxadd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        imm: i16,
    },

    // `sh[123]add` + load
    #[instruction(if = [Sh1add, Lb], if = [Sh2add, Lb], if = [Sh3add, Lb])]
    FusedShxaddLb {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1add, Lh], if = [Sh2add, Lh], if = [Sh3add, Lh])]
    FusedShxaddLh {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1add, Lw], if = [Sh2add, Lw], if = [Sh3add, Lw])]
    FusedShxaddLw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1add, Lbu], if = [Sh2add, Lbu], if = [Sh3add, Lbu])]
    FusedShxaddLbu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1add, Lhu], if = [Sh2add, Lhu], if = [Sh3add, Lhu])]
    FusedShxaddLhu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
}

const _: () = {
    // Fused instructions must not make an instruction set larger than it is without them, see
    // `upper_immediate_fits()` for what that budget looks like
    assert!(
        size_of::<Rv32ZbaFusedInstruction<Reg<u32>>>() == size_of::<Rv32Instruction<Reg<u32>>>(),
        "Fused instructions must fit into an instruction of the instruction set they extend"
    );
};

#[instruction]
const impl<Reg> Instruction for Rv32ZbaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
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
const impl<Reg> FusedInstruction for Rv32ZbaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn fuse(prev: Self, next: Self) -> (Self, Self) {
        match (prev, next) {
            // `addi` + `sh[123]add`
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Sh1add {
                    rd, rs1: base, rs2, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && prev_rd != rs2 => (
                Self::FusedAddiShxadd {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    imm,
                },
                next,
            ),
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Sh2add {
                    rd, rs1: base, rs2, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && prev_rd != rs2 => (
                Self::FusedAddiShxadd {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    imm,
                },
                next,
            ),
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Sh3add {
                    rd, rs1: base, rs2, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && prev_rd != rs2 => (
                Self::FusedAddiShxadd {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    imm,
                },
                next,
            ),
            // `sh[123]add` + load
            (
                Self::Sh1add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lb {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLb {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lb {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLb {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lb {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLb {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lh {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLh {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lh {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLh {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lh {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLh {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lw {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLw {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lw {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLw {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lw {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLw {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lbu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLbu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lbu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLbu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lbu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLbu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lhu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLhu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lhu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLhu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lhu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLhu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
        }
    }
}

#[instruction]
impl<Reg> fmt::Display for Rv32ZbaFusedInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    /// A fused instruction prints as the first instruction of the pair it replaced, see
    /// [module-level documentation](super::super)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FusedAddiShxadd {
                rd,
                rs1,
                rs2: _,
                shamt: _,
                imm,
            } => write!(f, "addi {rd}, {rs1}, {imm}"),
            Self::FusedShxaddLb {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddLh {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddLw {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddLbu {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddLhu {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add {rd}, {rs1}, {rs2}"),
        }
    }
}

#[instruction_execution]
const impl<Reg> ExecutableInstructionOperands for Rv32ZbaFusedInstruction<Reg> where
    Reg: Register<Type = u32>
{
}

#[instruction_execution]
const impl<Reg, Env> ExecutableInstructionCsr<Env> for Rv32ZbaFusedInstruction<Reg> where
    Reg: Register<Type = u32>
{
}

#[instruction_execution]
const impl<Reg, Regs, Env, Memory, PC> ExecutableInstruction<Regs, Env, Memory, PC>
    for Rv32ZbaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
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
        memory: &mut Memory,
        _program_counter: &mut PC,
    ) -> ExecutionResult<Self::Reg> {
        match self {
            Self::FusedAddiShxadd {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                imm,
            } => {
                let value = (rs1_value.wrapping_add(i32::from(imm).cast_unsigned()) << shamt)
                    .wrapping_add(rs2_value);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedShxaddLb {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = i32::from(memory.read::<i8>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedShxaddLh {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = i32::from(memory.read::<i16>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedShxaddLw {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = memory.read::<u32>(u64::from(addr))?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedShxaddLbu {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = memory.read::<u8>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedShxaddLhu {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = memory.read::<u16>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
        }
    }
}
