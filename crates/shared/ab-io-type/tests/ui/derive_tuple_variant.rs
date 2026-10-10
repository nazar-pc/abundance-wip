//! `TrivialType` derive must reject enum variants with unnamed fields, metadata of enum variants
//! always has field names

use ab_io_type::trivial_type::TrivialType;

#[derive(Copy, Clone, TrivialType)]
#[repr(u8)]
enum TupleVariant {
    A { value: u8 },
    B(u8),
}

fn main() {}
