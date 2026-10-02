//! RV32 F extension

use crate::instructions::Instruction;
use crate::registers::general_purpose::Register;
use core::any::TypeId;
use core::fmt;

/// RISC-V RV32 F instruction (placeholder)
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
#[doc(hidden)]
pub enum Rv32F<Reg> {
    F(Reg, !),
}

const impl<Reg> Instruction for Rv32F<Reg>
where
    Reg: [const] Register<Type = u32>,
{
    const IMPLEMENTED_EXTENSIONS: &'static [TypeId] = &[];

    const ALIGNMENT: u8 = align_of::<u32>() as u8;

    type Reg = Reg;

    #[inline(always)]
    fn try_decode(_instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

impl<Reg> fmt::Display for Rv32F<Reg>
where
    Reg: fmt::Display,
{
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rv32F::F(_, _) => {
                unreachable!("Impossible to construct F instruction")
            }
        }
    }
}
