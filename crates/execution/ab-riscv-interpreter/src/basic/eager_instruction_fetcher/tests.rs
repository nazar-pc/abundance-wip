//! Tests for what the decoded instruction stream contains and for
//! [`BasicEagerInstructionFetcher`]'s relative branch handling.
//!
//! [`ProgramCounter::set_pc_relative()`] moves within the decoded instruction stream rather than
//! resolving an address and converting it back, and its fast path deliberately skips the checks
//! [`ProgramCounter::set_pc()`] does. Most of the tests below pin down that the cases it skips are
//! still handled: the return trap, branches past the end of the stream, branches off its start and
//! unaligned targets.

use crate::basic::BasicMemory;
use crate::basic::eager_instruction_fetcher::{
    BasicEagerInstructionFetcher, BasicEagerInstructions,
};
use crate::fused::rv64::Rv64FusedInstruction;
use crate::{ExecutionError, FetchInstructionResult, InstructionFetcher, ProgramCounter};
use ab_riscv_primitives::prelude::*;
use alloc::vec::Vec;
use core::ops::ControlFlow;

const MEMORY_BASE_ADDRESS: u64 = 0x1000;
const MEMORY_SIZE: usize = 4 * 1024;
/// Address of the first instruction of [`code()`]
const BASE_ADDR: u64 = MEMORY_BASE_ADDRESS;

type I = Rv64Instruction<Reg<u64>>;
type Memory = BasicMemory<MEMORY_BASE_ADDRESS, MEMORY_SIZE>;

/// `addi x0, x0, 0`, the canonical `nop`
const NOP: u32 = 0x0000_0013;
/// `jalr x0, 0(x1)`, the canonical `ret`, so that the stream ends with a jump as the
/// constructor requires
const RET: u32 = 0x0000_8067;

/// Five 4-byte instructions at `BASE_ADDR`, `BASE_ADDR + 4`, ... `BASE_ADDR + 16`
fn code() -> Vec<u8> {
    [NOP, NOP, NOP, NOP, RET]
        .iter()
        .flat_map(|instruction| instruction.to_le_bytes())
        .collect()
}

/// Address one past the last instruction
const END_ADDR: u64 = BASE_ADDR + 5 * 4;

/// Stored in slots whose bytes do not decode
const FALLBACK: I = Rv64Instruction::Unimp {
    rs1: Reg::ZERO,
    rs2: Reg::ZERO,
};

/// Decode [`code()`], which [`new_fetcher()`] then walks
fn new_instructions(return_trap_address: u64) -> BasicEagerInstructions<I> {
    // SAFETY: The instruction stream ends with a jump, the return trap is outside of it and
    // the base address is aligned
    unsafe { BasicEagerInstructions::decode(&code(), FALLBACK, return_trap_address, BASE_ADDR) }
}

#[test]
fn every_alignment_step_of_guest_code_owns_a_slot() {
    // This instruction set has no compressed instructions, so its slots are words rather than
    // halfwords and there is nothing to decode in the middle of an instruction
    assert_eq!(I::ALIGNMENT, size_of::<u32>() as u8);

    let code = code();

    // Including guest code that ends in the middle of an instruction, which decodes as far as
    // whole alignment steps go and leaves the trailing bytes out
    for len in 0..=code.len() {
        // SAFETY: Nothing is executed here, only the decoded stream is inspected, so what
        // execution may reach doesn't come into play
        let instructions =
            unsafe { BasicEagerInstructions::<I>::decode(&code[..len], FALLBACK, 0, BASE_ADDR) };

        assert_eq!(
            instructions.instructions_len(),
            len / usize::from(I::ALIGNMENT),
            "{len} bytes of guest code"
        );
    }
}

#[test]
fn every_slot_holds_the_instruction_its_bytes_decode_to() {
    let instructions = new_instructions(END_ADDR);
    let memory = Memory::default();
    // SAFETY: This is the address of the first instruction of `code()`
    let mut fetcher = unsafe { instructions.fetcher(BASE_ADDR) };

    for encoded_instruction in [NOP, NOP, NOP, NOP, RET] {
        let expected = I::try_decode(encoded_instruction).expect("Valid instruction; qed");

        let FetchInstructionResult::Instruction(instruction) =
            InstructionFetcher::<I, Memory>::fetch_instruction(&mut fetcher, &memory)
        else {
            panic!("Expected an instruction");
        };

        assert_eq!(instruction, expected);
    }
}

/// Build a fetcher whose program counter sits just after the instruction at `BASE_ADDR + 4`,
/// which is the state relative branches are resolved from: the program counter is advanced
/// during instruction fetching, so `set_pc_relative(_, 4, offset)` branches from
/// `BASE_ADDR + 4`
fn new_fetcher(instructions: &BasicEagerInstructions<I>) -> BasicEagerInstructionFetcher<'_, I> {
    // SAFETY: Program counter is valid and aligned
    unsafe { instructions.fetcher(BASE_ADDR + 8) }
}

/// Branch by `offset` from the instruction at `BASE_ADDR + 4`
fn branch(
    fetcher: &mut BasicEagerInstructionFetcher<'_, I>,
    offset: i32,
) -> Result<ControlFlow<()>, ExecutionError<u64>> {
    let memory = Memory::default();
    fetcher.set_pc_relative(&memory, 4, offset)
}

#[test]
fn forward_and_backward_branches_move_within_the_stream() {
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    assert!(
        matches!(branch(&mut fetcher, 8), Ok(ControlFlow::Continue(()))),
        "Expected Continue"
    );
    assert_eq!(
        ProgramCounter::<u64, Memory>::get_pc(&fetcher),
        BASE_ADDR + 12
    );

    // Now branching from `BASE_ADDR + 8`, back to the very first instruction
    assert!(
        matches!(branch(&mut fetcher, -8), Ok(ControlFlow::Continue(()))),
        "Expected Continue"
    );
    assert_eq!(ProgramCounter::<u64, Memory>::get_pc(&fetcher), BASE_ADDR);
}

#[test]
fn branch_to_the_last_instruction_stays_in_bounds() {
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    assert!(
        matches!(branch(&mut fetcher, 12), Ok(ControlFlow::Continue(()))),
        "Expected Continue"
    );
    assert_eq!(
        ProgramCounter::<u64, Memory>::get_pc(&fetcher),
        END_ADDR - 4
    );
}

#[test]
fn branch_to_a_trap_below_the_stream_stops_execution() {
    // The usual arrangement: the trap sentinel sits outside the guest's code
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    // From `BASE_ADDR + 4` down to address 0
    let offset = -i32::try_from(BASE_ADDR + 4).unwrap();
    assert!(
        matches!(branch(&mut fetcher, offset), Ok(ControlFlow::Break(()))),
        "Expected Break"
    );
}

#[test]
fn branch_to_a_trap_above_the_stream_stops_execution() {
    let instructions = new_instructions(END_ADDR + 4);
    let mut fetcher = new_fetcher(&instructions);

    let offset = i32::try_from(END_ADDR + 4 - (BASE_ADDR + 4)).unwrap();
    assert!(
        matches!(branch(&mut fetcher, offset), Ok(ControlFlow::Break(()))),
        "Expected Break"
    );
}

#[test]
fn branch_past_the_end_of_the_stream_is_out_of_bounds() {
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    let error = branch(&mut fetcher, 1024).unwrap_err();
    assert!(
        matches!(error, ExecutionError::OutOfBoundsRead { address } if address.get() == BASE_ADDR + 4 + 1024),
        "Unexpected error {error:?}"
    );
}

#[test]
fn branch_off_the_start_of_the_stream_is_out_of_bounds() {
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    // Lands below `BASE_ADDR`, which underflows the stream rather than wrapping into it
    let error = branch(&mut fetcher, -1024).unwrap_err();
    assert!(
        matches!(error, ExecutionError::OutOfBoundsRead { address } if address.get() == BASE_ADDR + 4 - 1024),
        "Unexpected error {error:?}"
    );
}

#[test]
fn branch_to_an_unaligned_target_is_rejected() {
    // Branch immediates encode a halfword count, so the decoder cannot produce an odd offset
    // and this is not reachable through instruction execution. It is still the rule `set_pc()`
    // applies, and the fast path must not quietly round such a target to a slot boundary
    // instead.
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    let error = branch(&mut fetcher, 3).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnalignedInstruction { address } if address.get() == BASE_ADDR + 7),
        "Unexpected error {error:?}"
    );
}

#[test]
fn branch_into_the_middle_of_an_instruction_is_rejected() {
    // This instruction set has no compressed instructions, so a halfword-aligned target lands
    // between two instructions and must be refused rather than rounded to one
    assert_eq!(I::ALIGNMENT, size_of::<u32>() as u8);

    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    let error = branch(&mut fetcher, 2).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnalignedInstruction { address } if address.get() == BASE_ADDR + 6),
        "Unexpected error {error:?}"
    );
}

#[test]
fn branch_that_wraps_around_the_address_space_is_out_of_bounds() {
    let instructions = new_instructions(0);
    let mut fetcher = new_fetcher(&instructions);

    // Far enough back that the guest address itself wraps around zero
    let error = branch(&mut fetcher, i32::MIN).unwrap_err();
    assert!(
        matches!(error, ExecutionError::OutOfBoundsRead { address: _ }),
        "Unexpected error {error:?}"
    );
}

/// Instruction set with fused instructions, which [`BasicEagerInstructions::decode_fused()`]
/// produces and the tests below inspect
type FusedI = Rv64FusedInstruction<Reg<u64>>;

/// `addi a0, a1, 8`
const ADDI: u32 = 0x0085_8513;
/// `ld a0, -4(a0)`, which fuses with the `addi` above
const LD: u32 = 0xffc5_3503;
/// `ld a2, -4(a0)`, which does not: it leaves the value the `addi` wrote behind
const LD_OTHER_RD: u32 = 0xffc5_3603;

/// Stored in slots whose bytes do not decode
const FUSED_FALLBACK: FusedI = Rv64FusedInstruction::Unimp {
    rs1: Reg::ZERO,
    rs2: Reg::ZERO,
};

/// Decode `instructions` with fusion, at [`BASE_ADDR`] and with the return trap past the end
fn new_fused_instructions(instructions: &[u32]) -> BasicEagerInstructions<FusedI> {
    let code = instructions
        .iter()
        .flat_map(|instruction| instruction.to_le_bytes())
        .collect::<Vec<_>>();
    let return_trap_address = BASE_ADDR + code.len() as u64;

    // SAFETY: The instruction stream ends with a jump, the return trap is outside of it and the
    // base address is aligned
    unsafe {
        BasicEagerInstructions::decode_fused(&code, FUSED_FALLBACK, return_trap_address, BASE_ADDR)
    }
}

/// Instruction that the slot at `address` holds
fn slot_at(instructions: &BasicEagerInstructions<FusedI>, address: u64) -> FusedI {
    let memory = Memory::default();
    // SAFETY: Address of one of the decoded instructions
    let mut fetcher = unsafe { instructions.fetcher(address) };

    let FetchInstructionResult::Instruction(instruction) =
        InstructionFetcher::<FusedI, Memory>::fetch_instruction(&mut fetcher, &memory)
    else {
        panic!("Expected an instruction");
    };

    instruction
}

#[test]
fn fusion_replaces_the_first_instruction_of_a_pair_and_leaves_the_second_in_its_slot() {
    let instructions = new_fused_instructions(&[ADDI, LD, RET]);

    assert_eq!(
        slot_at(&instructions, BASE_ADDR),
        Rv64FusedInstruction::FusedAddiLd {
            rd: Reg::A0,
            rs1: Reg::A1,
            rs2: Reg::ZERO,
            imm: 8 - 4,
        }
    );
    // A branch may still target the second instruction of the pair directly, so it is executable
    // on its own exactly as it was
    assert_eq!(
        slot_at(&instructions, BASE_ADDR + 4),
        FusedI::try_decode(LD).expect("Valid instruction; qed")
    );
    assert_eq!(
        slot_at(&instructions, BASE_ADDR + 8),
        FusedI::try_decode(RET).expect("Valid instruction; qed")
    );
}

#[test]
fn a_pair_that_does_not_fuse_is_left_alone() {
    let instructions = new_fused_instructions(&[ADDI, LD_OTHER_RD, RET]);

    assert_eq!(
        slot_at(&instructions, BASE_ADDR),
        FusedI::try_decode(ADDI).expect("Valid instruction; qed")
    );
}

#[test]
fn a_fused_instruction_steps_over_the_whole_pair() {
    let instructions = new_fused_instructions(&[ADDI, LD, RET]);
    let memory = Memory::default();
    // SAFETY: This is the address of the first instruction
    let mut fetcher = unsafe { instructions.fetcher(BASE_ADDR) };

    let FetchInstructionResult::Instruction(_fused) =
        InstructionFetcher::<FusedI, Memory>::fetch_instruction(&mut fetcher, &memory)
    else {
        panic!("Expected an instruction");
    };

    // Fetching the fused instruction moved past both instructions it replaced rather than past the
    // first one only
    assert_eq!(
        ProgramCounter::<u64, Memory>::get_pc(&fetcher),
        BASE_ADDR + 8
    );
}

#[test]
fn fusion_only_looks_at_pairs_that_really_are_adjacent_instructions() {
    // The `addi` fuses with the `ld` that follows it, which leaves the second `ld` with nothing in
    // front of it to fuse with, since the walk resumes on the `ld` that was fused rather than on
    // the one after it
    let instructions = new_fused_instructions(&[ADDI, LD, LD, RET]);

    assert_eq!(
        slot_at(&instructions, BASE_ADDR + 8),
        FusedI::try_decode(LD).expect("Valid instruction; qed")
    );
}
