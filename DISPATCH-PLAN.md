# Implementation plan: generated per-instruction handlers

**Implemented.** Phases 1 and 2 are on `main`, merged in
[#766](https://github.com/nazar-pc/abundance-wip/pull/766); Phases 0, 3, 4 and 5 did not happen —
see §8 for what each of those means in practice.

Companion to `DISPATCH-HANDOFF.md`, which holds the measurements this is built on and which should
be read first.

This document is the plan as proposed, corrected in place. Everything the implementation did
differently is called out in a block quote beginning **"Correction"**, so the reasoning that led to
the original shape stays readable next to what replaced it. Where a section carries no such block,
it was built as written. §11 collects what came up during implementation that this plan did not
anticipate at all.

Code shown outside those blocks is the original sketch of intent, not what was generated. The
generated shape is in `crates/execution/ab-riscv-macros/src/build/execution_impl/generate_threaded_fns.rs`
and can be read post-expansion in `$OUT_DIR/{EnumName}_execution_impl.rs`.

## 0. The shape of the proposal in one paragraph

Keep `ExecutableInstruction::execute()` and its `match` emitter exactly as they are. Add a **second
emitter** that produces one handler function per variant plus a dispatch table, reached through one
new method — on a new `ThreadedExecutableInstruction` trait rather than on `ExecutableInstruction`,
for the reason in §3.4. Both emitters run by default, and the two paths coexist:
`match` where code size matters, handlers where throughput does.

Instruction bodies do not change — not one of the ~500 arms is rewritten. **Since `fb1eaaa` on
`main` they are already extracted into standalone per-variant functions**, so the handler emitter is
a third consumer of work that exists rather than something that has to re-derive it. §4 is now about
what wraps those functions, not about how to get them.

## 1. Why both paths, rather than a replacement

Measured on the CoreMark prototype, 147 variants, `zerostore`, x86-64:

| | `match` | handlers |
|---|---|---|
| text | 5104 B, one function | 9603 B, 148 functions |
| per variant | ~35 B | ~65 B |
| extrapolated to ~500 variants | ~17 KB | ~32 KB |
| cycles per guest instruction (Zen 4) | 11.34 | 6.14 |
| cost of an extension you never execute | +11% (89→147 arms) | none measured |
| toolchain | stable | needs `explicit_tail_calls` |

> **Correction.** The size and speed numbers in this table are the CoreMark prototype's and were
> never reproduced against the generated implementation — Phase 3 did not run. What was confirmed is
> the shape: in a release build of the interpreter's test binary every generated handler ends in an
> indirect `jmp` through a jump table (`jmp *(%rax,%r11,8)`) and contains no `call` at all, so
> `become` does emit real tail calls at the real variant count. Treat the ratios as the prototype's
> until `bench-dispatch.sh` says otherwise.

> **Correction on layout control.** "No `#[inline(always)]` on handler bodies" held: the generated
> handlers carry no inline attribute. `execute_threaded()` and the private `dispatch_*` function do
> carry `#[inline(always)]`, which is a different thing — neither is a link in the tail-call chain
> (see §4).

So handlers are **1.9x the code for 1.85x the speed**. That is a real trade, not a free win, and it
is why the `match` path stays: contract execution and anything size-sensitive should be able to keep
the compact one. The ratio is also better than it looks for the composed vector enum, because the
`match` cost is paid by every workload while the handler cost is paid only by the handlers actually
executed — 32 KB of handlers of which a workload touches 10 KB behaves like 10 KB, whereas 17 KB of
`match` behaves like 17 KB.

**Profile-guided optimisation is the other reason not to replace one with the other.** PGO
previously hurt because the hot body was one enormous function, which gives the optimiser nothing to
order. 500 small functions are individually addressable, so hot/cold splitting and function
reordering become possible. The emitter should therefore *not* try to control layout itself: emit
plain functions, no `#[inline(always)]` on handler bodies, no manual ordering, and let PGO or BOLT
place them. Worth measuring separately from the dispatch change itself, since the two are
independent.

## 2. What gets reused

The short answer is: all of it.

| trait | verdict | reasoning |
|---|---|---|
| `Instruction` | **reuse unchanged** | decode, size, alignment are orthogonal to dispatch |
| `ExecutionResult` | **reuse, and this is load-bearing** | it is what lets arms stay untouched — see §4 |
| `RegisterFile` | **reuse unchanged** | proven generic in the prototype at no cost |
| `VirtualMemory` | **reuse unchanged** | same; bounds checks stay, they are a correctness requirement |
| `ExecutableInstructionOperands` | **used by handlers after all** | see the correction below |
| `ExecutableInstruction` | **keep, untouched** | the compact path |
| `InstructionFetcher` / `ProgramCounter` | **reuse, unchanged** | see §3.1 |

> **Correction.** `ExecutableInstructionOperands` *is* used by handlers. Each handler opens with the
> whole-enum `instruction.get_rs1_rs2_operands()` and reads both source registers, exactly as the
> `match` loop does. That is not a regression to the shared preamble: the `let … else` destructure
> that follows it ends in `unreachable_unchecked()`, so the whole-enum match folds to the one
> variant's arm and dead-code elimination removes whichever reads the instruction does not use.
> Verified on release disassembly — the `Lui`, `Auipc`, `Jal`, `Fence`, `Ecall`, `Ebreak` and
> `Unimp` handlers contain no register-file read at all, while `Add` contains two.
The `&mut self` on `ProgramCounter`'s methods is not an obstacle, despite by-value having measured
1.66x faster. That measurement is about passing the program counter **as an argument** behind a
reference, which is a different thing: a handler takes the fetcher by value and calls `&mut self`
methods on its local copy, the reference never escapes, and the optimiser keeps the whole thing in
registers. §3.1 is the layout constraint that makes it true.

## 3. What is actually new

One method on `ExecutableInstruction` and one result type. No new traits at all.

### 3.1 The fetcher travels by value, and needs indirection to do it

> **Correction.** The by-value threading landed; the fetcher redesign did not. No `EagerFetcher`, no
> descriptor-plus-offset split, no 16-byte budget work — the existing `InstructionFetcher`
> implementations travel through the chain unchanged, whatever their size. This section is still the
> right thing to do and is now independent of the dispatch change: it can be done later without
> touching the emitter, and until it is, a large fetcher costs argument registers across the chain.
> The split between the reading half of the program counter (arms) and `set_pc`/`set_pc_relative`
> (the executor, here the handler epilogue) is honored as written.

The fetcher already holds the program counter, so thread the whole fetcher through the chain by
value rather than splitting out a position.

Sixteen bytes is the budget, and it does not stretch far. A real fetcher needs the decoded stream, a
base address to answer `get_pc`, a return trap address, and probably a second trap address for
future use — and `&[Instruction]` alone is already 16 bytes. So the by-value fetcher has to be a
**thin pointer to a heap- or stack-allocated descriptor, plus a position**, with the position as a
byte offset rather than a pointer:

```rust
#[derive(Copy, Clone)]
pub struct EagerFetcher<'a> {
    stream: &'a StreamInfo,   // instructions, base address, trap addresses
    offset: u64,              // byte offset into the stream
}
```

That costs an extra load for anything reached through the descriptor — the base address on
`AUIPC`/`JAL`, the trap addresses on `JALR` — which is acceptable because none of it is on the
common path. It also makes `Option<Self>` free: the reference gives the niche.

It wants to be at most 16 bytes, so that it fits in two argument registers. **That is not worth a
trait, or even a bound.** The size is a performance property rather than a correctness one — a
fetcher that grows past 16 bytes gets slower, not wrong — so anything existing only to assert it
would buy nothing but ceremony. If a specific fetcher's size needs pinning, that is a
`const { assert!(size_of::<EagerFetcher<'_>>() <= 16) }` next to its definition.

`Copy` is not needed either. The fetcher is moved into the handler and moved again into the tail
call, which is all threading it requires, exactly as with the owned state in §3.3.

**What handlers may do with the program counter.** Since the recent refactoring no instruction body
calls `set_pc()` — bodies describe control flow by returning `ExecutionResult` and the executor
applies it. Preserve that split explicitly: arms get the reading half (`get_pc`, `old_pc`), while
`set_pc` and `set_pc_relative` belong to the executor, which here is the generated handler prologue
rather than a loop.

### 3.2 The result type

What execution produces is an execution result, not a fetcher, and a handler that stops returns one
directly — the hidden out-pointer already gives it somewhere to write, so nothing extra has to be
threaded through the chain to carry a trap out.

```rust
pub struct ThreadedExecutionResult<IF, I, CustomError = CustomErrorPlaceholder>
where
    I: Instruction,
{
    /// Instruction fetcher as of the moment execution stopped
    pub instruction_fetcher: IF,
    /// Why execution stopped
    pub outcome: Result<(), ExecutionError<Address<I>, CustomError>>,
}
```

> **Correction.** It carries the **fetcher** back, not an address. The fetcher was moved into the
> chain by value, so something has to give it back or the caller loses it — `execute()` leaves the
> caller's fetcher in place and `execute_threaded()` has to end up equivalent. An address would not
> be enough to resume from. That adds a generic parameter (`IF`, first) and makes the struct larger
> than 16 bytes for any real fetcher, which the rest of this section already argued is the right
> trade: one out-pointer held across the chain, no per-instruction stores.
>
> `stopped()` and `failed()` constructors were added alongside it, the latter calling `cold_path()`,
> since every handler has four early-return sites that use them.

This does **not** need to be ≤16 bytes. It is returned once, from the outermost call; every handler
in between tail-calls and never writes to it. Being over 16 bytes means it returns through a hidden
out-pointer, which costs exactly one argument register held for the whole chain and **no
per-instruction stores** — a much better trade than contorting the type to fit two registers. With
`Control` gone there is nothing left that needed bundling.

### 3.3 The handler type, and where the registers went

Generic parameters are named and ordered exactly as
`ExecutableInstruction<Regs, ExtState, Memory, PC, InstructionHandler, CustomError>` already does,
with
`CustomError` appended and the instruction type first where one is needed — matching
`InstructionFetcher<I, Memory, CustomError>`. The emitter should not invent its own convention.

```rust
type Handler<I, Regs, ExtState, Memory, PC, InstructionHandler, CustomError> = fn(
    PC,                  // 2: fetcher by value, carries the program counter
    &mut Regs,           // 1
    ExtState,            // 0 when it is a ZST
    &mut Memory,         // 1
    InstructionHandler,  // 0 when it is a ZST
) -> ThreadedExecutionResult<I, CustomError>;   // 1 hidden out-pointer
```

**`ExtState` and `InstructionHandler` are owned rather than `&mut`.** This is the change that buys
the register budget back. An owned value can *be* a reference when there is state to reach, but it
can also be a zero-sized type when there is none — and a ZST argument occupies no register at all.
Most configurations have no extension state and an illegal-`ecall` handler with no fields, so both
vanish. The common case is therefore **five registers of six**: out-pointer, two for the fetcher,
one for registers, one for memory. Configurations that genuinely need state pay for exactly what
they need instead of a pointer whether or not there is anything behind it.

Neither needs to be `Copy`: a `&mut` moves into the handler and moves again into the tail call,
which is what threading means here.

> **Correction, and the one thing this forced elsewhere.** Owned `ExtState` and `InstructionHandler`
> landed as designed. What the plan did not anticipate is that a caller holding real state — such as
> `BasicInterpreterState`, which owns both — then has to pass `&mut` *as* the owned value, which
> only works if `&mut T` implements the same traits `T` does. So blanket forwarding implementations
> were added: `const impl … Csrs for &mut T` and `const impl … SystemInstructionHandler for &mut T`
> in `lib.rs`.
>
> `VectorRegisters` could not be forwarded that way. Its `read_vregs()` returns
> `&VectorRegisterFile<{ Self::VLEN }>`, and the compiler does not normalize
> `<&mut T as VectorRegisters>::VLEN` to `T::VLEN` while `Self` is generic — an associated const in
> const-argument position does not see through a generic impl. Enumerating the permutations does not
> help either: `Self = &mut T` stays generic, so it still fails to normalize, and multiple impls
> collide with E0119. The fallback is `impl_vector_registers_for_mut_ref!`, a macro that writes the
> forwarding impl for a concrete extension state type, mirroring the `impl_vector_registers!` hack
> the act4 runner already needed for the same reason. Filed upstream as
> [rust-lang/rust#161264](https://github.com/rust-lang/rust/issues/161264); both macros carry a TODO
> pointing at it.
>
> An earlier attempt at a wrapper type in `basic` holding a `&mut` was written and then deleted once
> the blanket implementations were shown to work. There is no `BorrowedState`.

### 3.4 The trait method

> **Correction — this is the largest divergence in the plan.** It is a **new trait**,
> `ThreadedExecutableInstruction`, not a method on `ExecutableInstruction`. The reason is hard:
> `ExecutableInstruction` is a `const trait`, and **calls through a function pointer are not allowed
> in `const fn`**. Dispatch is a table of function pointers, so `execute_threaded()` can never be
> `const`, and a trait cannot currently mix `const` and non-`const` methods.
>
> The new trait has the same generic parameters in the same order, requires
> `Self: ExecutableInstruction<…>` and `PC: InstructionFetcher<Self, Memory, CustomError>`, and
> declares exactly one method with the signature below (returning
> `ThreadedExecutionResult<PC, Self, CustomError>` per §3.2). Its generated `impl` relaxes the
> enum's `[const]` bounds to ordinary ones, since nothing about it is `const`. Its doc comment says
> so and says the method should move into `ExecutableInstruction` if mixing ever becomes possible.
>
> The rest of this section stands: no second generic parameter, the `where` bound supplies the
> fetching half, and the argument stays `instruction_fetcher` while the type parameter stays `PC`.

No new generic parameter.
`ExecutableInstruction<Regs, ExtState, Memory, PC, InstructionHandler, CustomError>` already carries
everything this needs, so a parallel trait declares one method:

```rust
fn execute_threaded(
    instruction_fetcher: PC,
    regs: &mut Regs,
    ext_state: ExtState,
    memory: &mut Memory,
    system_instruction_handler: InstructionHandler,
) -> ThreadedExecutionResult<Self, CustomError>
where
    PC: InstructionFetcher<Self, Memory, CustomError>;
```

No new generic parameter: the trait's `PC` is the same value, and the method simply requires more of
it. `execute()` only ever touches the program counter, so the trait names the parameter for the
smaller role; `execute_threaded` needs the fetching half too, and a `where` bound is enough to say
so. That leaves the argument named `instruction_fetcher` while the type parameter is called `PC`,
which is mildly inconsistent and much less trouble than a parallel generic would be.

**This is the entire public surface.** No `handler()`, no `HANDLERS`, no exposed table.

> **Correction on where the generated items live.** They are not nested inside `execute_threaded()`.
> Nested items cannot be referred to from a sibling item, and the handlers have to name each other
> (every handler reaches dispatch, and dispatch names all of them), so they are emitted as
> private free items in the generated file alongside the per-variant execution functions: one
> `execute_{enum}_{variant}_threaded` per variant, one `dispatch_{enum_snake}`, and one private
> `{Enum}ThreadedDispatchResult` enum. None of them is exported; the trait method is still the whole
> surface. The sketch's point about nested items not inheriting the enclosing generics applies
> unchanged — every generated item repeats the full generic parameter list and every call site uses
> an explicit turbofish.

The sketch as proposed:

```rust
fn execute_threaded(..) -> ThreadedExecutionResult<..> {
    // Nested items do not inherit the enclosing generics, so each is generated with its own
    fn Add<Regs, ExtState, Memory, PC, InstructionHandler, CustomError>(..)
        -> ThreadedExecutionResult<..> { .. }
    fn Sub<..>(..) -> .. { .. }
    // ... one per variant, all private to this function ...

    let handler = match instruction {
        Self::Add { .. } => Add::<Regs, ExtState, Memory, PC, InstructionHandler, CustomError>,
        Self::Sub { .. } => Sub::<..>,
        // ... generated exhaustively, no catch-all ...
    };
    become handler(instruction_fetcher, regs, ext_state, memory, system_instruction_handler)
}
```

**Matching on the instruction, not on a discriminant.** There is no need for the enum to expose its
discriminant as a number: a `match` over the variants says the same thing, and LLVM lowers a dense
match returning distinct function addresses to a discriminant load and a lookup table on its own.
Matching on the instruction also touches only the discriminant, so it narrows to a two-byte load
rather than a full decode. Being exhaustive with no catch-all means a new extension is a
compile error rather than a silent gap, which is the property the prototype's `ops!` table already
relies on.

A `match` rather than a table constant for a second reason too: a `const` item inside a function
cannot reference that function's generics, while a `match` arm can. Whether LLVM really produces the
lookup table at ~500 arms is worth verifying, and an associated constant on a `PhantomData` carrier
is the fallback if it does not.

> **Correction — verified, and it does.** At the real variant count the dispatch `match` lowers to a
> jump table: the tail call in every generated handler is a single `jmp *(%rax,%r11,8)` against a
> table of handler addresses. The `PhantomData` fallback is not needed. The match arms are
> `Enum::Variant { .. } => handler::<…>`, generated exhaustively with no catch-all, so a new
> extension is a compile error rather than a silent gap, as intended.

Keeping the surface this small is also what makes §7's ABI question survivable: if handlers ever
need an exotic calling convention, `execute_threaded` is the only thing that has to keep its
signature.

## 4. What the emitter generates

`fb1eaaa` on `main` did the expensive half already. `generate_variant_fns` splits the composed
`execute()` into one `#[inline(always)]` function per variant and replaces each match arm body with
a call to it, so the arms are standalone functions today and the `match` is a dispatcher over them.
Those functions are exactly what a handler needs to call.

The generated shape, read out of `CoremarkInstruction_execution_impl.rs`:

```rust
#[inline(always)]
fn execute_coremark_instruction_add<
    Reg, Regs, ExtState, Memory, PC, InstructionHandler, CustomError,
>(
    rd: Reg,                  // variant-specific, from the arm's pattern — only what it binds
    rs1_value: ..., rs2_value: ...,
    regs: &mut Regs,
    ext_state: &mut ExtState,
    memory: &mut Memory,
    program_counter: &mut PC,
    system_instruction_handler: &mut InstructionHandler,
) -> ExecutionResult<..., CustomError>
```

Naming is `execute_{enum_snake}_{variant_snake}`, functions are private to the generated file, and
the variant-specific parameters differ per variant (`add` binds `rd`; `addi` and `lui` bind `rd` and
`imm`; `sd` and `beq` bind only `imm`). They carry `#[inline(always)]`, the enum's `const`ness, and
the `no_panic` attribute where it applies — all three of which the handler emitter must preserve
rather than fight.

**Shared parameters, and the assumption the design rests on.** The shared half is taken once from
`execute()`'s signature, so every generated function takes `rs1_value` and `rs2_value` whether or
not it uses them — `lui` takes both and uses neither. That costs the `match` loop nothing, since it
reads both source registers anyway, but a handler has to read both to make the call, which is the
universal operand preamble of §2 of the notes.

`#[inline(always)]` is what removes it, and that is why it is there: once inlined, an unused
`regs.read(rs2)` in the caller is a pure load with a dead result and is eliminated. **This is taken
as given rather than listed as a risk** — it is deliberate in `fb1eaaa`'s design. It is recorded
here because it is load-bearing: if handler numbers ever fail to reproduce the prototype's, a
preamble that came back is the first thing to check, and the fix would be per-variant shared
parameters in `generate_variant_fns` rather than anything in this plan.

**On parameter naming.** `ExecutableInstruction` takes `PC` in fourth position because instruction
bodies only touch the program counter and never fetch. `execute_threaded` needs the fetching half as
well, which a `where PC: InstructionFetcher<..>` bound on the method supplies without a second
generic parameter — see §3.4.

So the handler no longer embeds the arm. It destructures the instruction, calls the generated
function, applies the `ExecutionResult` it returns, and tail-calls.

> **Correction — the handler does not fetch, and dispatch is not a link in the chain.** The sketch
> below has each handler fetch its own next instruction and `become dispatch(…)`, with `dispatch`
> doing `become handler(…)`. That cannot be built: **`become` requires the caller's and the callee's
> signatures to match exactly**, and a handler necessarily takes the instruction it is about to
> execute while dispatch necessarily does not. Written that way it produces thousands of mismatched-
> signature errors.
>
> What was built instead splits the two roles:
>
> * **The instruction is an argument.** A handler's first parameter is the already-decoded
>   instruction; it never fetches for itself. Its signature is
>   `fn(I, PC, &mut Regs, ExtState, &mut Memory, InstructionHandler) -> ThreadedExecutionResult<…>`,
>   and every handler has exactly that signature, which is what makes `become` legal between any
>   two of them.
> * **`dispatch_{enum}` is `#[inline(always)]` and returns.** It fetches, selects, and hands back
>   both halves through a private `{Enum}ThreadedDispatchResult<I, Handler, CustomError>` —
>   `Next { instruction, handler }`, `Break`, or `Err(…)`. The caller then does the `become` itself.
>   Being inlined and non-escaping, that enum never materializes; it is `FetchInstructionResult`
>   with the handler attached and the `Continue` variant resolved away.
> * **`FetchInstructionResult::Continue` is a loop inside dispatch**, not a re-entry into the chain,
>   which is what lets dispatch return a value rather than being a tail call.
> * **`execute_threaded()` is the one ordinary call in the whole chain.** It runs dispatch once and
>   calls the first handler normally; from there nothing returns until execution stops.
>
> The rest of the sketch is accurate: the `let … else` with `unreachable_unchecked()`, the
> `&mut instruction_fetcher` handed to the arm, the `ExecutionResult` applied in the epilogue, and
> the moves of the fetcher, extension state and system handler into the tail call.

The sketch as proposed:

```rust
// Private inside `execute_threaded`, generated once per variant
fn Add<Regs, ExtState, Memory, PC, InstructionHandler, CustomError>(
    mut instruction_fetcher: PC,
    regs: &mut Regs,
    ext_state: ExtState,
    memory: &mut Memory,
    system_instruction_handler: InstructionHandler,
) -> ThreadedExecutionResult<I, CustomError>
where /* the same bounds `execute()` has today, plus `PC: InstructionFetcher<..>` */
{
    // Advances the fetcher past this instruction, exactly as the `match` loop's fetch does
    let instruction = match instruction_fetcher.fetch_instruction(memory) {
        FetchInstructionResult::Instruction(instruction) => instruction,
        FetchInstructionResult::Continue => {
            become dispatch(
                instruction_fetcher, regs, ext_state, memory, system_instruction_handler,
            )
        }
        FetchInstructionResult::Break => {
            return ThreadedExecutionResult::stopped(&instruction_fetcher);
        }
        FetchInstructionResult::Err(error) => {
            return ThreadedExecutionResult::failed(&instruction_fetcher, error);
        }
    };
    let I::Add { rs1, rs2, rd } = instruction else {
        // SAFETY: this handler is only reachable for its own discriminant
        unsafe { unreachable_unchecked() }
    };

    // Takes both source values whether or not it uses them; the unused read is inlined away
    let Rs1Rs2Operands { rs1, rs2 } = instruction.get_rs1_rs2_operands();

    // The arm may take `&mut` to this; it never escapes, so it stays in registers
    let program_counter = &mut instruction_fetcher;

    match execute_coremark_instruction_add::<..>(
        rd,
        regs.read(rs1),
        regs.read(rs2),
        regs,
        ext_state,
        program_counter,
        system_instruction_handler,
    ) {
        ExecutionResult::Continue { rd, value } => regs.write(rd, value),
        ExecutionResult::ContinueNoWrite => {}
        ExecutionResult::Branch { offset } => {
            match program_counter.set_pc_relative(memory, instruction.size(), offset) {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(())) => {
                    return ThreadedExecutionResult::stopped(&instruction_fetcher);
                }
                Err(error) => {
                    return ThreadedExecutionResult::failed(&instruction_fetcher, error);
                }
            }
        }
        ExecutionResult::Jump { target } => { /* set_pc, same shape */ }
        ExecutionResult::Break => return ThreadedExecutionResult::stopped(&instruction_fetcher),
        ExecutionResult::Err(error) => {
            return ThreadedExecutionResult::failed(&instruction_fetcher, error);
        }
    }

    become dispatch(instruction_fetcher, regs, ext_state, memory, system_instruction_handler)
}

/// Selects the handler for the instruction at the fetcher's position and tail-calls it.
/// Also generated, also private, and the same code `execute_threaded` uses to enter the chain.
#[inline(always)]
fn dispatch<..>(
    instruction_fetcher: PC,
    regs: &mut Regs,
    ext_state: ExtState,
    memory: &mut Memory,
    system_instruction_handler: InstructionHandler,
) -> ThreadedExecutionResult<I, CustomError> {
    let handler = match instruction_fetcher.peek_instruction() {
        Self::Add { .. } => Add::<Regs, ExtState, Memory, PC, InstructionHandler, CustomError>,
        Self::Sub { .. } => Sub::<..>,
        // ... generated exhaustively, no catch-all ...
    };
    become handler(instruction_fetcher, regs, ext_state, memory, system_instruction_handler)
}
```

Note `let program_counter = &mut instruction_fetcher;`. This is the whole reason no API had to
change: the arm
sees the same `&mut` it sees today, the reference never escapes the handler, and the fetcher itself
travelled here in registers. The `match` loop and a handler present arms with identical interfaces.

> **Correction.** No separate `let` binding is generated. The reborrows are written directly at the
> call site (`&mut ext_state`, `&mut instruction_fetcher`, `&mut system_instruction_handler`) and
> binding the returned `ExecutionResult` to a local ends them before the epilogue reads the fetcher
> again. The property this paragraph describes is unchanged — the references never escape and the
> values stay in registers.

Note also that `instruction_fetcher`, `ext_state` and `system_instruction_handler` are moved into
the tail call rather than reborrowed. That is what threading owned values means, and it is why none
of them needs to be `Copy`.

**The `ExecutionResult` in the middle costs nothing.** It is a local that never escapes, so it
scalarises away entirely. Checked by building enum-mediated and hand-written versions of three
handlers — a tiny `add`, a `beq` whose arm picks between two variants at run time, and a large
16-lane multiply-accumulate standing in for a vector instruction. Each pair came out
**byte-identical**, folded to a single linked address, at both `-O` and
`-C opt-level=3 -C target-cpu=znver4`: no stack frames, no spilled discriminant, no `call` anywhere,
and `become` emitting a real indirect `jmp` every time. The large handler still auto-vectorised to
AVX-512 with the enum in the middle. That was a model with its own enum and a plain `[u64; 32]`
register file, so it shows the shape does not block the optimiser rather than proving the emitter's
own output will fold.

**One `become` per handler, at the end — never one inside each match arm.** (Honored: every
generated handler has exactly one `become`, after the `ExecutionResult` epilogue and the dispatch
step, with the four stopping paths taking early returns.) This is a hard
requirement on the emitter, and it is the one place the fold was observed to break. When the tail
dispatch is duplicated into every arm, a handler whose arm always yields the same variant is still
fine, because the optimiser deletes the unreachable arms and one `become` survives. But `beq`, whose
arm genuinely selects between two variants at run time, then has two independent tail-call exits,
and the pass that would turn the choice into a branchless `cmov` will not hoist across two separate
`become` sites. The result was 20 instructions with a real conditional branch instead of 16
branchless ones. Hoisting the `become` out of the match — arms produce a value or return early, and
one shared tail call follows — closed the gap and made it byte-identical again. The sketch above has
this shape deliberately.

Three consequences worth stating plainly:

- **~500 instruction bodies stay exactly as written.** One source of truth, and the `match` emitter
  and the handler emitter consume the same input. Instruction authors notice nothing. **This held.**
  Not one instruction body was touched. The only macro-side consequence is that `execute()`'s
  processed signature is now normalized before anything is derived from it — a leading `_` is
  stripped off arguments and wildcard-discarded destructured fields are bound by name — so every
  generated function takes the same argument names whether or not the implementation it came from
  had a use for them. Implementations still write `_memory` and `rs1_value: _` and are unaffected.
- **`ExecutionResult` must fold away.** It is a local enum that never escapes, so LLVM should
  scalarise it and leave no materialised value — the same argument that makes the prototype's `Env`
  struct free. If it does *not* fold, every instruction pays a tax and the plan needs rework. This
  is risk #1 in §7 and is the first thing to check.
- **The non-writing path needs to be visible in the type, not inferred at codegen time.** Today
  `ExecutionResult::ContinueNoWrite` makes this a matter of the result's discriminant rather than of
  value analysis, so a handler that never writes contains no register write and one that
  conditionally writes says so by returning one variant or the other. Without it, whether the
  discarded `write(x0, 0)` folds away depends entirely on the register file — with a branching one
  it vanishes, with an unconditional one it becomes a real store to slot 0 on every non-writing
  instruction.

## 5. There is no separate driver

`execute_threaded` is the entry point, the dispatcher and the trait method, all generated. A caller
does:

```rust
let result = CoremarkInstruction::execute_threaded(
    instruction_fetcher, &mut regs, ext_state, &mut memory, system_instruction_handler,
);
```

`BasicInterpreterState` keeps `execute()` and gains a parallel method that unpacks its fields into
that call, so a caller picks a path by which method it calls and nothing else changes.

> **Correction, minor.** Both held, with the entry point on the new trait rather than on
> `ExecutableInstruction` (§3.4). `BasicInterpreterState::execute_threaded()` exists and mirrors
> `execute()`'s `replace_with_or_abort_and_return` shape, taking the fetcher out and putting the
> returned one back. Because that state owns its extension state and system instruction handler, it
> passes `&mut` to each and so is bounded on
> `for<'a> ThreadedExecutableInstruction<Regs, &'a mut ExtState, …, &'a mut InstructionHandler>`. A
> caller whose two are zero-sized should call `I::execute_threaded()` directly and save the two
> argument registers; the method's doc says so.

## 6. When handlers are generated

Handlers are generated **by default**, not opted into, and generation belongs to the
instruction-execution macro rather than to the macro that declares the enum. The enum declaration
expresses the RISC-V specification — which instructions exist and what they inherit — and says
nothing about how execution happens; that is a separate macro's job, and it is the one that already
consumes the arms.

Default-on is safe because unused generated code costs nothing. Handlers are generic over the
register file, memory, fetcher and system handler, so a handler is only instantiated when something
actually calls it. A crate that only ever calls `execute()` never monomorphises a single handler and
never materialises the table, so it pays the `match` price and nothing else — no feature flag, no
`--gc-sections` dependency, no configuration for a caller to get wrong. The §1 size comparison is
therefore between two paths a given binary chooses between, not two costs it pays at once.

The one thing to confirm early is that this really holds for the `HANDLERS` constant. A `const` that
is never read should not be emitted, but a table of function pointers is exactly the shape that can
accidentally keep every handler alive if something forces it. Check with `nm` on a binary that uses
only the `match` path; it belongs with the other Phase 0 spikes.

> **Correction.** Default-on generation landed as described, in the instruction-execution macro. The
> `HANDLERS` constant does not exist — there is no table constant to keep anything alive, only a
> `match` inside an `#[inline(always)]` dispatch function (§3.4), so a crate that never calls
> `execute_threaded()` never monomorphizes a handler and the question does not arise in the form it
> is posed here. Not separately checked with `nm` on a `match`-only binary.
>
> One thing this section did not anticipate: `explicit_tail_calls` is a crate-level feature, so
> every crate that *instantiates* the generated code has to enable it, not just the crate that
> generates it. `ab-contract-file`, `ab-riscv-act4-runner` and `ab-riscv-coremark-runner` each
> gained `#![feature(explicit_tail_calls)]` and the matching `expect(incomplete_features)` for that
> reason alone, without using the threaded path.

## 7. Risks, in the order they should be retired

1. **Does the fetcher fit in 16 bytes with everything a handler needs reachable?** A thin pointer
   plus a byte offset does, at the cost of a load for anything behind the descriptor. The open part
   is whether that load lands anywhere hot — `AUIPC`, `JAL` and `JALR` all need the base address,
   and jumps are ~15% of the mix. If it hurts, the base address is the field to promote into the
   fetcher itself, which then needs the offset narrowed to `u32`.

2. **Does a dense `match` returning function pointers lower to a lookup table?** ~500 arms returning
   distinct function addresses should hit LLVM's switch-to-lookup-table pass. Verify on the real
   variant count rather than a toy one, since the pass has heuristics. Fallback is an associated
   constant on a `PhantomData` carrier, which the prototype already proved works.

3. **Can `ThreadedExecutionResult` be made ≤16 bytes, and does `become` accept a vector return
   type?** The first decides whether §9's SIMD return is needed at all; the second decides whether
   it is usable, since `become` requires an exact ABI match and a `__m256i` return has not been
   tried with it.

4. **~500 variants.** Compile time, text size, and the case the whole exercise exists to prove,
   since it is where `match` degrades and handlers should not.

Three things are taken as given rather than listed: `become` works through a generated trait method,
an unused table in a `match`-only crate costs compile time and nothing else, and `ExecutionResult`
stays ≤16 bytes with `ContinueNoWrite` in it.

> **Correction — where the four risks actually stand.** None of these was retired as a Phase 0 spike;
> they were resolved, or not, by building the thing.
>
> 1. **Fetcher in 16 bytes.** Not addressed — §3.1 was not implemented, so this is still open and
>    still worth doing.
> 2. **Dense `match` → lookup table.** **Retired.** It does, at the real variant count; see §3.4.
> 3. **`ThreadedExecutionResult` ≤16 bytes / vector return.** **Moot.** The type carries the fetcher
>    back (§3.2), so it is over 16 bytes by construction and returns through a hidden out-pointer.
>    §9's SIMD return was never needed and was not tried.
> 4. **~500 variants.** **Builds and passes.** The interpreter's 1708 tests pass on both paths, and
>    dedicated tests assert the two paths agree instruction-for-instruction on registers, memory,
>    program counter and error. Compile time and text size at that scale were not measured.
>
> Of the three things "taken as given": `become` through a generated trait method works, and
> `ExecutionResult` folding away is confirmed by the handler disassembly. The unused-table question
> is answered in §6 — there is no table.
>
> **One risk this section missed.** `become` requires an exact signature match between caller and
> callee. That constraint, not any of the four above, is what reshaped §4's dispatch.

## 8. Sequencing

- **Phase 0** — the four risks above. Stop and re-plan if 1 or 2 fails.
- **Phase 1** — `ThreadedExecutionResult`, plus one *hand-written* `execute_threaded` for a single
  small enum, to validate the shape end to end before generating anything.
- **Phase 2** — the emitter in `ab-riscv-macros`, in the instruction-execution macro, generating
  alongside the existing `match` rather than instead of it.
- **Phase 3** — wire the CoreMark runner and the benchmarks to the new path. Correctness gate is
  CoreMark's CRCs (`0xe714` / `0x1fd7` / `0x8e3a`) plus the existing test suite; performance gate is
  `bench-dispatch.sh` reproducing roughly the prototype's 1.85x on Zen 4. If it does not, the
  difference between this and the prototype is the thing to find.
- **Phase 4** — the vector composition. The proof.
- **Phase 5** — PGO, measured separately, since it is independent of everything above.

> **Correction — what was actually done.**
>
> * **Phase 0 — skipped.** The risks were resolved by building rather than by spikes; §7 records
>   where each landed.
> * **Phase 1 — done, but not as a hand-written prototype.** `ThreadedExecutionResult` and
>   `ThreadedExecutableInstruction` are in `lib.rs`. The intermediate step of hand-writing one
>   `execute_threaded()` was skipped and the emitter written directly, which is why the `become`
>   signature-match constraint (§4) surfaced as a wall of compiler errors rather than as a finding.
>   That was the wrong order.
> * **Phase 2 — done.** `generate_threaded_fns.rs` in `ab-riscv-macros`, in the
>   instruction-execution macro, generating alongside `execute()`. `#[instruction_execution]`'s
>   documentation in `ab-riscv-macros-impl/src/lib.rs` gained a paragraph describing it.
> * **Phase 3 — not done.** No runner or benchmark calls the threaded path; the runners were touched
>   only to enable `explicit_tail_calls` (§6). **No performance number in this document has been
>   reproduced against the generated implementation.** The correctness half is covered by the
>   interpreter's own tests, including `rv64/threaded_tests.rs`, which runs the same programs
>   through both paths and asserts they agree — arithmetic, memory, taken and untaken branches,
>   jumps, failures, and a 1000-iteration loop that would blow any stack if the chain were not real
>   tail calls.
> * **Phase 4 — partial.** The vector enums generate handlers and their tests pass, so the
>   composition builds; it has not been benchmarked, which is the part that was meant to be the
>   proof.
> * **Phase 5 — not started.**
>
> The gate used throughout implementation instead of a benchmark was release-disassembly comparison:
> every cleanup round was checked function-by-function against the previous build and required to
> come out byte-identical.

## 9. ABI ideas, evaluated

> **Correction.** None of this section was needed. §3.2's result type carries the fetcher back and
> is over 16 bytes by construction, so it returns through a hidden out-pointer — option 3 of the
> three below, which this section already expected to be the common answer. The 16-byte squeeze and
> the `__m256i` return were not attempted, and `become`'s exact-ABI-match requirement was never
> re-confirmed with a vector return type. `extern "rust-preserve-none"` was reconsidered once and
> rejected for the same reason recorded here: it cannot be combined with the `zerostore` and
> `extern "Rust"` requirements this project already has, on top of being an unstable feature that
> measured as a tie.

**Returning in vector registers to get past 16 bytes.** Works, but only in one specific shape, and
not the obvious one.

An opaque `#[repr(transparent)]` newtype over a **single** native vector type is the thing that
works. A struct of *two* `__m128i` does not: x86-64 SysV only passes an aggregate larger than two
eightbytes in registers when the first eightbyte is SSE and every other one is SSEUP, and
`{__m128i, __m128i}` classifies as SSE, SSEUP, SSE, SSEUP, which fails that and goes to memory.
Verified by compiling it — it returns through a hidden pointer exactly like a plain 24-byte struct.

What does work, verified the same way, with all six integer argument registers left untouched:

```asm
; #[repr(transparent)] struct R16(__m128i)  — 16 bytes, baseline SSE2
ret16:  vmovq %rdi, %xmm0 ; vmovq %rsi, %xmm1 ; vpunpcklqdq %xmm0, %xmm1, %xmm0 ; retq

; #[repr(transparent)] struct R32(__m256i)  — 32 bytes, needs AVX
ret32:  vmovq %rdi, %xmm0 ; vmovq %rsi, %xmm1 ; vpunpcklqdq %xmm0, %xmm1, %xmm0
        vmovq %rdx, %xmm1 ; vmovq %rcx, %xmm2 ; vpunpcklqdq %xmm1, %xmm2, %xmm1
        vinsertf128 $1, %xmm0, %ymm1, %ymm0 ; retq
```

Integer vector types throughout, so no float semantics are involved anywhere — the conversion is
`vmovq` and `vpunpcklqdq`, roughly seven instructions each way, paid once per execution rather than
per instruction. On aarch64 the equivalent is a homogeneous vector aggregate, returned in `v0`–`v3`.

**Where this leaves the result type.** The 16-byte case buys nothing, because a plain 16-byte struct
already comes back in `rax:rdx` with no out-pointer. The gain is entirely at 17–32 bytes, and it is
worth having: it is the difference between the fully-populated configuration fitting in six argument
registers and spilling. With a hidden out-pointer, real extension state and a real system handler
add up to seven; without one, six.

So the order of preference is:

1. **Squeeze the result into 16 bytes** and return it in `rax:rdx`. Portable, no SIMD, no
   out-pointer. Needs the same treatment `ExecutionError` already got — the program counter is not
   needed on the error path, since `ExecutionError` carries an address of its own, so an enum whose
   variants are `Stopped { program_counter }` and `Err(ExecutionError)` may fit if the discriminant
   lands in padding. Try this first.
2. **`#[repr(transparent)]` over `__m256i`** if it does not fit. x86-64 with AVX only, so it is a
   `cfg` with a fallback, and it makes the handler ABI depend on target features — which is fine
   because every handler is generated into one crate, but means `become`'s exact-ABI-match
   requirement has to be re-confirmed with a vector return type.
3. **Hidden out-pointer** everywhere else. One integer argument register across the chain and no
   per-instruction stores, which is what §3.2 already assumed.

**`extern "custom"` to give handlers a wider register convention.** **Not viable**, and the reason
is definitive rather than a judgement call. Compiling one against this nightly:

```
error: functions with the "custom" ABI must be unsafe
error: invalid signature for `extern "custom"` function
  = note: functions with the "custom" ABI cannot have any parameters or return type
error: items with the "custom" ABI can only be declared externally or defined via naked functions
error: functions with the "custom" ABI cannot be called
note: an `extern "custom"` function can only be called using inline assembly
```

No parameters, no return type, body must be `#[unsafe(naked)]` assembly, callable only from inline
assembly. It is a facility for hand-written assembly routines, not a way to give a Rust-bodied
function a custom register convention. Using it would mean writing all ~500 handlers in assembly,
which is the opposite of generating them from instruction arms. The register-freeing goal it was
meant to serve is better served by §3.3's owned `ExtState` and `InstructionHandler`, which already
gets the common
case to five registers of six.

**`extern "tail"`** (rust-lang/rust#157427). Part of the same tail-calls experiment as
`explicit_tail_calls`, described as "a calling convention supposed to be efficient for tail calls",
implemented on top of LLVM's `tailcc`. **Not needed for this design, and not a substitute for
anything in it.** `become` already produces real tail calls with `extern "Rust"` — verified in the
prototype, which contains no `call` in any handler and an indirect `jmp` in every one — so there is
no gap for it to fill. What it might offer is a *different register convention*, which is the same
axis as `extern "rust-preserve-none"`, and that measured as a tie on Zen 4. Worth re-testing only if
the signature outgrows six registers. It is also not implemented yet (the implementation PR is still
open), and LLVM supports `tailcc` on x86-64 and aarch64 only — enough for this project, but not
something to depend on.

**It does not solve the Windows problem.** The notes record that Windows x86-64 gives only four
argument registers under the default Rust ABI, so handlers would need pinning to `extern "sysv64"`
if Windows ever matters. `extern "tail"` says nothing about that: a tail-call convention derived
from the platform's own is still the platform's register allocation. Pinning remains the answer, and
whether `become` accepts an explicitly pinned ABI is an unverified detail to check at that point.

> **Correction — done, and `become` does accept a pinned ABI.** Windows x86-64 is in this project's
> CI, so it was not a hypothetical. Handlers and the function pointers dispatch selects between are
> generated as `extern "sysv64"` there, and left on the default convention everywhere else.
>
> Verified that `become` accepts it, by pinning to `extern "win64"` on Linux x86-64 — a genuinely
> non-default convention there, so a real test rather than a no-op: it compiles, works through
> generics and non-FFI-safe types, and dispatch still lowers to `jmp *(%r10,%rax,8)`. Then verified
> against the real emitter by forcing the branch on for Linux: the whole interpreter builds
> warning-clean, all tests pass, and every handler still contains zero `call` and ends in the
> indirect `jmp`.
>
> **The decision is made in the build script, and that is the only place it can be made correctly.**
> `CARGO_CFG_TARGET_ARCH` and `CARGO_CFG_TARGET_OS` describe the target being compiled for, and
> `OUT_DIR` is per-target so nothing goes stale. A proc macro could not do this: proc macros run on
> the **host**, so a `cfg!()` inside one describes the wrong machine under cross-compilation, and
> the only sound proc-macro alternative is emitting every handler twice behind `#[cfg]`. A
> `macro_rules!` wrapper would work but has to expand whole items — the ABI is part of an item's
> syntax, not an attribute — and would have to be `#[macro_export]`ed, putting an implementation
> detail of generated code into the crate's public API.
>
> Two costs worth knowing. First, pinning makes the handlers `extern` in the eyes of
> `improper_ctypes_definitions`, which objects to the instruction enum, the result type and the
> generic parameters; the generated items carry an `expect` for it, emitted only where the ABI is,
> and it is sound because every caller and callee is generated into the same crate. Second, an
> explicitly pinned ABI is **not free even where it coincides with the platform default** — forcing
> `sysv64` on Linux, where it is already the convention, moved 154 instructions across the binary,
> because `extern "Rust"` carries parameter optimization attributes an explicit ABI does not. On
> Windows that is against a baseline of four argument registers instead of six, so it should still
> be a clear win, but it is a trade rather than free, and it is unmeasured — Phase 3 has not run on
> any platform.

`extern "rust-preserve-none"` remains the real answer if the signature ever outgrows six registers —
twelve argument registers, measured, already understood.

## 10. Deliberately not in this plan

> **Correction.** Everything in this section stayed out, and nothing in it was revisited during
> implementation.

- **`#[loop_match]` / `#[const_continue]`** (rust-lang/rust#132306, RFC 3720) as a way to keep the
  loop and get handler-like codegen. It does not work for this, for a specific reason:
  `const_continue` requires the target arm to be **statically known** — the RFC restricts it to
  expressions that are const-promotable, so "runtime values read from memory do not qualify". An
  interpreter's next state *is* a runtime value read from memory, namely the next instruction's
  discriminant, so the mechanism the feature is built around never fires here.

  What is left is the `#[loop_match]` structure itself, which gives replicated dispatch — one jump
  site per arm instead of a shared loop header. That is a real threaded-interpreter technique, and
  it targets **the one cost this workload measured as small**: indirect mispredicts are 0.0097 per
  guest instruction on Zen 4, about 3% of runtime. Meanwhile it leaves all three costs §2 of the
  notes identifies, because they are properties of being one large function and `loop_match` keeps
  it one large function — the hoisted universal operand preamble, the re-materialised shared frame,
  and the +11% that 89→147 arms cost on work that never touched the added arms. It also keeps the
  single enormous body that made PGO unhelpful.

  It is cheap to try on the existing `match` path, which is being kept anyway, and it is
  self-contained enough to be an independent experiment. It is not an alternative to this plan.
- **Superinstructions.** 922 distinct adjacent pairs with the most common at 2.13%; no small hot set
  to exploit.
- **Handler pointers inline in the stream.** Removes a dependent load from a chain the branch
  predictor already hides, against a total misprediction budget of ~3.5%.
- **`extern "rust-preserve-none"`.** Ties the best `extern "Rust"` configuration, so it is an
  unstable feature for nothing — until the signature outgrows six arguments, at which point it is
  the answer.
- **Arms that compute positions directly, instead of going through `ExecutionResult`.** This was
  the fallback if the enum turned out not to fold. It does fold (§7, risk 1), so the fallback is not
  needed, and taking it anyway would cost the "bodies do not change" property for nothing.
- **A 33-slot register file with decode-time `x0` remapping.** The single largest identified win —
  store-forwarding failures are a quarter to a third of runtime — but it changes the shared register
  types, so it is a separate decision from this one. Recorded in the handoff.

## 11. Things that came up during implementation and are not anywhere above

- **The generated code has to be warning-clean, and that is a real design constraint.** Suppressing
  lints wholesale over generated code would hide problems in the instruction implementations that
  get inlined into it, so every suppression is placed at the narrowest site that needs it:
  `undocumented_unsafe_blocks` on the `let … else`, `rest_pattern_accessible_field` on dispatch's
  handler-selecting `match`, and `type_complexity` on the dispatch function's return type — the last
  because `become` needs an exact signature match and a type alias cannot capture the enclosing
  generics, so the handler function-pointer type has to be spelled out.

- **The two emitters share what can be shared and no more.** `generate_threaded_fns` reuses
  `variant_fn_name()` from `generate_variant_fns` so the handlers call functions by the same names
  they are generated under, and takes the field names it passes from the match arm's own pattern.
  It does not look up field types, does not re-derive the shared argument list, and does not see
  `execute()`'s signature at all.

- **`execute()` is never an ABI boundary in an optimized build.** The interpreter's release test
  binary contains 54 threaded handler symbols (only the variants its tests actually reach get
  monomorphized) and zero `ExecutableInstruction::execute` or per-variant execution function
  symbols — all of the latter are inlined away. This matters for reading disassembly: there
  is nothing to compare a handler against, only the handler itself.

- **A `#[cfg_attr(feature = "no-panic", …)]` on `execute()` is moved to the per-variant execution
  functions, and the threaded handlers do not carry it.** They wrap those functions rather than
  containing the logic, so the check still applies where the logic is.

- **Where to look.** Emitter:
  `crates/execution/ab-riscv-macros/src/build/execution_impl/generate_threaded_fns.rs`. Signature
  normalization and the call into the emitter: `…/build/execution_impl.rs`. Types and trait:
  `crates/execution/ab-riscv-interpreter/src/lib.rs`. Entry point for the basic interpreter:
  `…/src/basic.rs`. Cross-path tests: `…/src/rv64/threaded_tests.rs`. Generated output for any enum:
  `$OUT_DIR/{EnumName}_execution_impl.rs`.
