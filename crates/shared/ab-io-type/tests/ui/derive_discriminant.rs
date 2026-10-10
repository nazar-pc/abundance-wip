//! `TrivialType` derive must reject enums with explicit discriminants that are not equal to the
//! index of the variant, metadata identifies variants by their index and doesn't record
//! discriminants

use ab_io_type::trivial_type::TrivialType;

#[derive(Copy, Clone, TrivialType)]
#[repr(u8)]
enum Sparse {
    A,
    B = 5,
}

// Only the explicit discriminant is reported, the implicit one after it is wrong as a consequence
#[derive(Copy, Clone, TrivialType)]
#[repr(u8)]
enum Shifted {
    A = 1,
    B,
}

#[derive(Copy, Clone, TrivialType)]
#[repr(u8)]
enum Expression {
    A = Self::FIRST,
    B = Self::FIRST + 2,
}

impl Expression {
    const FIRST: u8 = 0;
}

#[derive(Copy, Clone, TrivialType)]
#[repr(u8)]
enum WithFields {
    A { value: u8 } = 1,
    B { value: u8 } = 0,
}

fn main() {}
