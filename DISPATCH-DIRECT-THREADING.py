#!/usr/bin/env python3
"""Apply the direct-threading experiment to the Coremark runner.

Run from the repository root. Reverse with `git checkout crates/`.
"""

# 1. `InstructionFetcher`: read the instruction without the `FetchInstructionResult` wrapper
#    (whose niche encoding costs a range check per dispatch), and read the handler stored
#    alongside it.
p = "crates/execution/ab-riscv-interpreter/src/lib.rs"
s = open(p).read()
old = "    unsafe fn advance(&mut self, instruction_size: u8);"
assert old in s
s = s.replace(
    old,
    old
    + '''

    /// EXPERIMENT (direct threading): read the instruction without the
    /// `FetchInstructionResult` wrapper, whose niche encoding otherwise costs a range check on
    /// every dispatch
    ///
    /// # Safety
    /// Only for fetchers whose position always holds a decoded instruction.
    unsafe fn peeked_instruction_raw(&self) -> I;

    /// EXPERIMENT (direct threading): handler stored next to the instruction just peeked
    ///
    /// # Safety
    /// Only valid between `peek_instruction()` and `advance()`, and only when the handler was
    /// filled in for every slot before execution started.
    #[inline(always)]
    unsafe fn peeked_handler(&self) -> *const () {
        ::core::ptr::null()
    }''',
)
open(p, "w").write(s)

# 2. `BasicInstructionFetcher` only needs to satisfy the trait, the experiment is wired up for
#    the Coremark fetcher alone.
p = "crates/execution/ab-riscv-interpreter/src/basic.rs"
s = open(p).read()
old = """{
    #[inline]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn peek_instruction(&mut self, memory: &Memory) -> FetchInstructionResult<I> {"""
assert old in s
s = s.replace(
    old,
    """{
    unsafe fn peeked_instruction_raw(&self) -> I {
        panic!("EXPERIMENT: direct threading is only wired up for the Coremark fetcher")
    }

    #[inline]
    #[cfg_attr(feature = "no-panic", no_panic_const::no_panic(const))]
    fn peek_instruction(&mut self, memory: &Memory) -> FetchInstructionResult<I> {""",
    1,
)
open(p, "w").write(s)

# 3. Coremark fetcher: 16-byte slot holding the instruction and its handler
p = "crates/execution/ab-riscv-coremark-runner/src/interpreter.rs"
s = open(p).read()
s = s.replace(
    "/// Instructions decoded upfront, which [`EagerInstructionFetcher`] walks",
    """/// EXPERIMENT (direct threading): decoded slot carrying the handler resolved at decode time
#[derive(Debug, Copy, Clone)]
#[repr(C)]
struct Slot {
    instruction: CoremarkInstruction,
    handler: *const (),
}

/// Instructions decoded upfront, which [`EagerInstructionFetcher`] walks""",
)
s = s.replace("NonNull<CoremarkInstruction>", "NonNull<Slot>")
s = s.replace("size_of::<CoremarkInstruction>()", "size_of::<Slot>()")
s = s.replace("align_of::<CoremarkInstruction>()", "align_of::<Slot>()")
s = s.replace("Layout::array::<CoremarkInstruction>(", "Layout::array::<Slot>(")
s = s.replace(".cast::<CoremarkInstruction>()", ".cast::<Slot>()")
s = s.replace(
    "        let instruction = unsafe { self.next_instruction.read() };",
    "        let instruction = unsafe { self.next_instruction.read() }.instruction;",
)
old = """        // SAFETY: The allocation was made for exactly this many instructions and is distinct from
        // the ones being copied in
        unsafe {
            instance.instructions().copy_from_nonoverlapping(
                NonNull::from(instructions).cast::<Slot>(),
                instructions_len,
            );
        }"""
assert old in s
s = s.replace(
    old,
    """        for (index, &instruction) in instructions.iter().enumerate() {
            // SAFETY: The allocation was made for exactly this many slots
            unsafe {
                instance.instructions().add(index).write(Slot {
                    instruction,
                    handler: ::core::ptr::null(),
                });
            }
        }""",
)
old = """    #[inline(always)]
    unsafe fn advance(&mut self, instruction_size: u8) {"""
assert old in s
s = s.replace(
    old,
    """    #[inline(always)]
    unsafe fn peeked_instruction_raw(&self) -> CoremarkInstruction {
        // SAFETY: Position always points at a decoded slot
        unsafe { (*self.next_instruction.as_ptr()).instruction }
    }

    #[inline(always)]
    unsafe fn peeked_handler(&self) -> *const () {
        // SAFETY: Position always points at a decoded slot
        unsafe { (*self.next_instruction.as_ptr()).handler }
    }

    #[inline(always)]
    unsafe fn advance(&mut self, instruction_size: u8) {""",
)
old = "    /// Create a fetcher positioned at the instruction that guest address `pc` corresponds to"
assert old in s
s = s.replace(
    old,
    """    /// EXPERIMENT (direct threading): resolve every decoded instruction to its handler once
    pub(super) fn fill_handlers(&mut self, handler_for: impl Fn(CoremarkInstruction) -> *const ()) {
        for index in 0..self.instructions_len() {
            // SAFETY: Index is within the decoded stream
            let slot = unsafe { self.instructions().add(index) };
            // SAFETY: Slot is initialized
            let instruction = unsafe { (*slot.as_ptr()).instruction };
            // SAFETY: Slot is initialized
            unsafe {
                (*slot.as_ptr()).handler = handler_for(instruction);
            }
        }
    }

"""
    + old,
)
open(p, "w").write(s)

# 4. Dispatch: take the handler from the slot instead of matching on the instruction, and drop
#    the `ThreadedDispatchResult` wrapper, whose niche encoding costs the same range check.
p = "crates/execution/ab-riscv-macros/src/build/execution_impl/generate_threaded_fns.rs"
s = open(p).read()
s = s.replace(
    '    let branch_failed_fn_name = format_ident!("{enum_snake_case}_threaded_branch_failed");',
    '    let branch_failed_fn_name = format_ident!("{enum_snake_case}_threaded_branch_failed");\n'
    '    let handler_for_fn_name = format_ident!("{enum_snake_case}_threaded_handler_for");',
)

old = """            let instruction = loop {
                match instruction_fetcher.peek_instruction(memory) {
                    FetchInstructionResult::Instruction(instruction) => {
                        break instruction;
                    }
                    FetchInstructionResult::Continue => {
                        ::core::hint::cold_path();
                    }
                    FetchInstructionResult::Break => {
                        ::core::hint::cold_path();
                        return #dispatch_result_name::Break;
                    }
                    FetchInstructionResult::Err(error) => {
                        ::core::hint::cold_path();
                        return #dispatch_result_name::Err(error);
                    }
                }
            };"""
assert old in s
s = s.replace(
    old,
    "            // EXPERIMENT (direct threading)\n"
    "            let instruction = unsafe { instruction_fetcher.peeked_instruction_raw() };",
)

old = """            #[expect(
                clippy::rest_pattern_accessible_field,
                reason = "Dispatch selects a handler by variant and never looks at any field"
            )]
            let handler = match instruction {
                #( #dispatch_arms )*
            };

            #dispatch_result_name::Next { instruction, handler }"""
assert old in s
s = s.replace(
    old,
    """            // EXPERIMENT (direct threading): the handler was resolved once, when the stream
            // was decoded, and lives next to the instruction
            // SAFETY: Filled in for every slot before execution starts
            let handler = unsafe {
                ::core::mem::transmute::<
                    *const (),
                    unsafe #abi fn(
                        #self_ty,
                        PC,
                        &mut Regs,
                        ExtState,
                        &mut Memory,
                        InstructionHandler,
                    ) -> #handler_result_ty,
                >(instruction_fetcher.peeked_handler())
            };

            (instruction, handler)""",
)

old = """        ) -> #dispatch_result_name<
            #self_ty,
            unsafe #abi fn(
                #self_ty,
                PC,
                &mut Regs,
                ExtState,
                &mut Memory,
                InstructionHandler,
            ) -> #handler_result_ty,
        >"""
assert old in s
s = s.replace(
    old,
    """        ) -> (
            #self_ty,
            unsafe #abi fn(
                #self_ty,
                PC,
                &mut Regs,
                ExtState,
                &mut Memory,
                InstructionHandler,
            ) -> #handler_result_ty,
        )""",
)

# `handler_for`, used once at decode time to fill the slots
anchor = """    generated_items.push(parse_quote! {
        #[expect(
            clippy::type_complexity,"""
assert anchor in s
s = s.replace(
    anchor,
    """    // EXPERIMENT (direct threading): resolve a variant to its handler once, at decode time
    generated_items.push(parse_quote! {
        #[inline(always)]
        #[expect(clippy::type_complexity, reason = "Experiment")]
        #[expect(
            clippy::rest_pattern_accessible_field,
            reason = "Dispatch selects a handler by variant and never looks at any field"
        )]
        pub(crate) fn #handler_for_fn_name<#generic_params>(
            instruction: #self_ty,
        ) -> unsafe #abi fn(
            #self_ty,
            PC,
            &mut Regs,
            ExtState,
            &mut Memory,
            InstructionHandler,
        ) -> #handler_result_ty
            #where_clause
        {
            match instruction {
                #( #dispatch_arms )*
            }
        }
    });

"""
    + anchor,
)

# Both call sites of the dispatch now take a tuple
import re

pattern = re.compile(
    r"( *)let \(instruction, handler\) = match #dispatch_fn_name::<#generic_params>\(\n"
    r"( *)&mut instruction_fetcher,\n"
    r" *memory,\n"
    r" *\) \{\n"
    r"(?:.*?\n)*?"
    r"\1\};"
)
s, n = pattern.subn(
    lambda m: (
        f"{m.group(1)}let (instruction, handler) = #dispatch_fn_name::<#generic_params>(\n"
        f"{m.group(2)}&mut instruction_fetcher,\n"
        f"{m.group(2)}memory,\n"
        f"{m.group(1)});"
    ),
    s,
)
assert n == 3, f"expected 3 dispatch call sites, patched {n}"
open(p, "w").write(s)

# 5. Fill the handlers in once, after decoding
p = "crates/execution/ab-riscv-coremark-runner/src/main.rs"
s = open(p).read()
s = s.replace(
    "use crate::interpreter::{EagerInstructions, GuestMemory};",
    "use crate::interpreter::{EagerInstructionFetcher, EagerInstructions, GuestMemory};",
)
old = """    // SAFETY: ELF was produced by a trusted compiler
    let instructions = unsafe { EagerInstructions::decode(text_data, TRAP_ADDRESS, text_addr) };"""
assert old in s
s = s.replace(
    old,
    """    // SAFETY: ELF was produced by a trusted compiler
    let mut instructions =
        unsafe { EagerInstructions::decode(text_data, TRAP_ADDRESS, text_addr) };
    // EXPERIMENT (direct threading)
    instructions.fill_handlers(|instruction| {
        crate::instruction::coremark_instruction_threaded_handler_for::<
            Reg<u64>,
            BasicRegisters<Reg<u64>, true>,
            &mut TimeCsrState,
            GuestMemory<MEMORY_BASE_ADDRESS, MEMORY_SIZE>,
            EagerInstructionFetcher<'_>,
            IllegalEcallSystemInstructionHandler,
        >(instruction) as *const ()
    });""",
)
open(p, "w").write(s)

print("direct-threading experiment applied")
