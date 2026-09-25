//! Counts what instruction fusion is actually worth, in dispatches rather than in seconds.
//!
//! Wall clock cannot resolve a change of a percent or two: the run-to-run spread of these
//! benchmarks on the same binaries is larger than that, and it changes sign between runs. The
//! number of instructions the interpreter dispatches does not move at all between runs, so that is
//! what this reports, next to how many adjacent pairs of the contract's instructions fuse at all.
//!
//! Run with `cargo run --release -p ab-riscv-benchmarks --example dispatch_count`.

use ab_blake3::OUT_LEN;
use ab_contract_file::ContractFile;
use ab_contract_file::instruction::{ContractInstruction, ContractRegisters};
use ab_core_primitives::ed25519::{Ed25519PublicKey, Ed25519Signature};
use ab_riscv_benchmarks::Benchmarks;
use ab_riscv_benchmarks::host_utils::{
    Blake3HashChunkInternalArgs, Ed25519VerifyInternalArgs, RISCV_CONTRACT_BYTES,
    UNDECODABLE_INSTRUCTION,
};
use ab_riscv_interpreter::basic::{
    BasicEagerInstructions, BasicInterpreterState, BasicMemory, CountingInstructionFetcher,
    IllegalEcallSystemInstructionHandler,
};
use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::{Instruction, Register};
use ed25519_dalek::{Signer, SigningKey};
use std::cell::Cell;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::{mem, ptr, slice};

const MEMORY_BASE_ADDRESS: u64 = 0x1000;
const TRAP_ADDRESS: u64 = 0;
const MEMORY_SIZE: usize = 128 * 1024;

enum RunType {
    Eager,
    EagerFused,
}

/// Decode the instructions of a contract, with or without fusing pairs of them
///
/// # Safety
/// Same as [`BasicEagerInstructions::decode()`]
unsafe fn decode(
    fused: bool,
    instructions: &[u8],
    fallback: ContractInstruction,
    return_trap_address: u64,
    base_addr: u64,
) -> BasicEagerInstructions<ContractInstruction> {
    if fused {
        // SAFETY: Guaranteed by function contract
        unsafe {
            BasicEagerInstructions::decode_fused(
                instructions,
                fallback,
                return_trap_address,
                base_addr,
            )
        }
    } else {
        // SAFETY: Guaranteed by function contract
        unsafe {
            BasicEagerInstructions::decode(instructions, fallback, return_trap_address, base_addr)
        }
    }
}

fn call_method<IA, CIA>(method_name: &str, create_internal_args: CIA, run_type: RunType) -> IA
where
    IA: Copy,
    CIA: FnOnce(u64) -> IA,
{
    let mut methods = HashMap::new();
    let contract_file = ContractFile::parse(RISCV_CONTRACT_BYTES, |contract_file_method| {
        methods.insert(
            contract_file_method.method_metadata_item.method_name,
            contract_file_method.address,
        );
        Ok(())
    })
    .unwrap();

    let mut memory = BasicMemory::<MEMORY_BASE_ADDRESS, MEMORY_SIZE>::default();

    let contract_memory_size = contract_file.contract_memory_size();
    if !contract_file.initialize_contract_memory({
        let output_memory = memory
            .get_mut_bytes(MEMORY_BASE_ADDRESS, contract_memory_size as usize)
            .unwrap();
        // SAFETY: Casting initialized memory into uninitialized memory of the same size is safe
        unsafe { mem::transmute::<&mut [u8], &mut [MaybeUninit<u8>]>(output_memory) }
    }) {
        panic!(
            "Failed to initialize contract memory of size {contract_memory_size} bytes at base \
            address 0x{MEMORY_BASE_ADDRESS:x}",
        );
    }

    let mut regs = ContractRegisters::<false>::default();

    // Internal arguments are the end of the memory region
    let internal_args_addr = MEMORY_BASE_ADDRESS + MEMORY_SIZE as u64 - size_of::<IA>() as u64;
    // Stack pointer must be 16-byte aligned, according to the psABI
    let stack_pointer = (internal_args_addr - 16).next_multiple_of(16);

    {
        let internal_args = create_internal_args(internal_args_addr);
        // SAFETY: Byte representation of `#[repr(C)]` without any padding, hence fully initialized
        let internal_args_bytes = unsafe {
            slice::from_raw_parts(ptr::from_ref(&internal_args).cast::<u8>(), size_of::<IA>())
        };

        memory
            .get_mut_bytes(internal_args_addr, size_of::<IA>())
            .unwrap()
            .copy_from_slice(internal_args_bytes);
    }

    regs.write(Register::A0, internal_args_addr);
    // Stack is between internal arguments and contract memory
    regs.write(Register::SP, stack_pointer);

    let pc = MEMORY_BASE_ADDRESS + u64::from(methods[method_name.as_bytes()]);
    let memory = match run_type {
        RunType::Eager | RunType::EagerFused => {
            // SAFETY: Contract code is trusted
            let instructions = unsafe {
                decode(
                    matches!(run_type, RunType::EagerFused),
                    contract_file.get_code(),
                    UNDECODABLE_INSTRUCTION,
                    TRAP_ADDRESS,
                    MEMORY_BASE_ADDRESS
                        + u64::from(contract_file.header().read_only_section_memory_size),
                )
            };
            // SAFETY: Program counter is trusted
            let instruction_fetcher =
                CountingInstructionFetcher::new(unsafe { instructions.fetcher(pc) });

            let mut state = BasicInterpreterState {
                regs,
                env: IllegalEcallSystemInstructionHandler,
                memory,
                instruction_fetcher,
            };
            state.execute().unwrap();

            record_dispatches(state.instruction_fetcher.dispatches());

            state.memory
        }
    };

    // SAFETY: Byte representation of `#[repr(C)]` without any padding, hence fully initialized
    *unsafe {
        memory
            .read_slice(internal_args_addr, size_of::<IA>() as u32)
            .unwrap()
            .as_ptr()
            .cast::<IA>()
            .as_ref_unchecked()
    }
}

thread_local! {
    /// Dispatches the last `call_method()` run performed
    static DISPATCHES_CELL: Cell<u64> = const { Cell::new(0) };
}

/// Remember what the run that has just finished dispatched
fn record_dispatches(dispatches: u64) {
    DISPATCHES_CELL.with(|cell| cell.set(dispatches));
}

/// Dispatches of the last run `record_dispatches()` saw
fn recorded_dispatches() -> u64 {
    DISPATCHES_CELL.with(Cell::get)
}

/// Decode the contract the way the interpreter does, so that pairs can be counted on what it sees
fn decode_contract() -> Vec<ContractInstruction> {
    let contract_file = ContractFile::parse(RISCV_CONTRACT_BYTES, |_| Ok(())).unwrap();
    let code = contract_file.get_code();

    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset < code.len() {
        let word = match code.get(offset..) {
            Some([byte_0, byte_1, byte_2, byte_3, ..]) => {
                u32::from_le_bytes([*byte_0, *byte_1, *byte_2, *byte_3])
            }
            Some([byte_0, byte_1, ..]) => u32::from_le_bytes([*byte_0, *byte_1, 0, 0]),
            _ => break,
        };

        match <ContractInstruction as Instruction>::try_decode(word) {
            Some(instruction) => {
                instructions.push(instruction);
                offset += usize::from(instruction.size());
            }
            None => {
                offset += usize::from(<ContractInstruction as Instruction>::ALIGNMENT);
            }
        }
    }

    instructions
}

/// The name of the variant an instruction is, which is what pairs are counted by
fn variant_name(instruction: ContractInstruction) -> String {
    let debug = format!("{instruction:?}");
    debug
        .split_once(' ')
        .map_or(debug.as_str(), |(name, _rest)| name)
        .to_string()
}

/// The value of a named register field of an instruction, if it has one
fn register_field(debug: &str, name: &str) -> Option<String> {
    let start = debug.find(&format!("{name}: "))? + name.len() + 2;
    let rest = debug.get(start..)?;
    let end = rest.find([',', ' ', '}']).unwrap_or(rest.len());
    Some(rest.get(..end)?.to_string())
}

/// How many adjacent pairs leave the first instruction's result dead, which is the ceiling for
/// any pairwise fusion, and which pairs those are
fn report_pairs(instructions: &[ContractInstruction]) {
    let fusable = instructions
        .windows(2)
        .filter(|pair| ContractInstruction::fuse(pair[0], pair[1]).0 != pair[0])
        .count();

    let mut histogram = HashMap::<(String, String), usize>::new();
    for pair in instructions.windows(2) {
        let previous = format!("{:?}", pair[0]);
        let next = format!("{:?}", pair[1]);

        let (Some(previous_rd), Some(rd), Some(rs1)) = (
            register_field(&previous, "rd"),
            register_field(&next, "rd"),
            register_field(&next, "rs1"),
        ) else {
            continue;
        };
        if previous_rd == "ZERO" || previous_rd != rd || previous_rd != rs1 {
            continue;
        }

        *histogram
            .entry((variant_name(pair[0]), variant_name(pair[1])))
            .or_default() += 1;
    }

    let dead = histogram.values().sum::<usize>();
    println!(
        "{} instructions, {fusable} of the adjacent pairs fuse, {dead} leave the first result dead",
        instructions.len()
    );

    let mut pairs = histogram.into_iter().collect::<Vec<_>>();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for ((previous, next), count) in pairs.iter().take(10) {
        let fused = instructions.windows(2).any(|pair| {
            variant_name(pair[0]) == *previous
                && variant_name(pair[1]) == *next
                && ContractInstruction::fuse(pair[0], pair[1]).0 != pair[0]
        });
        println!(
            "  {count:4}  {previous} + {next}{}",
            if fused { "  (fused)" } else { "" }
        );
    }
}

/// Dispatches one run of a method performs, with or without fusing pairs of instructions
fn count(method: &str, fused: bool) -> u64 {
    let run_type = if fused {
        RunType::EagerFused
    } else {
        RunType::Eager
    };

    match method {
        "blake3_hash_chunk" => {
            let data_to_hash = [1; _];
            let internal_args = call_method(
                "benchmarks_blake3_hash_chunk",
                |internal_args_addr| {
                    Blake3HashChunkInternalArgs::new(internal_args_addr, data_to_hash)
                },
                run_type,
            );
            assert_eq!(
                Benchmarks::blake3_hash_chunk(&data_to_hash),
                internal_args.result()
            );
        }
        "ed25519_verify" => {
            let signing_key = SigningKey::from([1; _]);
            let public_key = Ed25519PublicKey::from(signing_key.verifying_key());
            let message = [2; OUT_LEN];
            let signature = Ed25519Signature::from(signing_key.sign(&message));
            let internal_args = call_method(
                "benchmarks_ed25519_verify",
                |internal_args_addr| {
                    Ed25519VerifyInternalArgs::new(
                        internal_args_addr,
                        public_key,
                        signature,
                        message,
                    )
                },
                run_type,
            );
            assert!(internal_args.result.get());
        }
        _ => unreachable!("Only the methods asked for below"),
    }

    recorded_dispatches()
}

fn main() {
    report_pairs(&decode_contract());

    for method in ["blake3_hash_chunk", "ed25519_verify"] {
        let unfused = count(method, false);
        let fused = count(method, true);
        let saved = unfused - fused;
        println!(
            "{method}: {unfused} dispatches, {fused} fused, {saved} saved ({:.3}%)",
            saved as f64 * 100.0 / unfused as f64
        );
    }
}
