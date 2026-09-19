//! Helpers for fused instruction implementations, which end up in whichever crate composes
//! them

use ab_riscv_primitives::prelude::*;

/// Lower 12-bit signed part of an immediate that a `lui`/`auipc` and the instruction fused
/// with it add up to.
///
/// The pair is how an assembler encodes a value wider than 12 bits in the first place: `lui`/
/// `auipc` contributes a multiple of 4096 and the second instruction the signed remainder, so
/// the sum splits back into the two operands unambiguously and a fused instruction only has to
/// carry the sum.
#[inline(always)]
#[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
pub const fn lower_immediate(imm: i32) -> i32 {
    (imm << 20) >> 20
}

/// Upper part of an immediate, see [`lower_immediate()`]
#[inline(always)]
#[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
pub const fn upper_immediate(imm: i32) -> i32 {
    imm.wrapping_sub(lower_immediate(imm))
}

/// The immediate a fused `lui`/`auipc` carries, see [`upper_immediate_fits()`] for why it is
/// 24 bits wide
#[inline(always)]
#[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
pub const fn combine_upper_immediate(upper: i32, lower: i16) -> I24 {
    I24::from_i32(truncate_to_i24(upper.wrapping_add(lower as i32)))
}

/// Whether the immediate of a `lui`/`auipc` and the immediate of the instruction fused with it
/// add up to something [`combine_upper_immediate()`] can carry.
///
/// The pair covers 32 bits between them, while a fused instruction has 24 bits for the sum:
/// an instruction of an instruction set is 8 bytes, 2 of which are the discriminant and 3 the
/// register operands every instruction has room for, and a wider immediate would make every
/// instruction of every instruction set these are composed into larger.
#[inline(always)]
#[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
pub const fn upper_immediate_fits(upper: i32, lower: i16) -> bool {
    let combined = upper.wrapping_add(lower as i32);

    truncate_to_i24(combined) == combined
}

#[inline(always)]
#[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
const fn truncate_to_i24(value: i32) -> i32 {
    (value << u8::BITS) >> u8::BITS
}
