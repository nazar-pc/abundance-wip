//! Instruction fusion.
//!
//! Fusion replaces a pair of adjacent instructions with a single instruction that does the work of
//! both. It is the same trick hardware macro-fusion plays, and the set of pairs implemented here
//! follows the macro fusions LLVM knows how to emit code for (see `RISCVMacroFusion.td` there), so
//! that a guest built with the corresponding `-C target-feature` already contains the pairs in the
//! shape this expects.
//!
//! What an interpreter gets out of it is one dispatch instead of two, and one decoded instruction
//! read instead of two. The instruction it produces is exactly as wide as the pair it replaces (as
//! reported by [`Instruction::size()`]), so stepping over the fused instruction lands on whatever
//! followed the pair, and everything that walks the instruction stream keeps working unchanged.
//!
//! [`Instruction::size()`]: ab_riscv_primitives::prelude::Instruction::size
//!
//! ## Fusion is not decoding
//!
//! Fused instructions are deliberately absent from [`Instruction::try_decode()`]: nothing encodes
//! them, they only ever come from [`FusedInstruction::fuse()`] being handed two instructions that
//! were decoded the ordinary way. A fused extension inherits the extensions whose instructions it
//! matches on, which is what allows its `fuse()` to name their variants directly, and its own
//! `try_decode()` contributes nothing.
//!
//! [`Instruction::try_decode()`]: ab_riscv_primitives::prelude::Instruction::try_decode
//!
//! ## When a pair may be fused
//!
//! Every fusion implemented here requires the first instruction's destination register to be both
//! the second instruction's source and the second instruction's destination, and to not be the
//! zero register. That is what makes the intermediate value dead: nothing outside the pair can
//! observe it, so computing it at all is unnecessary and the fused instruction is exactly
//! equivalent to executing the two in sequence.
//!
//! The one thing that is not preserved is the state left behind by a pair whose second instruction
//! fails: unfused, the first instruction has already written its destination register by then,
//! while fused, nothing is written. Such a failure ends execution here with
//! [`ExecutionError`](crate::ExecutionError), so a guest can't observe the difference, only a host
//! inspecting the register file afterward can.
//!
//! ## What a fused instruction can carry
//!
//! A fused instruction is the same size as every other instruction of the instruction sets it is
//! composed into, which leaves it the operands of one instruction to describe two with. The pairs
//! implemented here fit because the second instruction of every one of them names no register the
//! first one doesn't, and because a pair of immediates collapses into one: two 12-bit offsets add
//! up to a single offset, and a `lui`/`auipc` immediate plus the immediate of the instruction
//! fused with it is one immediate the pair was splitting between them in the first place.
//!
//! Two consequences are visible from the outside. A `lui`/`auipc` pair is only fused when the sum
//! fits into the 24 bits there is room for, see
//! [`upper_immediate_fits()`](fused_helpers::upper_immediate_fits). And `addi` + load, whose two
//! immediates are added together rather than kept apart, prints as the load it is equivalent to,
//! while every other fused instruction prints as the first instruction of the pair it replaced.
//!
//! ## Composition
//!
//! A fused extension is composed like any other, and an instruction set that inherits several of
//! them gets a `fuse()` with the arms of all of them, in the order the extensions are inherited.
//! That order matters where fusions overlap: `bfext` covers every `slli`+`srli` pair, including the
//! ones `zexth`, `zextw` and `shifted-zextw` recognize, so the extension carrying the specialized
//! fusion has to come first for it to ever be reached. Within one extension the order is the order
//! the arms are written in.

pub mod fused_helpers;
pub mod rv32;
pub mod rv64;

use ab_riscv_primitives::prelude::*;

/// An instruction set whose instructions can be fused pairwise.
///
/// See [module-level documentation](self) for details.
pub const trait FusedInstruction
where
    Self: Instruction,
{
    /// Fuse a pair of adjacent instructions, `prev` immediately followed by `next`.
    ///
    /// Returns the pair to store in their place. A pair that is not fusable is returned unchanged,
    /// and a pair that is fusable is returned as the fused instruction followed by `next` as it
    /// was: `next` keeps its own slot in the instruction stream because a branch may still target
    /// it directly, in which case it is executed on its own.
    fn fuse(prev: Self, next: Self) -> (Self, Self);
}
