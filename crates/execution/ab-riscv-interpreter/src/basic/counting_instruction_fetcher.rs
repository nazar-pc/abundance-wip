use crate::{ExecutionError, FetchInstructionResult, InstructionFetcher, ProgramCounter};
use ab_riscv_primitives::prelude::*;
use core::ops::ControlFlow;

/// An instruction fetcher that counts how many instructions were dispatched through it.
///
/// Wall clock cannot resolve a change that removes a fraction of a percent of the dispatches, and
/// the run-to-run spread of a benchmark is larger than that, so what a change of that size needs
/// is a count rather than a measurement. This wraps any other fetcher and counts every instruction
/// it hands out, which is exactly the number of dispatches a run performs.
///
/// Meant for measurement rather than execution: the counter is an ordinary field rather than an
/// atomic, but it is still a store on the hot path, and threaded dispatch does not go through a
/// fetcher method per instruction at all, so only the `match` loop can be counted this way.
#[derive(Debug, Clone)]
pub struct CountingInstructionFetcher<IF> {
    inner: IF,
    dispatches: u64,
}

impl<IF> CountingInstructionFetcher<IF> {
    /// Wrap a fetcher, starting the count at zero
    #[inline(always)]
    pub fn new(inner: IF) -> Self {
        Self {
            inner,
            dispatches: 0,
        }
    }

    /// How many instructions were dispatched so far
    #[inline(always)]
    pub fn dispatches(&self) -> u64 {
        self.dispatches
    }
}

impl<IF, Address, Memory> ProgramCounter<Address, Memory> for CountingInstructionFetcher<IF>
where
    IF: ProgramCounter<Address, Memory>,
    Address: Copy,
{
    #[inline(always)]
    fn get_pc(&self) -> Address {
        self.inner.get_pc()
    }

    #[inline(always)]
    unsafe fn try_set_pc_relative(&mut self, instruction_size: u8, offset: i32) -> bool {
        // SAFETY: Guaranteed by function contract
        unsafe { self.inner.try_set_pc_relative(instruction_size, offset) }
    }

    #[inline(always)]
    unsafe fn failed_branch(
        &mut self,
        memory: &Memory,
    ) -> Result<ControlFlow<()>, ExecutionError<Address>> {
        // SAFETY: Guaranteed by function contract
        unsafe { self.inner.failed_branch(memory) }
    }

    #[inline(always)]
    fn set_pc(
        &mut self,
        memory: &Memory,
        pc: Address,
    ) -> Result<ControlFlow<()>, ExecutionError<Address>> {
        self.inner.set_pc(memory, pc)
    }
}

impl<IF, I, Memory> InstructionFetcher<I, Memory> for CountingInstructionFetcher<IF>
where
    IF: InstructionFetcher<I, Memory>,
    I: Instruction,
{
    type Peeked = IF::Peeked;

    #[inline(always)]
    fn peek_instruction(&mut self, memory: &Memory) -> FetchInstructionResult<I, Self::Peeked> {
        self.inner.peek_instruction(memory)
    }

    #[inline(always)]
    fn peeked_instruction<'a>(&'a self, peeked: &'a Self::Peeked) -> &'a I {
        self.inner.peeked_instruction(peeked)
    }

    #[inline(always)]
    unsafe fn advance(&mut self, instruction_size: u8) {
        // SAFETY: Guaranteed by function contract
        unsafe { self.inner.advance(instruction_size) }
    }

    #[inline(always)]
    fn fetch_instruction(&mut self, memory: &Memory) -> FetchInstructionResult<I> {
        let result = self.inner.fetch_instruction(memory);
        if let FetchInstructionResult::Instruction(_) = &result {
            // Wrapping because a counter that panics would be a panic on the hot path, and a run
            // long enough to wrap it does not exist
            self.dispatches = self.dispatches.wrapping_add(1);
        }
        result
    }
}
