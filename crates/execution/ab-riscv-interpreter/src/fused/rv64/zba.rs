//! Fused instructions of the RISC-V RV64 Zba extension

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
/// `shxadd-load` macro fusion, as well as `sh[123]add.uw` and `add.uw` with one too.
///
/// The shift amount of `sh[123]add` is a field rather than a variant of its own: an interpreter
/// pays nothing for a variable shift, and three times as many variants would cost instruction
/// cache in every dispatch table they end up in.
///
/// See [module-level documentation](super::super) for what fusion is and when a pair may be fused.
#[instruction(inherit = [Rv64Instruction, Rv64ZbaInstruction])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rv64ZbaFusedInstruction<Reg> {
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
    #[instruction(if = [Sh1add, Ld], if = [Sh2add, Ld], if = [Sh3add, Ld])]
    FusedShxaddLd {
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
    #[instruction(if = [Sh1add, Lwu], if = [Sh2add, Lwu], if = [Sh3add, Lwu])]
    FusedShxaddLwu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },

    // `sh[123]add.uw` + load
    #[instruction(if = [Sh1addUw, Lb], if = [Sh2addUw, Lb], if = [Sh3addUw, Lb])]
    FusedShxaddUwLb {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1addUw, Lh], if = [Sh2addUw, Lh], if = [Sh3addUw, Lh])]
    FusedShxaddUwLh {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1addUw, Lw], if = [Sh2addUw, Lw], if = [Sh3addUw, Lw])]
    FusedShxaddUwLw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1addUw, Ld], if = [Sh2addUw, Ld], if = [Sh3addUw, Ld])]
    FusedShxaddUwLd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1addUw, Lbu], if = [Sh2addUw, Lbu], if = [Sh3addUw, Lbu])]
    FusedShxaddUwLbu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1addUw, Lhu], if = [Sh2addUw, Lhu], if = [Sh3addUw, Lhu])]
    FusedShxaddUwLhu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },
    #[instruction(if = [Sh1addUw, Lwu], if = [Sh2addUw, Lwu], if = [Sh3addUw, Lwu])]
    FusedShxaddUwLwu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        shamt: u8,
        offset: i16,
    },

    // `add.uw` + load
    #[instruction(if = [AddUw, Lb])]
    FusedAddUwLb {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [AddUw, Lh])]
    FusedAddUwLh {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [AddUw, Lw])]
    FusedAddUwLw {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [AddUw, Ld])]
    FusedAddUwLd {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [AddUw, Lbu])]
    FusedAddUwLbu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [AddUw, Lhu])]
    FusedAddUwLhu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
    #[instruction(if = [AddUw, Lwu])]
    FusedAddUwLwu {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        offset: i16,
    },
}

const _: () = {
    // Fused instructions must not make an instruction set larger than it is without them, see
    // `upper_immediate_fits()` for what that budget looks like
    assert!(
        size_of::<Rv64ZbaFusedInstruction<Reg<u64>>>() == size_of::<Rv64Instruction<Reg<u64>>>(),
        "Fused instructions must fit into an instruction of the instruction set they extend"
    );
};

#[instruction]
const impl<Reg> Instruction for Rv64ZbaFusedInstruction<Reg>
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
const impl<Reg> FusedInstruction for Rv64ZbaFusedInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn fuse(prev: Self, next: Self) -> (Self, Self) {
        match (prev, next) {
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
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLd {
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
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLd {
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
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLd {
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
            (
                Self::Sh1add {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLwu {
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
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLwu {
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
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddLwu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),

            // `sh[123]add.uw` + load
            (
                Self::Sh1addUw {
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
                Self::FusedShxaddUwLb {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
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
                Self::FusedShxaddUwLb {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
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
                Self::FusedShxaddUwLb {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1addUw {
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
                Self::FusedShxaddUwLh {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
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
                Self::FusedShxaddUwLh {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
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
                Self::FusedShxaddUwLh {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1addUw {
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
                Self::FusedShxaddUwLw {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
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
                Self::FusedShxaddUwLw {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
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
                Self::FusedShxaddUwLw {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1addUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddUwLd {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddUwLd {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddUwLd {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1addUw {
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
                Self::FusedShxaddUwLbu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
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
                Self::FusedShxaddUwLbu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
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
                Self::FusedShxaddUwLbu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1addUw {
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
                Self::FusedShxaddUwLhu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
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
                Self::FusedShxaddUwLhu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
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
                Self::FusedShxaddUwLhu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),
            (
                Self::Sh1addUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddUwLwu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 1,
                    offset,
                },
                next,
            ),
            (
                Self::Sh2addUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddUwLwu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 2,
                    offset,
                },
                next,
            ),
            (
                Self::Sh3addUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedShxaddUwLwu {
                    rd,
                    rs1,
                    rs2,
                    shamt: 3,
                    offset,
                },
                next,
            ),

            // `add.uw` + load
            (
                Self::AddUw {
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
                Self::FusedAddUwLb {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::AddUw {
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
                Self::FusedAddUwLh {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::AddUw {
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
                Self::FusedAddUwLw {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::AddUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Ld {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedAddUwLd {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::AddUw {
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
                Self::FusedAddUwLbu {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::AddUw {
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
                Self::FusedAddUwLhu {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
            (
                Self::AddUw {
                    rd: prev_rd,
                    rs1,
                    rs2,
                },
                Self::Lwu {
                    rd,
                    rs1: base,
                    imm: offset,
                    ..
                },
            ) if prev_rd != Reg::ZERO && prev_rd == base && prev_rd == rd => (
                Self::FusedAddUwLwu {
                    rd,
                    rs1,
                    rs2,
                    offset,
                },
                next,
            ),
        }
    }
}

#[instruction]
impl<Reg> fmt::Display for Rv64ZbaFusedInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    /// A fused instruction prints as the first instruction of the pair it replaced, see
    /// [module-level documentation](super::super)
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::FusedShxaddLd {
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
            Self::FusedShxaddLwu {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLb {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLh {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLw {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLd {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLbu {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLhu {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedShxaddUwLwu {
                rd,
                rs1,
                rs2,
                shamt,
                offset: _,
            } => write!(f, "sh{shamt}add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLb {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLh {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLw {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLd {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLbu {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLhu {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
            Self::FusedAddUwLwu {
                rd,
                rs1,
                rs2,
                offset: _,
            } => write!(f, "add.uw {rd}, {rs1}, {rs2}"),
        }
    }
}

#[instruction_execution]
const impl<Reg> ExecutableInstructionOperands for Rv64ZbaFusedInstruction<Reg> where
    Reg: Register<Type = u64>
{
}

#[instruction_execution]
const impl<Reg, Env> ExecutableInstructionCsr<Env> for Rv64ZbaFusedInstruction<Reg> where
    Reg: Register<Type = u64>
{
}

#[instruction_execution]
const impl<Reg, Regs, Env, Memory, PC> ExecutableInstruction<Regs, Env, Memory, PC>
    for Rv64ZbaFusedInstruction<Reg>
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
        memory: &mut Memory,
        _program_counter: &mut PC,
    ) -> ExecutionResult<Self::Reg> {
        match self {
            Self::FusedShxaddLb {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i8>(addr)?);
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
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i16>(addr)?);
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
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i32>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedShxaddLd {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u64>(addr)?;
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
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u8>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
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
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u16>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedShxaddLwu {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (rs1_value << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u32>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedShxaddUwLb {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i8>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedShxaddUwLh {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i16>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedShxaddUwLw {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i32>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedShxaddUwLd {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u64>(addr)?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedShxaddUwLbu {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u8>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedShxaddUwLhu {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u16>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedShxaddUwLwu {
                rd,
                rs1: _,
                rs2: _,
                shamt,
                offset,
            } => {
                let addr = (u64::from(rs1_value as u32) << shamt)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u32>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedAddUwLb {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i8>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddUwLh {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i16>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddUwLw {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = i64::from(memory.read::<i32>(addr)?);
                ExecutionResult::Continue {
                    rd,
                    value: value.cast_unsigned(),
                }
            }
            Self::FusedAddUwLd {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u64>(addr)?;
                ExecutionResult::Continue { rd, value }
            }
            Self::FusedAddUwLbu {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u8>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedAddUwLhu {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u16>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
            Self::FusedAddUwLwu {
                rd,
                rs1: _,
                rs2: _,
                offset,
            } => {
                let addr = u64::from(rs1_value as u32)
                    .wrapping_add(rs2_value)
                    .wrapping_add(i64::from(offset).cast_unsigned());
                let value = memory.read::<u32>(addr)?;
                ExecutionResult::Continue {
                    rd,
                    value: u64::from(value),
                }
            }
        }
    }
}
