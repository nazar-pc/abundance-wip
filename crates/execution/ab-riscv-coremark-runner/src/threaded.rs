//! Tail-call-threaded dispatch over the pre-decoded instruction stream.
//!
//! One handler function per instruction variant, generated from the single [`ops!`] table below,
//! chained with guaranteed tail calls (`become`) so that a handler never returns to a driver loop.
//! Handlers do far less work per instruction than the generic `BasicInterpreterState::execute()`
//! loop, because each one knows statically how wide its instruction is, which operands it actually
//! needs, and whether it can branch at all. The generic loop cannot know any of that, so it pays
//! for the union of what every instruction might need before it dispatches.
//!
//! Nothing here is specialized to this runner's concrete types and no handler contains `unsafe`:
//!
//! * registers go through `RegisterFile`, memory through `VirtualMemory`, so both are generic and
//!   *borrowed* rather than owned by a state struct
//! * the components are passed as separate arguments rather than bundled behind one pointer, so
//!   each one stays in its own argument register across a tail call instead of being reloaded
//! * the instruction pointer is a `&I` newtype rather than a raw pointer, and stopping is expressed
//!   with `Option` rather than a null pointer
//!
//! The unsafe surface is two one-line helpers ([`Ip::advance`] and [`Ip::discriminant`]), both in
//! this scaffolding rather than in generated per-instruction code.
//!
//! The register file stays a type parameter, because that is the design conclusion rather than an
//! open question: [`ZeroStoreRegisters`] won on Zen 4 and is the only one implemented here, but
//! nothing in the handlers knows that.
//!
//! Set `COREMARK_DISPATCH` to any value to run this; leaving it unset runs the generic loop, which
//! is the baseline to compare against. Both live in the same binary.
//!
//! What was measured and lost is not here but is in the git history of this file: the raw-pointer
//! `match`, `call`, `become` and plain-tail-call back ends, the `basic` and `branchless` register
//! files, and `extern "rust-preserve-none"` handlers.

use crate::instruction::CoremarkInstruction as I;
use ab_riscv_interpreter::basic::BasicRegister;
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::instructions::utils::{I24, I24WithZeroedBits};
use ab_riscv_primitives::prelude::*;
use core::hint::{cold_path, unreachable_unchecked};
use core::marker::PhantomData;
use core::{fmt, ptr};
use std::time::Instant;

/// Register type used by the Coremark runner
type R = Reg<u64>;

/// Size of the dispatch table.
///
/// The index is a `u8` (see [`Ip::discriminant`]), so a table this size is indexable without a
/// bounds check and without `unsafe`.
const VARIANTS: usize = 256;

/// Why execution stopped
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Reached the return trap address
    Done,
    /// Out-of-bounds memory access
    OutOfBounds,
    /// Jump to an invalid or unaligned address
    BadJump,
    /// Illegal or unimplemented instruction
    IllegalInstruction,
    /// CSR access this runner does not implement
    UnsupportedCsr(u16),
    /// Discriminant with no handler in the dispatch table. Unreachable as long as the [`ops!`]
    /// table stays exhaustive over `CoremarkInstruction`, which it is checked to be.
    Unsupported(u16),
}

/// The instruction table.
///
/// Each entry is `Variant, slots, { bound fields }, body`, where `body` is a block that evaluates
/// to the instruction pointer to continue at. Bodies may use `ctx`, `ip` and `next`, the last
/// being `ip` already advanced past this instruction.
///
/// This table is exhaustive over `CoremarkInstruction`, deliberately: there is no catch-all arm,
/// so adding an extension to the enum fails to compile here instead of trapping at run time.
macro_rules! ops {
    ($emit:ident) => {
        $emit! {
            ctx, ip, next,
            // Listed in `CoremarkInstruction` declaration order so that this table can be checked
            // against the generated enum definition. `slots` is how many slots of the decoded
            // stream the instruction occupies: one per two guest bytes, so two for a 32-bit
            // instruction and one for a compressed one.

            // ----- RV64I, register-register -----
            Add, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sub, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_sub(reg_read!(ctx, rs2))); next };
            Sll, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) << (reg_read!(ctx, rs2) & 0x3f)); next };
            Slt, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).cast_signed() < reg_read!(ctx, rs2).cast_signed())); next };
            Sltu, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) < reg_read!(ctx, rs2))); next };
            Xor, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ reg_read!(ctx, rs2)); next };
            Srl, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) >> (reg_read!(ctx, rs2) & 0x3f)); next };
            Sra, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1).cast_signed() >> (reg_read!(ctx, rs2) & 0x3f)).cast_unsigned()); next };
            Or, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | reg_read!(ctx, rs2)); next };
            And, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & reg_read!(ctx, rs2)); next };
            Addw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_add(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Subw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_sub(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Sllw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((((reg_read!(ctx, rs1) as u32) << (reg_read!(ctx, rs2) & 0x1f))).cast_signed()).cast_unsigned()); next };
            Srlw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((((reg_read!(ctx, rs1) as u32) >> (reg_read!(ctx, rs2) & 0x1f))).cast_signed()).cast_unsigned()); next };
            Sraw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32) >> (reg_read!(ctx, rs2) & 0x1f)).cast_unsigned()); next };

            // ----- RV64I, register-immediate -----
            Addi, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned())); next };
            Slti, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).cast_signed() < i64::from(imm))); next };
            Sltiu, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) < i64::from(imm).cast_unsigned())); next };
            Xori, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ i64::from(imm).cast_unsigned()); next };
            Ori, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | i64::from(imm).cast_unsigned()); next };
            Andi, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & i64::from(imm).cast_unsigned()); next };
            Slli, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) << shamt); next };
            Srli, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) >> shamt); next };
            Srai, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1).cast_signed() >> shamt).cast_unsigned()); next };
            Addiw, 2, { rs1: R, rd: R, imm: i16 }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_add(i32::from(imm))).cast_unsigned()); next };
            Slliw, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, i64::from(((reg_read!(ctx, rs1) as u32) << shamt).cast_signed()).cast_unsigned()); next };
            Srliw, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, i64::from(((reg_read!(ctx, rs1) as u32) >> shamt).cast_signed()).cast_unsigned()); next };
            Sraiw, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32) >> shamt).cast_unsigned()); next };

            // ----- RV64I, loads -----
            Lb, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i8, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Lh, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i16, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Lw, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Ld, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            Lbu, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u8, a); reg_write!(ctx, rd, u64::from(v)); next };
            Lhu, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u16, a); reg_write!(ctx, rd, u64::from(v)); next };
            Lwu, 2, { rs1: R, rd: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u32, a); reg_write!(ctx, rd, u64::from(v)); next };

            // ----- RV64I, indirect jump -----
            Jalr, 2, { rs1: R, rd: R, imm: i16 }, { let pc = current_pc!(ctx, ip); let target = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()) & !1u64; reg_write!(ctx, rd, pc.wrapping_add(size_of::<u32>() as u64)); jump_absolute!(ctx, target) };

            // ----- RV64I, stores -----
            Sb, 2, { rs1: R, rs2: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u8, a, reg_read!(ctx, rs2) as u8); next };
            Sh, 2, { rs1: R, rs2: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u16, a, reg_read!(ctx, rs2) as u16); next };
            Sw, 2, { rs1: R, rs2: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            Sd, 2, { rs1: R, rs2: R, imm: i16 }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };

            // ----- RV64I, branches -----
            Beq, 2, { rs1: R, rs2: R, imm: I24 }, { if reg_read!(ctx, rs1) == reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Bne, 2, { rs1: R, rs2: R, imm: I24 }, { if reg_read!(ctx, rs1) != reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Blt, 2, { rs1: R, rs2: R, imm: I24 }, { if reg_read!(ctx, rs1).cast_signed() < reg_read!(ctx, rs2).cast_signed() { jump_relative!(ctx, ip, imm) } else { next } };
            Bge, 2, { rs1: R, rs2: R, imm: I24 }, { if reg_read!(ctx, rs1).cast_signed() >= reg_read!(ctx, rs2).cast_signed() { jump_relative!(ctx, ip, imm) } else { next } };
            Bltu, 2, { rs1: R, rs2: R, imm: I24 }, { if reg_read!(ctx, rs1) < reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Bgeu, 2, { rs1: R, rs2: R, imm: I24 }, { if reg_read!(ctx, rs1) >= reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };

            // ----- RV64I, upper immediate and direct jump -----
            Lui, 2, { rd: R, imm: I24WithZeroedBits<12> }, { reg_write!(ctx, rd, i64::from(imm).cast_unsigned()); next };
            Auipc, 2, { rd: R, imm: I24WithZeroedBits<12> }, { let pc = current_pc!(ctx, ip); reg_write!(ctx, rd, pc.wrapping_add(i64::from(imm).cast_unsigned())); next };
            Jal, 2, { rd: R, imm: I24 }, { let pc = current_pc!(ctx, ip); reg_write!(ctx, rd, pc.wrapping_add(size_of::<u32>() as u64)); jump_relative!(ctx, ip, imm) };

            // ----- RV64I, system -----
            Fence, 2, {}, { next };
            FenceTso, 2, {}, { next };
            Ecall, 2, {}, { trap!(ctx, IllegalInstruction) };
            Ebreak, 2, {}, { next };
            Unimp, 2, {}, { trap!(ctx, IllegalInstruction) };

            // ----- M -----
            Mul, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_mul(reg_read!(ctx, rs2))); next };
            Mulh, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, ((i128::from(reg_read!(ctx, rs1).cast_signed()) * i128::from(reg_read!(ctx, rs2).cast_signed())) >> 64) as u64); next };
            Mulhsu, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, ((i128::from(reg_read!(ctx, rs1).cast_signed()) * i128::from(reg_read!(ctx, rs2))) >> 64) as u64); next };
            Mulhu, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, ((u128::from(reg_read!(ctx, rs1)) * u128::from(reg_read!(ctx, rs2))) >> 64) as u64); next };
            Div, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1).cast_signed(); let b = reg_read!(ctx, rs2).cast_signed(); let v = if b == 0 { -1i64 } else if a == i64::MIN && b == -1 { i64::MIN } else { a / b }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Divu, 2, { rs1: R, rs2: R, rd: R }, { let b = reg_read!(ctx, rs2); reg_write!(ctx, rd, reg_read!(ctx, rs1).checked_div(b).unwrap_or(u64::MAX)); next };
            Rem, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1).cast_signed(); let b = reg_read!(ctx, rs2).cast_signed(); let v = if b == 0 { a } else if a == i64::MIN && b == -1 { 0 } else { a % b }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Remu, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1); let b = reg_read!(ctx, rs2); reg_write!(ctx, rd, if b == 0 { a } else { a % b }); next };
            Mulw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_mul(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Divw, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1) as i32; let b = reg_read!(ctx, rs2) as i32; let v = if b == 0 { -1i64 } else if a == i32::MIN && b == -1 { i64::from(i32::MIN) } else { i64::from(a / b) }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Divuw, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1) as u32; let b = reg_read!(ctx, rs2) as u32; let v = match a.checked_div(b) { Some(v) => i64::from(v.cast_signed()).cast_unsigned(), None => u64::MAX }; reg_write!(ctx, rd, v); next };
            Remw, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1) as i32; let b = reg_read!(ctx, rs2) as i32; let v = if b == 0 { i64::from(a) } else if a == i32::MIN && b == -1 { 0 } else { i64::from(a % b) }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Remuw, 2, { rs1: R, rs2: R, rd: R }, { let a = reg_read!(ctx, rs1) as u32; let b = reg_read!(ctx, rs2) as u32; let v = if b == 0 { a.cast_signed() } else { (a % b).cast_signed() }; reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };

            // ----- Zba -----
            AddUw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u32).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh1add, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh1addUw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh2add, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 2).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh2addUw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 2).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh3add, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 3).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh3addUw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 3).wrapping_add(reg_read!(ctx, rs2))); next };
            SlliUw, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u32) << shamt); next };

            // ----- Zbb -----
            Andn, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & !reg_read!(ctx, rs2)); next };
            Orn, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | !reg_read!(ctx, rs2)); next };
            Xnor, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, !(reg_read!(ctx, rs1) ^ reg_read!(ctx, rs2))); next };
            Clz, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).leading_zeros())); next };
            Clzw, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from((reg_read!(ctx, rs1) as u32).leading_zeros())); next };
            Ctz, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).trailing_zeros())); next };
            Ctzw, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from((reg_read!(ctx, rs1) as u32).trailing_zeros())); next };
            Cpop, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).count_ones())); next };
            Cpopw, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from((reg_read!(ctx, rs1) as u32).count_ones())); next };
            Max, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).cast_signed().max(reg_read!(ctx, rs2).cast_signed()).cast_unsigned()); next };
            Maxu, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).max(reg_read!(ctx, rs2))); next };
            Min, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).cast_signed().min(reg_read!(ctx, rs2).cast_signed()).cast_unsigned()); next };
            Minu, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).min(reg_read!(ctx, rs2))); next };
            Sextb, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, i64::from(reg_read!(ctx, rs1) as i8).cast_unsigned()); next };
            Sexth, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, i64::from(reg_read!(ctx, rs1) as i16).cast_unsigned()); next };
            Zexth, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u16)); next };
            Rol, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).rotate_left((reg_read!(ctx, rs2) & 0x3f) as u32)); next };
            Rolw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as u32).rotate_left((reg_read!(ctx, rs2) & 0x1f) as u32).cast_signed()).cast_unsigned()); next };
            Ror, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).rotate_right((reg_read!(ctx, rs2) & 0x3f) as u32)); next };
            Rori, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).rotate_right(u32::from(shamt & 0x3f))); next };
            Roriw, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as u32).rotate_right(u32::from(shamt & 0x1f)).cast_signed()).cast_unsigned()); next };
            Rorw, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as u32).rotate_right((reg_read!(ctx, rs2) & 0x1f) as u32).cast_signed()).cast_unsigned()); next };
            Orcb, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, rv64_zbb_helpers::orc_b(reg_read!(ctx, rs1))); next };
            Rev8, 2, { rs1: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).swap_bytes()); next };

            // ----- Zbs -----
            Bset, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | (1u64 << (reg_read!(ctx, rs2) & 0x3f))); next };
            Bseti, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | (1u64 << shamt)); next };
            Bclr, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & !(1u64 << (reg_read!(ctx, rs2) & 0x3f))); next };
            Bclri, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & !(1u64 << shamt)); next };
            Binv, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ (1u64 << (reg_read!(ctx, rs2) & 0x3f))); next };
            Binvi, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ (1u64 << shamt)); next };
            Bext, 2, { rs1: R, rs2: R, rd: R }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) >> (reg_read!(ctx, rs2) & 0x3f)) & 1); next };
            Bexti, 2, { rs1: R, rd: R, shamt: u8 }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) >> shamt) & 1); next };

            // ----- Zca, quadrant 0 -----
            CAddi4spn, 1, { rd: R, nzuimm: u16 }, { reg_write!(ctx, rd, reg_read!(ctx, R::Sp).wrapping_add(u64::from(nzuimm))); next };
            CLw, 1, { rs1: R, rd: R, uimm: u8 }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            CLd, 1, { rs1: R, rd: R, uimm: u8 }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            CSw, 1, { rs1: R, rs2: R, uimm: u8 }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            CSd, 1, { rs1: R, rs2: R, uimm: u8 }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };

            // ----- Zca, quadrant 1 -----
            CNop, 1, {}, { next };
            CAddi, 1, { rd: R, nzimm: i8 }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_add(i64::from(nzimm).cast_unsigned())); next };
            CAddiw, 1, { rd: R, imm: i8 }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rd) as i32).wrapping_add(i32::from(imm))).cast_unsigned()); next };
            CLi, 1, { rd: R, imm: i8 }, { reg_write!(ctx, rd, i64::from(imm).cast_unsigned()); next };
            CAddi16sp, 1, { nzimm: i16 }, { reg_write!(ctx, R::Sp, reg_read!(ctx, R::Sp).wrapping_add(i64::from(nzimm).cast_unsigned())); next };
            CLui, 1, { rd: R, nzimm: I24 }, { reg_write!(ctx, rd, i64::from(nzimm).cast_unsigned()); next };
            CSrli, 1, { rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rd) >> shamt); next };
            CSrai, 1, { rd: R, shamt: u8 }, { reg_write!(ctx, rd, (reg_read!(ctx, rd).cast_signed() >> shamt).cast_unsigned()); next };
            CAndi, 1, { rd: R, imm: i8 }, { reg_write!(ctx, rd, reg_read!(ctx, rd) & i64::from(imm).cast_unsigned()); next };
            CSub, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_sub(reg_read!(ctx, rs2))); next };
            CXor, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rd) ^ reg_read!(ctx, rs2)); next };
            COr, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rd) | reg_read!(ctx, rs2)); next };
            CAnd, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rd) & reg_read!(ctx, rs2)); next };
            CSubw, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rd) as i32).wrapping_sub(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            CAddw, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rd) as i32).wrapping_add(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            CJ, 1, { imm: i16 }, { jump_relative!(ctx, ip, imm) };
            CBeqz, 1, { rs1: R, imm: i16 }, { if reg_read!(ctx, rs1) == 0 { jump_relative!(ctx, ip, imm) } else { next } };
            CBnez, 1, { rs1: R, imm: i16 }, { if reg_read!(ctx, rs1) != 0 { jump_relative!(ctx, ip, imm) } else { next } };

            // ----- Zca, quadrant 2 -----
            CSlli, 1, { rd: R, shamt: u8 }, { reg_write!(ctx, rd, reg_read!(ctx, rd) << shamt); next };
            CLwsp, 1, { rd: R, uimm: u8 }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            CLdsp, 1, { rd: R, uimm: u16 }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            CJr, 1, { rs1: R }, { jump_absolute!(ctx, reg_read!(ctx, rs1) & !1u64) };
            CMv, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rs2)); next };
            CEbreak, 1, {}, { next };
            CJalr, 1, { rs1: R }, { let pc = current_pc!(ctx, ip); let target = reg_read!(ctx, rs1) & !1u64; reg_write!(ctx, R::Ra, pc.wrapping_add(size_of::<u16>() as u64)); jump_absolute!(ctx, target) };
            CAdd, 1, { rs2: R, rd: R }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_add(reg_read!(ctx, rs2))); next };
            CSwsp, 1, { rs2: R, uimm: u8 }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            CSdsp, 1, { rs2: R, uimm: u16 }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };
            CUnimp, 1, {}, { trap!(ctx, IllegalInstruction) };

            // ----- Zicsr -----
            //
            // The only CSR this runner implements is `time`, and it is read-only, so anything but
            // a pure read of it stops execution rather than quietly returning a wrong value.
            Csrrw, 2, {}, { csr_illegal!(ctx) };
            Csrrs, 2, { rs1: R, rd: R, csr_index: u16 }, { csr_read!(ctx, rd, csr_index, reg_read!(ctx, rs1)); next };
            Csrrc, 2, { rs1: R, rd: R, csr_index: u16 }, { csr_read!(ctx, rd, csr_index, reg_read!(ctx, rs1)); next };
            Csrrwi, 2, {}, { csr_illegal!(ctx) };
            Csrrsi, 2, { rd: R, zimm: u8, csr_index: u16 }, { csr_read!(ctx, rd, csr_index, u64::from(zimm)); next };
            Csrrci, 2, { rd: R, zimm: u8, csr_index: u16 }, { csr_read!(ctx, rd, csr_index, u64::from(zimm)); next };
        }
    };
}

// --------------------------------------------------------------------------------------------
// Variant 1: one big `match`, each arm advancing the instruction pointer itself
// --------------------------------------------------------------------------------------------

// --------------------------------------------------------------------------------------------
// Entry point
// --------------------------------------------------------------------------------------------

// --------------------------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------------------------

/// Position in the decoded instruction stream
#[derive(Debug, Copy, Clone)]
pub(crate) struct Ip<'a>(&'a I);

/// The decoded program, plus what is needed to turn a guest address into a position in it.
///
/// This is the equivalent of an instruction fetcher: it is borrowed by the interpreter, not owned.
#[derive(Debug)]
pub(crate) struct Stream<'a> {
    instructions: &'a [I],
    base_addr: u64,
    return_trap: u64,
}

/// Stand-in for extension state.
///
/// Also carries why execution stopped, so that a handler can return just an instruction pointer.
/// Returning `Result<Ip, Stop>` instead costs a hidden `sret` pointer, which pushes the sixth
/// argument onto the stack and gives every handler a stack frame.
#[derive(Debug)]
pub(crate) struct Ext {
    start: Instant,
    stop: Stop,
}

/// Stand-in for a system instruction handler, present so that the handler signature carries the
/// same six arguments a real implementation would need
#[derive(Debug)]
pub(crate) struct Sys;

/// Result of a handler: where to continue, or `None` when execution stopped, in which case the
/// reason is in [`Ext::stop`]. `Option<Ip>` is pointer-sized thanks to the null niche, so it comes
/// back in a register.
type SafeNext<'a> = Option<Ip<'a>>;

/// A single instruction handler.
///
/// Six arguments, which is exactly the number of integer argument registers x86-64 SysV has, so
/// nothing spills. Adding a seventh would, once per dispatch — see the notes on
/// `extern "rust-preserve-none"`, which has twelve and is the answer if that ever happens.
type SafeHandler<Regs, Memory> =
    for<'a> fn(Ip<'a>, &mut Regs, &mut Memory, &mut Ext, &Stream<'a>, &mut Sys) -> SafeNext<'a>;

impl<'a> Ip<'a> {
    /// Position of the instruction at guest address `address`
    #[inline(always)]
    fn at_address(stream: &Stream<'a>, address: u64) -> Option<Self> {
        let offset = address.checked_sub(stream.base_addr)?;

        if !offset.is_multiple_of(size_of::<u16>() as u64) {
            cold_path();
            return None;
        }

        Self::at_slot(stream, (offset / size_of::<u16>() as u64) as usize)
    }

    /// Position of slot `slot` of the stream
    #[inline(always)]
    fn at_slot(stream: &Stream<'a>, slot: usize) -> Option<Self> {
        Some(Self(stream.instructions.get(slot)?))
    }

    /// Slot this position refers to
    #[inline(always)]
    fn slot(self, stream: &Stream<'a>) -> usize {
        // Plain address arithmetic, no dereference, so this stays outside `unsafe`
        (self.0 as *const I as usize - stream.instructions.as_ptr() as usize) / size_of::<I>()
    }

    /// The instruction at this position
    #[inline(always)]
    fn get(self) -> &'a I {
        self.0
    }

    /// Advance by `slots` slots.
    ///
    /// # Safety
    /// The stream must continue for at least `slots` more slots. `Ctx::new()` guarantees this for
    /// every instruction that can fall through, because the decoded stream ends with a jump.
    #[inline(always)]
    unsafe fn advance(self, slots: usize) -> Self {
        // SAFETY: guaranteed by function contract
        Self(unsafe { &*(self.0 as *const I).add(slots) })
    }

    /// Discriminant of the instruction at this position
    #[inline(always)]
    fn discriminant(self) -> u8 {
        // SAFETY: the enum is `#[repr(u16)]`, so its first two bytes are the discriminant
        let discriminant = unsafe { (self.0 as *const I).cast::<u16>().read() };
        // Masking rather than truncating on purpose: it makes the dispatch table index provably
        // in bounds for a 256-entry table, so the lookup needs no bounds check and no `unsafe`
        discriminant as u8
    }
}

/// The borrowed components, reassembled from the handler arguments.
///
/// Purely a naming convenience for the instruction table: it is a local whose address never
/// escapes, so it is scalarized away and each field stays in the register it arrived in.
struct Env<'a, 'r, Regs, Memory> {
    regs: &'r mut Regs,
    memory: &'r mut Memory,
    ext: &'r mut Ext,
    stream: &'r Stream<'a>,
    sys: &'r mut Sys,
}

// The instruction table below is expanded a second time against these definitions of the helper
// macros; `macro_rules!` resolves by textual order, so these shadow the raw-pointer versions above
// for every expansion that follows.

macro_rules! reg_read {
    ($ctx:ident, $reg:expr) => {
        RegisterFile::read($ctx.regs, $reg)
    };
}

macro_rules! reg_write {
    ($ctx:ident, $reg:expr, $value:expr) => {
        RegisterFile::write($ctx.regs, $reg, $value)
    };
}

/// Record why execution stopped and unwind out of the handler chain
macro_rules! stop {
    ($ctx:ident, $stop:expr) => {{
        cold_path();
        $ctx.ext.stop = $stop;
        return None;
    }};
}

macro_rules! mem_read {
    ($ctx:ident, $ty:ty, $addr:expr) => {
        match VirtualMemory::read::<$ty>($ctx.memory, $addr) {
            Ok(value) => value,
            Err(_error) => stop!($ctx, Stop::OutOfBounds),
        }
    };
}

macro_rules! mem_write {
    ($ctx:ident, $ty:ty, $addr:expr, $value:expr) => {
        match VirtualMemory::write::<$ty>($ctx.memory, $addr, $value) {
            Ok(()) => {}
            Err(_error) => stop!($ctx, Stop::OutOfBounds),
        }
    };
}

macro_rules! csr_read {
    ($ctx:ident, $rd:expr, $csr_index:expr, $write_operand:expr) => {{
        const CSR_TIME: u16 = 0xC01;

        if $csr_index != CSR_TIME || $write_operand != 0 {
            stop!($ctx, Stop::UnsupportedCsr($csr_index));
        }

        let elapsed = $ctx.ext.start.elapsed().as_nanos() as u64;
        reg_write!($ctx, $rd, elapsed);
    }};
}

macro_rules! csr_illegal {
    ($ctx:ident) => {
        stop!($ctx, Stop::UnsupportedCsr(0))
    };
}

macro_rules! trap {
    ($ctx:ident, $stop:ident) => {
        stop!($ctx, Stop::$stop)
    };
}

macro_rules! current_pc {
    ($ctx:ident, $ip:ident) => {
        $ctx.stream
            .base_addr
            .wrapping_add(($ip.slot($ctx.stream) * size_of::<u16>()) as u64)
    };
}

macro_rules! jump_relative {
    ($ctx:ident, $ip:ident, $imm:expr) => {{
        // Arithmetic shift rather than `/ 2`: branch and jump immediates always have their low
        // bit clear, so this is exact, and it avoids the round-toward-zero fixup that signed
        // division expands into
        let slot = $ip
            .slot($ctx.stream)
            .wrapping_add_signed((i64::from($imm) >> 1) as isize);

        match Ip::at_slot($ctx.stream, slot) {
            Some(target) => target,
            None => stop!($ctx, Stop::BadJump),
        }
    }};
}

macro_rules! jump_absolute {
    ($ctx:ident, $target:expr) => {{
        let target = $target;

        if target == $ctx.stream.return_trap {
            stop!($ctx, Stop::Done);
        }

        match Ip::at_address($ctx.stream, target) {
            Some(target) => target,
            None => stop!($ctx, Stop::BadJump),
        }
    }};
}

macro_rules! emit_safe_handlers {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident: $ty:ty),* $(,)? }, $body:block);* $(;)?
    ) => {
        mod safe_handlers {
            use super::*;

            $(
                #[expect(non_snake_case, reason = "One handler per instruction variant")]
                #[allow(
                    unused_variables,
                    unreachable_code,
                    reason = "Unconditional jumps do not use the advanced pointer, and trapping \
                        instructions never reach it"
                )]
                pub(super) fn $name<'a, Regs, Memory>(
                    $ip: Ip<'a>,
                    regs: &mut Regs,
                    memory: &mut Memory,
                    ext: &mut Ext,
                    stream: &Stream<'a>,
                    sys: &mut Sys,
                ) -> SafeNext<'a>
                where
                    Regs: RegisterFile<R>,
                    Memory: VirtualMemory,
                {
                    let I::$name { $($field,)* .. } = *$ip.get() else {
                        // SAFETY: this handler is only ever reached for its own variant
                        unsafe { unreachable_unchecked() }
                    };
                    let $ctx = Env { regs, memory, ext, stream, sys };
                    // SAFETY: the decoded stream ends with a jump, so anything that can fall
                    // through has at least this many slots left
                    let $next = unsafe { $ip.advance($slots) };
                    let $next = $body;
                    let handler = table::<Regs, Memory>($next);
                    become handler(
                        $next,
                        $ctx.regs,
                        $ctx.memory,
                        $ctx.ext,
                        $ctx.stream,
                        $ctx.sys,
                    )
                }

            )*

            pub(super) fn unsupported<'a, Regs, Memory>(
                ip: Ip<'a>,
                _regs: &mut Regs,
                _memory: &mut Memory,
                ext: &mut Ext,
                _stream: &Stream<'a>,
                _sys: &mut Sys,
            ) -> SafeNext<'a>
            where
                Regs: RegisterFile<R>,
                Memory: VirtualMemory,
            {
                cold_path();
                ext.stop = Stop::Unsupported(u16::from(ip.discriminant()));
                None
            }


            /// Dispatch table, in enum declaration order.
            ///
            /// `CoremarkInstruction` is `#[repr(u16)]` with no explicit discriminants, so variant
            /// N has discriminant N and this table can be built at compile time rather than by
            /// scanning the program.
            pub(super) const fn build<Regs, Memory>() -> [SafeHandler<Regs, Memory>; VARIANTS]
            where
                Regs: RegisterFile<R>,
                Memory: VirtualMemory,
            {
                let mut table: [SafeHandler<Regs, Memory>; VARIANTS] = [unsupported; VARIANTS];
                let mut index = 0;
                let listed: &[SafeHandler<Regs, Memory>] = &[$($name,)*];

                while index < listed.len() {
                    table[index] = listed[index];
                    index += 1;
                }

                table
            }
        }
    };
}

ops!(emit_safe_handlers);

/// Handler for the instruction at `ip`
#[inline(always)]
fn table<Regs, Memory>(ip: Ip<'_>) -> SafeHandler<Regs, Memory>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    const {
        // Every variant must be listed, otherwise dispatch would silently fall back to
        // `unsupported` for the missing ones
        assert!(size_of::<I>() == 8);
    }

    // The index is a `u8` and the table has 256 entries, so this is in bounds by construction and
    // needs neither a check nor `unsafe`
    Handlers::<Regs, Memory>::TABLE[usize::from(ip.discriminant())]
}

struct Handlers<Regs, Memory>(PhantomData<(Regs, Memory)>);

impl<Regs, Memory> Handlers<Regs, Memory>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    const TABLE: [SafeHandler<Regs, Memory>; VARIANTS] = safe_handlers::build::<Regs, Memory>();
}

/// A register file with no branch *and* no conditional move.
///
/// Writes to `x0` have to be discarded. Branching on it costs a branch, and steering it into a sink
/// slot costs a `cmov`; here the write lands unconditionally and slot zero is re-zeroed afterwards,
/// which restores the invariant that makes reads unconditional. That trades three ALU ops for one
/// extra store and leaves a handler with no internal control flow at all.
///
/// It won on Zen 4 by 6.3% over the ordinary register file with `-C target-cpu=znver4` and 8.8%
/// without, with the sink-slot variant between the two. Neither Xeon could separate the three.
///
/// Note this is the opposite of the right trade for the generic `match` loop, where the same
/// change measured 8.5% *slower*: there, reads dominate and staying branchy avoids loading `x0`
/// for the many instructions whose `rs2` is only a placeholder.
#[derive(Debug, Clone)]
pub(crate) struct ZeroStoreRegisters {
    regs: [u64; 32],
}

impl Default for ZeroStoreRegisters {
    #[inline(always)]
    fn default() -> Self {
        Self { regs: [0; _] }
    }
}

impl RegisterFile<R> for ZeroStoreRegisters {
    #[inline(always)]
    fn read(&self, reg: R) -> u64 {
        // SAFETY: `BasicRegister::offset()` is guaranteed to be below 32
        *unsafe {
            self.regs
                .get_unchecked(usize::from(BasicRegister::offset(reg)))
        }
    }

    #[inline(always)]
    fn write(&mut self, reg: R, value: u64) {
        // SAFETY: `BasicRegister::offset()` is guaranteed to be below 32
        *unsafe {
            self.regs
                .get_unchecked_mut(usize::from(BasicRegister::offset(reg)))
        } = value;
        // Writes to `x0` have to be discarded. Rather than branching or selecting a sink slot,
        // let the write land and put the zero back; reads then never need a check.
        // SAFETY: the register file always has at least one slot
        *unsafe { self.regs.get_unchecked_mut(0) } = 0;
    }
}

// --------------------------------------------------------------------------------------------
// Direct threading
// --------------------------------------------------------------------------------------------

/// One decoded instruction with its handler resolved next to it.
///
/// This is the whole of direct threading: the discriminant and the table lookup it feeds go away,
/// and the jump target comes straight out of the stream. It costs the slot going from 8 bytes to
/// 16, and the slot is already 8 bytes for every 2 bytes of guest code, so the decoded program
/// goes from 4x the size of what it interprets to 8x.
#[repr(C)]
pub(crate) struct DirectSlot<Regs, Memory> {
    handler: DirectHandler<Regs, Memory>,
    instruction: I,
}

// Derived would demand `Regs: Copy`, which is not needed: the fields are a function pointer and a
// `Copy` enum.
impl<Regs, Memory> Copy for DirectSlot<Regs, Memory> {}

impl<Regs, Memory> Clone for DirectSlot<Regs, Memory> {
    fn clone(&self) -> Self {
        *self
    }
}

/// Position in the direct-threaded stream, the counterpart of [`Ip`]
pub(crate) struct DirectIp<'a, Regs, Memory>(&'a DirectSlot<Regs, Memory>);

impl<Regs, Memory> Copy for DirectIp<'_, Regs, Memory> {}

impl<Regs, Memory> Clone for DirectIp<'_, Regs, Memory> {
    fn clone(&self) -> Self {
        *self
    }
}

/// Result of a direct-threaded handler, the counterpart of [`SafeNext`]
type DirectNext<'a, Regs, Memory> = Option<DirectIp<'a, Regs, Memory>>;

/// A single direct-threaded handler, the counterpart of [`SafeHandler`].
///
/// Recursive through [`DirectSlot`], which is fine: a function pointer breaks the cycle.
type DirectHandler<Regs, Memory> = for<'a> fn(
    DirectIp<'a, Regs, Memory>,
    &mut Regs,
    &mut Memory,
    &mut Ext,
    &DirectStream<'a, Regs, Memory>,
    &mut Sys,
) -> DirectNext<'a, Regs, Memory>;

/// The direct-threaded program, the counterpart of [`Stream`]
pub(crate) struct DirectStream<'a, Regs, Memory> {
    instructions: &'a [DirectSlot<Regs, Memory>],
    base_addr: u64,
    return_trap: u64,
}

impl<'a, Regs, Memory> DirectIp<'a, Regs, Memory> {
    /// Position of the instruction at guest address `address`
    #[inline(always)]
    fn at_address(stream: &DirectStream<'a, Regs, Memory>, address: u64) -> Option<Self> {
        let offset = address.checked_sub(stream.base_addr)?;

        if !offset.is_multiple_of(size_of::<u16>() as u64) {
            cold_path();
            return None;
        }

        Self::at_slot(stream, (offset / size_of::<u16>() as u64) as usize)
    }

    /// Position of slot `slot` of the stream
    #[inline(always)]
    fn at_slot(stream: &DirectStream<'a, Regs, Memory>, slot: usize) -> Option<Self> {
        Some(Self(stream.instructions.get(slot)?))
    }

    /// Slot this position refers to
    #[inline(always)]
    fn slot(self, stream: &DirectStream<'a, Regs, Memory>) -> usize {
        // Plain address arithmetic, no dereference, so this stays outside `unsafe`
        (ptr::from_ref(self.0).addr() - stream.instructions.as_ptr().addr())
            / size_of::<DirectSlot<Regs, Memory>>()
    }

    /// The instruction at this position
    #[inline(always)]
    fn get(self) -> &'a I {
        &self.0.instruction
    }

    /// The handler for the instruction at this position.
    ///
    /// This is what replaces the discriminant read and the table lookup.
    #[inline(always)]
    fn handler(self) -> DirectHandler<Regs, Memory> {
        self.0.handler
    }

    /// Advance by `slots` slots.
    ///
    /// # Safety
    /// The stream must continue for at least `slots` more slots, which the decoded stream ending
    /// with a jump guarantees for everything that can fall through.
    #[inline(always)]
    unsafe fn advance(self, slots: usize) -> Self {
        // SAFETY: guaranteed by function contract
        Self(unsafe { &*ptr::from_ref(self.0).add(slots) })
    }
}

/// Emits the direct-threaded handlers from the same instruction table the others are built from.
///
/// Identical to [`emit_safe_handlers`] apart from the dispatch step: where that one reads a
/// discriminant and indexes a table, this one takes the handler out of the slot. The `use`
/// aliases are what let the shared table's bodies expand unchanged - the helper macros they call
/// spell `Ip::at_slot` and `Stream`, which resolve here to the direct versions.
macro_rules! emit_direct_handlers {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident: $ty:ty),* $(,)? }, $body:block);* $(;)?
    ) => {
        mod direct_handlers {
            use super::DirectIp as Ip;
            use super::DirectStream as Stream;
            use super::*;

            /// The borrowed components, as [`super::Env`] but over the direct-threaded stream.
            /// A local item shadows the glob import, so the shared instruction table's bodies
            /// build this one.
            struct Env<'a, 'r, Regs, Memory> {
                regs: &'r mut Regs,
                memory: &'r mut Memory,
                ext: &'r mut Ext,
                stream: &'r Stream<'a, Regs, Memory>,
                sys: &'r mut Sys,
            }

            $(
                #[expect(non_snake_case, reason = "One handler per instruction variant")]
                #[allow(
                    unused_variables,
                    unreachable_code,
                    reason = "Unconditional jumps do not use the advanced pointer, and trapping \
                        instructions never reach it"
                )]
                pub(super) fn $name<'a, Regs, Memory>(
                    $ip: Ip<'a, Regs, Memory>,
                    regs: &mut Regs,
                    memory: &mut Memory,
                    ext: &mut Ext,
                    stream: &Stream<'a, Regs, Memory>,
                    sys: &mut Sys,
                ) -> DirectNext<'a, Regs, Memory>
                where
                    Regs: RegisterFile<R>,
                    Memory: VirtualMemory,
                {
                    let I::$name { $($field,)* .. } = *$ip.get() else {
                        // SAFETY: this handler is only ever reached for its own variant
                        unsafe { unreachable_unchecked() }
                    };
                    let $ctx = Env { regs, memory, ext, stream, sys };
                    // SAFETY: the decoded stream ends with a jump, so anything that can fall
                    // through has at least this many slots left
                    let $next = unsafe { $ip.advance($slots) };
                    let $next = $body;
                    // The whole point: no discriminant, no table. A free function rather than a
                    // method so that it also accepts the `!` a trapping instruction's body
                    // evaluates to, exactly as `table()` does on the other back end.
                    let handler = direct_handler::<Regs, Memory>($next);
                    become handler(
                        $next,
                        $ctx.regs,
                        $ctx.memory,
                        $ctx.ext,
                        $ctx.stream,
                        $ctx.sys,
                    )
                }
            )*

            pub(super) fn unsupported<'a, Regs, Memory>(
                _ip: Ip<'a, Regs, Memory>,
                _regs: &mut Regs,
                _memory: &mut Memory,
                ext: &mut Ext,
                _stream: &Stream<'a, Regs, Memory>,
                _sys: &mut Sys,
            ) -> DirectNext<'a, Regs, Memory>
            where
                Regs: RegisterFile<R>,
                Memory: VirtualMemory,
            {
                cold_path();
                ext.stop = Stop::Unsupported(0);
                None
            }

            /// Dispatch table, used once per instruction when the stream is built rather than once
            /// per instruction executed
            pub(super) const fn build<Regs, Memory>() -> [DirectHandler<Regs, Memory>; VARIANTS]
            where
                Regs: RegisterFile<R>,
                Memory: VirtualMemory,
            {
                let mut table: [DirectHandler<Regs, Memory>; VARIANTS] = [unsupported; VARIANTS];
                let mut index = 0;
                let listed: &[DirectHandler<Regs, Memory>] = &[$($name,)*];

                while index < listed.len() {
                    table[index] = listed[index];
                    index += 1;
                }

                table
            }
        }
    };
}

ops!(emit_direct_handlers);

/// Handler for the instruction at `ip`, the counterpart of [`table()`].
///
/// A free function rather than a method so that it also accepts the `!` that a trapping
/// instruction's body evaluates to, which is what [`table()`] relies on too.
#[inline(always)]
fn direct_handler<Regs, Memory>(ip: DirectIp<'_, Regs, Memory>) -> DirectHandler<Regs, Memory> {
    ip.handler()
}

struct DirectHandlers<Regs, Memory>(PhantomData<(Regs, Memory)>);

impl<Regs, Memory> DirectHandlers<Regs, Memory>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    const TABLE: [DirectHandler<Regs, Memory>; VARIANTS] = direct_handlers::build::<Regs, Memory>();
}

/// Resolve every decoded instruction to its handler, once, before execution starts.
///
/// This is the work that token threading does per dispatch instead.
fn build_direct_stream<Regs, Memory>(instructions: &[I]) -> Vec<DirectSlot<Regs, Memory>>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    instructions
        .iter()
        .map(|instruction| {
            // SAFETY: the enum is `#[repr(u16)]`, so its first two bytes are the discriminant
            let discriminant = unsafe { ptr::from_ref(instruction).cast::<u16>().read() };

            DirectSlot {
                // Masked rather than truncated for the same reason as `Ip::discriminant`
                handler: DirectHandlers::<Regs, Memory>::TABLE[usize::from(discriminant as u8)],
                instruction: *instruction,
            }
        })
        .collect()
}

/// Run the program with the direct-threaded back end
pub(crate) fn run_direct_threaded<Regs, Memory>(
    instructions: &[I],
    base_addr: u64,
    return_trap: u64,
    pc: u64,
    regs: &mut Regs,
    memory: &mut Memory,
) -> Stop
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    const {
        // Two pointers per decoded instruction, against one for token threading
        assert!(size_of::<DirectSlot<ZeroStoreRegisters, ()>>() == 16);
    }

    let slots = build_direct_stream::<Regs, Memory>(instructions);
    let stream = DirectStream {
        instructions: &slots,
        base_addr,
        return_trap,
    };
    let mut ext = Ext {
        start: Instant::now(),
        stop: Stop::Done,
    };
    let mut sys = Sys;

    let Some(ip) = DirectIp::at_address(&stream, pc) else {
        return Stop::BadJump;
    };
    let handler = ip.handler();
    handler(ip, regs, memory, &mut ext, &stream, &mut sys);

    ext.stop
}

// --------------------------------------------------------------------------------------------
// Direct threading with a packed 8-byte slot
// --------------------------------------------------------------------------------------------

/// One decoded operand, as it is stored in a packed slot.
///
/// Two widths, both whole bytes, because the point of the exercise is to keep operands *byte
/// aligned* so that extracting one stays the single `movzbl`/`movswl` it is with the decoded enum.
/// The narrow width is used only by the variants whose wide layout does not fit into four bytes.
trait Field: Copy + PartialEq + fmt::Debug {
    /// Width when this operand is stored at its natural size
    const WIDE: u32;
    /// Width when the variant holding it does not fit that way
    const NARROW: u32;

    fn to_bits(self) -> u32;

    /// Read this operand out of `width` bits starting at byte `byte`
    fn from_bytes(bytes: &[u8; 4], byte: usize, width: u32) -> Self;
}

/// Sign-extend the low `width` bits of `bits`
#[inline(always)]
const fn sign_extend(bits: u32, width: u32) -> i32 {
    ((bits << (u32::BITS - width)) as i32) >> (u32::BITS - width)
}

/// Keep the low `width` bits of `bits`
#[inline(always)]
const fn mask(bits: u32, width: u32) -> u32 {
    if width >= u32::BITS {
        bits
    } else {
        bits & ((1 << width) - 1)
    }
}

impl Field for R {
    const WIDE: u32 = 8;
    const NARROW: u32 = 8;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        u32::from(BasicRegister::offset(self))
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, _width: u32) -> Self {
        // SAFETY: these bits came out of `to_bits()` on a real register, so they name one
        unsafe { Register::from_bits(bytes[byte]).unwrap_unchecked() }
    }
}

impl Field for u8 {
    const WIDE: u32 = 8;
    const NARROW: u32 = 8;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        u32::from(self)
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, _width: u32) -> Self {
        bytes[byte]
    }
}

impl Field for u16 {
    const WIDE: u32 = 16;
    const NARROW: u32 = 16;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        u32::from(self)
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, _width: u32) -> Self {
        u16::from_le_bytes([bytes[byte], bytes[byte + 1]])
    }
}

impl Field for i8 {
    const WIDE: u32 = 8;
    const NARROW: u32 = 8;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        self as u32
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, _width: u32) -> Self {
        bytes[byte].cast_signed()
    }
}

impl Field for i16 {
    const WIDE: u32 = 16;
    const NARROW: u32 = 16;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        self as u32
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, _width: u32) -> Self {
        i16::from_le_bytes([bytes[byte], bytes[byte + 1]])
    }
}

impl Field for I24 {
    const WIDE: u32 = 24;
    // The only variants that do not fit their operands into four bytes are the six B-type
    // branches, `rs1 + rs2 + I24`, and a B-type offset is thirteen bits signed. Sixteen holds it
    // with room to spare and, being a whole number of bytes, keeps the operand a `movswl` rather
    // than a shift and a mask. `pack()` checks the value survives.
    const NARROW: u32 = 16;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        self.to_i32() as u32
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, width: u32) -> Self {
        // `width` is a constant here, so only one of these is emitted, and each is a single load
        I24::from_i32(if width == Self::NARROW {
            i32::from(i16::from_le_bytes([bytes[byte], bytes[byte + 1]]))
        } else {
            sign_extend(
                u32::from_le_bytes([bytes[byte], bytes[byte + 1], bytes[byte + 2], 0]),
                Self::WIDE,
            )
        })
    }
}

impl<const LOW_ZEROED_BITS: u8> Field for I24WithZeroedBits<LOW_ZEROED_BITS> {
    const WIDE: u32 = 24;
    const NARROW: u32 = 24;

    #[inline(always)]
    fn to_bits(self) -> u32 {
        // The stored form is the value shifted down, which is what fits in 24 bits
        (self.to_i32() >> LOW_ZEROED_BITS) as u32
    }

    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4], byte: usize, _width: u32) -> Self {
        Self::from_i32(
            sign_extend(
                u32::from_le_bytes([bytes[byte], bytes[byte + 1], bytes[byte + 2], 0]),
                Self::WIDE,
            ) << LOW_ZEROED_BITS,
        )
    }
}

/// The operands of one instruction, packed into the four bytes that the handler reference does not
/// use.
///
/// Every operand lands on a byte boundary, in every format. The one that does not fit at its
/// natural size is the B-type immediate, which narrows from three bytes to two - see
/// [`Field::NARROW`] - and stays byte-aligned doing it, so branches extract their operands exactly
/// as cheaply as everything else.
trait Operands: Copy + PartialEq + fmt::Debug {
    /// Total width with every operand at its natural size
    const WIDE_TOTAL: u32;
    /// Whether that fits into the four bytes available
    const WIDE_FITS: bool = Self::WIDE_TOTAL <= u32::BITS;
    /// Total width actually used
    const TOTAL: u32;

    fn pack(self) -> [u8; 4];

    fn unpack(bytes: &[u8; 4]) -> Self;
}

/// Width to store `$field` at, given whether this variant's operands fit at their natural size
macro_rules! width {
    ($self:ty, $field:ty) => {
        if <$self>::WIDE_FITS {
            <$field as Field>::WIDE
        } else {
            <$field as Field>::NARROW
        }
    };
}

/// Pack `$fields` at consecutive byte offsets, checking that every one of them survives it
macro_rules! pack_fields {
    ($self:ty, $this:ident, $($index:tt: $field:ty),* $(,)?) => {{
        const { assert!(<$self>::TOTAL <= u32::BITS, "Operands do not fit into four bytes") };

        let mut bits = 0;
        let mut offset = 0;
        $(
            bits |= mask($this.$index.to_bits(), width!($self, $field)) << offset;
            offset += width!($self, $field);
        )*
        // The last operand's advance is dead, which is what makes the offsets constants
        let _ = offset;

        let bytes = bits.to_le_bytes();

        debug_assert_eq!(
            <$self>::unpack(&bytes),
            $this,
            "Packed operands do not round-trip"
        );

        bytes
    }};
}

/// Unpack `$fields` from the consecutive byte offsets `pack_fields!` put them at
macro_rules! unpack_fields {
    ($self:ty, $bytes:ident, $($field:ty),* $(,)?) => {{
        let mut byte = 0;
        let operands = (
            $(
                {
                    let field = <$field as Field>::from_bytes(
                        $bytes,
                        byte,
                        width!($self, $field),
                    );
                    byte += (width!($self, $field) / u8::BITS) as usize;
                    field
                },
            )*
        );
        // The last operand's advance is dead, which is what makes the offsets constants
        let _ = byte;

        operands
    }};
}

impl Operands for () {
    const WIDE_TOTAL: u32 = 0;
    const TOTAL: u32 = 0;

    #[inline(always)]
    fn pack(self) -> [u8; 4] {
        [0; 4]
    }

    #[inline(always)]
    fn unpack(_bytes: &[u8; 4]) -> Self {}
}

impl<A> Operands for (A,)
where
    A: Field,
{
    const WIDE_TOTAL: u32 = A::WIDE;
    const TOTAL: u32 = if Self::WIDE_FITS {
        Self::WIDE_TOTAL
    } else {
        A::NARROW
    };

    #[inline(always)]
    fn pack(self) -> [u8; 4] {
        pack_fields!(Self, self, 0: A)
    }

    #[inline(always)]
    fn unpack(bytes: &[u8; 4]) -> Self {
        unpack_fields!(Self, bytes, A)
    }
}

impl<A, B> Operands for (A, B)
where
    A: Field,
    B: Field,
{
    const WIDE_TOTAL: u32 = A::WIDE + B::WIDE;
    const TOTAL: u32 = if Self::WIDE_FITS {
        Self::WIDE_TOTAL
    } else {
        A::NARROW + B::NARROW
    };

    #[inline(always)]
    fn pack(self) -> [u8; 4] {
        pack_fields!(Self, self, 0: A, 1: B)
    }

    #[inline(always)]
    fn unpack(bytes: &[u8; 4]) -> Self {
        unpack_fields!(Self, bytes, A, B)
    }
}

impl<A, B, C> Operands for (A, B, C)
where
    A: Field,
    B: Field,
    C: Field,
{
    const WIDE_TOTAL: u32 = A::WIDE + B::WIDE + C::WIDE;
    const TOTAL: u32 = if Self::WIDE_FITS {
        Self::WIDE_TOTAL
    } else {
        A::NARROW + B::NARROW + C::NARROW
    };

    #[inline(always)]
    fn pack(self) -> [u8; 4] {
        pack_fields!(Self, self, 0: A, 1: B, 2: C)
    }

    #[inline(always)]
    fn unpack(bytes: &[u8; 4]) -> Self {
        unpack_fields!(Self, bytes, A, B, C)
    }
}

/// A decoded instruction as a handler and its operands, in the eight bytes the decoded enum costs.
///
/// The handler is a 32-bit offset from an anchor rather than a pointer, which is what makes room
/// for the operands; see the notes in `DISPATCH-HANDOFF.md` §8 question 3 for why that costs
/// nothing despite needing three more instructions at the jump.
#[repr(C)]
pub(crate) struct PackedSlot<Regs, Memory> {
    handler: i32,
    operands: [u8; 4],
    _phantom: PhantomData<PackedHandler<Regs, Memory>>,
}

impl<Regs, Memory> Copy for PackedSlot<Regs, Memory> {}

impl<Regs, Memory> Clone for PackedSlot<Regs, Memory> {
    fn clone(&self) -> Self {
        *self
    }
}

/// Position in the packed stream
pub(crate) struct PackedIp<'a, Regs, Memory>(&'a PackedSlot<Regs, Memory>);

impl<Regs, Memory> Copy for PackedIp<'_, Regs, Memory> {}

impl<Regs, Memory> Clone for PackedIp<'_, Regs, Memory> {
    fn clone(&self) -> Self {
        *self
    }
}

/// Result of a packed handler
type PackedNext<'a, Regs, Memory> = Option<PackedIp<'a, Regs, Memory>>;

/// A single packed handler
type PackedHandler<Regs, Memory> = for<'a> fn(
    PackedIp<'a, Regs, Memory>,
    &mut Regs,
    &mut Memory,
    &mut Ext,
    &PackedStream<'a, Regs, Memory>,
    &mut Sys,
) -> PackedNext<'a, Regs, Memory>;

/// The packed program
pub(crate) struct PackedStream<'a, Regs, Memory> {
    instructions: &'a [PackedSlot<Regs, Memory>],
    base_addr: u64,
    return_trap: u64,
}

impl<'a, Regs, Memory> PackedIp<'a, Regs, Memory> {
    /// Position of the instruction at guest address `address`
    #[inline(always)]
    fn at_address(stream: &PackedStream<'a, Regs, Memory>, address: u64) -> Option<Self> {
        let offset = address.checked_sub(stream.base_addr)?;

        if !offset.is_multiple_of(size_of::<u16>() as u64) {
            cold_path();
            return None;
        }

        Self::at_slot(stream, (offset / size_of::<u16>() as u64) as usize)
    }

    /// Position of slot `slot` of the stream
    #[inline(always)]
    fn at_slot(stream: &PackedStream<'a, Regs, Memory>, slot: usize) -> Option<Self> {
        Some(Self(stream.instructions.get(slot)?))
    }

    /// Slot this position refers to
    #[inline(always)]
    fn slot(self, stream: &PackedStream<'a, Regs, Memory>) -> usize {
        (ptr::from_ref(self.0).addr() - stream.instructions.as_ptr().addr())
            / size_of::<PackedSlot<Regs, Memory>>()
    }

    /// The packed operands at this position
    #[inline(always)]
    fn operands(self) -> &'a [u8; 4] {
        &self.0.operands
    }

    /// The handler for the instruction at this position
    #[inline(always)]
    fn handler(self) -> PackedHandler<Regs, Memory>
    where
        Regs: RegisterFile<R>,
        Memory: VirtualMemory,
    {
        let anchor =
            packed_handlers::unsupported::<Regs, Memory> as PackedHandler<Regs, Memory> as usize;
        let address = anchor.wrapping_add_signed(self.0.handler as isize);

        // SAFETY: built by `build_packed_stream()` as a real handler's offset from that anchor
        unsafe { core::mem::transmute::<usize, PackedHandler<Regs, Memory>>(address) }
    }

    /// Advance by `slots` slots.
    ///
    /// # Safety
    /// The stream must continue for at least `slots` more slots, which the decoded stream ending
    /// with a jump guarantees for everything that can fall through.
    #[inline(always)]
    unsafe fn advance(self, slots: usize) -> Self {
        // SAFETY: guaranteed by function contract
        Self(unsafe { &*ptr::from_ref(self.0).add(slots) })
    }
}

/// Emits the packed handlers from the same instruction table the others are built from, plus the
/// decode-time packer that fills the stream they walk.
macro_rules! emit_packed_handlers {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident: $ty:ty),* $(,)? }, $body:block);* $(;)?
    ) => {
        mod packed_handlers {
            use super::PackedIp as Ip;
            use super::PackedStream as Stream;
            use super::*;

            /// As [`super::Env`], over the packed stream
            struct Env<'a, 'r, Regs, Memory> {
                regs: &'r mut Regs,
                memory: &'r mut Memory,
                ext: &'r mut Ext,
                stream: &'r Stream<'a, Regs, Memory>,
                sys: &'r mut Sys,
            }

            $(
                #[expect(non_snake_case, reason = "One handler per instruction variant")]
                #[allow(
                    unused_variables,
                    unreachable_code,
                    reason = "Unconditional jumps do not use the advanced pointer, and trapping \
                        instructions never reach it"
                )]
                pub(super) fn $name<'a, Regs, Memory>(
                    $ip: Ip<'a, Regs, Memory>,
                    regs: &mut Regs,
                    memory: &mut Memory,
                    ext: &mut Ext,
                    stream: &Stream<'a, Regs, Memory>,
                    sys: &mut Sys,
                ) -> PackedNext<'a, Regs, Memory>
                where
                    Regs: RegisterFile<R>,
                    Memory: VirtualMemory,
                {
                    // The operands come back out of four bytes rather than out of an eight-byte
                    // enum, and byte-aligned, so this is the same extraction it was
                    let ($($field,)*): ($($ty,)*) = Operands::unpack($ip.operands());
                    let $ctx = Env { regs, memory, ext, stream, sys };
                    // SAFETY: the decoded stream ends with a jump, so anything that can fall
                    // through has at least this many slots left
                    let $next = unsafe { $ip.advance($slots) };
                    let $next = $body;
                    let handler = packed_handler::<Regs, Memory>($next);
                    become handler(
                        $next,
                        $ctx.regs,
                        $ctx.memory,
                        $ctx.ext,
                        $ctx.stream,
                        $ctx.sys,
                    )
                }
            )*

            pub(super) fn unsupported<'a, Regs, Memory>(
                _ip: Ip<'a, Regs, Memory>,
                _regs: &mut Regs,
                _memory: &mut Memory,
                ext: &mut Ext,
                _stream: &Stream<'a, Regs, Memory>,
                _sys: &mut Sys,
            ) -> PackedNext<'a, Regs, Memory>
            where
                Regs: RegisterFile<R>,
                Memory: VirtualMemory,
            {
                cold_path();
                ext.stop = Stop::Unsupported(0);
                None
            }

            /// Pack one decoded instruction's operands, once, when the stream is built
            #[expect(
                clippy::rest_pattern_accessible_field,
                reason = "Each variant packs the operands its handler reads and no others"
            )]
            pub(super) fn pack(instruction: I) -> [u8; 4] {
                match instruction {
                    $( I::$name { $($field,)* .. } => Operands::pack(($($field,)*)), )*
                }
            }

            /// Dispatch table, used once per instruction when the stream is built rather than once
            /// per instruction executed
            pub(super) const fn build<Regs, Memory>() -> [PackedHandler<Regs, Memory>; VARIANTS]
            where
                Regs: RegisterFile<R>,
                Memory: VirtualMemory,
            {
                let mut table: [PackedHandler<Regs, Memory>; VARIANTS] = [unsupported; VARIANTS];
                let mut index = 0;
                let listed: &[PackedHandler<Regs, Memory>] = &[$($name,)*];

                while index < listed.len() {
                    table[index] = listed[index];
                    index += 1;
                }

                table
            }
        }
    };
}

ops!(emit_packed_handlers);

/// Handler for the instruction at `ip`.
///
/// A free function rather than a method so that it also accepts the `!` that a trapping
/// instruction's body evaluates to.
#[inline(always)]
fn packed_handler<Regs, Memory>(ip: PackedIp<'_, Regs, Memory>) -> PackedHandler<Regs, Memory>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    ip.handler()
}

struct PackedHandlers<Regs, Memory>(PhantomData<(Regs, Memory)>);

impl<Regs, Memory> PackedHandlers<Regs, Memory>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    const TABLE: [PackedHandler<Regs, Memory>; VARIANTS] = packed_handlers::build::<Regs, Memory>();
}

/// Resolve every decoded instruction to its handler and pack its operands, once, before execution
fn build_packed_stream<Regs, Memory>(instructions: &[I]) -> Vec<PackedSlot<Regs, Memory>>
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    let anchor =
        packed_handlers::unsupported::<Regs, Memory> as PackedHandler<Regs, Memory> as usize;

    instructions
        .iter()
        .map(|instruction| {
            // SAFETY: the enum is `#[repr(u16)]`, so its first two bytes are the discriminant
            let discriminant = unsafe { ptr::from_ref(instruction).cast::<u16>().read() };
            // Masked rather than truncated for the same reason as `Ip::discriminant`
            let handler = PackedHandlers::<Regs, Memory>::TABLE[usize::from(discriminant as u8)];

            PackedSlot {
                handler: (handler as usize).wrapping_sub(anchor) as i32,
                operands: packed_handlers::pack(*instruction),
                _phantom: PhantomData,
            }
        })
        .collect()
}

/// Run the program with the packed direct-threaded back end
pub(crate) fn run_packed_threaded<Regs, Memory>(
    instructions: &[I],
    base_addr: u64,
    return_trap: u64,
    pc: u64,
    regs: &mut Regs,
    memory: &mut Memory,
) -> Stop
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    const {
        // The whole point: the same eight bytes per slot that the decoded enum costs
        assert!(size_of::<PackedSlot<ZeroStoreRegisters, ()>>() == 8);
    }

    let slots = build_packed_stream::<Regs, Memory>(instructions);
    let stream = PackedStream {
        instructions: &slots,
        base_addr,
        return_trap,
    };
    let mut ext = Ext {
        start: Instant::now(),
        stop: Stop::Done,
    };
    let mut sys = Sys;

    let Some(ip) = PackedIp::at_address(&stream, pc) else {
        return Stop::BadJump;
    };
    let handler = ip.handler();
    handler(ip, regs, memory, &mut ext, &stream, &mut sys);

    ext.stop
}

/// Run the program with the tail-call-threaded back end
pub(crate) fn run_threaded<Regs, Memory>(
    instructions: &[I],
    base_addr: u64,
    return_trap: u64,
    pc: u64,
    regs: &mut Regs,
    memory: &mut Memory,
) -> Stop
where
    Regs: RegisterFile<R>,
    Memory: VirtualMemory,
{
    let stream = Stream {
        instructions,
        base_addr,
        return_trap,
    };
    let mut ext = Ext {
        start: Instant::now(),
        stop: Stop::Done,
    };
    let mut sys = Sys;

    let Some(ip) = Ip::at_address(&stream, pc) else {
        return Stop::BadJump;
    };
    let handler = table::<Regs, Memory>(ip);
    handler(ip, regs, memory, &mut ext, &stream, &mut sys);

    ext.stop
}
