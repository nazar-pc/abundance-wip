//! RV64 D extension

use crate::instructions::Instruction;
use crate::registers::general_purpose::Register;
use core::any::TypeId;
use core::fmt;

/// RISC-V RV64 D instruction (placeholder)
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
#[doc(hidden)]
pub enum Rv64D<Reg> {
    D(Reg, !),
}

const impl<Reg> Instruction for Rv64D<Reg>
where
    Reg: [const] Register<Type = u64>,
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

impl<Reg> fmt::Display for Rv64D<Reg>
where
    Reg: fmt::Display,
{
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rv64D::D(_, _) => {
                unreachable!("Impossible to construct D instruction")
            }
        }
    }
}
