extern crate alloc;

use crate::fused::FusedInstruction;
use crate::fused::rv32::zba::Rv32ZbaFusedInstruction;
use crate::rv32::test_utils::{TEST_BASE_ADDR, TestInterpreterState, execute, initialize_state};
use crate::{RegisterFile, VirtualMemory};
use ab_riscv_primitives::prelude::*;
use alloc::format;

type Fused = Rv32ZbaFusedInstruction<Reg<u32>>;

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

/// Puts values into the registers the `addi` + `sh[123]add` fusion combines
fn prepare_addi_shxadd(state: &mut TestInterpreterState<Fused>) {
    state.regs.write(Reg::A1, 0x1234);
    state.regs.write(Reg::A2, 0x5678);
}

#[test]
fn test_fuse_addi_shxadd() {
    for (shamt, next) in [
        (
            1,
            Fused::Sh1add {
                rd: Reg::A0,
                rs1: Reg::A0,
                rs2: Reg::A2,
            },
        ),
        (
            2,
            Fused::Sh2add {
                rd: Reg::A0,
                rs1: Reg::A0,
                rs2: Reg::A2,
            },
        ),
        (
            3,
            Fused::Sh3add {
                rd: Reg::A0,
                rs1: Reg::A0,
                rs2: Reg::A2,
            },
        ),
    ] {
        assert_fused_as(
            Fused::Addi {
                rd: Reg::A0,
                rs1: Reg::A1,
                rs2: Reg::Zero,
                imm: 4,
            },
            next,
            Fused::FusedAddiShxadd {
                rd: Reg::A0,
                rs1: Reg::A1,
                rs2: Reg::A2,
                shamt,
                imm: 4,
            },
            "addi a0, a1, 4",
        );
    }

    // The intermediate result has to die, which it does not when the second instruction writes
    // somewhere else
    assert_not_fused(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::Sh3add {
            rd: Reg::A3,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
    );
    // The second instruction reads a third register, and it reads it after the first instruction
    // would have written its own result, so a pair that reads the very same register there cannot
    // be fused without reading a stale value
    assert_not_fused(
        Fused::Addi {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A0,
        },
    );
    // Writing `zero` discards the result, so the first instruction is not dead code to begin with
    assert_not_fused(
        Fused::Addi {
            rd: Reg::Zero,
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::Sh3add {
            rd: Reg::Zero,
            rs1: Reg::Zero,
            rs2: Reg::A2,
        },
    );
}

#[test]
fn test_execute_addi_shxadd() {
    for next in [
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A2,
        },
    ] {
        for imm in [2047, -2048, 0] {
            assert_same_execution(
                Fused::Addi {
                    rd: Reg::A0,
                    rs1: Reg::A1,
                    rs2: Reg::Zero,
                    imm,
                },
                next,
                prepare_addi_shxadd,
            );
        }
    }
}

#[test]
fn test_fuse_shxadd_load() {
    assert_fused(
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLb {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 1,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLb {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lb {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLb {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 3,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLh {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 1,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLh {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lh {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLh {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 3,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLw {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 1,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLw {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLw {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 3,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLbu {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 1,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLbu {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lbu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLbu {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 3,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLhu {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 1,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLhu {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 2,
            offset: 4,
        },
    );
    assert_fused(
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::FusedShxaddLhu {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
            shamt: 3,
            offset: 4,
        },
    );

    assert_not_fused(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lw {
            rd: Reg::A3,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
    );
}

#[test]
fn test_execute_shxadd_load() {
    assert_same_execution(
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
        Fused::Sh1add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Sh2add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
        },
        Fused::Lhu {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            imm: 4,
        },
        prepare_load,
    );
    assert_same_execution(
        Fused::Sh3add {
            rd: Reg::A0,
            rs1: Reg::A2,
            rs2: Reg::A1,
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
