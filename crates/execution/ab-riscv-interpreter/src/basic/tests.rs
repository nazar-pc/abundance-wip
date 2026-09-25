use crate::RegisterFile;
use crate::basic::{BasicInterpreterState, BasicRegisters, CountingInstructionFetcher};
use crate::rv64::test_utils::initialize_state;
use ab_riscv_primitives::prelude::*;

#[test]
fn test_registers_read_write() {
    {
        // Basic read/write
        let mut regs = BasicRegisters::<Reg<u64>>::default();
        regs.write(Reg::A0, 0xdead_beef);
        assert_eq!(regs.read(Reg::A0), 0xdead_beef);
    }

    {
        // Write to multiple registers
        let mut regs = BasicRegisters::<Reg<u64>>::default();
        regs.write(Reg::A0, 100);
        regs.write(Reg::A1, 200);
        regs.write(Reg::T0, 300);

        assert_eq!(regs.read(Reg::A0), 100);
        assert_eq!(regs.read(Reg::A1), 200);
        assert_eq!(regs.read(Reg::T0), 300);
    }

    {
        // Overwrite register
        let mut regs = BasicRegisters::<Reg<u64>>::default();
        regs.write(Reg::A0, 100);
        regs.write(Reg::A0, 200);
        assert_eq!(regs.read(Reg::A0), 200);
    }

    {
        // Full 64-bit values
        let mut regs = BasicRegisters::<Reg<u64>>::default();
        regs.write(Reg::A0, u64::MAX);
        assert_eq!(regs.read(Reg::A0), u64::MAX);

        regs.write(Reg::A1, 0x0123_4567_89ab_cdef);
        assert_eq!(regs.read(Reg::A1), 0x0123_4567_89ab_cdef);
    }
}

#[test]
fn test_registers_zero_register() {
    {
        // Zero register always reads 0
        let regs = BasicRegisters::<Reg<u64>>::default();
        assert_eq!(regs.read(Reg::Zero), 0);
    }

    {
        // Writes to zero register are ignored
        let mut regs = BasicRegisters::<Reg<u64>>::default();
        regs.write(Reg::Zero, 0xdead_beef);
        assert_eq!(regs.read(Reg::Zero), 0);
    }

    {
        // Multiple writes to zero register
        let mut regs = BasicRegisters::<Reg<u64>>::default();
        regs.write(Reg::Zero, 100);
        regs.write(Reg::Zero, 200);
        regs.write(Reg::Zero, u64::MAX);
        assert_eq!(regs.read(Reg::Zero), 0);
    }
}

#[test]
fn test_registers_all_registers() {
    // Test all 32 registers can be written and read independently
    let mut regs = BasicRegisters::<Reg<u64>>::default();

    for i in 1..32 {
        let reg = Reg::from_bits(i).unwrap();
        regs.write(reg, u64::from(i) * 1000);
    }

    for i in 1..32 {
        let reg = Reg::from_bits(i).unwrap();
        assert_eq!(regs.read(reg), u64::from(i) * 1000, "Register {i} failed");
    }

    // Zero should still be zero
    assert_eq!(regs.read(Reg::Zero), 0);
}

#[test]
fn test_eregisters_read_write() {
    {
        // Basic read/write
        let mut regs = BasicRegisters::<_, false>::default();
        regs.write(EReg::<u64>::A0, 0xdead_beef);
        assert_eq!(regs.read(EReg::<u64>::A0), 0xdead_beef);
    }

    {
        // Write to multiple registers
        let mut regs = BasicRegisters::<_, false>::default();
        regs.write(EReg::<u64>::A0, 100);
        regs.write(EReg::<u64>::A1, 200);
        regs.write(EReg::<u64>::T0, 300);

        assert_eq!(regs.read(EReg::<u64>::A0), 100);
        assert_eq!(regs.read(EReg::<u64>::A1), 200);
        assert_eq!(regs.read(EReg::<u64>::T0), 300);
    }

    {
        // Overwrite register
        let mut regs = BasicRegisters::<_, false>::default();
        regs.write(EReg::<u64>::A0, 100);
        regs.write(EReg::<u64>::A0, 200);
        assert_eq!(regs.read(EReg::<u64>::A0), 200);
    }

    {
        // Full 64-bit values
        let mut regs = BasicRegisters::<_, false>::default();
        regs.write(EReg::<u64>::A0, u64::MAX);
        assert_eq!(regs.read(EReg::<u64>::A0), u64::MAX);

        regs.write(EReg::<u64>::A1, 0x0123_4567_89ab_cdef);
        assert_eq!(regs.read(EReg::<u64>::A1), 0x0123_4567_89ab_cdef);
    }
}

#[test]
fn test_eregisters_zero_register() {
    {
        // Zero register always reads 0
        let regs = BasicRegisters::<_, false>::default();
        assert_eq!(regs.read(EReg::<u64>::Zero), 0);
    }

    {
        // Writes to zero register are ignored
        let mut regs = BasicRegisters::<_, false>::default();
        regs.write(EReg::<u64>::Zero, 0xdead_beef);
        assert_eq!(regs.read(EReg::<u64>::Zero), 0);
    }

    {
        // Multiple writes to zero register
        let mut regs = BasicRegisters::<_, false>::default();
        regs.write(EReg::<u64>::Zero, 100);
        regs.write(EReg::<u64>::Zero, 200);
        regs.write(EReg::<u64>::Zero, u64::MAX);
        assert_eq!(regs.read(EReg::<u64>::Zero), 0);
    }
}

#[test]
fn test_eregisters_all_registers() {
    // Test all 16 registers can be written and read independently
    let mut regs = BasicRegisters::<_, false>::default();

    for i in 1..16 {
        let reg = EReg::<u64>::from_bits(i).unwrap();
        regs.write(reg, u64::from(i) * 1000);
    }

    for i in 1..16 {
        let reg = EReg::<u64>::from_bits(i).unwrap();
        assert_eq!(regs.read(reg), u64::from(i) * 1000, "Register {i} failed");
    }

    // Zero should still be zero
    assert_eq!(regs.read(EReg::<u64>::Zero), 0);
}

#[test]
fn test_counting_instruction_fetcher() {
    type I = Rv64Instruction<Reg<u64>>;

    /// Run `instructions` through a counting fetcher and report what it dispatched
    fn dispatches<Instructions>(instructions: Instructions) -> u64
    where
        Instructions: IntoIterator<Item = I>,
    {
        let state = initialize_state(instructions);
        let mut state = BasicInterpreterState {
            regs: state.regs,
            env: state.env,
            memory: state.memory,
            instruction_fetcher: CountingInstructionFetcher::new(state.instruction_fetcher),
        };
        state.execute::<I>().unwrap();

        state.instruction_fetcher.dispatches()
    }

    // Straight-line code dispatches every instruction once
    assert_eq!(
        dispatches([
            I::Addi {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::Zero,
                imm: 1,
            },
            I::Addi {
                rd: Reg::A0,
                rs1: Reg::A0,
                rs2: Reg::Zero,
                imm: 2,
            },
        ]),
        2
    );

    // A loop dispatches what it executes rather than what it is made of, which is the whole point
    // of counting rather than reading the instructions off
    assert_eq!(
        dispatches([
            I::Addi {
                rd: Reg::A0,
                rs1: Reg::Zero,
                rs2: Reg::Zero,
                imm: 3,
            },
            I::Addi {
                rd: Reg::A0,
                rs1: Reg::A0,
                rs2: Reg::Zero,
                imm: -1,
            },
            I::Bne {
                rs1: Reg::A0,
                rs2: Reg::Zero,
                imm: -4,
            },
        ]),
        1 + 3 * 2
    );
}
