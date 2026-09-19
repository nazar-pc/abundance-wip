extern crate alloc;

use crate::fused::FusedInstruction;
use crate::fused::rv64::m::Rv64MFusedInstruction;
use crate::rv64::test_utils::{TestInterpreterState, execute, execute_threaded, initialize_state};
use crate::{OpaqueThreadedExecutionResult, RegisterFile};
use ab_riscv_primitives::prelude::*;
use alloc::format;

type Fused = Rv64MFusedInstruction<Reg<u64>>;

/// Fusing `prev` with `next` must produce `expected`, leave `next` where it was, cover exactly as
/// many bytes as the pair it replaced and print as the first instruction of that pair
#[track_caller]
fn assert_fused(prev: Fused, next: Fused, expected: Fused) {
    assert_eq!(Fused::fuse(prev, next), (expected, next));
    assert_eq!(expected.size(), prev.size() + next.size());
    assert_eq!(format!("{expected}"), format!("{prev}"));
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
    // above
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

fn prepare(state: &mut TestInterpreterState<Fused>) {
    state.regs.write(Reg::A1, 0x1234_5678);
    state.regs.write(Reg::A2, 0x9abc_def0);
    state.regs.write(Reg::A3, 0x0fed_cba9);
}

#[test]
fn test_fuse_mul_add() {
    assert_fused(
        Fused::Mul {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedMulAdd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );
    assert_fused(
        Fused::Mulw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Addw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        Fused::FusedMulwAddw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            rs3: Reg::A3,
        },
    );

    // The register added to the product is read after the multiply wrote its destination, so a
    // pair that hands it that destination must not be fused
    assert_not_fused(
        Fused::Mul {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A0,
        },
    );
}

#[test]
fn test_execute_mul_add() {
    assert_same_execution(
        Fused::Mul {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Add {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare,
    );
    assert_same_execution(
        Fused::Mulw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Addw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::A3,
        },
        prepare,
    );
}
