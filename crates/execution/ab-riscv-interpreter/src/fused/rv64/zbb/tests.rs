extern crate alloc;

use crate::fused::FusedInstruction;
use crate::fused::rv64::zbb::Rv64ZbbFusedInstruction;
use crate::rv64::test_utils::{TestInterpreterState, execute, execute_threaded, initialize_state};
use crate::{OpaqueThreadedExecutionResult, RegisterFile};
use ab_riscv_primitives::prelude::*;
use alloc::format;

type Fused = Rv64ZbbFusedInstruction<Reg<u64>>;

/// Fusing `prev` with `next` must produce `expected`, leave `next` where it was, cover exactly as
/// many bytes as the pair it replaced and print as `display`
#[track_caller]
fn assert_fused_as(prev: Fused, next: Fused, expected: Fused, display: &str) {
    assert_eq!(Fused::fuse(prev, next), (expected, next));
    assert_eq!(expected.size(), prev.size() + next.size());
    assert_eq!(format!("{expected}"), display);
}

/// [`assert_fused_as()`] for a fused instruction that prints as the first instruction of the pair,
/// which is all of them here
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
fn assert_same_execution(prev: Fused, next: Fused) {
    let (fused, _next) = Fused::fuse(prev, next);
    assert_ne!(fused, prev, "pair was not fused at all");

    let mut unfused_state = initialize_state([prev, next]);
    prepare_bits(&mut unfused_state);
    execute(&mut unfused_state).unwrap();

    let mut fused_state = initialize_state([fused]);
    prepare_bits(&mut fused_state);
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
        prepare_bits(&mut threaded_state);
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

/// Puts values with interesting bit patterns into the registers the fusions here combine
fn prepare_bits(state: &mut TestInterpreterState<Fused>) {
    state.regs.write(Reg::A1, 0x0f0f_0f0f_0f0f_0f0f);
    state.regs.write(Reg::A2, 0x3333_3333_5555_5555);
}

#[test]
fn test_fuse_xor_rotate() {
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 0,
        },
        Fused::FusedXorRori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            shamt: 0,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 1,
        },
        Fused::FusedXorRori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            shamt: 1,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 63,
        },
        Fused::FusedXorRori {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            shamt: 63,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Roriw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 0,
        },
        Fused::FusedXorRoriw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            shamt: 0,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Roriw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 1,
        },
        Fused::FusedXorRoriw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            shamt: 1,
        },
    );
    assert_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Roriw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 31,
        },
        Fused::FusedXorRoriw {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
            shamt: 31,
        },
    );

    // The intermediate result has to die, which it does not when the second instruction writes
    // somewhere else
    assert_not_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A3,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 7,
        },
    );
    // ... or when it does not read it in the first place
    assert_not_fused(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A3,
            rs2: Reg::Zero,
            shamt: 7,
        },
    );
    // Writing `zero` discards the result, so the first instruction is not dead code to begin with
    assert_not_fused(
        Fused::Xor {
            rd: Reg::Zero,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::Zero,
            rs1: Reg::Zero,
            rs2: Reg::Zero,
            shamt: 7,
        },
    );
}

#[test]
fn test_execute_xor_rotate() {
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 0,
        },
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 1,
        },
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Rori {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 63,
        },
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Roriw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 0,
        },
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Roriw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 1,
        },
    );
    assert_same_execution(
        Fused::Xor {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::A2,
        },
        Fused::Roriw {
            rd: Reg::A0,
            rs1: Reg::A0,
            rs2: Reg::Zero,
            shamt: 31,
        },
    );
}
