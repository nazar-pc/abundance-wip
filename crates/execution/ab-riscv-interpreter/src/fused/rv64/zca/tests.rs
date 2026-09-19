extern crate alloc;

use crate::fused::FusedInstruction;
use crate::fused::rv64::zca::Rv64ZcaFusedInstruction;
use crate::rv64::test_utils::{TestInterpreterState, execute, execute_threaded, initialize_state};
use crate::{OpaqueThreadedExecutionResult, RegisterFile};
use ab_riscv_primitives::prelude::*;
use alloc::format;

type Fused = Rv64ZcaFusedInstruction<Reg<u64>>;

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
        let Some(reg) = <Reg<u64> as Register>::from_bits(bits) else {
            continue;
        };

        assert_eq!(
            fused_state.regs.read(reg),
            unfused_state.regs.read(reg),
            "register {reg}"
        );
    }

    // Threaded dispatch is generated from the very same arms, so it has to agree with all of the
    // above, including on stepping over the whole pair
    if OpaqueThreadedExecutionResult::<Fused>::platform_supported() {
        let mut threaded_state = initialize_state([fused]);
        prepare(&mut threaded_state);
        execute_threaded(&mut threaded_state).outcome.unwrap();

        for bits in 0..32 {
            let Some(reg) = <Reg<u64> as Register>::from_bits(bits) else {
                continue;
            };

            assert_eq!(
                threaded_state.regs.read(reg),
                unfused_state.regs.read(reg),
                "register {reg} after threaded dispatch"
            );
        }
    }
}

#[test]
fn test_fuse_branch_cmv() {
    assert_fused(
        Fused::Beq {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedBeqCMv {
            rs1: Reg::A1,
            rs2: Reg::A2,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::Bne {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedBneCMv {
            rs1: Reg::A1,
            rs2: Reg::A2,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::Blt {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedBltCMv {
            rs1: Reg::A1,
            rs2: Reg::A2,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::Bge {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedBgeCMv {
            rs1: Reg::A1,
            rs2: Reg::A2,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::Bltu {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedBltuCMv {
            rs1: Reg::A1,
            rs2: Reg::A2,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::Bgeu {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedBgeuCMv {
            rs1: Reg::A1,
            rs2: Reg::A2,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::CBeqz {
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedCBeqzCMv {
            rs1: Reg::A1,
            rs2: Reg::Zero,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );
    assert_fused(
        Fused::CBnez {
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 4,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
        Fused::FusedCBnezCMv {
            rs1: Reg::A1,
            rs2: Reg::Zero,
            rd: Reg::A0,
            mv_rs2: Reg::A3,
        },
    );

    // A branch that goes anywhere other than exactly past the move is a branch, not a move
    assert_not_fused(
        Fused::Beq {
            rs1: Reg::A1,
            rs2: Reg::A2,
            imm: 8,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
    );
    assert_not_fused(
        Fused::CBeqz {
            rs1: Reg::A1,
            rs2: Reg::Zero,
            imm: 6,
        },
        Fused::CMv {
            rd: Reg::A0,
            rs1: Reg::Zero,
            rs2: Reg::A3,
        },
    );
}

/// Both outcomes of every condition have to be checked, so each of these is run with `a1` below,
/// equal to and above `a2`, with `a0` and `a3` holding values that tell a move from a missing one
fn prepare_condition(rs1_value: u64) -> impl Fn(&mut TestInterpreterState<Fused>) {
    move |state| {
        state.regs.write(Reg::A0, 0x1111_1111);
        state.regs.write(Reg::A1, rs1_value);
        state.regs.write(Reg::A2, 0);
        state.regs.write(Reg::A3, 0x2222_2222);
    }
}

#[test]
fn test_execute_branch_cmv() {
    for rs1_value in [u64::MAX, 0, 1] {
        assert_same_execution(
            Fused::Beq {
                rs1: Reg::A1,
                rs2: Reg::A2,
                imm: 6,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::Bne {
                rs1: Reg::A1,
                rs2: Reg::A2,
                imm: 6,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::Blt {
                rs1: Reg::A1,
                rs2: Reg::A2,
                imm: 6,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::Bge {
                rs1: Reg::A1,
                rs2: Reg::A2,
                imm: 6,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::Bltu {
                rs1: Reg::A1,
                rs2: Reg::A2,
                imm: 6,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::Bgeu {
                rs1: Reg::A1,
                rs2: Reg::A2,
                imm: 6,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::CBeqz {
                rs1: Reg::A1,
                rs2: Reg::Zero,
                imm: 4,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
        assert_same_execution(
            Fused::CBnez {
                rs1: Reg::A1,
                rs2: Reg::Zero,
                imm: 4,
            },
            Fused::CMv {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::A3,
            },
            prepare_condition(rs1_value),
        );
    }
}
