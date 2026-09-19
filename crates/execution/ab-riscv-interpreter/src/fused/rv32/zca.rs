//! Fused instructions of the RISC-V RV32 Zca extension

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

/// A branch that jumps over a single `c.mv`, fused into a conditional move, which is LLVM's
/// `conditional-cmv` macro fusion.
///
/// This is the one fusion here that is worth more to an interpreter than the dispatch it saves: a
/// data-dependent branch in the guest becomes a select in the host, so a guest branch that no
/// predictor can get right stops costing a misprediction on every execution of it.
///
/// Both the moved value and the current value of the destination register are read unconditionally
/// for that reason, and the one that the condition selects is written back.
///
/// See [module-level documentation](super::super) for what fusion is and when a pair may be fused.
/// The condition of this one is different from the rest: there is no intermediate value to be
/// dead, what has to hold instead is that the branch jumps exactly over the move and nothing else,
/// which the branch offset says outright.
#[instruction(inherit = [Rv32Instruction, Rv32ZcaInstruction])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rv32ZcaFusedInstruction<Reg> {
    // Branch over `c.mv`
    #[instruction(if = [Beq, CMv])]
    FusedBeqCMv {
        rs1: Reg,
        rs2: Reg,
        rd: Reg,
        mv_rs2: Reg,
    },
    #[instruction(if = [Bne, CMv])]
    FusedBneCMv {
        rs1: Reg,
        rs2: Reg,
        rd: Reg,
        mv_rs2: Reg,
    },
    #[instruction(if = [Blt, CMv])]
    FusedBltCMv {
        rs1: Reg,
        rs2: Reg,
        rd: Reg,
        mv_rs2: Reg,
    },
    #[instruction(if = [Bge, CMv])]
    FusedBgeCMv {
        rs1: Reg,
        rs2: Reg,
        rd: Reg,
        mv_rs2: Reg,
    },
    #[instruction(if = [Bltu, CMv])]
    FusedBltuCMv {
        rs1: Reg,
        rs2: Reg,
        rd: Reg,
        mv_rs2: Reg,
    },
    #[instruction(if = [Bgeu, CMv])]
    FusedBgeuCMv {
        rs1: Reg,
        rs2: Reg,
        rd: Reg,
        mv_rs2: Reg,
    },

    // Compressed branch over `c.mv`
    #[instruction(if = [CBeqz, CMv])]
    FusedCBeqzCMv { rs1: Reg, rd: Reg, mv_rs2: Reg },
    #[instruction(if = [CBnez, CMv])]
    FusedCBnezCMv { rs1: Reg, rd: Reg, mv_rs2: Reg },
}

const _: () = {
    // Fused instructions must not make an instruction set larger than it is without them, see
    // `upper_immediate_fits()` for what that budget looks like
    assert!(
        size_of::<Rv32ZcaFusedInstruction<Reg<u32>>>() == size_of::<Rv32Instruction<Reg<u32>>>(),
        "Fused instructions must fit into an instruction of the instruction set they extend"
    );
};

#[instruction]
const impl<Reg> Instruction for Rv32ZcaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
{
    const ALIGNMENT: u8 = align_of::<u16>() as u8;

    type Reg = Reg;

    /// Fused instructions are not encoded by anything, they only ever come out of
    /// [`FusedInstruction::fuse()`]
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        match self {
            // Compressed branch + `c.mv`
            Self::FusedCBeqzCMv { .. } | Self::FusedCBnezCMv { .. } => 2 * size_of::<u16>() as u8,
            // Full-size branch + `c.mv`
            _ => (size_of::<u32>() + size_of::<u16>()) as u8,
        }
    }
}

#[instruction]
const impl<Reg> FusedInstruction for Rv32ZcaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn fuse(prev: Self, next: Self) -> (Self, Self) {
        match (prev, next) {
            // Branch over `c.mv`
            (
                Self::Beq { rs1, rs2, imm: 6 },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (
                Self::FusedBeqCMv {
                    rs1,
                    rs2,
                    rd,
                    mv_rs2,
                },
                next,
            ),
            (
                Self::Bne { rs1, rs2, imm: 6 },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (
                Self::FusedBneCMv {
                    rs1,
                    rs2,
                    rd,
                    mv_rs2,
                },
                next,
            ),
            (
                Self::Blt { rs1, rs2, imm: 6 },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (
                Self::FusedBltCMv {
                    rs1,
                    rs2,
                    rd,
                    mv_rs2,
                },
                next,
            ),
            (
                Self::Bge { rs1, rs2, imm: 6 },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (
                Self::FusedBgeCMv {
                    rs1,
                    rs2,
                    rd,
                    mv_rs2,
                },
                next,
            ),
            (
                Self::Bltu { rs1, rs2, imm: 6 },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (
                Self::FusedBltuCMv {
                    rs1,
                    rs2,
                    rd,
                    mv_rs2,
                },
                next,
            ),
            (
                Self::Bgeu { rs1, rs2, imm: 6 },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (
                Self::FusedBgeuCMv {
                    rs1,
                    rs2,
                    rd,
                    mv_rs2,
                },
                next,
            ),

            // Compressed branch over `c.mv`
            (
                Self::CBeqz { rs1, imm: 4, .. },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (Self::FusedCBeqzCMv { rs1, rd, mv_rs2 }, next),
            (
                Self::CBnez { rs1, imm: 4, .. },
                Self::CMv {
                    rd, rs2: mv_rs2, ..
                },
            ) => (Self::FusedCBnezCMv { rs1, rd, mv_rs2 }, next),
        }
    }
}

#[instruction]
impl<Reg> fmt::Display for Rv32ZcaFusedInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    /// A fused instruction prints as the first instruction of the pair it replaced, see
    /// [module-level documentation](super::super)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FusedBeqCMv {
                rs1,
                rs2,
                rd: _,
                mv_rs2: _,
            } => write!(f, "beq {rs1}, {rs2}, 6"),
            Self::FusedBneCMv {
                rs1,
                rs2,
                rd: _,
                mv_rs2: _,
            } => write!(f, "bne {rs1}, {rs2}, 6"),
            Self::FusedBltCMv {
                rs1,
                rs2,
                rd: _,
                mv_rs2: _,
            } => write!(f, "blt {rs1}, {rs2}, 6"),
            Self::FusedBgeCMv {
                rs1,
                rs2,
                rd: _,
                mv_rs2: _,
            } => write!(f, "bge {rs1}, {rs2}, 6"),
            Self::FusedBltuCMv {
                rs1,
                rs2,
                rd: _,
                mv_rs2: _,
            } => write!(f, "bltu {rs1}, {rs2}, 6"),
            Self::FusedBgeuCMv {
                rs1,
                rs2,
                rd: _,
                mv_rs2: _,
            } => write!(f, "bgeu {rs1}, {rs2}, 6"),
            Self::FusedCBeqzCMv {
                rs1,
                rd: _,
                mv_rs2: _,
            } => write!(f, "c.beqz {rs1}, 4"),
            Self::FusedCBnezCMv {
                rs1,
                rd: _,
                mv_rs2: _,
            } => write!(f, "c.bnez {rs1}, 4"),
        }
    }
}

#[instruction_execution]
const impl<Reg> ExecutableInstructionOperands for Rv32ZcaFusedInstruction<Reg> where
    Reg: Register<Type = u32>
{
}

#[instruction_execution]
const impl<Reg, Env> ExecutableInstructionCsr<Env> for Rv32ZcaFusedInstruction<Reg> where
    Reg: Register<Type = u32>
{
}

#[instruction_execution]
const impl<Reg, Regs, Env, Memory, PC> ExecutableInstruction<Regs, Env, Memory, PC>
    for Rv32ZcaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
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
            Self::FusedBeqCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value == rs2_value {
                    old_value
                } else {
                    moved_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedBneCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value == rs2_value {
                    moved_value
                } else {
                    old_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedBltCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value.cast_signed() < rs2_value.cast_signed() {
                    old_value
                } else {
                    moved_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedBgeCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value.cast_signed() >= rs2_value.cast_signed() {
                    old_value
                } else {
                    moved_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedBltuCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value < rs2_value {
                    old_value
                } else {
                    moved_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedBgeuCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value >= rs2_value {
                    old_value
                } else {
                    moved_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedCBeqzCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value == 0 {
                    old_value
                } else {
                    moved_value
                };
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedCBnezCMv {
                rs1: _,
                rs2: _,
                rd,
                mv_rs2,
            } => {
                let moved_value = regs.read(mv_rs2);
                let old_value = regs.read(rd);
                let value = if rs1_value == 0 {
                    moved_value
                } else {
                    old_value
                };
                ExecutionResult::Continue { rd, value }
            }
        }
    }
}
