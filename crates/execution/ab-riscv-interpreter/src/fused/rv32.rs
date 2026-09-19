//! Fused instructions of the base RISC-V RV32 instruction set

pub mod m;
#[cfg(test)]
mod tests;
pub mod zba;
pub mod zca;

use crate::fused::{FusedInstruction, fused_helpers};
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

/// Fused instructions of the base RISC-V RV32 instruction set.
///
/// These are the macro fusions LLVM emits code for that need nothing beyond the base instruction
/// set: `addi-load`, `add-load`, `ld-add` (which is `add-load` with a zero offset), `add-mem`,
/// `auipc-addi`, `auipc-load`, `lui-addi`, `lui-load`, `logic-reg-reg`, `logic-reg-imm`,
/// `logic-imm-reg`, `zexth`, `bfext` and `shift-bit-extract`.
///
/// See [module-level documentation](super) for what fusion is and when a pair may be fused.
#[instruction(inherit = [Rv32Instruction])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rv32FusedInstruction<Reg> {
    // `addi` + load
    #[instruction(if = [Addi, Lb])]
    FusedAddiLb { rd: Reg, rs1: Reg, imm: i16 },
    #[instruction(if = [Addi, Lh])]
    FusedAddiLh { rd: Reg, rs1: Reg, imm: i16 },
    #[instruction(if = [Addi, Lw])]
    FusedAddiLw { rd: Reg, rs1: Reg, imm: i16 },
    #[instruction(if = [Addi, Lbu])]
    FusedAddiLbu { rd: Reg, rs1: Reg, imm: i16 },
    #[instruction(if = [Addi, Lhu])]
    FusedAddiLhu { rd: Reg, rs1: Reg, imm: i16 },

    // `add` + load
    #[instruction(if = [Add, Lb])]
    FusedAddLb {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [Add, Lh])]
    FusedAddLh {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [Add, Lw])]
    FusedAddLw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [Add, Lbu])]
    FusedAddLbu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [Add, Lhu])]
    FusedAddLhu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },

    // `add` + store
    #[instruction(if = [Add, Sb])]
    FusedAddSb {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Add, Sh])]
    FusedAddSh {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Add, Sw])]
    FusedAddSw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },

    // `auipc` + `addi`
    #[instruction(if = [Auipc, Addi])]
    FusedAuipcAddi { rd: Reg, imm: I24 },

    // `auipc` + load
    #[instruction(if = [Auipc, Lb])]
    FusedAuipcLb { rd: Reg, imm: I24 },
    #[instruction(if = [Auipc, Lh])]
    FusedAuipcLh { rd: Reg, imm: I24 },
    #[instruction(if = [Auipc, Lw])]
    FusedAuipcLw { rd: Reg, imm: I24 },
    #[instruction(if = [Auipc, Lbu])]
    FusedAuipcLbu { rd: Reg, imm: I24 },
    #[instruction(if = [Auipc, Lhu])]
    FusedAuipcLhu { rd: Reg, imm: I24 },

    // `lui` + `addi`
    #[instruction(if = [Lui, Addi])]
    FusedLuiAddi { rd: Reg, imm: I24 },

    // `lui` + load
    #[instruction(if = [Lui, Lb])]
    FusedLuiLb { rd: Reg, imm: I24 },
    #[instruction(if = [Lui, Lh])]
    FusedLuiLh { rd: Reg, imm: I24 },
    #[instruction(if = [Lui, Lw])]
    FusedLuiLw { rd: Reg, imm: I24 },
    #[instruction(if = [Lui, Lbu])]
    FusedLuiLbu { rd: Reg, imm: I24 },
    #[instruction(if = [Lui, Lhu])]
    FusedLuiLhu { rd: Reg, imm: I24 },

    // Logic operation + logic operation
    #[instruction(if = [And, And])]
    FusedAndAnd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [And, Or])]
    FusedAndOr {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [And, Xor])]
    FusedAndXor {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Or, And])]
    FusedOrAnd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Or, Or])]
    FusedOrOr {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Or, Xor])]
    FusedOrXor {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Xor, And])]
    FusedXorAnd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Xor, Or])]
    FusedXorOr {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },
    #[instruction(if = [Xor, Xor])]
    FusedXorXor {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        rs3: Reg,
    },

    // Logic operation + logic operation with an immediate
    #[instruction(if = [And, Andi])]
    FusedAndAndi {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [And, Ori])]
    FusedAndOri {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [And, Xori])]
    FusedAndXori {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Or, Andi])]
    FusedOrAndi {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Or, Ori])]
    FusedOrOri {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Or, Xori])]
    FusedOrXori {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Xor, Andi])]
    FusedXorAndi {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Xor, Ori])]
    FusedXorOri {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Xor, Xori])]
    FusedXorXori {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },

    // Logic operation with an immediate + logic operation
    #[instruction(if = [Andi, And])]
    FusedAndiAnd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Andi, Or])]
    FusedAndiOr {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Andi, Xor])]
    FusedAndiXor {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Ori, And])]
    FusedOriAnd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Ori, Or])]
    FusedOriOr {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Ori, Xor])]
    FusedOriXor {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Xori, And])]
    FusedXoriAnd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Xori, Or])]
    FusedXoriOr {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },
    #[instruction(if = [Xori, Xor])]
    FusedXoriXor {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        imm: i16,
    },

    // Shift left + shift right
    #[instruction(if = [Slli, Srli])]
    FusedSlliSrliZexth { rd: Reg, rs1: Reg },
    #[instruction(if = [Slli, Srli])]
    FusedSlliSrli {
        rd: Reg,
        rs1: Reg,
        shamt: u8,
        right_shamt: u8,
    },
    #[instruction(if = [Slli, Srai])]
    FusedSlliSrai {
        rd: Reg,
        rs1: Reg,
        shamt: u8,
        right_shamt: u8,
    },
}

const _: () = {
    // Fused instructions must not make an instruction set larger than it is without them, see
    // `fused_helpers::upper_immediate_fits()` for what that budget looks like
    assert!(
        size_of::<Rv32FusedInstruction<Reg<u32>>>() == size_of::<Rv32Instruction<Reg<u32>>>(),
        "Fused instructions must fit into an instruction of the instruction set they extend"
    );
};

#[instruction]
const impl<Reg> Instruction for Rv32FusedInstruction<Reg>
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
const impl<Reg> FusedInstruction for Rv32FusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn fuse(prev: Self, next: Self) -> (Self, Self) {
        match (prev, next) {
            // `addi` + load
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Lb {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && imm.checked_add(offset).is_some() =>
            {
                (
                    Self::FusedAddiLb {
                        rd,
                        rs1,
                        imm: imm.wrapping_add(offset),
                    },
                    next,
                )
            }
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Lh {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && imm.checked_add(offset).is_some() =>
            {
                (
                    Self::FusedAddiLh {
                        rd,
                        rs1,
                        imm: imm.wrapping_add(offset),
                    },
                    next,
                )
            }
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Lw {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && imm.checked_add(offset).is_some() =>
            {
                (
                    Self::FusedAddiLw {
                        rd,
                        rs1,
                        imm: imm.wrapping_add(offset),
                    },
                    next,
                )
            }
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Lbu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && imm.checked_add(offset).is_some() =>
            {
                (
                    Self::FusedAddiLbu {
                        rd,
                        rs1,
                        imm: imm.wrapping_add(offset),
                    },
                    next,
                )
            }
            (
                Self::Addi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Lhu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && imm.checked_add(offset).is_some() =>
            {
                (
                    Self::FusedAddiLhu {
                        rd,
                        rs1,
                        imm: imm.wrapping_add(offset),
                    },
                    next,
                )
            }

            // `add` + load
            (
                Self::Add {
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
                Self::FusedAddLb {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::Add {
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
                Self::FusedAddLh {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::Add {
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
                Self::FusedAddLw {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::Add {
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
                Self::FusedAddLbu {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::Add {
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
                Self::FusedAddLhu {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),

            // `add` + store, where the address the `add` computed stays live and
            // the fused instruction writes it back after storing through it
            (
                Self::Add { rd, rs1, rs2 },
                Self::Sb {
                    rs1: base,
                    rs2: rs3,
                    imm: 0,
                },
            ) if rd != Reg::ZERO && rd == base && rd != rs1 && rd != rs2 && rd != rs3 => {
                (Self::FusedAddSb { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Add { rd, rs1, rs2 },
                Self::Sh {
                    rs1: base,
                    rs2: rs3,
                    imm: 0,
                },
            ) if rd != Reg::ZERO && rd == base && rd != rs1 && rd != rs2 && rd != rs3 => {
                (Self::FusedAddSh { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Add { rd, rs1, rs2 },
                Self::Sw {
                    rs1: base,
                    rs2: rs3,
                    imm: 0,
                },
            ) if rd != Reg::ZERO && rd == base && rd != rs1 && rd != rs2 && rd != rs3 => {
                (Self::FusedAddSw { rd, rs1, rs2, rs3 }, next)
            }

            // `auipc` + `addi`
            (
                Self::Auipc {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Addi {
                    rd,
                    rs1: base,
                    imm: lower,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), lower) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), lower);
                (Self::FusedAuipcAddi { rd, imm }, next)
            }

            // `auipc` + load
            (
                Self::Auipc {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lb {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedAuipcLb { rd, imm }, next)
            }
            (
                Self::Auipc {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lh {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedAuipcLh { rd, imm }, next)
            }
            (
                Self::Auipc {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lw {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedAuipcLw { rd, imm }, next)
            }
            (
                Self::Auipc {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lbu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedAuipcLbu { rd, imm }, next)
            }
            (
                Self::Auipc {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lhu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedAuipcLhu { rd, imm }, next)
            }

            // `lui` + `addi`
            (
                Self::Lui {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Addi {
                    rd,
                    rs1: base,
                    imm: lower,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), lower) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), lower);
                (Self::FusedLuiAddi { rd, imm }, next)
            }

            // `lui` + load
            (
                Self::Lui {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lb {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedLuiLb { rd, imm }, next)
            }
            (
                Self::Lui {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lh {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedLuiLh { rd, imm }, next)
            }
            (
                Self::Lui {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lw {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedLuiLw { rd, imm }, next)
            }
            (
                Self::Lui {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lbu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedLuiLbu { rd, imm }, next)
            }
            (
                Self::Lui {
                    rd: prev_rd,
                    imm: upper,
                    ..
                },
                Self::Lhu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO
                && prev_rd == base
                && prev_rd == rd
                && fused_helpers::upper_immediate_fits(upper.to_i32(), offset) =>
            {
                let imm = fused_helpers::combine_upper_immediate(upper.to_i32(), offset);
                (Self::FusedLuiLhu { rd, imm }, next)
            }

            // Logic operation + logic operation. The second operand of the second
            // instruction is read after the first one wrote its destination, so it must not
            // be that destination, which a fused instruction reads the stale value of
            (
                Self::And {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::And {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedAndAnd { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::And {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Or {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedAndOr { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::And {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Xor {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedAndXor { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Or {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::And {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedOrAnd { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Or {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Or {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedOrOr { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Or {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Xor {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedOrXor { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::And {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedXorAnd { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Or {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedXorOr { rd, rs1, rs2, rs3 }, next)
            }
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Xor {
                    rd,
                    rs1: base,
                    rs2: rs3,
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs3 => {
                (Self::FusedXorXor { rd, rs1, rs2, rs3 }, next)
            }

            // Logic operation + logic operation with an immediate
            (
                Self::And {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Andi {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedAndAndi { rd, rs1, rs2, imm }, next)
            }
            (
                Self::And {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ori {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedAndOri { rd, rs1, rs2, imm }, next)
            }
            (
                Self::And {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Xori {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedAndXori { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Or {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Andi {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedOrAndi { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Or {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ori {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedOrOri { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Or {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Xori {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedOrXori { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Andi {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedXorAndi { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ori {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedXorOri { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Xor {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Xori {
                    rd, rs1: base, imm, ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedXorXori { rd, rs1, rs2, imm }, next)
            }

            // Logic operation with an immediate + logic operation
            (
                Self::Andi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::And { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedAndiAnd { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Andi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Or { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedAndiOr { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Andi {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Xor { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedAndiXor { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Ori {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::And { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedOriAnd { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Ori {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Or { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedOriOr { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Ori {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Xor { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedOriXor { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Xori {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::And { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedXoriAnd { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Xori {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Or { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedXoriOr { rd, rs1, rs2, imm }, next)
            }
            (
                Self::Xori {
                    rd: prev_rd,
                    rs1,
                    imm,
                    ..
                },
                Self::Xor { rd, rs1: base, rs2 },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd && rd != rs2 => {
                (Self::FusedXoriXor { rd, rs1, rs2, imm }, next)
            }

            // Shift left + shift right, with the specialized forms first so that the general
            // bitfield extract below doesn't shadow them
            (
                Self::Slli {
                    rd: prev_rd,
                    rs1,
                    shamt: 16,
                    ..
                },
                Self::Srli {
                    rd,
                    rs1: base,
                    shamt: 16,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => {
                (Self::FusedSlliSrliZexth { rd, rs1 }, next)
            }
            (
                Self::Slli {
                    rd: prev_rd,
                    rs1,
                    shamt,
                    ..
                },
                Self::Srli {
                    rd,
                    rs1: base,
                    shamt: right_shamt,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedSlliSrli {
                    rd,
                    rs1,
                    shamt,
                    right_shamt,
                },
                next,
            ),
            (
                Self::Slli {
                    rd: prev_rd,
                    rs1,
                    shamt,
                    ..
                },
                Self::Srai {
                    rd,
                    rs1: base,
                    shamt: right_shamt,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedSlliSrai {
                    rd,
                    rs1,
                    shamt,
                    right_shamt,
                },
                next,
            ),
        }
    }
}

#[instruction]
impl<Reg> fmt::Display for Rv32FusedInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    /// A fused instruction prints as the first instruction of the pair it replaced, so that
    /// walking a decoded instruction stream slot by slot prints what it did before fusion: the
    /// second instruction of the pair is still in the slot that follows.
    ///
    /// The exception is `addi` + load, which carries the two immediates of the pair added together
    /// rather than both of them, and prints as the load it is equivalent to instead.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FusedAddiLb { rd, rs1, imm } => write!(f, "lb {rd}, {imm}({rs1})"),
            Self::FusedAddiLh { rd, rs1, imm } => write!(f, "lh {rd}, {imm}({rs1})"),
            Self::FusedAddiLw { rd, rs1, imm } => write!(f, "lw {rd}, {imm}({rs1})"),
            Self::FusedAddiLbu { rd, rs1, imm } => write!(f, "lbu {rd}, {imm}({rs1})"),
            Self::FusedAddiLhu { rd, rs1, imm } => write!(f, "lhu {rd}, {imm}({rs1})"),
            Self::FusedAddLb {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddLh {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddLw {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddLbu {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddLhu {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddSb {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddSh {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAddSw {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "add {rd}, {rs1}, {rs2}"),
            Self::FusedAuipcAddi { rd, imm } => write!(
                f,
                "auipc {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedAuipcLb { rd, imm } => write!(
                f,
                "auipc {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedAuipcLh { rd, imm } => write!(
                f,
                "auipc {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedAuipcLw { rd, imm } => write!(
                f,
                "auipc {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedAuipcLbu { rd, imm } => write!(
                f,
                "auipc {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedAuipcLhu { rd, imm } => write!(
                f,
                "auipc {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedLuiAddi { rd, imm } => write!(
                f,
                "lui {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedLuiLb { rd, imm } => write!(
                f,
                "lui {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedLuiLh { rd, imm } => write!(
                f,
                "lui {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedLuiLw { rd, imm } => write!(
                f,
                "lui {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedLuiLbu { rd, imm } => write!(
                f,
                "lui {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedLuiLhu { rd, imm } => write!(
                f,
                "lui {rd}, 0x{:x}",
                fused_helpers::upper_immediate(imm.to_i32()) >> 12
            ),
            Self::FusedAndAnd {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "and {rd}, {rs1}, {rs2}"),
            Self::FusedAndOr {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "and {rd}, {rs1}, {rs2}"),
            Self::FusedAndXor {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "and {rd}, {rs1}, {rs2}"),
            Self::FusedOrAnd {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "or {rd}, {rs1}, {rs2}"),
            Self::FusedOrOr {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "or {rd}, {rs1}, {rs2}"),
            Self::FusedOrXor {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "or {rd}, {rs1}, {rs2}"),
            Self::FusedXorAnd {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedXorOr {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedXorXor {
                rd,
                rs1,
                rs2,
                rs3: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedAndAndi {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "and {rd}, {rs1}, {rs2}"),
            Self::FusedAndOri {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "and {rd}, {rs1}, {rs2}"),
            Self::FusedAndXori {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "and {rd}, {rs1}, {rs2}"),
            Self::FusedOrAndi {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "or {rd}, {rs1}, {rs2}"),
            Self::FusedOrOri {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "or {rd}, {rs1}, {rs2}"),
            Self::FusedOrXori {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "or {rd}, {rs1}, {rs2}"),
            Self::FusedXorAndi {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedXorOri {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedXorXori {
                rd,
                rs1,
                rs2,
                imm: _,
            } => write!(f, "xor {rd}, {rs1}, {rs2}"),
            Self::FusedAndiAnd {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "andi {rd}, {rs1}, {imm}"),
            Self::FusedAndiOr {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "andi {rd}, {rs1}, {imm}"),
            Self::FusedAndiXor {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "andi {rd}, {rs1}, {imm}"),
            Self::FusedOriAnd {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "ori {rd}, {rs1}, {imm}"),
            Self::FusedOriOr {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "ori {rd}, {rs1}, {imm}"),
            Self::FusedOriXor {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "ori {rd}, {rs1}, {imm}"),
            Self::FusedXoriAnd {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "xori {rd}, {rs1}, {imm}"),
            Self::FusedXoriOr {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "xori {rd}, {rs1}, {imm}"),
            Self::FusedXoriXor {
                rd,
                rs1,
                rs2: _,
                imm,
            } => write!(f, "xori {rd}, {rs1}, {imm}"),
            Self::FusedSlliSrliZexth { rd, rs1 } => write!(f, "slli {rd}, {rs1}, 16"),
            Self::FusedSlliSrli {
                rd,
                rs1,
                shamt,
                right_shamt: _,
            } => write!(f, "slli {rd}, {rs1}, {shamt}"),
            Self::FusedSlliSrai {
                rd,
                rs1,
                shamt,
                right_shamt: _,
            } => write!(f, "slli {rd}, {rs1}, {shamt}"),
        }
    }
}

#[instruction_execution]
const impl<Reg> ExecutableInstructionOperands for Rv32FusedInstruction<Reg> where
    Reg: Register<Type = u32>
{
}

#[instruction_execution]
const impl<Reg, Env> ExecutableInstructionCsr<Env> for Rv32FusedInstruction<Reg> where
    Reg: Register<Type = u32>
{
}

#[instruction_execution]
const impl<Reg, Regs, Env, Memory, PC> ExecutableInstruction<Regs, Env, Memory, PC>
    for Rv32FusedInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
    Regs: [const] RegisterFile<Reg>,
    Memory: [const] VirtualMemory,
    PC: [const] ProgramCounter<Reg::Type, Memory>,
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
        memory: &mut Memory,
        program_counter: &mut PC,
    ) -> ExecutionResult<Self::Reg> {
        match self {
            Self::FusedAddiLb { rd, rs1: _, imm } => {
                let addr = rs1_value.wrapping_add(i32::from(imm).cast_unsigned());
                let value = i32::from(memory.read::<i8>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddiLh { rd, rs1: _, imm } => {
                let addr = rs1_value.wrapping_add(i32::from(imm).cast_unsigned());
                let value = i32::from(memory.read::<i16>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddiLw { rd, rs1: _, imm } => {
                let addr = rs1_value.wrapping_add(i32::from(imm).cast_unsigned());
                let value = memory.read::<u32>(u64::from(addr))?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAddiLbu { rd, rs1: _, imm } => {
                let addr = rs1_value.wrapping_add(i32::from(imm).cast_unsigned());
                let value = memory.read::<u8>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedAddiLhu { rd, rs1: _, imm } => {
                let addr = rs1_value.wrapping_add(i32::from(imm).cast_unsigned());
                let value = memory.read::<u16>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedAddLb {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = rs1_value
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = i32::from(memory.read::<i8>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddLh {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = rs1_value
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = i32::from(memory.read::<i16>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddLw {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = rs1_value
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = memory.read::<u32>(u64::from(addr))?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAddLbu {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = rs1_value
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = memory.read::<u8>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedAddLhu {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = rs1_value
                    .wrapping_add(rs2_value)
                    .wrapping_add(i32::from(offset).cast_unsigned());
                let value = memory.read::<u16>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedAddSb {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let addr = rs1_value.wrapping_add(rs2_value);
                memory.write(u64::from(addr), regs.read(rs3) as u8)?;
                ExecutionResult::Continue { rd, value: addr }
            }
            Self::FusedAddSh {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let addr = rs1_value.wrapping_add(rs2_value);
                memory.write(u64::from(addr), regs.read(rs3) as u16)?;
                ExecutionResult::Continue { rd, value: addr }
            }
            Self::FusedAddSw {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let addr = rs1_value.wrapping_add(rs2_value);
                memory.write(u64::from(addr), regs.read(rs3))?;
                ExecutionResult::Continue { rd, value: addr }
            }
            Self::FusedAuipcAddi { rd, imm } => {
                let old_pc = program_counter.old_pc(2 * size_of::<u32>() as u8);
                ExecutionResult::Continue {
                    rd,
                    value: old_pc.wrapping_add(imm.to_i32().cast_unsigned()),
                }
            }
            Self::FusedAuipcLb { rd, imm } => {
                let old_pc = program_counter.old_pc(2 * size_of::<u32>() as u8);
                let addr = old_pc.wrapping_add(imm.to_i32().cast_unsigned());
                let value = i32::from(memory.read::<i8>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAuipcLh { rd, imm } => {
                let old_pc = program_counter.old_pc(2 * size_of::<u32>() as u8);
                let addr = old_pc.wrapping_add(imm.to_i32().cast_unsigned());
                let value = i32::from(memory.read::<i16>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAuipcLw { rd, imm } => {
                let old_pc = program_counter.old_pc(2 * size_of::<u32>() as u8);
                let addr = old_pc.wrapping_add(imm.to_i32().cast_unsigned());
                let value = memory.read::<u32>(u64::from(addr))?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAuipcLbu { rd, imm } => {
                let old_pc = program_counter.old_pc(2 * size_of::<u32>() as u8);
                let addr = old_pc.wrapping_add(imm.to_i32().cast_unsigned());
                let value = memory.read::<u8>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedAuipcLhu { rd, imm } => {
                let old_pc = program_counter.old_pc(2 * size_of::<u32>() as u8);
                let addr = old_pc.wrapping_add(imm.to_i32().cast_unsigned());
                let value = memory.read::<u16>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedLuiAddi { rd, imm } => ExecutionResult::Continue {
                rd,
                value: imm.to_i32().cast_unsigned(),
            },
            Self::FusedLuiLb { rd, imm } => {
                let addr = imm.to_i32().cast_unsigned();
                let value = i32::from(memory.read::<i8>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedLuiLh { rd, imm } => {
                let addr = imm.to_i32().cast_unsigned();
                let value = i32::from(memory.read::<i16>(u64::from(addr))?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedLuiLw { rd, imm } => {
                let addr = imm.to_i32().cast_unsigned();
                let value = memory.read::<u32>(u64::from(addr))?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedLuiLbu { rd, imm } => {
                let addr = imm.to_i32().cast_unsigned();
                let value = memory.read::<u8>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedLuiLhu { rd, imm } => {
                let addr = imm.to_i32().cast_unsigned();
                let value = memory.read::<u16>(u64::from(addr))?;
                ExecutionResult::Continue {
                    rd,
                    value: u32::from(value),
                }
            }
            Self::FusedAndAnd {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value & rs2_value) & regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndOr {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value & rs2_value) | regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndXor {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value & rs2_value) ^ regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOrAnd {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value | rs2_value) & regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOrOr {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value | rs2_value) | regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOrXor {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value | rs2_value) ^ regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorAnd {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value ^ rs2_value) & regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorOr {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value ^ rs2_value) | regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorXor {
                rd,
                rs1: _,
                rs2: _,
                rs3,
            } => {
                let value = (rs1_value ^ rs2_value) ^ regs.read(rs3);
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndAndi {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value & rs2_value) & i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndOri {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value & rs2_value) | i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndXori {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value & rs2_value) ^ i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOrAndi {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value | rs2_value) & i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOrOri {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value | rs2_value) | i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOrXori {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value | rs2_value) ^ i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorAndi {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value ^ rs2_value) & i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorOri {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value ^ rs2_value) | i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXorXori {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value ^ rs2_value) ^ i32::from(imm).cast_unsigned();
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndiAnd {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value & i32::from(imm).cast_unsigned()) & rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndiOr {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value & i32::from(imm).cast_unsigned()) | rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAndiXor {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value & i32::from(imm).cast_unsigned()) ^ rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOriAnd {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value | i32::from(imm).cast_unsigned()) & rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOriOr {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value | i32::from(imm).cast_unsigned()) | rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedOriXor {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value | i32::from(imm).cast_unsigned()) ^ rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXoriAnd {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value ^ i32::from(imm).cast_unsigned()) & rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXoriOr {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value ^ i32::from(imm).cast_unsigned()) | rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedXoriXor {
                rd,
                rs1: _,
                rs2: _,
                imm,
            } => {
                let value = (rs1_value ^ i32::from(imm).cast_unsigned()) ^ rs2_value;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedSlliSrliZexth { rd, rs1: _ } => {
                let value = rs1_value & 0xffff;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedSlliSrli {
                rd,
                rs1: _,
                shamt,
                right_shamt,
            } => {
                let value = (rs1_value << shamt) >> right_shamt;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedSlliSrai {
                rd,
                rs1: _,
                shamt,
                right_shamt,
            } => {
                let value = (rs1_value << shamt).cast_signed() >> right_shamt;
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
        }
    }
}
