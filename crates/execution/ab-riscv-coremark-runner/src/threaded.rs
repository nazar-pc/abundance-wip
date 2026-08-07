//! Dispatch-style experiments over the pre-decoded instruction stream.
//!
//! Every variant in here executes the *same* instruction semantics, expressed once in the
//! [`ops!`] table below. Only the way control gets from one instruction to the next differs:
//!
//! * [`run_match`] — one big `match` over the discriminant, each arm advancing the instruction
//!   pointer itself (this is the "specialized match loop")
//! * [`run_call`] — one function per instruction, reached through a function pointer table, called
//!   from a driver loop
//! * [`run_tail`] — the same functions, chained with guaranteed tail calls (`become`)
//! * [`run_plain_tail`] — the same again, but with an ordinary call in tail position, to check
//!   whether the unstable `explicit_tail_calls` feature is actually needed to get sibling calls
//!
//! All three do far less work per instruction than the generic `BasicInterpreterState::execute()`
//! loop, because each instruction knows statically how wide it is, which operands it actually
//! needs, and whether it can branch at all. The generic loop cannot know any of that, so it pays
//! for the union of what every instruction might need before it dispatches.
//!
//! Select one with `COREMARK_DISPATCH=match|call|tail|plaintail`; leaving it unset runs the
//! generic loop.

use crate::instruction::CoremarkInstruction as I;
use ab_riscv_interpreter::basic::BasicRegister;
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use core::hint::{cold_path, unreachable_unchecked};
use std::time::Instant;

/// Register type used by the Coremark runner
type R = Reg<u64>;

/// Number of variants in `CoremarkInstruction`, i.e. the size of the dispatch table
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
    /// Instruction not implemented by this prototype
    Unsupported(u16),
}

/// Result of a handler: the instruction pointer to continue at, or null when execution stopped (in
/// which case the reason is in [`Ctx::stop`]).
type Next = *const I;

/// A single instruction handler
type Handler = fn(*const I, &mut Ctx) -> Next;

/// Interpreter state, laid out as one contiguous block so that a single base pointer reaches
/// memory, registers and everything else
#[repr(C, align(64))]
pub(crate) struct Ctx {
    /// Guest RAM. Guest base address is zero, so a guest address is directly an index here.
    pub(crate) mem: [u8; crate::MEMORY_SIZE],
    /// GPRs. Slot 32 is a sink that writes to `x0` are redirected into, which keeps slot 0 zero
    /// forever and lets reads be unconditional.
    regs: [u64; 33],
    /// Start of the decoded instruction stream
    base: *const I,
    /// Length of the decoded instruction stream, in slots
    slots: usize,
    /// Guest address the decoded instruction stream starts at
    base_addr: u64,
    /// Guest address that stops execution when jumped to
    return_trap: u64,
    /// Discriminant-indexed handler table, used by [`run_call`] and [`run_tail`]
    handlers: [Handler; VARIANTS],
    /// For the `time` CSR
    start: Instant,
    /// Why execution stopped
    pub(crate) stop: Stop,
}

/// Read the `#[repr(u16)]` discriminant of an instruction
///
/// # Safety
/// `instruction` must point at a valid instruction inside the decoded stream.
#[inline(always)]
unsafe fn discriminant(instruction: *const I) -> u16 {
    // SAFETY: the enum is `#[repr(u16)]`, so its first two bytes are the discriminant
    unsafe { instruction.cast::<u16>().read() }
}

macro_rules! reg_read {
    ($ctx:ident, $reg:expr) => {
        // SAFETY: register offsets are always below 32
        *unsafe { $ctx.regs.get_unchecked(usize::from(BasicRegister::offset($reg))) }
    };
}

macro_rules! reg_write {
    ($ctx:ident, $reg:expr, $value:expr) => {{
        let offset = usize::from(BasicRegister::offset($reg));
        // Branchless discard of writes to `x0`: they land in the sink slot instead
        let offset = if offset == 0 { 32 } else { offset };
        // SAFETY: offset is always below 33
        *unsafe { $ctx.regs.get_unchecked_mut(offset) } = $value;
    }};
}

macro_rules! mem_read {
    ($ctx:ident, $ty:ty, $addr:expr) => {{
        let addr = $addr;
        if addr.saturating_add(size_of::<$ty>() as u64) > crate::MEMORY_SIZE as u64 {
            cold_path();
            $ctx.stop = Stop::OutOfBounds;
            return core::ptr::null();
        }
        // SAFETY: bounds were just checked, and only plain integers are read
        unsafe {
            $ctx.mem
                .as_ptr()
                .cast::<$ty>()
                .byte_add(addr as usize)
                .read_unaligned()
        }
    }};
}

macro_rules! mem_write {
    ($ctx:ident, $ty:ty, $addr:expr, $value:expr) => {{
        let addr = $addr;
        let value = $value;
        if addr.saturating_add(size_of::<$ty>() as u64) > crate::MEMORY_SIZE as u64 {
            cold_path();
            $ctx.stop = Stop::OutOfBounds;
            return core::ptr::null();
        }
        // SAFETY: bounds were just checked, and only plain integers are written
        unsafe {
            $ctx.mem
                .as_mut_ptr()
                .cast::<$ty>()
                .byte_add(addr as usize)
                .write_unaligned(value);
        }
    }};
}

/// Guest address of the instruction `$ip` points at.
///
/// Every two bytes of guest code get one slot in the decoded stream, hence the shift by two rather
/// than by three.
macro_rules! current_pc {
    ($ctx:ident, $ip:ident) => {
        $ctx.base_addr
            .wrapping_add((($ip as usize - $ctx.base as usize) >> 2) as u64)
    };
}

/// Instruction pointer for a PC-relative jump by `$imm` guest bytes.
///
/// PC-relative targets stay inside the decoded stream, so this is pointer arithmetic plus a bounds
/// check rather than the full address-to-slot conversion [`jump_absolute`] has to do.
macro_rules! jump_relative {
    ($ctx:ident, $ip:ident, $imm:expr) => {{
        // Two guest bytes per 8-byte slot, so one guest byte is four bytes of stream
        let byte_offset = (i64::from($imm) as isize).wrapping_mul(4);
        // SAFETY: the result is bounds-checked below before it is ever dereferenced
        let target = unsafe { $ip.byte_offset(byte_offset) };

        if (target as usize) < ($ctx.base as usize)
            || (target as usize) >= ($ctx.base as usize + $ctx.slots * size_of::<I>())
        {
            cold_path();
            $ctx.stop = Stop::BadJump;
            return core::ptr::null();
        }

        target
    }};
}

/// Instruction pointer for a jump to absolute guest address `$target`
macro_rules! jump_absolute {
    ($ctx:ident, $target:expr) => {{
        let target = $target;

        if target == $ctx.return_trap {
            cold_path();
            $ctx.stop = Stop::Done;
            return core::ptr::null();
        }

        let Some(offset) = target.checked_sub($ctx.base_addr) else {
            cold_path();
            $ctx.stop = Stop::BadJump;
            return core::ptr::null();
        };

        if !offset.is_multiple_of(size_of::<u16>() as u64) {
            cold_path();
            $ctx.stop = Stop::BadJump;
            return core::ptr::null();
        }

        let slot = (offset / size_of::<u16>() as u64) as usize;

        if slot >= $ctx.slots {
            cold_path();
            $ctx.stop = Stop::BadJump;
            return core::ptr::null();
        }

        // SAFETY: slot was just bounds-checked
        unsafe { $ctx.base.add(slot) }
    }};
}

/// The instruction table.
///
/// Each entry is `Variant, slots, { bound fields }, body`, where `slots` is how many slots of the
/// decoded stream the instruction occupies (one per two guest bytes) and `body` is a block that
/// evaluates to the instruction pointer to continue at. Bodies may use `ctx`, `ip` and `next`, the
/// last being `ip` already advanced past this instruction.
macro_rules! ops {
    ($emit:ident) => {
        $emit! {
            ctx, ip, next,
            // ----- RV64I register-register -----
            Add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sub, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_sub(reg_read!(ctx, rs2))); next };
            Slt, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1).cast_signed() < reg_read!(ctx, rs2).cast_signed())); next };
            Xor, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ reg_read!(ctx, rs2)); next };
            Or, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | reg_read!(ctx, rs2)); next };
            Addw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_add(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Subw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_sub(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };

            // ----- RV64I register-immediate -----
            Addi, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned())); next };
            Addiw, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_add(i32::from(imm))).cast_unsigned()); next };
            Sltiu, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) < i64::from(imm).cast_unsigned())); next };
            Xori, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) ^ i64::from(imm).cast_unsigned()); next };
            Ori, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) | i64::from(imm).cast_unsigned()); next };
            Andi, 2, { rs1, rd, imm }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) & i64::from(imm).cast_unsigned()); next };
            Slli, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) << shamt); next };
            Srli, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, reg_read!(ctx, rs1) >> shamt); next };
            Srai, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1).cast_signed() >> shamt).cast_unsigned()); next };
            Slliw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from(((reg_read!(ctx, rs1) as u32) << shamt).cast_signed()).cast_unsigned()); next };
            Srliw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from(((reg_read!(ctx, rs1) as u32) >> shamt).cast_signed()).cast_unsigned()); next };
            Sraiw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32) >> shamt).cast_unsigned()); next };
            Lui, 2, { rd, imm }, { reg_write!(ctx, rd, i64::from(imm).cast_unsigned()); next };
            Auipc, 2, { rd, imm }, { let pc = current_pc!(ctx, ip); reg_write!(ctx, rd, pc.wrapping_add(i64::from(imm).cast_unsigned())); next };

            // ----- RV64I loads and stores -----
            Lh, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i16, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Lw, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            Ld, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            Lbu, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u8, a); reg_write!(ctx, rd, u64::from(v)); next };
            Lhu, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u16, a); reg_write!(ctx, rd, u64::from(v)); next };
            Lwu, 2, { rs1, rd, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); let v = mem_read!(ctx, u32, a); reg_write!(ctx, rd, u64::from(v)); next };
            Sb, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u8, a, reg_read!(ctx, rs2) as u8); next };
            Sh, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u16, a, reg_read!(ctx, rs2) as u16); next };
            Sw, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            Sd, 2, { rs1, rs2, imm }, { let a = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };

            // ----- RV64I control flow -----
            Beq, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) == reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Bne, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) != reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Blt, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1).cast_signed() < reg_read!(ctx, rs2).cast_signed() { jump_relative!(ctx, ip, imm) } else { next } };
            Bge, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1).cast_signed() >= reg_read!(ctx, rs2).cast_signed() { jump_relative!(ctx, ip, imm) } else { next } };
            Bltu, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) < reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Bgeu, 2, { rs1, rs2, imm }, { if reg_read!(ctx, rs1) >= reg_read!(ctx, rs2) { jump_relative!(ctx, ip, imm) } else { next } };
            Jal, 2, { rd, imm }, { let pc = current_pc!(ctx, ip); reg_write!(ctx, rd, pc.wrapping_add(size_of::<u32>() as u64)); jump_relative!(ctx, ip, imm) };
            Jalr, 2, { rs1, rd, imm }, { let pc = current_pc!(ctx, ip); let t = reg_read!(ctx, rs1).wrapping_add(i64::from(imm).cast_unsigned()) & !1u64; reg_write!(ctx, rd, pc.wrapping_add(size_of::<u32>() as u64)); jump_absolute!(ctx, t) };

            // ----- M -----
            Mul, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).wrapping_mul(reg_read!(ctx, rs2))); next };
            Mulw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, i64::from((reg_read!(ctx, rs1) as i32).wrapping_mul(reg_read!(ctx, rs2) as i32)).cast_unsigned()); next };
            Mulhu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, ((u128::from(reg_read!(ctx, rs1)) * u128::from(reg_read!(ctx, rs2))) >> 64) as u64); next };
            Divu, 2, { rs1, rs2, rd }, { let b = reg_read!(ctx, rs2); reg_write!(ctx, rd, if b == 0 { u64::MAX } else { reg_read!(ctx, rs1) / b }); next };
            Divuw, 2, { rs1, rs2, rd }, { let b = reg_read!(ctx, rs2) as u32; let v = if b == 0 { u32::MAX } else { (reg_read!(ctx, rs1) as u32) / b }; reg_write!(ctx, rd, i64::from(v.cast_signed()).cast_unsigned()); next };

            // ----- B -----
            Sh1add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh2add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 2).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh3add, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) << 3).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh1addUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 1).wrapping_add(reg_read!(ctx, rs2))); next };
            Sh2addUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, (u64::from(reg_read!(ctx, rs1) as u32) << 2).wrapping_add(reg_read!(ctx, rs2))); next };
            AddUw, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u32).wrapping_add(reg_read!(ctx, rs2))); next };
            SlliUw, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u32) << shamt); next };
            Sexth, 2, { rs1, rd }, { reg_write!(ctx, rd, i64::from(reg_read!(ctx, rs1) as i16).cast_unsigned()); next };
            Zexth, 2, { rs1, rd }, { reg_write!(ctx, rd, u64::from(reg_read!(ctx, rs1) as u16)); next };
            Max, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).cast_signed().max(reg_read!(ctx, rs2).cast_signed()).cast_unsigned()); next };
            Min, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).cast_signed().min(reg_read!(ctx, rs2).cast_signed()).cast_unsigned()); next };
            Maxu, 2, { rs1, rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs1).max(reg_read!(ctx, rs2))); next };
            Bexti, 2, { rs1, rd, shamt }, { reg_write!(ctx, rd, (reg_read!(ctx, rs1) >> shamt) & 1); next };

            // ----- Zicsr (only `time` is ever read by Coremark) -----
            Csrrs, 2, { rd, csr_index }, { let v = if csr_index == 0xC01 { ctx.start.elapsed().as_nanos() as u64 } else { 0 }; reg_write!(ctx, rd, v); next };

            // ----- Zca, quadrant 0 -----
            CAddi4spn, 1, { rd, nzuimm }, { reg_write!(ctx, rd, reg_read!(ctx, R::Sp).wrapping_add(u64::from(nzuimm))); next };
            CLw, 1, { rs1, rd, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, i32, a); reg_write!(ctx, rd, i64::from(v).cast_unsigned()); next };
            CLd, 1, { rs1, rd, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); let v = mem_read!(ctx, u64, a); reg_write!(ctx, rd, v); next };
            CSw, 1, { rs1, rs2, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            CSd, 1, { rs1, rs2, uimm }, { let a = reg_read!(ctx, rs1).wrapping_add(u64::from(uimm)); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };

            // ----- Zca, quadrant 1 -----
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
            CJalr, 1, { rs1 }, { let pc = current_pc!(ctx, ip); let t = reg_read!(ctx, rs1) & !1u64; reg_write!(ctx, R::Ra, pc.wrapping_add(size_of::<u16>() as u64)); jump_absolute!(ctx, t) };
            CMv, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rs2)); next };
            CAdd, 1, { rs2, rd }, { reg_write!(ctx, rd, reg_read!(ctx, rd).wrapping_add(reg_read!(ctx, rs2))); next };
            CSwsp, 1, { rs2, uimm }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); mem_write!(ctx, u32, a, reg_read!(ctx, rs2) as u32); next };
            CSdsp, 1, { rs2, uimm }, { let a = reg_read!(ctx, R::Sp).wrapping_add(u64::from(uimm)); mem_write!(ctx, u64, a, reg_read!(ctx, rs2)); next };
        }
    };
}

// --------------------------------------------------------------------------------------------
// Variant 1: one big `match`, each arm advancing the instruction pointer itself
// --------------------------------------------------------------------------------------------

macro_rules! emit_match {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident),* $(,)? }, $body:block);* $(;)?
    ) => {
        fn run_match($ctx: &mut Ctx, mut $ip: *const I) -> Next {
            loop {
                // SAFETY: `ip` always points at a valid slot of the decoded stream
                match unsafe { *$ip } {
                    $(
                        #[allow(
                            unused_variables,
                            reason = "Unconditional jumps do not use the advanced pointer"
                        )]
                        I::$name { $($field,)* .. } => {
                            // SAFETY: the stream ends with a jump, so advancing stays in bounds
                            let $next = unsafe { $ip.add($slots) };
                            $ip = $body;
                        }
                    )*
                    _ => {
                        cold_path();
                        // SAFETY: `ip` points at a valid instruction
                        $ctx.stop = Stop::Unsupported(unsafe { discriminant($ip) });
                        return core::ptr::null();
                    }
                }
            }
        }
    };
}

ops!(emit_match);

// --------------------------------------------------------------------------------------------
// Variants 2-4: one function per instruction
// --------------------------------------------------------------------------------------------

macro_rules! emit_handlers {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident),* $(,)? }, $body:block);* $(;)?
    ) => {
        mod handlers {
            use super::*;

            $(
                #[expect(non_snake_case, reason = "One handler per instruction variant")]
                #[allow(
                    unused_variables,
                    reason = "Unconditional jumps do not use the advanced pointer"
                )]
                pub(super) fn $name($ip: *const I, $ctx: &mut Ctx) -> Next {
                    // SAFETY: `ip` points at a slot holding exactly this variant, which is how
                    // this handler was selected in the first place
                    let I::$name { $($field,)* .. } = (unsafe { *$ip }) else {
                        // SAFETY: guaranteed by handler selection
                        unsafe { unreachable_unchecked() }
                    };
                    // SAFETY: the stream ends with a jump, so advancing stays in bounds
                    let $next = unsafe { $ip.add($slots) };
                    $body
                }
            )*

            pub(super) fn unsupported(ip: *const I, ctx: &mut Ctx) -> Next {
                cold_path();
                // SAFETY: `ip` points at a valid instruction
                ctx.stop = Stop::Unsupported(unsafe { discriminant(ip) });
                core::ptr::null()
            }
        }

        /// Pick the handler for an already decoded instruction
        fn handler_for(instruction: I) -> Handler {
            match instruction {
                $( I::$name { .. } => handlers::$name as Handler, )*
                _ => handlers::unsupported as Handler,
            }
        }
    };
}

ops!(emit_handlers);

macro_rules! emit_tail_handlers {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident),* $(,)? }, $body:block);* $(;)?
    ) => {
        mod tail_handlers {
            use super::*;

            $(
                #[expect(non_snake_case, reason = "One handler per instruction variant")]
                #[allow(
                    unused_variables,
                    reason = "Unconditional jumps do not use the advanced pointer"
                )]
                pub(super) fn $name($ip: *const I, $ctx: &mut Ctx) -> Next {
                    // SAFETY: `ip` points at a slot holding exactly this variant, which is how
                    // this handler was selected in the first place
                    let I::$name { $($field,)* .. } = (unsafe { *$ip }) else {
                        // SAFETY: guaranteed by handler selection
                        unsafe { unreachable_unchecked() }
                    };
                    // SAFETY: the stream ends with a jump, so advancing stays in bounds
                    let $next = unsafe { $ip.add($slots) };
                    let $next = $body;
                    // Every path that stops execution returns early, so `next` is always a valid
                    // instruction here
                    let handler = dispatch($ctx, $next);
                    become handler($next, $ctx)
                }
            )*

            pub(super) fn unsupported(ip: *const I, ctx: &mut Ctx) -> Next {
                cold_path();
                // SAFETY: `ip` points at a valid instruction
                ctx.stop = Stop::Unsupported(unsafe { discriminant(ip) });
                core::ptr::null()
            }
        }

        /// Pick the tail-calling handler for an already decoded instruction
        fn tail_handler_for(instruction: I) -> Handler {
            match instruction {
                $( I::$name { .. } => tail_handlers::$name as Handler, )*
                _ => tail_handlers::unsupported as Handler,
            }
        }
    };
}

macro_rules! emit_plain_tail_handlers {
    (
        $ctx:ident, $ip:ident, $next:ident,
        $($name:ident, $slots:expr, { $($field:ident),* $(,)? }, $body:block);* $(;)?
    ) => {
        mod plain_tail_handlers {
            use super::*;

            $(
                #[expect(non_snake_case, reason = "One handler per instruction variant")]
                #[allow(
                    unused_variables,
                    reason = "Unconditional jumps do not use the advanced pointer"
                )]
                pub(super) fn $name($ip: *const I, $ctx: &mut Ctx) -> Next {
                    // SAFETY: `ip` points at a slot holding exactly this variant, which is how
                    // this handler was selected in the first place
                    let I::$name { $($field,)* .. } = (unsafe { *$ip }) else {
                        // SAFETY: guaranteed by handler selection
                        unsafe { unreachable_unchecked() }
                    };
                    // SAFETY: the stream ends with a jump, so advancing stays in bounds
                    let $next = unsafe { $ip.add($slots) };
                    let $next = $body;
                    // Every path that stops execution returns early, so `next` is always a valid
                    // instruction here
                    let handler = dispatch($ctx, $next);
                    // Deliberately NOT `become`: this measures whether LLVM turns an ordinary
                    // call in tail position into a sibling call on its own
                    handler($next, $ctx)
                }
            )*

            pub(super) fn unsupported(ip: *const I, ctx: &mut Ctx) -> Next {
                cold_path();
                // SAFETY: `ip` points at a valid instruction
                ctx.stop = Stop::Unsupported(unsafe { discriminant(ip) });
                core::ptr::null()
            }
        }

        /// Pick the plain-call handler for an already decoded instruction
        fn plain_tail_handler_for(instruction: I) -> Handler {
            match instruction {
                $( I::$name { .. } => plain_tail_handlers::$name as Handler, )*
                _ => plain_tail_handlers::unsupported as Handler,
            }
        }
    };
}

ops!(emit_plain_tail_handlers);

ops!(emit_tail_handlers);

/// Look up the handler for the instruction at `ip`
#[inline(always)]
fn dispatch(ctx: &Ctx, ip: *const I) -> Handler {
    // SAFETY: `ip` points at a valid instruction and the table covers every discriminant
    unsafe {
        *ctx.handlers
            .get_unchecked(usize::from(discriminant(ip)))
    }
}

/// Driver loop calling one handler per instruction through the dispatch table
fn run_call(ctx: &mut Ctx, mut ip: *const I) -> Next {
    loop {
        let handler = dispatch(ctx, ip);
        ip = handler(ip, ctx);

        if ip.is_null() {
            cold_path();
            return ip;
        }
    }
}

/// Enter the tail-call-threaded chain
fn run_tail(ctx: &mut Ctx, ip: *const I) -> Next {
    let handler = dispatch(ctx, ip);
    handler(ip, ctx)
}

/// Enter the chain that relies on LLVM turning tail-position calls into sibling calls
fn run_plain_tail(ctx: &mut Ctx, ip: *const I) -> Next {
    let handler = dispatch(ctx, ip);
    handler(ip, ctx)
}

// --------------------------------------------------------------------------------------------
// Entry point
// --------------------------------------------------------------------------------------------

/// Which dispatch strategy to use
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Dispatch {
    /// One big `match` with per-arm instruction pointer advance
    Match,
    /// Function pointer table, called from a driver loop
    Call,
    /// Function pointer table, chained with guaranteed tail calls
    Tail,
    /// Function pointer table, chained with ordinary calls in tail position
    PlainTail,
}

impl Dispatch {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "match" => Some(Self::Match),
            "call" => Some(Self::Call),
            "tail" => Some(Self::Tail),
            "plaintail" => Some(Self::PlainTail),
            _ => None,
        }
    }
}

impl Ctx {
    /// Build a context around an already decoded instruction stream
    pub(crate) fn new(
        instructions: &[I],
        base_addr: u64,
        return_trap: u64,
        dispatch: Dispatch,
    ) -> Box<Self> {
        let mut ctx = Box::<Self>::new_uninit();
        let ctx_ptr = ctx.as_mut_ptr();

        // SAFETY: writing every field of freshly allocated storage exactly once, without ever
        // creating a reference to the partially initialized value. Guest RAM is zeroed in place so
        // that it never goes through a stack temporary.
        let mut ctx = unsafe {
            core::ptr::write_bytes(
                (&raw mut (*ctx_ptr).mem).cast::<u8>(),
                0,
                crate::MEMORY_SIZE,
            );
            (&raw mut (*ctx_ptr).regs).write([0; 33]);
            (&raw mut (*ctx_ptr).base).write(instructions.as_ptr());
            (&raw mut (*ctx_ptr).slots).write(instructions.len());
            (&raw mut (*ctx_ptr).base_addr).write(base_addr);
            (&raw mut (*ctx_ptr).return_trap).write(return_trap);
            (&raw mut (*ctx_ptr).handlers).write([handlers::unsupported as Handler; VARIANTS]);
            (&raw mut (*ctx_ptr).start).write(Instant::now());
            (&raw mut (*ctx_ptr).stop).write(Stop::Done);
            ctx.assume_init()
        };

        // The dispatch table is filled in from the program itself: every discriminant that can
        // ever be dispatched appears in the decoded stream, so a single pass over it is enough and
        // no discriminant values need to be hardcoded anywhere
        for &instruction in instructions {
            // SAFETY: the enum is `#[repr(u16)]`
            let discriminant = usize::from(unsafe { discriminant(&raw const instruction) });
            ctx.handlers[discriminant] = match dispatch {
                Dispatch::Tail => tail_handler_for(instruction),
                Dispatch::PlainTail => plain_tail_handler_for(instruction),
                Dispatch::Match | Dispatch::Call => handler_for(instruction),
            };
        }

        ctx
    }

    pub(crate) fn write_u64(&mut self, address: u64, value: u64) {
        self.mem[address as usize..][..size_of::<u64>()].copy_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn set_reg(&mut self, reg: R, value: u64) {
        reg_write!(self, reg, value);
    }

    /// Run the program from `pc`
    pub(crate) fn run(&mut self, dispatch: Dispatch, pc: u64) -> Stop {
        let slot = ((pc - self.base_addr) / size_of::<u16>() as u64) as usize;
        // SAFETY: caller passes an in-bounds program counter
        let ip = unsafe { self.base.add(slot) };

        self.stop = Stop::Done;
        self.start = Instant::now();

        let ctx = &mut *self;
        match dispatch {
            Dispatch::Match => run_match(ctx, ip),
            Dispatch::Call => run_call(ctx, ip),
            Dispatch::Tail => run_tail(ctx, ip),
            Dispatch::PlainTail => run_plain_tail(ctx, ip),
        };

        self.stop
    }
}

/// Only the parts of the interface that setting up and inspecting the guest needs; the interpreter
/// itself goes straight at [`Ctx::mem`]
impl VirtualMemory for Ctx {
    fn read<T>(&self, _address: u64) -> Result<T, VirtualMemoryError>
    where
        T: BasicInt,
    {
        unimplemented!("Not used during setup")
    }

    unsafe fn read_unchecked<T>(&self, _address: u64) -> T
    where
        T: BasicInt,
    {
        unimplemented!("Not used during setup")
    }

    fn read_slice(&self, _address: u64, _len: u32) -> Result<&[u8], VirtualMemoryError> {
        unimplemented!("Not used during setup")
    }

    fn read_slice_up_to(&self, address: u64, len: u32) -> &[u8] {
        let Some(remaining) = self.mem.get(address as usize..) else {
            return &[];
        };
        remaining.get(..len as usize).unwrap_or(remaining)
    }

    fn write<T>(&mut self, address: u64, value: T) -> Result<(), VirtualMemoryError>
    where
        T: BasicInt,
    {
        let Some(target) = self
            .mem
            .get_mut(address as usize..)
            .and_then(|data| data.get_mut(..size_of::<T>()))
        else {
            return Err(VirtualMemoryError::OutOfBoundsWrite { address });
        };

        // SAFETY: bounds were just checked, and only plain integers are written
        unsafe {
            target.as_mut_ptr().cast::<T>().write_unaligned(value);
        }

        Ok(())
    }

    fn write_slice(&mut self, address: u64, data: &[u8]) -> Result<(), VirtualMemoryError> {
        let Some(target) = self
            .mem
            .get_mut(address as usize..)
            .and_then(|data_slot| data_slot.get_mut(..data.len()))
        else {
            return Err(VirtualMemoryError::OutOfBoundsWrite { address });
        };

        target.copy_from_slice(data);

        Ok(())
    }
}
