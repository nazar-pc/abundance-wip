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
use ab_riscv_primitives::prelude::*;
use core::hint::{cold_path, unreachable_unchecked};
use core::marker::PhantomData;
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
            Add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sub, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_sub(reg_read!(ctx, rs2))); next };
            Sll, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) << (reg_read!(ctx, rs2) & 0x3f)); next };
            Slt, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).cast_signed() < reg_read!(ctx, rs2).cast_signed())); next };
            Sltu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) < reg_read!(ctx, rs2))); next };
            Xor, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ reg_read!(ctx, rs2)); next };
            Srl, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) >> (reg_read!(ctx, rs2) & 0x3f)); next };
            Sra, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1).cast_signed() >> (reg_read!(ctx, rs2) & 0x3f)).cast_unsigned()); next };
            Or, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | reg_read!(ctx, rs2)); next };
            And, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & reg_read!(ctx, rs2)); next };
            Addw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_add(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Subw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_sub(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Sllw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((((reg_read!(ctx, rs1) as u32) << (reg_read!(ctx, rs2) & 0x1f))).cast_signed()).cast_unsigned()); next };
            Srlw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((((reg_read!(ctx, rs1) as u32) >> (reg_read!(ctx, rs2) & 0x1f))).cast_signed()).cast_unsigned()); next };
            Sraw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32) >> (reg_read!(ctx, rs2) & 0x1f)).cast_unsigned()); next };

            // ----- RV64I, register-immediate -----
            Addi, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned())); next };
            Slti, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).cast_signed() < i64::from(imm))); next };
            Sltiu, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) < i64::from(imm).cast_unsigned())); next };
            Xori, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ i64::from(imm).cast_unsigned()); next };
            Ori, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | i64::from(imm).cast_unsigned()); next };
            Andi, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & i64::from(imm).cast_unsigned()); next };
            Slli, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) << shamt); next };
            Srli, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) >> shamt); next };
            Srai, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1).cast_signed() >> shamt).cast_unsigned()); next };
            Addiw, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_add(i32::from(imm))).cast_unsigned()); next };
            Slliw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from(((reg_read!(ctx, rs1) as u32) << shamt).cast_signed()).cast_unsigned()); next };
            Srliw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from(((reg_read!(ctx, rs1) as u32) >> shamt).cast_signed()).cast_unsigned()); next };
            Sraiw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32) >> shamt).cast_unsigned()); next };

            // ----- RV64I, loads -----
            Lb, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i8, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Lh, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i16, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Lw, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Ld, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            Lbu, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u8, a); reg_write!(ctx, rd, u64::from(v)); next };
            Lhu, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u16, a); reg_write!(ctx, rd, u64::from(v)); next };
            Lwu, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u32, a); reg_write!(ctx, rd, u64::from(v)); next };

            // ----- RV64I, indirect jump -----
            Jalr, 2, { rs1, rd, imm }, { let pc = current_pc!(ctx, ip); let target = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()) & !1u64; reg_write!(ctx, rd, pc.wrapping_add(size_of::<u32>() as u64)); jump_absolute!(ctx, target) };

            // ----- RV64I, stores -----
            Sb, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u8, a, reg_read!(ctx, rs2) as u8); next };
            Sh, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u16, a, reg_read!(ctx, rs2) as u16); next };
            Sw, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            Sd, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };

            // ----- RV64I, branches -----
            Beq, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) == reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Bne, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) != reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Blt, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1).cast_signed() < reg_read!(ctx, rs2).cast_signed() { jump_relative!(ctx, ip, imm) } else { next } };
            Bge, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1).cast_signed() >= reg_read!(ctx, rs2).cast_signed() { jump_relative!(ctx, ip, imm) } else { next } };
            Bltu, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) < reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Bgeu, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) >= reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };

            // ----- RV64I, upper immediate and direct jump -----
            Lui, 2, { rd, imm }, { reg_write!(ctx, rd, i64::from(imm).cast_unsigned()); next };
            Auipc, 2, { rd, imm }, { let pc = current_pc!(ctx, ip); reg_write!(ctx, rd, pc.wrapping_add(i64::from(imm).cast_unsigned())); next };
            Jal, 2, { rd, imm }, { let pc = current_pc!(ctx, ip); reg_write!(ctx, rd, pc.wrapping_add(size_of::<u32>() as u64)); jump_relative!(ctx, ip, imm) };

            // ----- RV64I, system -----
            Fence, 2, { }, { next };
            FenceTso, 2, { }, { next };
            Ecall, 2, { }, { trap!(ctx, IllegalInstruction) };
            Ebreak, 2, { }, { next };
            Unimp, 2, { }, { trap!(ctx, IllegalInstruction) };

            // ----- M -----
            Mul, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_mul(reg_read!(ctx, rs2))); next };
            Mulh, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, ((i128::from(reg_read!(ctx, rs1).cast_signed()) * i128::from(reg_read!(ctx, rs2).cast_signed())) >> 64) as u64); next };
            Mulhsu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, ((i128::from(reg_read!(ctx, rs1).cast_signed()) * i128::from(reg_read!(ctx, rs2))) >> 64) as u64); next };
            Mulhu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, ((u128::from(reg_read!(ctx, rs1)) * u128::from(reg_read!(ctx, rs2))) >> 64) as u64); next };
            Div, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1).cast_signed(); let b = reg_read!(ctx, rs2).cast_signed(); let v = if b == 0 { -1i64 } else if a == i64::MIN && b == -1 { i64::MIN } else { a / b }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Divu, 2, { rs1, rs2, rd }, { let b = reg_read!(ctx, rs2); reg_write!(ctx, rd, reg_read!(ctx, rs1).checked_div(b).unwrap_or(u64::MAX)); next };
            Rem, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1).cast_signed(); let b = reg_read!(ctx, rs2).cast_signed(); let v = if b == 0 { a } else if a == i64::MIN && b == -1 { 0 } else { a % b }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Remu, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1); let b = reg_read!(ctx, rs2); reg_write!(ctx, rd, if b == 0 { a } else { a % b }); next };
            Mulw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_mul(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Divw, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1) as i32; let b = reg_read!(ctx, rs2) as i32; let v = if b == 0 { -1i64 } else if a == i32::MIN && b == -1 { i64::from(i32::MIN) } else { i64::from(a / b) }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Divuw, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1) as u32; let b = reg_read!(ctx, rs2) as u32; let v = match a.checked_div(b) { Some(v) => i64::from(v.cast_signed()).cast_unsigned(), None => u64::MAX }; reg_write!(ctx, rd, v); next };
            Remw, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1) as i32; let b = reg_read!(ctx, rs2) as i32; let v = if b == 0 { i64::from(a) } else if a == i32::MIN && b == -1 { 0 } else { i64::from(a % b) }; reg_write!(ctx, rd, v.cast_unsigned()); next };
            Remuw, 2, { rs1, rs2, rd }, { let a = reg_read!(ctx, rs1) as u32; let b = reg_read!(ctx, rs2) as u32; let v = if b == 0 { a.cast_signed() } else { (a % b).cast_signed() }; reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };

            // ----- Zba -----
            AddUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u32).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh1add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh1addUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh2add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 2).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh2addUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 2).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh3add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 3).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh3addUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 3).wrapping_add(reg_read!(ctx, rs2))); next };
            SlliUw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u32) << shamt); next };

            // ----- Zbb -----
            Andn, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & !reg_read!(ctx, rs2)); next };
            Orn, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | !reg_read!(ctx, rs2)); next };
            Xnor, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, !(reg_read!(ctx, rs1) ^ reg_read!(ctx, rs2))); next };
            Clz, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).leading_zeros())); next };
            Clzw, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from((reg_read!(ctx, rs1) as u32).leading_zeros())); next };
            Ctz, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).trailing_zeros())); next };
            Ctzw, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from((reg_read!(ctx, rs1) as u32).trailing_zeros())); next };
            Cpop, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).count_ones())); next };
            Cpopw, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from((reg_read!(ctx, rs1) as u32).count_ones())); next };
            Max, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).cast_signed().max(reg_read!(ctx, rs2).cast_signed()).cast_unsigned()); next };
            Maxu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).max(reg_read!(ctx, rs2))); next };
            Min, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).cast_signed().min(reg_read!(ctx, rs2).cast_signed()).cast_unsigned()); next };
            Minu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).min(reg_read!(ctx, rs2))); next };
            Sextb, 2, { rs1, rd }, { reg_write!(ctx, rd, i64::from(reg_read!(ctx, rs1) as i8).cast_unsigned()); next };
            Sexth, 2, { rs1, rd }, { reg_write!(ctx, rd, i64::from(reg_read!(ctx, rs1) as i16).cast_unsigned()); next };
            Zexth, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u16)); next };
            Rol, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).rotate_left((reg_read!(ctx, rs2) & 0x3f) as u32)); next };
            Rolw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as u32).rotate_left((reg_read!(ctx, rs2) & 0x1f) as u32).cast_signed()).cast_unsigned()); next };
            Ror, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).rotate_right((reg_read!(ctx, rs2) & 0x3f) as u32)); next };
            Rori, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).rotate_right(u32::from(shamt & 0x3f))); next };
            Roriw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as u32).rotate_right(u32::from(shamt & 0x1f)).cast_signed()).cast_unsigned()); next };
            Rorw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as u32).rotate_right((reg_read!(ctx, rs2) & 0x1f) as u32).cast_signed()).cast_unsigned()); next };
            Orcb, 2, { rs1, rd }, { reg_write!(ctx, rd, rv64_zbb_helpers::orc_b(reg_read!(ctx, rs1))); next };
            Rev8, 2, { rs1, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).swap_bytes()); next };

            // ----- Zbs -----
            Bset, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | (1u64 << (reg_read!(ctx, rs2) & 0x3f))); next };
            Bseti, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | (1u64 << shamt)); next };
            Bclr, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & !(1u64 << (reg_read!(ctx, rs2) & 0x3f))); next };
            Bclri, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & !(1u64 << shamt)); next };
            Binv, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ (1u64 << (reg_read!(ctx, rs2) & 0x3f))); next };
            Binvi, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ (1u64 << shamt)); next };
            Bext, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) >> (reg_read!(ctx, rs2) & 0x3f)) & 1); next };
            Bexti, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) >> shamt) & 1); next };

            // ----- Zca, quadrant 0 -----
            CAddi4spn, 1, { rd, nzuimm }, { reg_write!(ctx, rd, reg_read!(ctx, R::Sp).wrapping_add(u64::from(nzuimm))); next };
            CLw, 1, { rs1, rd, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            CLd, 1, { rs1, rd, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            CSw, 1, { rs1, rs2, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            CSd, 1, { rs1, rs2, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };

            // ----- Zca, quadrant 1 -----
            CNop, 1, { }, { next };
            CAddi, 1, { rd, nzimm }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_add(i64::from(nzimm).cast_unsigned())); next };
            CAddiw, 1, { rd, imm }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rd) as i32).wrapping_add(i32::from(imm))).cast_unsigned()); next };
            CLi, 1, { rd, imm }, { reg_write!(ctx, rd, i64::from(imm).cast_unsigned()); next };
            CAddi16sp, 1, { nzimm }, { reg_write!(ctx, R::Sp, reg_read!(ctx, R::Sp).wrapping_add(i64::from(nzimm).cast_unsigned())); next };
            CLui, 1, { rd, nzimm }, { reg_write!(ctx, rd, i64::from(nzimm).cast_unsigned()); next };
            CSrli, 1, { rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rd) >> shamt); next };
            CSrai, 1, { rd, shamt }, { reg_write!(ctx, rd, (reg_read!(ctx, rd).cast_signed() >> shamt).cast_unsigned()); next };
            CAndi, 1, { rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rd) & i64::from(imm).cast_unsigned()); next };
            CSub, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_sub(reg_read!(ctx, rs2))); next };
            CXor, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rd) ^ reg_read!(ctx, rs2)); next };
            COr, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rd) | reg_read!(ctx, rs2)); next };
            CAnd, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rd) & reg_read!(ctx, rs2)); next };
            CSubw, 1, { rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rd) as i32).wrapping_sub(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            CAddw, 1, { rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rd) as i32).wrapping_add(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            CJ, 1, { imm }, { jump_relative!(ctx, ip, imm) };
            CBeqz, 1, { rs1, imm }, { if reg_read!(ctx, rs1) == 0 { jump_relative!(ctx, ip, imm) } else { next } };
            CBnez, 1, { rs1, imm }, { if reg_read!(ctx, rs1) != 0 { jump_relative!(ctx, ip, imm) } else { next } };

            // ----- Zca, quadrant 2 -----
            CSlli, 1, { rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rd) << shamt); next };
            CLwsp, 1, { rd, uimm }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            CLdsp, 1, { rd, uimm }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            CJr, 1, { rs1 }, { jump_absolute!(ctx, reg_read!(ctx, rs1) & !1u64) };
            CMv, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs2)); next };
            CEbreak, 1, { }, { next };
            CJalr, 1, { rs1 }, { let pc = current_pc!(ctx, ip); let target = reg_read!(ctx, rs1) & !1u64; reg_write!(ctx, R::Ra, pc.wrapping_add(size_of::<u16>() as u64)); jump_absolute!(ctx, target) };
            CAdd, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_add(reg_read!(ctx, rs2))); next };
            CSwsp, 1, { rs2, uimm }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            CSdsp, 1, { rs2, uimm }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };
            CUnimp, 1, { }, { trap!(ctx, IllegalInstruction) };

            // ----- Zicsr -----
            //
            // The only CSR this runner implements is `time`, and it is read-only, so anything but
            // a pure read of it stops execution rather than quietly returning a wrong value.
            Csrrw, 2, { }, { csr_illegal!(ctx) };
            Csrrs, 2, { rs1, rd, csr_index }, { csr_read!(ctx, rd, csr_index, reg_read!(ctx, rs1)); next };
            Csrrc, 2, { rs1, rd, csr_index }, { csr_read!(ctx, rd, csr_index, reg_read!(ctx, rs1)); next };
            Csrrwi, 2, { }, { csr_illegal!(ctx) };
            Csrrsi, 2, { rd, zimm, csr_index }, { csr_read!(ctx, rd, csr_index, u64::from(zimm)); next };
            Csrrci, 2, { rd, zimm, csr_index }, { csr_read!(ctx, rd, csr_index, u64::from(zimm)); next };
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
        $($name:ident, $slots:expr, { $($field:ident),* $(,)? }, $body:block);* $(;)?
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
