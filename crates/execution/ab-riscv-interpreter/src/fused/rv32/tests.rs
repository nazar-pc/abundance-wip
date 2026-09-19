extern crate alloc;

use crate::fused::FusedInstruction;
use crate::fused::rv32::Rv32FusedInstruction;
use crate::rv32::test_utils::{TEST_BASE_ADDR, TestInterpreterState, execute, initialize_state};
use crate::{RegisterFile, VirtualMemory};
use ab_riscv_primitives::prelude::*;
use alloc::format;

type Fused = Rv32FusedInstruction<Reg<u32>>;

/// Address of the data the fused loads below read
const DATA_ADDR: u32 = TEST_BASE_ADDR + 0x100;

/// Fusing `prev` with `next` must produce `expected`, leave `next` where it was, cover exactly as
/// many bytes as the pair it replaced and print as `display`
#[track_caller]
fn assert_fused_as(prev: Fused, next: Fused, expected: Fused, display: &str) {
    assert_eq!(Fused::fuse(prev, next), (expected, next));
    assert_eq!(expected.size(), prev.size() + next.size());
    assert_eq!(format!("{expected}"), display);
}

/// [`assert_fused_as()`] for a fused instruction that prints as the first instruction of the pair,
/// which is all of them but the ones that don't carry its operands separately
#[track_caller]
fn assert_fused(prev: Fused, next: Fused, expected: Fused) {
    assert_fused_as(prev, next, expected, &format!("{prev}"));
}

/// Fusing `prev` with `next` must leave both of them alone
#[track_caller]
fn assert_not_fused(prev: Fused, next: Fused) {
    assert_eq!(Fused::fuse(prev, next), (prev, next));
}

/// Executing the instruction `prev` and `next` fuse into must leave behind exactly what executing
/// the two of them in sequence does
#[track_caller]
fn assert_same_execution<Prepare>(prev: Fused, next: Fused, prepare: Prepare)
where
    Prepare: Fn(&mut TestInterpreterState<Fused>),
{
    let (fused, _next) = Fused::fuse(prev, next);
    assert_ne!(fused, prev, "pair was not fused at all");

    let mut unfused_state = initialize_state([prev, next]);
    prepare(&mut unfused_state);
    execute(&mut unfused_state).unwrap();

    let mut fused_state = initialize_state([fused]);
    prepare(&mut fused_state);
    execute(&mut fused_state).unwrap();

    for bits in 0..32 {
        let Some(reg) = <Reg<u32> as Register>::from_bits(bits) else {
            continue;
        };

        assert_eq!(
            fused_state.regs.read(reg),
            unfused_state.regs.read(reg),
            "register {reg}"
        );
    }

    // A fused instruction can store, so the memory it wrote has to agree as well
    assert_eq!(
        fused_state
            .memory
            .read_slice(u64::from(DATA_ADDR) - 16, 64)
            .unwrap(),
        unfused_state
            .memory
            .read_slice(u64::from(DATA_ADDR) - 16, 64)
            .unwrap(),
        "memory"
    );
}

/// Puts the address the fused loads read into `a1` and something with bytes that differ in sign
/// into the memory it points at
fn prepare_load(state: &mut TestInterpreterState<Fused>) {
    state.regs.write(Reg::A1, DATA_ADDR - 4);
    state
        .memory
        .write::<u64>(u64::from(DATA_ADDR), 0x8899_aabb_ccdd_eeff)
        .unwrap();
}

#[test]
fn test_fuse_addi_load() {
    assert_fused_as(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        Fused::FusedAddiLb {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        "lb a0, 4(a1)",
    );
    assert_fused_as(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        Fused::FusedAddiLh {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        "lh a0, 4(a1)",
    );
    assert_fused_as(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        Fused::FusedAddiLw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        "lw a0, 4(a1)",
    );
    assert_fused_as(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        Fused::FusedAddiLbu {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        "lbu a0, 4(a1)",
    );
    assert_fused_as(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        Fused::FusedAddiLhu {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        "lhu a0, 4(a1)",
    );

    // The intermediate value must be dead, which it is not when the load reads or writes a
    // different register than `addi` wrote
    assert_not_fused(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lw {
            rd: Reg::A2,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
    );
    assert_not_fused(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::Zero,
            imm: -4,
        },
    );
    // `addi` into the zero register writes nothing, so the load doesn't read what it computed
    assert_not_fused(
        Fused::Addi {
            rd: Reg::Zero,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lw {
            rd: Reg::Zero,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: -4,
        },
    );
}

#[test]
fn test_execute_addi_load() {
    assert_same_execution(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 8,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -4,
        },
        prepare_load,
    );
}

#[test]
fn test_fuse_add_load() {
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedAddLb {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedAddLh {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedAddLw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedAddLbu {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedAddLhu {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            offset: 4,
        },
    );

    // `ld-add` is this same fusion with a zero offset
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0,
        },
        Fused::FusedAddLw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            offset: 0,
        },
    );
}

#[test]
fn test_execute_add_load() {
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
}

#[test]
fn test_fuse_upper_addi() {
    assert_fused(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -8,
        },
        Fused::FusedAuipcAddi {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1000 - 8),
        },
    );
    assert_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x12_3000),
        },
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x456,
        },
        Fused::FusedLuiAddi {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x12_3456),
        },
    );

    // A sum that doesn't fit into the 24 bits a fused instruction has for it is not fused
    assert_not_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x100_0000),
        },
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0,
        },
    );
}

#[test]
fn test_execute_upper_addi() {
    assert_same_execution(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -8,
        },
        |_state| {},
    );
    assert_same_execution(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(-0x1000),
        },
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -8,
        },
        |_state| {},
    );
}

#[test]
fn test_fuse_upper_load() {
    assert_fused(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedAuipcLb {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedLuiLb {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedAuipcLh {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedLuiLh {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedAuipcLw {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedLuiLw {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedAuipcLbu {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedLuiLbu {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedAuipcLhu {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
    assert_fused(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        Fused::FusedLuiLhu {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24::from_i32(0x1100),
        },
    );
}

#[test]
fn test_execute_upper_load() {
    assert_same_execution(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0),
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0),
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0),
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0),
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Auipc {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0),
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Lui {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            imm: I24WithZeroedBits::from_i32(0x1000),
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x100,
        },
        prepare_load,
    );
}

#[test]
fn test_fuse_slli_srli() {
    assert_fused(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 16,
        },
        Fused::Srli {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 16,
        },
        Fused::FusedSlliSrliZexth {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
        },
    );
    // Anything the specialized forms above don't cover is a plain bitfield extract
    assert_fused(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
        },
        Fused::Srli {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 9,
        },
        Fused::FusedSlliSrli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
            right_shamt: 9,
        },
    );

    assert_not_fused(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
        },
        Fused::Srli {
            rd: Reg::A2,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 9,
        },
    );
}

#[test]
fn test_execute_slli_srli() {
    let prepare = |state: &mut TestInterpreterState<Fused>| {
        state.regs.write(Reg::A1, 0x8899_aabb);
    };

    assert_same_execution(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 16,
        },
        Fused::Srli {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 16,
        },
        prepare,
    );
    assert_same_execution(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
        },
        Fused::Srli {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 9,
        },
        prepare,
    );
    assert_same_execution(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 9,
        },
        Fused::Srli {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 5,
        },
        prepare,
    );
}

/// Puts values with interesting bit patterns into the registers the logic and shift fusions
/// combine
fn prepare_bits(state: &mut TestInterpreterState<Fused>) {
    state.regs.write(Reg::A0, 0x0f0f_0f0f);
    state.regs.write(Reg::A1, 0x3333_3333);
    state.regs.write(Reg::A2, 0x5555_5555);
    state.regs.write(Reg::A3, 0x00ff_00ff);
}

#[test]
fn test_fuse_logic() {
    assert_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedAndAnd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedAndOr {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedAndXor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedOrAnd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedOrOr {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedOrXor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedXorAnd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedXorOr {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedXorXor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedAndAndi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedAndOri {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedAndXori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedOrAndi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedOrOri {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedOrXori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedXorAndi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedXorOri {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::FusedXorXori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedAndiAnd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedAndiOr {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedAndiXor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedOriAnd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedOriOr {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedOriXor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedXoriAnd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedXoriOr {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );
    assert_fused(
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 0x123,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::FusedXoriXor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 0x123,
        },
    );

    // The register the second instruction reads is read after the first one wrote its
    // destination, so a pair that hands it that destination reads a value that no longer exists
    // by the time the fused instruction runs, and must not be fused
    assert_not_fused(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A0,
        },
    );
    assert_not_fused(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 1,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A0,
        },
    );
}

#[test]
fn test_execute_logic() {
    assert_same_execution(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Andi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Ori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::And {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::Or {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
    assert_same_execution(
        Fused::Xori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: -0x123,
        },
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        prepare_bits,
    );
}

#[test]
fn test_fuse_add_store() {
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sb {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
        Fused::FusedAddSb {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sh {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
        Fused::FusedAddSh {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sw {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
        Fused::FusedAddSw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );

    // A non-zero offset addresses something other than what the `add` computed
    assert_not_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sb {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 4,
        },
    );
    // The stored value is read after the `add` wrote the address, so it must not be the address
    assert_not_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sb {
            rs1: Reg::A0,
            rs2: Reg::A0,
            imm: 0,
        },
    );
    // The address stays live after the store, so it must not be one of the `add`'s own sources
    assert_not_fused(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::Sb {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
    );
}

#[test]
fn test_execute_add_store() {
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sb {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
        |state| {
            prepare_load(state);
            state.regs.write(Reg::A2, 4);
            state.regs.write(Reg::A3, 0x1234_5678);
        },
    );
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sh {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
        |state| {
            prepare_load(state);
            state.regs.write(Reg::A2, 4);
            state.regs.write(Reg::A3, 0x1234_5678);
        },
    );
    assert_same_execution(
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Sw {
            rs1: Reg::A0,
            rs2: Reg::A3,
            imm: 0,
        },
        |state| {
            prepare_load(state);
            state.regs.write(Reg::A2, 4);
            state.regs.write(Reg::A3, 0x1234_5678);
        },
    );
}

#[test]
fn test_fuse_shift_bit_extract() {
    assert_fused(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
        },
        Fused::Srai {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 9,
        },
        Fused::FusedSlliSrai {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
            right_shamt: 9,
        },
    );
}

#[test]
fn test_execute_shift_bit_extract() {
    assert_same_execution(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 5,
        },
        Fused::Srai {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 9,
        },
        |state| {
            state.regs.write(Reg::A1, 0x8899_aabb);
        },
    );
    assert_same_execution(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 9,
        },
        Fused::Srai {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 5,
        },
        |state| {
            state.regs.write(Reg::A1, 0x8899_aabb);
        },
    );
    assert_same_execution(
        Fused::Slli {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            shamt: 0,
        },
        Fused::Srai {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 3,
        },
        |state| {
            state.regs.write(Reg::A1, 0x8899_aabb);
        },
    );
}
