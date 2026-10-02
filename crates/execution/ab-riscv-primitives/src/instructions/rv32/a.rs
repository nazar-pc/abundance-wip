//! RV32 A extension

pub mod zaamo;
pub mod zalrsc;

use crate::instructions::isa::{IsaExtension, MAX_ISA_EXTENSIONS};
use crate::instructions::rv32::a::zaamo::Rv32ZaamoInstruction;
use crate::instructions::rv32::a::zalrsc::Rv32ZalrscInstruction;
use crate::instructions::{Instruction, InstructionIsa};
use crate::registers::general_purpose::Register;
use ab_riscv_macros::instruction;
use core::fmt;

/// RISC-V RV32 A (Zaamo + Zalrsc) instruction
#[instruction(
    inherit = [Rv32ZaamoInstruction, Rv32ZalrscInstruction]
)]
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
pub enum Rv32AInstruction<Reg> {}

#[instruction]
const impl<Reg> Instruction for Rv32AInstruction<Reg>
where
    Reg: [const] Register<Type = u32>,
{
    const ALIGNMENT: u8 = align_of::<u32>() as u8;

    type Reg = Reg;

    #[inline(always)]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

#[instruction]
impl<Reg, Cfg> InstructionIsa<Cfg> for Rv32AInstruction<Reg>
where
    Reg: Register<Type = u32>,
{
    const OWN_ISA_EXTENSIONS: &'static [IsaExtension] = &[IsaExtension::new("a", 2, 1)];
}

#[instruction]
impl<Reg> fmt::Display for Rv32AInstruction<Reg>
where
    Reg: fmt::Display + Copy,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {}
    }
}
