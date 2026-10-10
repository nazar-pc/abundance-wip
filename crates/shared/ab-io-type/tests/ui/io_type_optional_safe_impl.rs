//! `IoTypeOptional` can't be implemented without `unsafe`. `#[contract]` relies on it to give
//! access to `#[slot]` and `#[tmp]` storage that may be empty, so a type implementing it must not
//! expose uninitialized memory when its size is `0`, which `TrivialType` does.

use ab_io_type::IoTypeOptional;
use ab_io_type::trivial_type::TrivialType;

#[derive(Copy, Clone, TrivialType)]
#[repr(C)]
struct Value {
    value: u64,
}

impl IoTypeOptional for Value {}

fn main() {}
