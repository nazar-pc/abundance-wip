extern crate alloc;

use ab_blake3::{CHUNK_LEN, OUT_LEN};
use ab_contract_file::instruction::{ContractInstruction, ContractRegister};
use ab_core_primitives::ed25519::{Ed25519PublicKey, Ed25519Signature};
use ab_io_type::bool::Bool;
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::hint::cold_path;
use core::mem::offset_of;
use core::ops::ControlFlow;
use core::ptr::NonNull;
use core::{ptr, slice};

/// Contract file bytes
pub const RISCV_CONTRACT_BYTES: &[u8] = cfg_select! {
    target_env = "abundance" => &[],
    _ => {
        include_bytes!(env!("CONTRACT_PATH"))
    }
};

// TODO: Generate similar helper data structures in the `#[contract]` macro itself, maybe introduce
//  `SimpleInternalArgs` data trait for this or something
/// Helper data structure for [`Benchmarks::blake3_hash_chunk()`] method
///
/// [`Benchmarks::blake3_hash_chunk()`]: crate::Benchmarks::blake3_hash_chunk
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct Blake3HashChunkInternalArgs {
    chunk_ptr: u64,
    chunk_size: u32,
    chunk_capacity: u32,
    result_ptr: u64,
    chunk: [u8; CHUNK_LEN],
    result: [u8; OUT_LEN],
}

impl Blake3HashChunkInternalArgs {
    /// Create a new instance
    pub fn new(internal_args_addr: u64, chunk: [u8; CHUNK_LEN]) -> Self {
        Self {
            chunk_ptr: internal_args_addr + offset_of!(Self, chunk) as u64,
            chunk_size: CHUNK_LEN as u32,
            chunk_capacity: CHUNK_LEN as u32,
            result_ptr: internal_args_addr + offset_of!(Self, result) as u64,
            chunk,
            result: [0; _],
        }
    }

    /// Extract result
    pub fn result(&self) -> [u8; OUT_LEN] {
        self.result
    }
}

// TODO: Generate similar helper data structures in the `#[contract]` macro itself, maybe introduce
//  `SimpleInternalArgs` data trait for this or something
/// Helper data structure for [`Benchmarks::ed25519_verify()`] method
///
/// [`Benchmarks::ed25519_verify()`]: crate::Benchmarks::ed25519_verify
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct Ed25519VerifyInternalArgs {
    pub public_key_ptr: u64,
    pub public_key_size: u32,
    pub public_key_capacity: u32,
    pub signature_ptr: u64,
    pub signature_size: u32,
    pub signature_capacity: u32,
    pub message_ptr: u64,
    pub message_size: u32,
    pub message_capacity: u32,
    pub result_ptr: u64,
    pub public_key: Ed25519PublicKey,
    pub signature: Ed25519Signature,
    pub message: [u8; OUT_LEN],
    pub result: Bool,
}

impl Ed25519VerifyInternalArgs {
    /// Create a new instance
    pub fn new(
        internal_args_addr: u64,
        public_key: Ed25519PublicKey,
        signature: Ed25519Signature,
        message: [u8; OUT_LEN],
    ) -> Self {
        Self {
            public_key_ptr: internal_args_addr + offset_of!(Self, public_key) as u64,
            public_key_size: Ed25519PublicKey::SIZE as u32,
            public_key_capacity: Ed25519PublicKey::SIZE as u32,
            signature_ptr: internal_args_addr + offset_of!(Self, signature) as u64,
            signature_size: Ed25519Signature::SIZE as u32,
            signature_capacity: Ed25519Signature::SIZE as u32,
            message_ptr: internal_args_addr + offset_of!(Self, message) as u64,
            message_size: OUT_LEN as u32,
            message_capacity: OUT_LEN as u32,
            result_ptr: internal_args_addr + offset_of!(Self, result) as u64,
            public_key,
            signature,
            message,
            result: Bool::new(false),
        }
    }

    /// Extract result
    pub fn result(&self) -> Bool {
        self.result
    }
}

/// Simple test memory implementation
#[derive(Debug, Copy, Clone)]
#[repr(align(16))]
pub struct TestMemory<const BASE_ADDR: u64, const SIZE: usize> {
    data: [u8; SIZE],
}

impl<const BASE_ADDR: u64, const SIZE: usize> VirtualMemory for TestMemory<BASE_ADDR, SIZE> {
    #[inline(always)]
    fn read<T>(&self, address: u64) -> Result<T, VirtualMemoryError>
    where
        T: BasicInt,
    {
        let offset = address.wrapping_sub(BASE_ADDR);

        if offset.saturating_add(size_of::<T>() as u64) > self.data.len() as u64 {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsRead { address });
        }

        // SAFETY: Only reading basic integers from initialized memory
        unsafe {
            Ok(self
                .data
                .as_ptr()
                .cast::<T>()
                .byte_add(offset as usize)
                .read_unaligned())
        }
    }

    #[inline(always)]
    unsafe fn read_unchecked<T>(&self, address: u64) -> T
    where
        T: BasicInt,
    {
        // SAFETY: Guaranteed by function contract
        unsafe {
            let offset = address.unchecked_sub(BASE_ADDR) as usize;
            self.data
                .as_ptr()
                .cast::<T>()
                .byte_add(offset)
                .read_unaligned()
        }
    }

    fn read_slice(&self, address: u64, len: u32) -> Result<&[u8], VirtualMemoryError> {
        let offset = address.wrapping_sub(BASE_ADDR);

        if offset > self.data.len() as u64 {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsRead { address });
        }

        self.data
            .get(offset as usize..)
            .and_then(|data| data.get(..len as usize))
            .ok_or(VirtualMemoryError::OutOfBoundsRead { address })
    }

    fn read_slice_up_to(&self, address: u64, len: u32) -> &[u8] {
        let offset = address.wrapping_sub(BASE_ADDR);

        if offset > self.data.len() as u64 {
            cold_path();
            return &[];
        }

        let remaining = self.data.get(offset as usize..).unwrap_or_default();
        remaining.get(..len as usize).unwrap_or(remaining)
    }

    #[inline(always)]
    fn write<T>(&mut self, address: u64, value: T) -> Result<(), VirtualMemoryError>
    where
        T: BasicInt,
    {
        let offset = address.wrapping_sub(BASE_ADDR);

        if offset.saturating_add(size_of::<T>() as u64) > self.data.len() as u64 {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsWrite { address });
        }

        // SAFETY: Only writing basic integers to initialized memory
        unsafe {
            self.data
                .as_mut_ptr()
                .cast::<T>()
                .byte_add(offset as usize)
                .write_unaligned(value);
        }

        Ok(())
    }

    fn write_slice(&mut self, address: u64, data: &[u8]) -> Result<(), VirtualMemoryError> {
        let offset = address.wrapping_sub(BASE_ADDR);

        if offset > self.data.len() as u64 {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsWrite { address });
        }

        let len = data.len();
        let Some(target_data) = self
            .data
            .get_mut(offset as usize..)
            .and_then(|data| data.get_mut(..len))
        else {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsWrite { address });
        };

        target_data.copy_from_slice(data);

        Ok(())
    }
}

impl<const BASE_ADDR: u64, const SIZE: usize> Default for TestMemory<BASE_ADDR, SIZE> {
    fn default() -> Self {
        Self { data: [0; SIZE] }
    }
}

impl<const BASE_ADDR: u64, const SIZE: usize> TestMemory<BASE_ADDR, SIZE> {
    /// Get a mutable slice of memory
    pub fn get_mut_bytes(
        &mut self,
        address: u64,
        size: usize,
    ) -> Result<&mut [u8], VirtualMemoryError> {
        let Some(offset) = address.checked_sub(BASE_ADDR) else {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsRead { address });
        };
        let offset = offset as usize;

        let Some(slice) = self
            .data
            .get_mut(offset..)
            .and_then(|data| data.get_mut(..size))
        else {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsRead { address });
        };

        Ok(slice)
    }
}

/// Lazy instruction fetcher implementation
#[derive(Debug, Copy, Clone)]
pub struct LazyInstructionFetcher {
    return_trap_address: u64,
    pc: u64,
}

impl<Memory> ProgramCounter<u64, Memory> for LazyInstructionFetcher
where
    Memory: VirtualMemory,
{
    #[inline(always)]
    fn get_pc(&self) -> u64 {
        self.pc
    }

    #[inline(always)]
    fn set_pc_relative(
        &mut self,
        memory: &Memory,
        instruction_size: u8,
        offset: i32,
    ) -> Result<ControlFlow<()>, ExecutionError<u64>> {
        let old_pc = <Self as ProgramCounter<_, Memory, _>>::old_pc(self, instruction_size);
        self.set_pc(memory, old_pc.wrapping_add_signed(i64::from(offset)))
    }

    #[inline]
    fn set_pc(&mut self, memory: &Memory, pc: u64) -> Result<ControlFlow<()>, ExecutionError<u64>> {
        if pc == self.return_trap_address {
            cold_path();
            return Ok(ControlFlow::Break(()));
        }

        if !pc.is_multiple_of(u64::from(
            ContractInstruction::<ContractRegister>::alignment(),
        )) {
            cold_path();
            return Err(ExecutionError::UnalignedInstruction {
                address: PackedAddress::new(pc),
            });
        }

        // Note: This will not allow reading a 16-bit instruction at the very end of memory range,
        // but that is going to be the case here anyway since code is followed by read-write memory
        // anyway
        if let Err(error) = memory.read::<u32>(pc) {
            cold_path();
            return Err(error.into());
        }

        self.pc = pc;

        Ok(ControlFlow::Continue(()))
    }
}

impl<Memory> InstructionFetcher<ContractInstruction, Memory> for LazyInstructionFetcher
where
    Memory: VirtualMemory,
{
    #[inline]
    fn fetch_instruction(
        &mut self,
        memory: &Memory,
    ) -> FetchInstructionResult<ContractInstruction> {
        // SAFETY: Constructor guarantees that the last instruction is a jump, which means going
        // through `Self::set_pc()` method does the necessary bounds check and advancing forward by
        // one instruction can't result in out-of-bounds access.
        let instruction = unsafe { memory.read_unchecked(self.pc) };
        // SAFETY: All instructions are valid, according to the constructor contract
        let instruction =
            unsafe { ContractInstruction::try_decode(instruction).unwrap_unchecked() };

        self.pc += u64::from(instruction.size());

        FetchInstructionResult::Instruction(instruction)
    }
}

impl LazyInstructionFetcher {
    /// Create a new instance.
    ///
    /// `return_trap_address` is the address at which the interpreter will stop execution
    /// (gracefully).
    ///
    /// # Safety
    /// The program counter must be valid and aligned, the instructions processed must be valid and
    /// end with a jump instruction.
    #[inline(always)]
    pub unsafe fn new(return_trap_address: u64, pc: u64) -> Self {
        Self {
            return_trap_address,
            pc,
        }
    }
}

/// Eager instruction handler eagerly decodes all instructions upfront
///
/// Instead of storing decoded instructions in a `Box<[ContractInstruction]>` alongside a pointer
/// into it, the allocation is owned directly through a raw pointer and freed/cloned manually.
/// `Box` asserts unique (`noalias`) access to its contents, so keeping a second, independently
/// derived pointer to the same data alive across `Box` accesses is unsound (Miri's aliasing model
/// flags it); owning the allocation as a raw pointer avoids that conflict entirely, while still
/// letting the position in the decoded stream be tracked as a plain pointer instead of a byte
/// offset that needs to be re-added to a base pointer on every fetch.
#[derive(Debug)]
#[repr(C, align(16))]
pub struct EagerTestInstructionFetcher {
    // The next instruction to be fetched: `fetch_instruction()` advances this past the just-read
    // instruction, so it never points at the instruction that was last fetched. A simple raw
    // pointer field also helps LLVM with SROA and aliasing analysis, so it can retain this pointer
    // in a native register instead of recomputing it from an offset on every fetch
    next_instruction: NonNull<ContractInstruction>,
    // Start of the owned allocation, used for bounds checks and to free/clone it
    instructions_ptr: NonNull<ContractInstruction>,
    instructions_len: usize,
    base_addr: u64,
    return_trap_address: u64,
}

impl Drop for EagerTestInstructionFetcher {
    fn drop(&mut self) {
        // SAFETY: `instructions_ptr`/`instructions_len` were created from `Box::into_raw()` on a
        // `Box<[ContractInstruction]>` of that exact length and are dropped here exactly once
        drop(unsafe {
            Box::from_raw(ptr::slice_from_raw_parts_mut(
                self.instructions_ptr.as_ptr(),
                self.instructions_len,
            ))
        });
    }
}

impl Clone for EagerTestInstructionFetcher {
    fn clone(&self) -> Self {
        // SAFETY: `instructions_ptr`/`instructions_len` describe a valid, initialized slice
        let instructions =
            unsafe { slice::from_raw_parts(self.instructions_ptr.as_ptr(), self.instructions_len) };
        let cloned_instructions = Box::<[ContractInstruction]>::from(instructions);
        // Preserve the offset of the next instruction within the cloned allocation
        let next_instruction_offset =
            // SAFETY: Both pointers are derived from the same allocation
            unsafe { self.next_instruction.offset_from(self.instructions_ptr) };

        let instructions_ptr = Box::into_raw(cloned_instructions).cast::<ContractInstruction>();
        // SAFETY: `Box::into_raw()` never returns a null pointer
        let instructions_ptr = unsafe { NonNull::new_unchecked(instructions_ptr) };
        // SAFETY: `next_instruction_offset` is in bounds of the original allocation, and the
        // cloned allocation has the same length
        let next_instruction = unsafe { instructions_ptr.offset(next_instruction_offset) };

        Self {
            next_instruction,
            instructions_ptr,
            instructions_len: self.instructions_len,
            base_addr: self.base_addr,
            return_trap_address: self.return_trap_address,
        }
    }
}

impl<Memory> ProgramCounter<u64, Memory> for EagerTestInstructionFetcher
where
    Memory: VirtualMemory,
{
    #[inline(always)]
    fn get_pc(&self) -> u64 {
        let decoded_instruction_byte_offset = self
            .next_instruction
            .as_ptr()
            .addr()
            .wrapping_sub(self.instructions_ptr.as_ptr().addr());

        self.base_addr
            + decoded_instruction_byte_offset as u64 * size_of::<u16>() as u64
                / size_of::<ContractInstruction>() as u64
    }

    /// Moves within the decoded stream instead of resolving an address and converting it back,
    /// which is what going through [`Self::set_pc()`] would do.
    ///
    /// One comparison and one test are all this needs to hand every case it does not handle itself
    /// over to [`Self::set_pc()`]: a target past the end of the decoded stream, a backwards branch
    /// that ran off its start, and an unaligned target. The return trap sits outside the decoded
    /// stream, so a branch to it fails the bounds check here and is stopped by [`Self::set_pc()`]
    /// like any other jump to it.
    #[inline(always)]
    fn set_pc_relative(
        &mut self,
        memory: &Memory,
        instruction_size: u8,
        offset: i32,
    ) -> Result<ControlFlow<()>, ExecutionError<u64>> {
        // Byte offset from the instruction being executed to the branch target. The program counter
        // is advanced during instruction fetching, so that instruction starts `instruction_size`
        // bytes back.
        let offset = offset as isize - isize::from(instruction_size);
        // Every `size_of::<u16>()` of guest code owns one decoded instruction, so the target is
        // reached by moving within the decoded stream
        let byte_delta =
            offset * (size_of::<ContractInstruction>() / size_of::<u16>()).cast_signed();
        // This may land outside the decoded stream (including before its start), which is fine:
        // `wrapping_byte_offset()` only computes an address, it never dereferences the pointer, and
        // the bounds check below rejects such a target before it is ever used
        let new_next_instruction = self
            .next_instruction
            .as_ptr()
            .wrapping_byte_offset(byte_delta);
        let decoded_instruction_byte_offset = new_next_instruction
            .addr()
            .wrapping_sub(self.instructions_ptr.as_ptr().addr());

        // A target that does not land on a decoded instruction sits between two guest
        // instructions, which makes it an unaligned instruction rather than something to round to
        // the start of one. That rule lives in `set_pc()`, so rather than restating it here, where
        // it could drift, such a target simply fails to qualify for the fast path, as does one past
        // the end of the decoded stream, which a backwards branch that ran off its start wraps
        // around into.
        if decoded_instruction_byte_offset
            >= self.instructions_len * size_of::<ContractInstruction>()
            || !decoded_instruction_byte_offset.is_multiple_of(size_of::<ContractInstruction>())
        {
            cold_path();
            let pc = <Self as ProgramCounter<_, Memory>>::get_pc(self);
            return self.set_pc(memory, pc.wrapping_add_signed(offset as i64));
        }

        // SAFETY: Just checked that `new_next_instruction` lands exactly on a decoded
        // instruction within the bounds of the (non-empty) decoded stream
        self.next_instruction = unsafe { NonNull::new_unchecked(new_next_instruction) };

        Ok(ControlFlow::Continue(()))
    }

    #[inline]
    fn set_pc(
        &mut self,
        _memory: &Memory,
        pc: u64,
    ) -> Result<ControlFlow<()>, ExecutionError<u64>> {
        let address = pc;

        if address == self.return_trap_address {
            cold_path();
            return Ok(ControlFlow::Break(()));
        }

        if !address.is_multiple_of(size_of::<u16>() as u64) {
            cold_path();
            return Err(ExecutionError::UnalignedInstruction {
                address: PackedAddress::new(address),
            });
        }

        let Some(offset) = address.checked_sub(self.base_addr) else {
            cold_path();
            return Err(ExecutionError::OutOfBoundsRead {
                address: PackedAddress::new(address),
            });
        };
        let offset = offset as usize;
        let instruction_offset = offset / size_of::<u16>();

        if instruction_offset >= self.instructions_len {
            cold_path();
            return Err(VirtualMemoryError::OutOfBoundsRead { address }.into());
        }

        // SAFETY: `instruction_offset` was just checked to be within bounds of the decoded stream
        self.next_instruction = unsafe {
            NonNull::new_unchecked(self.instructions_ptr.as_ptr().add(instruction_offset))
        };

        Ok(ControlFlow::Continue(()))
    }
}

impl<Memory> InstructionFetcher<ContractInstruction, Memory> for EagerTestInstructionFetcher
where
    Memory: VirtualMemory,
{
    #[inline(always)]
    fn fetch_instruction(
        &mut self,
        _memory: &Memory,
    ) -> FetchInstructionResult<ContractInstruction> {
        // SAFETY: Constructor guarantees that the last instruction is a jump, which means going
        // through `Self::set_pc()` method does the necessary bounds check and advancing forward by
        // one instruction can't result in out-of-bounds access.
        let instruction = unsafe { self.next_instruction.read() };
        let byte_advance =
            usize::from(instruction.size()) / size_of::<u16>() * size_of::<ContractInstruction>();
        // SAFETY: Same as above: advancing by one instruction from a valid position can't go out of
        // bounds
        self.next_instruction = unsafe {
            NonNull::new_unchecked(self.next_instruction.as_ptr().byte_add(byte_advance))
        };

        FetchInstructionResult::Instruction(instruction)
    }
}

impl EagerTestInstructionFetcher {
    /// Create a new instance with the specified instructions and base address.
    ///
    /// Instructions are decoded during instantiation of the instruction fetcher, and the base
    /// address corresponds to the first instruction.
    ///
    /// `return_trap_address` is the address at which the interpreter will stop execution
    /// (gracefully).
    ///
    /// # Safety
    /// The program counter must be valid and aligned, the instructions processed must end with a
    /// jump instruction, and `return_trap_address` must not fall inside them. Instruction fetching
    /// does not compare against the return trap, so an address inside the instructions would stop
    /// execution when jumped to but not when reached by falling through.
    #[inline(always)]
    pub unsafe fn new(
        instructions: &[u8],
        return_trap_address: u64,
        base_addr: u64,
        pc: u64,
    ) -> Self {
        let mut decoded_instructions: Vec<ContractInstruction> =
            Vec::with_capacity(instructions.len() / size_of::<u16>());

        let mut offset = 0;
        while let Some(instruction_bytes) = instructions.get(offset..offset + size_of::<u32>()) {
            let decoded_instruction = u32::from_le_bytes([
                instruction_bytes[0],
                instruction_bytes[1],
                instruction_bytes[2],
                instruction_bytes[3],
            ]);
            // Use `Unimp` as a fallback, though contract is expected to only contain legal
            // instructions
            let decoded_instruction = Instruction::try_decode(decoded_instruction).unwrap_or(
                ContractInstruction::Unimp {
                    rs1: Register::ZERO,
                    rs2: Register::ZERO,
                },
            );
            decoded_instructions.push(decoded_instruction);
            match decoded_instruction.size() {
                2 => {
                    offset += 2;
                }
                4 => {
                    // The second half of a 32-bit instruction is a valid offset and may or may not
                    // decode to a valid instruction on its own. Try to decode it but ignore
                    // decoding failures.

                    offset += 2;

                    // Could be both 16-bit and 32-bit instruction, need to handle end of the
                    // instruction stream
                    let instruction_word = if let Some(instruction_bytes) =
                        instructions.get(offset..offset + size_of::<u32>())
                    {
                        u32::from_le_bytes([
                            instruction_bytes[0],
                            instruction_bytes[1],
                            instruction_bytes[2],
                            instruction_bytes[3],
                        ])
                    } else {
                        u32::from_le_bytes([instruction_bytes[2], instruction_bytes[3], 0, 0])
                    };

                    decoded_instructions.push(Instruction::try_decode(instruction_word).unwrap_or(
                        ContractInstruction::Unimp {
                            rs1: Register::ZERO,
                            rs2: Register::ZERO,
                        },
                    ));
                    offset += 2;
                }
                instruction_size => {
                    unreachable!("Invalid instruction size {instruction_size}, expected 2 or 4");
                }
            }
        }

        let remainder_bytes = instructions.get(offset..).unwrap_or(&[]);

        if remainder_bytes.len() == size_of::<u16>() {
            let instruction_word =
                u32::from_le_bytes([remainder_bytes[0], remainder_bytes[1], 0, 0]);
            decoded_instructions.push(Instruction::try_decode(instruction_word).unwrap_or(
                ContractInstruction::Unimp {
                    rs1: Register::ZERO,
                    rs2: Register::ZERO,
                },
            ));
        }

        let instructions_len = decoded_instructions.len();
        let instructions_ptr =
            Box::into_raw(decoded_instructions.into_boxed_slice()).cast::<ContractInstruction>();
        // SAFETY: `Box::into_raw()` never returns a null pointer
        let instructions_ptr = unsafe { NonNull::new_unchecked(instructions_ptr) };

        let instruction_offset = (pc - base_addr) as usize / size_of::<u16>();
        // SAFETY: Constructor's contract guarantees `pc` is valid, meaning `instruction_offset` is
        // within bounds of the decoded stream
        let next_instruction = unsafe { instructions_ptr.add(instruction_offset) };

        Self {
            next_instruction,
            instructions_ptr,
            instructions_len,
            base_addr,
            return_trap_address,
        }
    }
}
