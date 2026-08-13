#![feature(abi_custom)]
#![allow(incomplete_features)]
extern "custom" fn h(x: u64) -> u64 { x + 1 }
fn main() { println!("{}", h(1)); }
