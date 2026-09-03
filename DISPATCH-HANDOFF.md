# RISC-V interpreter: faster instruction dispatch

Working notes from replacing the interpreter's central `match` with per-instruction handler
functions. Everything stated as a measurement was measured; everything else is flagged as untested.

## 0. Start here

**State: built, merged, measured.** The exploration is finished, the design is settled, and the
emitter that turns per-variant arms into `become`-chained handler functions is on `main` — the
design and the emitter in
[#766](https://github.com/nazar-pc/abundance-wip/pull/766), the runner and benchmarks calling it in
the work that followed. The interpreter's default path *is* the threaded one now. Nothing in here
is a proposal any more; it is the record of why the thing on `main` has the shape it has, what is
left (§7) and what is still open (§8).

Read §2 (why the `match` is slow), §3 (what won and by how much) and §6 (why only one machine's
numbers count) first. **§7a and §7b are the newest measurements**: §7a is the generated path
measured for the first time, what it cost to get it there, and why the last of it ran into layout
noise rather than into anything about dispatch; §7b is what the counters say once PGO removes that
noise, and is the only evidence here about what the interpreter is waiting on now. §8 question 11
is the newest investigation, and the only one whose subject is a regression that has not happened.

**What is on this branch, and why it is not merged.** Only these notes and the dispatch prototypes
in `crates/execution/ab-riscv-coremark-runner/src/threaded.rs`, which are a hand-written model of
dispatch shapes the emitter does not implement — direct threading, and direct threading with the
handler reference and the operands packed into eight bytes (§8 question 3). They are kept because a
question about one of them can then be answered by measuring rather than by arguing, and they are
not merged because nothing ships them: the shipped path is the generated one.

**To reproduce anything here**, you need `gcc-riscv64-unknown-elf` installed and then:

```bash
COREMARK_ITERATIONS=3000 ROUNDS=5 CORE=<core> \
    ./crates/execution/ab-riscv-coremark-runner/bench-dispatch.sh
COREMARK_ITERATIONS=3000 CORE=<core> \
    ./crates/execution/ab-riscv-coremark-runner/profile-dispatch.sh
```

Read §5 before running either and §6 before believing any number either produces.

**Three things that will waste your time if you do not know them.** The sandbox changes CPU between
sessions and sometimes within one, so numbers are only comparable inside a single run of a single
script (§6). `COREMARK_ITERATIONS` is baked in at build time, so a partial rebuild silently produces
incomparable binaries (§5). And the build script scans crate sources textually for the instruction
attribute macro names, so writing one literally in a doc comment makes it try to parse that file as
an instruction definition — refer to them by name in prose (§1).

## 1. The interpreter as it exists today

`ab-riscv-interpreter` executes pre-decoded RISC-V. The pieces that matter here:

**Instruction execution.** `ExecutableInstruction::execute()` takes the instruction by value along
with `rs1`/`rs2` values, the register file, the execution environment and memory, and the program
counter, and returns:

```rust
pub enum ExecutionResult<Reg>
where Reg: Register
{
    Continue { rd: Reg, value: Reg::Type },   // write the register, fall through
    ContinueNoWrite,                          // fall through, writing no register
    Branch { offset: i32 },                   // relative to *this* instruction
    Jump { target: Reg::Type },               // absolute guest address
    Break,                                    // stop
    Err(ExecutionError<Reg::Type>),
}
```

`ContinueNoWrite` is what an arm that does not end in an explicit result gets. Being a variant
rather than a `Continue` writing to `x0` is what lets an instruction that writes nothing — every
branch, every store, the fences — skip the register write entirely, including the `x0` handling that
§8 question 2 measures as a quarter to a third of the threaded loop's runtime.

Instructions *describe* control flow rather than performing it — they say where execution goes next
instead of moving the program counter themselves. This is what keeps instruction bodies independent
of how the program counter is represented, and it is a precondition for handler functions that
carry the program counter in a register rather than behind a `&mut`.

The type is ≤16 bytes so it comes back in two registers rather than through a hidden out-pointer.
That constraint drives several design details; see §4.

**Instruction fetch.** `InstructionFetcher` has three methods. `fetch_instruction()` returns
`FetchInstructionResult<I>` — `Instruction(I)`, `Continue`, `Break` or `Err` — and moves past what
it read; that is what the `match` loop calls. `peek_instruction()` returns the same thing without
moving and `advance()` moves by a given size, and *that* pair is what the handler chain uses:
dispatch peeks, and the handler advances by a size the compiler folds to a constant because by then
the variant is known. Fetching in one step instead makes the address of the next instruction depend
on decoding the current one, which is a load-to-load dependency in the middle of every dispatch
step.

**Program counter.** `ProgramCounter` provides `get_pc`, `set_pc`, `old_pc`, `set_pc_relative` and
the two halves `set_pc_relative` is built out of, `try_set_pc_relative` and `failed_branch`. The
relative ones exist so a pre-decoded fetcher can move *within the decoded stream* rather than
resolving a branch offset into a guest address and converting that back into a stream position —
the round trip measured ~30% slower on the eager path. The split into a `bool`-returning attempt
and a `#[cold]` explanation is §7a point 2, and is what keeps a branch's failure path out of its
handler.

**The execution environment** is one parameter carrying whatever an extension needs — extension
registers, CSR state, the system-instruction handler — rather than several. Instruction bodies
constrain it through traits, so a composition that uses no extension can pass `()`. In the handler
signature this matters directly: it is one argument register rather than three (§4).

**Eager fetchers** decode the whole instruction stream up front, one slot per guest halfword, and
are split in two: an owning type holding the decoded stream, and a plain 16-byte `Copy` cursor that
borrows it and is what actually travels through the handler chain. `EagerTestInstructions` /
`EagerTestInstructionFetcher` (in `ab-riscv-benchmarks/src/host_utils.rs`) and `EagerInstructions` /
`EagerInstructionFetcher` (in `ab-riscv-coremark-runner/src/interpreter.rs`) are the two pairs, and
they are near-identical. The split is not tidiness — it is §7a point 1, and a destructor on the
cursor costs every fallible handler a stack frame. The lazy fetcher decodes on demand and is useful
as an experimental control, since changes to the eager path should not move it.

**The loop** lives in `BasicInterpreterState::execute()` in `basic.rs`: fetch, read `rs1`/`rs2`,
call `execute()`, match the result, apply the register write or the control flow, repeat.

### How instruction implementations are assembled

This is central to how the dispatch change will be made, so it is worth understanding before
touching anything.

**Instruction bodies are not compiled where they are written.** They are parsed at build time and
re-emitted. Each crate's `build.rs` calls `ab_riscv_macros::process_instruction_macros()`, which
reads the sources marked with the instruction and instruction-execution attribute macros and
writes `{EnumName}_execution_impl.rs` into `OUT_DIR`.

Note that the build script scans crate sources textually to decide what to process, so writing those
attribute names literally in a doc comment is enough to make it try to parse that file as an
instruction definition. Refer to them by name in prose rather than in their `#[..]` form.

`extract_matches.rs` is the part that matters. It requires `execute()`'s body to be a single tail
expression that is a `match` on literal `self`, and it rejects arms with guards. It then splits that
`match` into **per-variant `(variant ident, arm)` pairs**, inserting
`ExecutionResult::ContinueNoWrite` as the tail of any arm that does not end in a result explicitly.
So the individual arm body — not the enclosing `match` — is already the unit the build system stores
and manipulates.

**Since `fb1eaaa`, those arms are emitted as standalone functions.** `generate_variant_fns` turns
each into an `#[inline(always)] fn execute_{enum_snake}_{variant_snake}<..>(..) -> ExecutionResult`
and replaces the arm body with a call to it, so `execute()` is now a dispatcher over per-variant
functions rather than a wall of inline bodies. Variant-specific parameters come from what the arm's
pattern binds, so they differ per variant; the shared ones are taken once from `execute()`'s
signature, so **every generated function takes `rs1_value` and `rs2_value` whether or not it uses
them**. That costs the `match` loop nothing, because it reads both anyway — and in a handler the
`#[inline(always)]` removes it, which is why it is there: an unused register read is a pure load
with a dead result. That is deliberate and assumed rather than open, but it is load-bearing, so a
preamble that came back is the first thing to check if handler numbers ever fail to reproduce the
prototype's.

Those pairs are keyed by enum name in a build-time `state`, and
`collect_original_enum_execution_impls_from_dependencies()` pulls them across crate boundaries. The
`links` key in `Cargo.toml` — `ab-riscv-primitives` and `ab-riscv-interpreter` each declare one —
plus the `DEP_*` variables Cargo derives from it are what let a downstream crate collect arms
defined upstream. This is how a composed instruction enum — `ContractInstruction`, the vector
compositions, `CoremarkInstruction` — gets an `execute()` assembled out of arms that live in
several different crates.

Two consequences worth carrying:

- **The same arm text is expanded into more than one crate**, so a `crate::`-relative path inside an
  arm resolves against whichever crate the arm landed in, which will usually be the wrong one.
  The established way around this is not absolute paths but **helper modules re-exported through the
  prelude** — `rv64_zbb_helpers`, `rv32_zbb_helpers` and friends live next to the instructions that
  need them and are re-exported from `prelude`, so an arm can name them unqualified wherever it is
  expanded. Anything new that instruction bodies need to reach should follow the same pattern.
- **Each arm already names exactly the operands its instruction uses.** That is precisely the
  information a per-instruction handler needs in order to avoid the universal preamble described in
  §2 — it is present in the build system today and thrown away when the arms are stitched back into
  one `match`.

## 2. Why a big `match` is the bottleneck

The loop is **throughput-bound, not misprediction-bound** — roughly 10.2 cycles per guest
instruction against ~40 x86 instructions of preamble. Adding ~2 uops per instruction to the loop
cost 8.5%, which a misprediction-bound loop would not notice. This matters because the usual
argument for threaded dispatch is about branch-target prediction, and that argument does not apply
here. The real costs are:

- **A universal operand preamble.** Disassembling the specialised big-`match` prototype showed LLVM
  hoisting operand decoding (`rs1`/`rs2`/`rd`/`imm`, plus loading both source registers) *before*
  the jump table, because it is common to most arms. Every instruction pays for operands it never
  uses. Per-instruction handlers touch only what they name.
- **Re-materialising a shared frame**, because arm bodies vary in width.
- **Table size.** Growing the op table from 89 to 147 arms cost `match` **+11%** on a workload that
  never executes the added arms. `call` and `tail` back ends were unaffected. This is the mechanism
  behind the vector-extension regression: with a `match`, adding an extension you never execute
  still costs you.

## 3. What was prototyped, and what was built

The prototypes live in `crates/execution/ab-riscv-coremark-runner/src/threaded.rs`, on this branch.
They are a hand-rolled model of the back end rather than the back end: a single `ops!` macro table
covers all 147 `CoremarkInstruction` variants exhaustively, with no catch-all, so adding an
extension is a compile error rather than a silent `Unsupported` at run time, and emitter macros
expand that one table into per-instruction handler functions. The shipped implementation does the
same thing to the arms the build system already extracts (§9); the prototype is what the numbers
below were taken on, and is still where a dispatch shape the emitter does not implement gets
measured.

Five back ends were measured. Four of them lost and have been removed, so that further work is not
spent maintaining them, and **§3's table below is all that is left of them** — the exploration
branch they lived on was collapsed into `main` when the dispatch work merged and is gone. Reading
the table is enough to know why each one lost; reviving one means writing it again, which is
cheap next to maintaining four dead back ends and is only worth doing for a question the table
cannot answer. The most likely such question is re-running the table-size experiment (§2) against
the vector composition, which needs the `match` back end.

### The back ends vary along four independent axes

This is the thing most worth internalising before reading the numbers: the named back ends are
*combinations*, not alternatives on a single scale.

**Axis 1 — how the next handler is reached.**

| | |
|---|---|
| `match` | one big jump table, everything inline in the loop |
| `call` | one function per instruction, ordinary call, returns to the loop each time |
| `become` | one function per instruction, guaranteed sibling call, never returns |
| plain tail call | identical body, ordinary call in tail position, *deliberately* not `become` |

Orthogonal to that is how the handler is *found*, which only becomes a free variable once dispatch
is per-instruction: *indirect* (token) threading stores a discriminant in the slot and indexes a
table with it, *direct* threading stores the handler itself in the slot. See §8 question 3 — direct
threading is measured at +11% over indirect, and with the operands packed alongside a 32-bit handler
offset it costs no more memory than indirect does.

The last of the four exists only to answer whether `become` is load-bearing. It is not, today — LLVM performs
the sibling call anyway at `-O3` — but it is not guaranteed to keep doing so, and `become` turns it
into a compiler-enforced property. `become` works on x86-64, aarch64 and riscv64, inside
`#[inline(always)]` helpers, and with generics and HRTB function-pointer tables. It requires exact
signature *and* ABI match, so trait methods with differing `Self` will not tail-call into each
other.

**Axis 2 — how interpreter state is carried.**

- *Raw-pointer context*: `fn(ip: *const I, ctx: &mut Ctx) -> Next`, where `Ctx` **owns** guest
  memory.
- *Safe and generic*: `fn<Regs, Memory>(.., regs: &mut Regs, memory: &mut Memory, ..)` with
  `Regs: RegisterFile<R>` and `Memory: VirtualMemory`, both **borrowed** from whoever owns them.

**Axis 3 — the register file**, which only becomes a free variable once axis 2 is generic, because
the handlers are then generic over `Regs`:

- *basic* — the ordinary register file.
- *branchless* — writes to `x0` are discarded without a branch.
- *zerostore* — writes to `x0` land in a scratch slot, so the write is unconditional.

These are prototype names. In the interpreter the surviving one is not a separate type but a
const-generic flag: `BasicRegisters<Reg, const ZEROSTORE: bool = false>`, `false` for the `match`
loop and `true` for the threaded path, which is what the axis-3 numbers below say each wants. The
prototype's own type is called `ZeroStoreRegisters` and is what the disassembly listings quote.

**Axis 4 — the handler calling convention**, which only becomes a free variable once axis 1 is
`become`, because only then does a handler never return:

- `extern "Rust"` — the default.
- `extern "rust-preserve-none"` — every register caller-saved, so no prologue or epilogue. See §4.

### How the measured back ends map onto those axes

CoreMark, isolated per-back-end binaries so only one is ever linked in, interleaved, best-of.
The first Xeon column is `COREMARK_ITERATIONS=300`; the second is a *different* Xeon in a later
session at `COREMARK_ITERATIONS=3000`, so read it only down its own column.

| back end | dispatch | state | registers | ABI | Xeon A | Xeon B | Zen 4 (`znver4`, pinned) |
|---|---|---|---|---|---|---|---|
| generic loop | `match` | owned, concrete | basic | — | 0.469 s / 1.00x | 3.855 s / 1.00x | 1333 |
| `match` | `match` | raw pointer | basic | Rust | 0.195 s / 2.41x | removed | 1764 |
| `call` | call | raw pointer | basic | Rust | 0.167 s / 2.81x | removed | 1875 |
| `tail` | `become` | raw pointer | basic | Rust | 0.134 s / 3.50x | removed | 2500 |
| `plaintail` | plain tail | raw pointer | basic | Rust | 0.133 s / 3.53x | removed | 2500 |
| `basic` | `become` | safe generic | basic | Rust | 0.138 s / 3.40x | 1.385 s / 2.78x | 2500 |
| `branchless` | `become` | safe generic | branchless | Rust | 0.141 s / 3.33x | 1.436 s / 2.68x | **2666** |
| `zerostore` | `become` | safe generic | zerostore | Rust | 0.134 s / 3.50x | 1.370 s / 2.81x | **2666** |

### Zen 4, and what it settled

Threadripper 7970X, pinned, `COREMARK_ITERATIONS=3000`, five interleaved rounds, median seconds.
This is the run that answers axes 3 and 4, because it is the only machine whose spread (0.6–3.6%)
is narrower than the effects being compared.

| configuration | `-C target-cpu=znver4` | default | vs `basic` |
|---|---|---|---|
| generic loop | 2.282 | 2.294 | — |
| `basic` | 1.219 | 1.221 | — |
| `branchless` | 1.190 | 1.194 | −2.3% |
| `zerostore` | **1.142** | **1.113** | −6.3% / −8.8% |
| `basic-pn` | 1.202 | 1.212 | −1.4% / −0.7% |
| `branchless-pn` | 1.249 | 1.226 | +5.0% / +2.7% vs `branchless` |
| `zerostore-pn` | 1.229 | 1.217 | +7.6% / +9.3% vs `zerostore` |

Three things fall out of it:

- **`zerostore` is the register file.** It wins on both builds, by 6–9% over `basic`, with
  `branchless` sitting between them rather than level with `zerostore`. Axis 3 is settled.
- **The whole tail-threaded family is worth about 1.9–2.1x on Zen 4, not the 3.4x the Xeons show.**
  That is not a regression, it is Zen 4's wider out-of-order engine coping better with the generic
  loop's big `match`, so the baseline it is measured against is relatively stronger. The earlier
  score-based Zen 4 column says the same thing: 2500/1333 is also 1.88x.
- **`-C target-cpu=znver4` buys nothing here**, and on the fastest configuration the default build is
  2.5% *ahead*. That is close to the spread, so it is a hint rather than a finding, but it means
  znver4 should not be assumed to be the build that matters.

### The preserve-none argument-order trap, and what fixing it showed

The first preserve-none attempt lost on Zen 4, and lost *in proportion to the text it added*:
+2.0% text cost `basic` nothing, +3.7% cost `branchless` ~3%, +6.8% cost `zerostore` ~8%. That is a
clean enough correlation to name the mechanism, and the mechanism turned out to be fixable — see §4.
Re-run with the arguments ordered coldest first, same machine, `znver4`:

| configuration | best | median | spread | vs its `extern "Rust"` twin |
|---|---|---|---|---|
| generic loop | 2.117 | 2.129 | 1.5% | — |
| `basic` | 1.223 | 1.248 | 2.2% | — |
| `basic-pn` | 1.134 | 1.178 | 4.9% | **−5.6%** |
| `branchless` | 1.233 | 1.258 | 2.5% | — |
| `branchless-pn` | 1.126 | 1.154 | 3.7% | **−8.3%** |
| `zerostore` | 1.126 | 1.162 | 4.1% | — |
| `zerostore-pn` | 1.137 | 1.149 | 2.1% | −1.1%, inside the spread |

The reorder turned a 9% loss into a 6–8% win for `basic` and `branchless`. But the useful reading is
not "preserve-none is good now":

- **It did not beat the best `extern "Rust"` configuration.** `zerostore` was already at the floor,
  and `zerostore-pn` neither improves on it nor loses to it — 1.1% against a 2–4% spread, with the
  best-of and median rankings inverting between the two, which is what a tie looks like.
- **What preserve-none actually buys is making the register file stop mattering.** Under
  `extern "Rust"` the three files span 7.4% (1.162 to 1.258); under preserve-none they span 2.5%
  (1.149 to 1.178), and all three land where `zerostore` already was. The freed registers absorb the
  `x0`-handling work that separated them.
- So the field converges on ~1.13–1.18 s, four configurations deep, with nothing separating the top
  four. Whatever now limits this loop is not dispatch.

**Practical conclusion: `zerostore` with `extern "Rust"` is the configuration to build.** It reaches
the floor with no unstable ABI feature attached. preserve-none earns its keep only if the handler
signature outgrows six arguments (§4), which is plausible for the eventual generic implementation
but is not the case today.

Note also that the generic baseline moved from 2.282 to 2.129 between these two Zen 4 sessions with
no change to its code — 6.7%, on a dedicated machine whose within-run spread is 1.5%. Compare within
a run, never across.

Read down the columns rather than across the rows, and the numbers say something the flat list
hides:

- **`tail` vs `safe` is not a tradeoff between speed and safety.** Both dispatch with `become`; they
  differ only on axis 2. Holding dispatch constant, the safe generic borrowed state costs nothing
  measurable — 0.134 vs 0.138 s on Xeon A, identical on Zen 4 — and once the register file varies,
  the safe variants are *ahead* on Zen 4. Raw pointers and owned memory bought nothing.
- **Almost the entire win is axis 1**, and most of that is `call` → `become` (2.81x → 3.50x).
- **Axis 3 is worth 6–9% on Zen 4** and nothing on either Xeon, which is a good illustration of why
  only Zen 4 numbers count (§6).
- **Axis 4 is inside the noise on both Xeons**, and decisive on Zen 4. Every preserve-none delta on
  a Xeon is under 1% against a 3–7% spread; on Zen 4 the same binaries lose by up to 9%.

The `xeon B` speedups are lower than `xeon A`'s across the board because the *baseline* got faster
— the generic loop picked up the "instructions describe control flow" refactor in the meantime —
not because the handlers regressed. The handler times are nearly unchanged per iteration.

### What was built

**Safe generic handlers, dispatched with `become`, with the register file left as a type
parameter.** It is the fastest and most consistent combination on Zen 4, the only machine where
measurement is reliable, and it is ordinary safe Rust with memory and the decoded stream borrowed
rather than owned. Neither raw pointers nor a plain tail call was worth building: the first bought
nothing, and the second is what `become` already gives with a guarantee attached.

The handlers are generated, not written. Instruction implementations stay as they are — a `match` on
`self` in the source — and the change happened in `ab-riscv-macros`, where the per-variant arms
already extracted (§1) are emitted as standalone functions as well as being stitched back into one
`match`. Instruction authors keep writing arms; what the build system does with them changed.
`generate_threaded_fns.rs` is the emitter, and §9 is what it produces and where the built shape
diverged from the planned one.

Axis 3 stayed a type parameter and axis 4 an ABI annotation, so neither needed a separate
implementation: the handlers are generic over `Regs`, and the ABI is a `macro_rules!` metavariable
that the build script fills in per target (§9).

Practical notes, which were written as advice and held:

- Anything a generated handler needs to call should reach it through a helper module re-exported
  from `prelude`, for the reason above.
- `extract_matches.rs` hardcodes the expected "no explicit tail" expression as a token-stream string
  comparison against `ExecutionResult::ContinueNoWrite`. Anything that changes what an arm may end
  with has to be changed there too.
- Arms must remain guard-free and the body must remain a single `match` on literal `self`; the
  extractor enforces both, and handler emission depends on the same property.
- Generated code lands in `OUT_DIR` as `{EnumName}_execution_impl.rs`. Reading those files is the
  fastest way to see what an emitter actually produced, and errors reported against them are
  errors in the emitter, not in the instruction sources.

## 4. Constraints that shape the design

### Calling convention

- x86-64 SysV gives **6** integer argument registers. A return value of ≤16 bytes comes back in
  `rax:rdx`; anything larger uses sret, which consumes one argument register — the pointer is then
  forwarded down the tail chain, so it is a one-time cost rather than per-instruction traffic, but
  it also means a store in every handler and a load in the loop. Handlers avoid that entirely by
  returning the outcome in registers instead, which is what `OpaqueThreadedExecutionResult` is for
  (§9 divergence 3).
- The default Rust ABI on **Windows x86-64 gives only 4** argument registers, and Windows x86-64 is
  in this project's CI, so handlers need pinning to `extern "sysv64"` there. **The pinning ended up
  being unconditional on x86-64 anyway**, for the return rather than the arguments: `extern "Rust"`
  returns `OpaqueThreadedExecutionResult` through memory where the platform's own convention would
  return it in a register, which is a rustc bug
  ([rust-lang/rust#161381](https://github.com/rust-lang/rust/issues/161381)) rather than anything
  inherent. Everywhere else handlers are pinned to `extern "C"`. §9 divergence 4 records how the
  decision is taken, that `become` accepts an explicitly pinned ABI, and that pinning is not free
  even where it coincides with the platform default.

### `extern "rust-preserve-none"`

Explored. It works, it does what it promises structurally, and on x86-64 it comes with a cost that
was not anticipated.

**It composes with everything else.** `become` accepts it — the requirement is that caller and
callee ABIs match, which they do when every handler carries the same annotation — and it works
through a generic HRTB function-pointer table and across monomorphizations. It needs
`#![feature(rust_preserve_none_cc)]` (rust-lang/rust#151401), and the ABI string can be a
`macro_rules!` `$abi:literal` metavariable in `extern $abi fn`, so one emitter can produce both
shapes rather than two copies that drift apart. All six configurations produce CoreMark's known-good
CRCs.

**The prologue saving is real.** Disassembling the 140 generated handlers per binary:

| configuration | instructions | text bytes | handlers with push/pop |
|---|---|---|---|
| `basic` | 4162 | 12064 | 34 |
| `basic-pn` | 3834 | 12304 | 2 |
| `branchless` | 3544 | 11216 | 31 |
| `branchless-pn` | 3605 | 11632 | 2 |
| `zerostore` | 3250 | 10176 | 27 |
| `zerostore-pn` | 3245 | 10864 | 3 |

Roughly a fifth to a quarter of handlers were spilling callee-saved registers under `extern "Rust"`;
preserve-none removes essentially all of it.

**Argument order is load-bearing, and getting it wrong costs more than the prologues are worth.**
LLVM's `preserve_none` passes arguments in **R12, R13, R14, R15, RDI, RSI, RDX, RCX, R8, R9, R11,
RAX** — twelve registers rather than six, but with the high registers *first*. (Verified from
codegen rather than from documentation: give a `preserve-none` function twelve arguments, store each
one, and read off the sources.) On x86-64 those first two are exactly the registers that encode
worst: a memory operand based on **R12 always costs an extra SIB byte**, and one based on **R13
always costs an extra displacement byte**. Both are unavoidable properties of ModRM — R12 and R13
are the REX-extended twins of RSP and RBP, and they inherit the escape meanings those two encodings
carry — so it is not something the register allocator can route around.

#### Why the order is "backwards", and when it is right

It looks inverted against every other x86-64 convention, but it is deliberate. **The first four
`preserve_none` argument registers are precisely the four callee-saved GPRs that SysV leaves
generally allocatable** — R12, R13, R14, R15. So a `preserve_none` function that calls an ordinary
C function keeps its first four arguments across that call *for free*, while later arguments have to
be rescued. Compiling six pointers live across an opaque `extern "C"` call shows it exactly:

```asm
across_c_call:
        pushq   %rbp
        movq    %rsi, %rbx          # arg 6 evacuated, RSI is caller-saved
        movq    %rdi, %rbp          # arg 5 evacuated, RDI is caller-saved
        callq   *opaque@GOTPCREL(%rip)
        movq    (%r13), %rax        # args 1-4 still live, untouched
        addq    (%r12), %rax
        addq    (%r14), %rax
        addq    (%r15), %rax
        addq    (%rbp), %rax
        addq    (%rbx), %rax
        popq    %rbp
        retq
```

Args 1–4 survive with no spill; args 5–6 cost two moves, a `push` and a `pop` — reintroducing
exactly the prologue `preserve_none` exists to remove. So the ordering optimises for
`preserve_none` code that interoperates with ordinary C, which is the general case.

It is the wrong trade for **this** interpreter, because the hot path calls nothing: 2 handlers out
of 140 contain a `call` at all, and those are the cold CSR paths. We get none of the interop benefit
and pay the encoding cost on every dereference. Hence coldest-first.

**This is the one thing to re-derive rather than inherit if the real implementation's handlers ever
call out on the hot path** — a non-inlined `VirtualMemory` access, a real system-instruction
handler, anything behind a `dyn`. The moment a hot handler makes a call, the persistent state wants
to be in R12–R15 after all, and the right order flips back.

The obvious signature puts the two hottest pointers — the instruction pointer, dereferenced by every
handler to read its operands, and the register file, dereferenced by nearly all of them — in exactly
those two positions, so every single dereference pays. That cost every configuration 2–7% more text
and, on Zen 4, up to 9% in time.

The fix is to order the parameters **coldest first**, so that the arguments this interpreter never
dereferences absorb R12 and R13, and the hot pointers land in RDI and RSI. The same `Add` handler,
across all three:

```
extern "Rust"                     preserve-none, hot first        preserve-none, cold first
  movzbl 0x2(%rdi),%r10d   5 B      movzbl 0x2(%r12),%ecx    6 B     movzbl 0x2(%rdi),%eax   4 B
  mov    (%rsi,%r10,8),%r10 4 B     mov 0x0(%r13,%rcx,8),%rcx 5 B    mov (%rsi,%rcx,8),%rcx  4 B
```

Cold-first is *smaller than `extern "Rust"`*, not merely even with it, because preserve-none leaves
more scratch registers free, so LLVM picks low registers (`eax`, `ecx`) for temporaries and drops
the REX prefix that `r10d`/`r11d` forced. All three end in `jmp *(%reg,%rax,8)`, so all three are
genuine sibling calls.

Across the 140 generated handlers, with the ordering fixed:

| configuration | instructions | text bytes | handlers with push/pop | text vs `extern "Rust"` |
|---|---|---|---|---|
| `basic` | 4162 | 12064 | 34 | — |
| `basic-pn` | 4141 | 11296 | 2 | **−6.4%** |
| `branchless` | 3544 | 11216 | 31 | — |
| `branchless-pn` | 3425 | 10096 | 2 | **−10.0%** |
| `zerostore` | 3250 | 10176 | 27 | — |
| `zerostore-pn` | 3268 | 9552 | 3 | **−6.1%** |

Before the reorder those last figures were +2.0%, +3.7% and +6.8%, so a pure argument permutation
moved text size by 8–17 percentage points. Instruction counts are within ~3% of the `extern "Rust"`
build either way; the prologue removal holds in both.

That leaves preserve-none strictly better on paper — same instruction count, almost no prologues,
6–10% less text — which is a different proposition from the one Zen 4 rejected, and needs
re-measuring there. §2 established that this loop is throughput-bound *and* sensitive to text size,
and cold-first is now on the right side of both.

Three things to carry:

- **The handler signature sits exactly on the SysV limit.** Six arguments, six integer argument
  registers. That is why `extern "Rust"` holds its own here: nothing spills. Add a seventh piece of
  state — and the eventual generic implementation plausibly needs one, since this prototype's `Sys`
  and `Ext` are stand-ins for a real system-instruction handler and real extension state — and
  `extern "Rust"` starts passing it on the stack, once per dispatch, forever. preserve-none has
  twelve. **That, not the prologues, is the case where preserve-none wins**, and it is worth
  re-testing at the point the signature grows rather than deciding now.
- This is an x86-64 problem specifically. aarch64 and riscv64 encode all their registers uniformly,
  so the R12/R13 penalty does not exist there and argument order should cost nothing — worth
  confirming rather than assuming, if either becomes a host target.
- `rust_preserve_none_cc` is unstable (rust-lang/rust#151401). Since it currently buys nothing over
  `zerostore` + `extern "Rust"`, that is a dependency with no return attached to it today.

### Type layout

Settle layout questions with `const { assert!(size_of::<T>() == N) }` probes rather than reasoning
about them. It takes one compile and is reliable; layout intuition here is not.

- Anything over 16 bytes returns through a hidden out-pointer. Keeping `ExecutionResult` and
  `FetchInstructionResult` under that is why they carry their error as a variant instead of being
  wrapped in a `Result` — a `Result` of two 16-byte arms needs a discriminant word of its own and
  lands at 24.
- A `(Reg, Reg::Type)` **tuple** is `(u8, u64)`, which pads to 16 bytes, and a tuple's padding is
  not available to the enclosing enum's discriminant. Named fields are what let the discriminant sit
  in the tail after `rd`. This is why `Continue` has fields rather than a tuple.
- `repr(packed)` **cannot be applied to enums**, and `repr(align(N))` only raises alignment. The
  only way to lower an enum's alignment is to lower its fields' — which is what `PackedAddress`
  (alignment 4) does for the addresses inside `ExecutionError`. Every address field needs it,
  including the ones that are always `u64` rather than generic; one field left at natural alignment
  puts the enum back over the limit. Packed fields cannot be borrowed, so `Debug`/`Display`/
  `LowerHex` are hand-written to copy the field out first.

### Program counter representation

Passing the PC **by value** — as a 16-byte `Copy` struct occupying two registers, or as two separate
arguments; the two are indistinguishable at 1.344 vs 1.342 ns/insn — is **1.66x faster** than
passing it behind a `&mut`. What matters is by-value versus behind-a-reference, not how it is
spelled. This is the main reason instruction bodies must not touch the program counter directly,
and why `ExecutionResult` describes control flow instead.

### Register pressure

The loop sits on a **register-pressure cliff**, and this is the easiest way to lose the gains by
accident. Caching a loop-invariant bound in a struct field, rather than re-reading
`instructions.len()` on each use, cost 7% on a generic `x86-64` target, 13% with
`-C target-cpu=native`, and **50% on Zen 4**. Holding one more value live across the loop can cost
more than the load it saves, and the penalty grows with how much else the codegen configuration is
already keeping live.

### Codegen details

- `imm / 2` on a signed type compiles to a 6-instruction round-toward-zero sequence. Use `>> 1`, or
  the `/ size_of::<u16>() * size_of::<Instruction>()` idiom on an unsigned value.
- Relative branches should move within the decoded stream. Resolving an offset into a guest address
  and converting it back into a stream position measured ~30% slower.

## 5. Running the benchmarks

### Dispatch modes

On this branch, from the workspace root:

There is one binary with five dispatch modes in it, and two scripts:

```bash
COREMARK_ITERATIONS=3000 ROUNDS=5 CORE=<core> \
    ./crates/execution/ab-riscv-coremark-runner/bench-dispatch.sh
COREMARK_ITERATIONS=3000 CORE=<core> \
    ./crates/execution/ab-riscv-coremark-runner/profile-dispatch.sh
```

`COREMARK_DISPATCH` selects the mode, and an unset variable selects the shipped path, so running
the binary with no environment at all measures what the interpreter actually does:

| value | what it runs |
|---|---|
| unset, or `threaded` | the generated per-instruction handlers — **the shipped path** |
| `loop` | the generic `match` loop, `BasicInterpreterState::execute()` |
| `token` | prototype: indirect threading, discriminant in the slot plus a handler table |
| `direct` | prototype: direct threading, full handler pointer, sixteen-byte slot |
| `packed` | prototype: direct threading, handler offset plus packed operands, eight-byte slot |

The first two are the interpreter itself; the last three are `threaded.rs` and exist to answer §8
question 3. `threaded` and `token` are the same dispatch shape reached two different ways, one
generated and one hand-written, which makes them the pair to check when a generated number and a
prototype number disagree.

`bench-dispatch.sh` builds once and compares the modes. It exists so that the methodology is not
something to remember: it interleaves the modes round-robin rather than running all of one and then
the other, reports best-of, and prints the run-to-run spread next to every timing so that an effect
narrower than the noise is visibly not a result. It refuses to print a table when any mode returns
wrong CRCs or exits non-zero. `MODES`, `ROUNDS`, `CORE` and `COREMARK_REPEAT` control it; `MODES`
defaults to all five, and its first entry is the baseline the rest are expressed against.

`profile-dispatch.sh` runs `perf stat` over each mode and normalises every counter **per guest
instruction**, which is the only unit in which they are comparable. The denominator is not
guessed: `COREMARK_HISTOGRAM` counts the dynamic instruction mix exactly, so the script derives a
true cycles-per-guest-instruction figure. It probes which PMU events the machine has rather than
assuming, so the AMD-specific indirect-branch counters appear when they exist and are skipped when
they do not. It finishes with a flat per-handler profile of `RECORD_MODE` — tail calls mean there
are no stack frames, so self-cost is all there is, which is what is wanted.

Both find the binary by asking cargo where it put it, so `CARGO_TARGET_DIR`, `build.target-dir` and
`--target` all work.

There are no dispatch features left — one configuration survived, so there is nothing to select at
build time. `build-elf-required` is the only relevant feature, and it makes a missing RISC-V
toolchain a build error rather than an empty ELF and a confusing run-time message.

The other environment variables: `COREMARK_ITERATIONS` fixes the amount of work **at build time**
(**essential** — the default of 0 means autodetect, so the workload scales with interpreter speed
and results are not comparable), `COREMARK_REPEAT` runs the guest N times in-process and reports
the best, `COREMARK_HISTOGRAM` dumps the dynamic instruction mix and the adjacent-pair distribution
instead of running a timed pass.

Two things about CoreMark's own output. The score quantisation is removed via `-DHAS_FLOAT=1` (plus
a small `math.h` shim providing `modf`, and `-lgcc` for soft-float); without it `secs_ret` is an
integer and the reported score moves in 5–10% steps, coarser than most of the effects being
measured. And below `COREMARK_ITERATIONS` of roughly 20000, CoreMark prints `ERROR! Must execute for
at least 10 secs for a valid result!` and `Errors detected` — that is its *reporting* rule, not a
correctness failure. Check the per-algorithm CRCs, not that line.

The toolchain is `gcc-riscv64-unknown-elf` (Ubuntu package of that name). Without it the ELF is
empty and every run fails with "Coremark ELF not found".

### Criterion benchmarks

```bash
taskset -c <core> cargo bench -- --sample-size 10 --baseline before interpreter
```

To A/B two revisions without rebuilding, build each once and run the binaries directly:

```bash
cargo bench --no-run --bench riscv -p ab-riscv-benchmarks   # prints the binary path
# target/release/build/ab-riscv-benchmarks/<hash>/out/riscv-<hash>
$BIN --bench --noplot --warm-up-time 1 --measurement-time 3 --sample-size 20 \
    "blake3_hash_chunk/interpreter/threaded/eager"
```

Interleave the two binaries round-robin across several rounds and compare best-of, rather than
running all of A then all of B.

**`RUSTFLAGS` replaces `.cargo/config.toml`'s flags rather than merging**, so any manual build must
carry the project's forward:

```bash
RUSTFLAGS="-Znext-solver=globally -Zmin-recursion-limit=256 -C target-cpu=znver4"
```

### Checks worth keeping in the loop

- The three interpreter benchmarks in each group are `interpreter/loop/lazy`,
  `interpreter/loop/eager` and `interpreter/threaded/eager`, the last being the shipped path with
  the zero-store register file. **`blake3_hash_chunk/interpreter/loop/lazy` is the control**: the
  lazy fetcher should not move when the eager path changes. If it does, the measurement is suspect.
- **CoreMark's own CRCs** — `crclist` `0xe714`, `crcmatrix` `0x1fd7`, `crcstate` `0x8e3a`. Broad
  correctness across a far wider instruction mix than the microbenchmarks reach. These are the
  values to check; "Correct operation validated" only appears when the run also happens to exceed
  ten seconds, so it is not a usable signal at benchmarking iteration counts.
- Build only the four crates that depend on the interpreter — `ab-contract-file`,
  `ab-riscv-act4-runner`, `ab-riscv-benchmarks`, `ab-riscv-coremark-runner`. Building the whole
  workspace gets OOM-killed.

## 6. Measuring performance: only Zen 4 counts

Development-sandbox timings for this workload disagreed with the Zen 4 target **in direction, not
just magnitude**, on four separate occasions — changes measuring +17% in the sandbox measured −34%
on real hardware. Contributing factors, all observed:

- Interleaving A/B within a single batch was not sufficient. The same two binaries measured
  base 77 µs / new 65 µs in one batch and base 44 µs / new 47 µs in another, with the ordering
  reversed.
- Single runs carry ±11% noise; a semantically no-op change measured 83 vs 74.5 µs.
- Sandbox clock drifted 2.5–3.25 GHz within a session, so cross-batch comparisons are meaningless.
- A generic `x86-64` build and a `-C target-cpu=native` build of the same change produced opposite
  signs, because codegen configuration decides which side of the register-pressure cliff the change
  lands on.

**Practical rule:** a development machine can settle *structural* questions — type sizes, whether a
value is returned via sret, instruction counts in a disassembly, correctness — but cannot settle
performance questions. Every performance claim needs confirming on Zen 4 built with
`-C target-cpu=znver4` and pinned to a fixed core.

**Sandbox sessions do not even get the same machine twice.** "Xeon A" in §3 was a Skylake-SP the
sandbox happened to allocate in one session; "Xeon B" was a different 4-core Skylake-SP
(family 6 model 85, 2.8 GHz, KVM guest) in another. Check `lscpu` at the start of a session and
treat numbers from a previous one as a different experiment, not a baseline. The machine changed
mid-session at least once while these notes were being written, from a 2.8 GHz Skylake-SP to a
2.1 GHz one, which moved every speedup by roughly 10% with no code change at all.
`bench-dispatch.sh` and `profile-dispatch.sh` print the CPU for exactly this reason.

Within a single session the run-to-run spread on Xeon B was 3–7% best-to-worst across seven
interleaved rounds at `COREMARK_ITERATIONS=3000`, and 7–16% at 300, because a run that short is
dominated by process start-up and ELF decoding rather than by the interpreter. Whatever the spread
turns out to be, it is the floor an effect has to clear before it means anything.

## 7. Where this stands

**Everything above is now on `main`, and it is the default path.** The emitter that turns the
per-variant arms into standalone `become`-chained handler functions (§3, §9) shipped in
[#766](https://github.com/nazar-pc/abundance-wip/pull/766); the Coremark runner and the interpreter
benchmarks call it; §7a is what that measured and what it cost to get there; §7b is what the
counters say once PGO removes the layout noise §7a ran into. The scratch branch the phase-3 work
was done on is gone — `main` is the reference for anything about the generated path.

The exploration has answered what it set out to answer. The back end is settled — safe generic
per-instruction handlers, dispatched with `become`, register file as a type parameter — and the
alternatives that lost are recorded in §3 with the measurements that killed them. Dispatch is no
longer what limits the loop: it costs 6.14 cycles per guest instruction against the generic loop's
11.34, with caches, branch prediction and issue width all ruled out, and what remains is a
store-forwarding problem in the register file rather than anything about how handlers are reached.

**The generated path reproduces the prototype, and then passes it.** That was the open question
this document used to end on, and it is closed: Zen 4 Coremark reached 2948 through the emitter
against the prototype's best of 2666 (§7a). On a shared build of the Coremark runner the two are
level within noise — `threaded` and `token` are the same dispatch shape reached two different ways,
and they measure the same, which is the check to re-run whenever a generated number and a prototype
number disagree.

What the implementation also established, independent of any benchmark: every generated handler
contains no `call` and ends in an indirect `jmp` through a jump table, so `become` produces real
tail calls at the real variant count; and the two paths agree instruction-for-instruction on
registers, memory, program counter and error, which the interpreter's tests assert directly.

**What is left.** Three things, in the order they are worth doing. The ~500-variant vector
composition has never been benchmarked, only built and tested (question 6), and it is where the
existing regression lives and where `match` is known to degrade while handlers do not — that is the
case worth proving. The 33-slot register file with decode-time `x0` remapping (question 2) is the
single largest identified win and needs a decision about the shared register types rather than an
experiment. And the packed eight-byte direct-threaded slot (question 3) is now measured at +11% over
indirect threading at no memory cost, which is a different proposition from the one that was set
aside, and it wants re-deriving through the emitter rather than the prototype.

What is *not* next, and why, is worth stating so it does not get relitigated: superinstructions
(question 5's data kills them), plain 16-byte direct threading (question 3 — the packed form beats
it at half the footprint) and `extern "rust-preserve-none"` (question 1) were each measured and set
aside. Nor is splitting a conditional branch's dispatch site in two, the change that was worth 30%
to Stitch and 50% to Wasmi: the generated handlers already have one site per outcome, and
question 11 has the disassembly, the reason it holds, and the one change on this list that would
end it.

## 7a. The generated path measured, and four things that turned out to matter

Zen 4 (Threadripper 7970X) Coremark went **2384 → 2472 → 2599 → 2948** over the four changes below,
which is past the prototype's 2666. Xeon B numbers are given where they were the ones measured
against; where the two machines disagree, that is the point being made.

**1. Drop glue on the instruction fetcher.** The fetcher is moved through the handler chain by
value, so whichever handler ends execution drops it — and every handler that *can* fail is a
candidate. LLVM therefore gave each load, store, branch and jump handler a stack frame with
callee-saved pushes and a spill of the fetcher, in the **hot** path, for cleanup only reached on
failure. `lw` was 27 instructions before its dispatch tail where it needed 19.

The fix is that the fetcher must have no destructor: ownership of the decoded stream moved into a
separate type that the fetcher borrows, leaving it a plain 16-byte `Copy` cursor, asserted
`!needs_drop`. Callers that run the same program repeatedly no longer need a clone either. Xeon B
Coremark 1949 → 2195 (**+12.6%**), BLAKE3 19.6 → 18.1 µs, ed25519 1.076 → 0.985 ms.

This is worth stating as a rule rather than a fix: **anything moved through a `become` chain by
value should be `!needs_drop`**, because the cost lands on every handler that has a failure path,
not on the one that actually drops it.

**2. The failure half of a relative branch.** Working out what is wrong with a branch target — the
return trap, unaligned, outside the program — was inlined into the handler of every branch and jump:
`beq` was 71 instructions, 29 of which never execute.

Moving it out with an ordinary call is *worse*, because the handler resumes afterwards, so
everything it holds must survive a call that clobbers every volatile register — six pushes and pops
land in the hot path. A tail call costs nothing instead, since the handler is not coming back, but
`become` demands identical signatures and there is no argument left to carry the error in.

So the fast path stopped producing one. `ProgramCounter::set_pc_relative()` split into
`try_set_pc_relative()`, which answers with a `bool` and leaves a refused target in the program
counter, and `failed_branch()`, which is `#[cold]` and reads it back to say what was wrong with it.
Because the refused target is *in* the program counter, the continuation needs no arguments of its
own, which is what lets it match the handler signature — so it is **one** cold function per
instruction set rather than one per variant. `beq` is now 31 instructions with no frame at all, `cj`
60 → 21. Xeon B Coremark +3.1% median of five interleaved runs; Zen 4 2472 → 2599.

**3. Handler alignment, and how much of all this is placement.** Change 2 left every hot handler
byte-for-byte identical and merely *moved* them — `add` went from starting at offset 0 of a cache
line to offset 16 — and on Zen 4 the interpreter benchmarks moved **15%** for it, in the opposite
direction to Coremark in the same build. Handlers are entered by jumping to them, so one that starts
part-way into a line spans one more line than it needs to, and that is paid on every guest
instruction.

`#![feature(fn_align)]` and `#[rustc_align(64)]` on every generated handler (~4 KiB of padding per
instruction set) took Zen 4 Coremark to **2948**, the best number anywhere in these documents,
prototypes included, and the threaded benchmarks to their best as well. On Xeon B it is a wash on
the mean — Coremark +1.8%, benchmarks −1.8% — but it halved the run-to-run spread on all three
workloads.

**4. …and the limit of that, which is where this stops.** Aligning the handlers left the `match`
loop to the linker, so the loop moved instead: `BasicInterpreterState::execute()` went from
line-aligned to offset 32, and on Zen 4 the loop benchmarks lost 20% in the same build that set the
Coremark record. Aligning the loop too was tried, and it is one attribute rather than 145, since the
loop is a single function and anchoring its entry fixes the offset of every block inside it.

It did not settle anything. On Zen 4 the loop gained 6% and **the threaded path lost 11% — and the
threaded path shares no code with the function that was aligned.** On Xeon B the whole spread across
the three builds was 4%, which is why that machine could not see any of this.

That is the conclusion worth keeping, and it is a methodology result rather than a dispatch result.
**In a binary this size, layout noise is larger than the effects being chased, and source-level
alignment cannot fix it** — aligning one thing re-lays out everything else, so each attempt is a
fresh roll rather than a convergence. The tools that would actually settle it are PGO or BOLT,
neither of which was tried. Until one of them is:

- Read single-configuration A/B results on the interpreter benchmarks as having a **±15% floor on
  Zen 4**, and do not chase anything smaller through them.
- Prefer Coremark, which is one binary running one program, over the Criterion benchmarks, which
  link the interpreter next to BLAKE3, ed25519 and Criterion itself and move whenever any of that
  moves.
- Treat a change that moves a benchmark sharing **no code** with it as evidence about layout, not
  about the change.

Both alignment changes are on `main` — `#[rustc_align(64)]` on every generated handler in
`generate_threaded_fns.rs`, and the same on `BasicInterpreterState::execute()`. The handler one is
supported by the Coremark record and by halving the spread on three workloads; the loop one is not
supported by anything and is a candidate to revert if it ever gets in the way.

Two things measured along the way and not taken:

- **`-C relocation-model=static`** removes the `leaq` that re-materialises the dispatch table
  address, one instruction per dispatch: Xeon B Coremark 2137 → 2237, **+4.7%**, robust across five
  interleaved runs. It costs ASLR of the executable image and cannot go in `RUSTFLAGS` (proc-macro
  crates fail to link non-PIC), only `cargo rustc -- -C relocation-model=static` on the final
  target. This revises question 2's "unlikely to show up"; it does show up, it is simply not free.
- **LLVM `-hot-cold-split`** does nothing to these functions, and
  `#[unsafe(link_section = ".text.unlikely")]` on the cold continuation does not move it either —
  lld merges the section into `.text` without reordering unless given
  `-z keep-text-section-prefix`.

## 7b. What the PGO build is limited by, and why prefetching is not it

PGO settles the layout problem §7a point 4 ran into — it is the tool for it, and with it the Zen 4
benchmarks land at **20.2 µs** for BLAKE3 and **863 µs** for ed25519 threaded, both best-ever. It
also makes the machine measurable again, so `perf stat` over the BLAKE3 run finally says what the
interpreter is waiting on. Four consecutive one-second samples, counters multiplexed at ~27%:

| | IPC | branches | mispredicted | ≈ mispredict cost | front end idle | L1i misses | L1d misses |
|---|---|---|---|---|---|---|---|
| | | per insn | of branches | of cycles | of cycles | per G insn | per G insn |
| t+7 | 3.54 | 9.8% | 0.47% | 2.9% | 20.8% | 200 | 69 283 |
| t+8 | 3.46 | 9.9% | 0.42% | 2.6% | 22.8% | 206 | 71 235 |
| t+9 | 3.41 | 9.8% | 0.70% | 4.2% | 22.7% | 189 | 69 522 |
| t+10 | 3.13 | 9.8% | 1.34% | 7.4% | 26.0% | 250 | 71 164 |

Mispredict cost assumes ~18 cycles. Read the shape, not the third decimal — the counters are
multiplexed and the same run's TLB ratios come out impossible.

**Both caches are effectively perfect.** 200 instruction-cache misses per *billion* instructions,
and 71 000 data-cache misses per billion — under a tenth of a per mille of instructions, on a
workload whose guest data is a 1 KiB chunk and whose decoded stream is walked strictly forwards, so
the hardware prefetcher has the easiest possible job. Handler alignment and PGO between them took
the instruction side to zero.

**The limiter is the front end**, idle 21–26% of cycles with no instruction-cache misses under it.
That is not memory: it is dispatch redirects. Every guest instruction ends in an indirect jump, so
every guest instruction truncates a fetch window and asks the front end for a new one; 9.8% of all
retired instructions are branches. Mispredicts are a real but secondary 2.6–7.4%. IPC of 3.1–3.5
says the *back* end is doing fine — this is a machine that can execute faster than it can be fed.

**Which is why `std::hint::prefetch_read`/`prefetch_write` are the wrong tool here**, and not
narrowly:

- They are **data** prefetches. `hint_prefetch` lowers to `prefetch_read_data`/`prefetch_write_data`
  — `prefetcht0` and friends. There is no instruction-cache prefetch in the API, and x86 has nothing
  to lower one to outside very recent Intel parts. The cache that would matter cannot be reached.
- There is **nothing to prefetch** on the data side either. A prefetch buys the latency of a miss
  that would otherwise happen; at 0.007% of instructions there is no such miss to buy.
- And the cost lands on **exactly the scarce resource**. A prefetch is an instruction: it is
  fetched and decoded like any other. One per dispatch, against a handler of 12–16 instructions,
  is 6–8% more front-end work — spent to relieve a resource that is 99.99% hit, taken from the one
  that is idle a quarter of the time. It is negative-sum before it does anything.

The general form: *prefetching relieves the back end by spending the front end, and this interpreter
is front-end bound.* The addresses are also not available early enough to help even where a guest
program does miss — a load's address comes out of a guest register during that instruction's own
handler, so there is nowhere earlier to put the hint.

What *does* target the limiter, in order of how much: fewer dispatches (superinstructions — killed
on distribution grounds, question 5), and fewer instructions per dispatch. Note that the second is
what direct threading does — it removes two front-end instructions per guest instruction — so of the
alternatives measured, it is the only one aimed at what the machine is actually short of. That does
not change the 8x memory verdict, but it does mean the 4.4% it measured is unlikely to be noise in
the way the padding result was.

## 8. Open questions

1. ~~**Does `extern "rust-preserve-none"` pay?**~~ **Answered: not today.** With the arguments
   ordered coldest first it rescues `basic` (−5.6%) and `branchless` (−8.3%), but it ties with
   `zerostore` + `extern "Rust"`, which was already at the floor. Since the feature is unstable, a
   tie is a reason not to take it. Revisit **only** when the handler signature grows past six
   arguments (§4), which is the case where `extern "Rust"` starts spilling and preserve-none's
   twelve registers actually pay for themselves.
2. **What is the loop limited by?** **Answered: store-to-load forwarding failures, and the
   `x0` handling that inflates them.** Zen 4, per guest instruction:

   | mode | cycles | host-insn | IPC | ind-mispred | i$ | d$ | loads | stores | stlf | stlf-fail |
   |---|---|---|---|---|---|---|---|---|---|---|
   | generic | 11.34 | 44.55 | 3.93 | 0.0038 | 0.0000 | 0.0014 | 8.04 | 1.43 | 0.31 | 0.0331 |
   | threaded | 6.14 | 11.75 | 1.91 | 0.0108 | 0.0000 | 0.0012 | 6.58 | 1.58 | 0.37 | **0.1513** |

   Caches, branch prediction and issue width were all ruled out. What is left is `stlf-fail`: loads
   that could **not** be forwarded from an in-flight store and had to wait for it to reach the
   cache. The threaded loop suffers **4.6x more of them than the generic loop**, and at a plausible
   10–15 cycles each that is **1.5–2.3 cycles of the 6.14, a quarter to a third of all runtime**.
   Successful forwards (`stlf`, 0.37) are not the problem; they cost about what an L1d hit costs.

   `cycles:pp` attribution inside `CMv`, the hottest handler, says where:

   ```
    11.36    movzbl  0x3(%rdi), %eax        # rs2 index, out of the instruction
     7.64    movzbl  0x4(%rdi), %r10d       # rd index, out of the instruction
     9.59    leaq    0x58bd8(%rip), %r11    # re-materialise the dispatch table address
    11.70    movq    (%rsi,%rax,8), %rax    # read the guest register
    18.03    movq    %rax, (%rsi,%r10,8)    # write the guest register
     7.96    movzbl  0x8(%rdi), %eax        # next discriminant
     5.29    addq    $0x8, %rdi
    12.64    movq    $0x0, (%rsi)           # ZeroStoreRegisters re-zeroing x0
    15.78    jmpq    *(%r11,%rax,8)         # dispatch
   ```

   The two stores together are **30.7%** of the handler, and one of them exists only to undo a write
   to `x0` that may not even have happened. Both store addresses are also derived from a *load* (the
   `rd` byte), so they resolve late, and a store with an unresolved address is exactly what blocks a
   younger load from forwarding.

   Note this reframes the earlier axis-3 result rather than contradicting it. `ZeroStoreRegisters`
   beat the branching and `cmov` files because both of those put even more work between the loaded
   `rd` byte and the resolved store address. All three were paying the same tax; `zerostore` paid it
   least.

   **The fix is to stop doing `x0` handling at run time at all.** The stream is pre-decoded, so the
   decoder can rewrite the `rd` field of any instruction whose destination is `x0` to a sink slot,
   giving a 33-slot register file where slot 0 is never written and reads of `x0` are therefore
   always zero. That leaves one store per register write, no branch, no conditional move, and no
   re-zeroing store — strictly better than all three register files measured so far, at zero run-time
   cost, and it is only available *because* the stream is decoded ahead of time. Expected to remove
   ~0.8 stores per guest instruction and a large share of the forwarding failures.

   **Status: understood, deliberately not acted on yet.** `Reg<Type>` is an enum with one variant
   per architectural register, not a newtype over an offset, so a 33rd slot is a change to the shared
   register types rather than something a prototype can fake. The register file and register types
   are generic on purpose, so an optimised implementation is free to require 33 slots and to rewrite
   instructions after decoding — the option exists and is worth remembering, but taking it is a
   design decision about the shared types, not a local experiment.

   Two smaller things visible in the same listing, both closed:

   - `leaq` re-materialises the dispatch table address in **every** handler, one instruction in nine.
     `-C relocation-model=static` does not build: it applies to every crate including proc-macro
     dependencies, which must be position-independent, so `syn` fails to link with
     `R_X86_64_32 ... recompile with -fPIC or link with -no-pie`. Restricting it to the final crate
     (`cargo rustc -p ab-riscv-coremark-runner --release -- -C relocation-model=static
     -C link-arg=-no-pie`) would avoid that, but the instruction is independent and the loop is
     latency-bound, so removing one of eleven instructions is unlikely to show up. Not worth
     pursuing. The other route — passing the table base as a seventh argument — remains the concrete
     case for `extern "rust-preserve-none"` noted in question 1.
   - The load and store handlers spend 8–9% on the `VirtualMemory` bounds check (`cmpq`/`jae`).
     **This is required and is not a target.** The interpreter runs blockchain consensus, so bounds
     checking is part of the correctness contract, not overhead to be optimised away.

3. ~~**How handlers are reached.**~~ **Reopened and measured; still not taken.** It was dropped on
   the reasoning that an inline handler pointer only removes a dependent load the branch predictor
   already hides, so the whole misprediction budget it could recover is 3.5%. That reasoning was
   about the wrong thing: what it actually removes is two *instructions* per dispatch, and it is
   worth more than the misprediction budget.

   `COREMARK_DISPATCH=direct` is now a mode of the prototype, sharing the `ops!` instruction
   table with the other two. Xeon B, four interleaved rounds, median: token threading **2160**,
   direct threading **2256**, **+4.4%**. The same change made through the real emitter, on a
   throwaway branch, measured **+5.3%**.

   The dispatch tail is what changes:

   ```
   token                              direct
   mov    0x10(%rsi),%rdi             lea    0x20(%rax),%rsi
   add    $0x10,%rsi                  mov    0x20(%rax),%rdi
   movzwl %di,%eax                    jmp    *0x28(%rax)
   lea    table(%rip),%r11
   jmp    *(%r11,%rax,8)
   ```

   Note the `leaq` that question 2 below calls "one instruction in nine" and closes as not worth
   pursuing: direct threading removes it as a side effect, along with the discriminant extraction
   and the table load. Removing it *alone* is indeed not worth pursuing; it comes free here.

   **The cost is the reason it is not taken.** The slot goes from 8 bytes to 16, and the slot is
   already 8 bytes for every 2 bytes of guest code, so the decoded program goes from 4x the size of
   what it interprets to **8x**. For Coremark, whose `.text` is 27,580 bytes, that is 108 KiB
   today against 215 KiB.

   **The part of it that looked like a padding effect was noise.** Measured through the real
   emitter, padding the slot to 16 bytes *with nothing in it* — same token-threaded dispatch, same
   instructions, purely a wider stride — came out **+2.2%** on Xeon B, which was written up here as
   the interesting half and as something to chase with an explicit prefetch. It is neither.
   +2.2% is *inside* that machine's own spread (§7a point 4 puts it at 3.3% on Coremark, and the
   two distributions overlap: 2203–2252 against 2233–2299), and §7b shows there is nothing for a
   prefetch to do. Read the whole **+5.3%** as the dispatch change, and treat "the wider slot pays
   for itself" as unsupported.

   **The 8x is avoidable, and it is faster than paying it.** The handler reference does not have to
   be a pointer. A 32-bit offset from an anchor handler costs three extra instructions at the jump
   — x86-64 has no `jmp *m32`, so it becomes `movslq`, `lea anchor(%rip)`, `add`, `jmp *reg` where a
   pointer needed only `jmp *mem` — and measures free anyway, because dropping the pointer also
   drops the need to keep the pre-advance slot address live, which had been costing a `push`/`pop`
   of a callee-saved register. Both `Add` handlers come out at exactly 12 instructions.

   That leaves four bytes, and **the decoded operands fit in them for every format**, because the
   handler reference is what replaces the discriminant: R-type is `rs1 + rs2 + rd` = 3 bytes, I-type
   `rs1 + rd + i16` = 4, S-type `rs1 + rs2 + i16` = 4, J-type and U-type `rd + I24` = 4. The one
   over budget is B-type at `rs1 + rs2 + I24` = 5, and only because branches carried an `I24`
   immediate; a B-type offset is thirteen bits signed, so narrowing that operand alone to `i16`
   brings it to 4 **and keeps it byte-aligned**, which is the whole point. That narrowing has since
   been made in the decoded instructions themselves rather than only in the prototype's packing, so
   every B-type variant carries an `i16` and the budget is met without a special case. `Jal`
   (21 bits), `CLui` (18) and `Lui`/`Auipc` (20 once shifted) are the `I24` uses that genuinely
   need the third byte.

   `COREMARK_DISPATCH=packed` is that: `{ i32 handler_offset, [u8; 4] operands }`, eight bytes,
   exactly what the decoded enum costs today. Xeon B, fourteen interleaved rounds, median:

   | back end | slot | it/s | vs indirect |
   |---|---|---|---|
   | indirect (token) threading | 8 B | 1856 | — |
   | direct, 8-byte handler pointer | 16 B | 1987 | +7.1% |
   | **direct, packed** | **8 B** | **2060** | **+11.0%** |

   So the packed form is not a compromise against the 16-byte one — it **beats** it by 3.7%, at half
   its footprint, which is the same footprint as indirect threading. The handlers are the same size
   where it matters (`Add` 12 instructions in both, `Beq` 27 in both); `Lw`, `Sd` and the compressed
   forms are two longer. What is left over is the stream being half as big.

   Two things had to be right, and each was worth several per cent on its own:

   - **Every operand must be its own load.** Loading the packed word once and extracting from the
     register it lands in keeps that register live across the handler and costs a callee-saved
     spill: `Add` went to 19 instructions with a two-register frame. Reading each operand at its own
     constant byte offset — `movzbl 0x4(%rdi)`, `movzbl 0x5(%rdi)`, `movzbl 0x6(%rdi)` — is 12 with
     no frame.
   - **The narrow operand must stay a whole number of bytes.** Narrowing B-type by bit-packing
     everything (`rs1` and `rs2` to five bits) put `Beq` at 38 instructions against 27. Narrowing
     only the immediate, from three bytes to two, puts it back at 27, because it is still one
     `movswl`.

   And it is worth being explicit that this is *not* the same as storing the raw four-byte guest
   instruction and re-decoding in the handler. Raw RISC-V scatters its immediates — B-type is bits
   31, 7, 30:25, 11:8 — which is six to eight operations per branch against one `movswl`, and puts
   register fields at bit rather than byte offsets. The decode is worth keeping; it is the
   *discriminant* that becomes redundant.

   Two things it costs. Per-format packing breaks the uniform `get_rs1_rs2_operands()` that the
   `match` loop uses to read `rs1`/`rs2` before dispatching, so the two paths would stop sharing a
   decoded stream. And fusion (question 5) gets a hard budget rather than a soft one: a fused pair's
   combined operands have to fit the same four bytes, which two R-type instructions would not.

   Measured, in the prototype, over all 147 variants. Direct threading costs no memory at all.

   Two traps, both of which make a naive direct-threaded build come out *slower* rather than
   faster, and which cost most of the time spent on this:

   - **`FetchInstructionResult` has a niche.** Its non-instruction variants are encoded in
     out-of-range values of the instruction's own discriminant, so constructing it from a raw read
     compiles to a range check against the variant count plus a branch into a twelve-way switch
     that builds an `ExecutionError`. Today that check is free because the `match instruction`
     selecting the handler absorbs it into the same jump table. Remove the table and it
     materialises in every handler.
   - **The dispatch-result enum has the same niche, for the same reason.** It is free today and is
     not free once the table is gone; the emitter experiment had to return a plain tuple instead.

   Between them, those two were the difference between 1700 and 2354 iterations/sec. Anyone
   measuring a direct-threaded variant that comes out slower should look there first. In the
   prototype neither exists, because it reads the instruction out of the slot directly.
4. **How `ExecutableInstruction` exposes its handlers.** An associated *slice* — no `COUNT` constant
   is needed. The open part is how that composes across extension traits.
5. **Superinstructions, and the case against them here.** Fusing an adjacent pair removes one
   dispatch *and* one register-file round trip, which is exactly what question 2 says is expensive.
   `COREMARK_HISTOGRAM` now reports adjacent executed pairs, counting only those where the first
   instruction fell through — the condition under which the two are also stream-adjacent, so a pair
   it counts is one that could actually be fused. The distribution is flat: **913 distinct pairs,
   the most common at 2.14%, and the top 20 together only 37%**. 88.7% of instructions have a
   fusable successor, so the opportunity is real, but capturing it needs tens of superinstructions
   for a fraction of the gain each. Worth knowing before anyone assumes fusion is the obvious next
   move.

   **Compare-and-branch specifically, since that is the pair Wasmi's fix is about.** RISC-V does not
   need it fused: a B-type instruction *is* a compare and a branch, so `beq` and friends are already
   the direct equivalent of Wasmi's `branch_i32_lt_ri`, and question 11 is about exactly those
   handlers. What fusion would add here is *compute*-and-branch, and Coremark's top four such pairs
   are `c.mv+c.bnez` 2.04%, `c.addw+bltu` 1.93%, `addi+beq` 1.76% and `lh+bne` 0.96% — the same flat
   distribution as everything else, and the same verdict.

   **It would not change question 11's answer either**, which is worth stating because it looks like
   it should. What decides whether a branch handler's two outcomes share a dispatch site is what the
   *taken* arm does, not how many guest instructions the handler covers: a fused `addi+beq` still
   validates its target and still tail-calls the cold continuation when the target is refused, so
   its two arms are still not interchangeable. If anything fusion helps, because a fused branch is a
   handler of its own and therefore a dispatch site of its own. The one way fusion *would* make
   question 11 live is the way already recorded there — if the fusing pass resolves branch targets
   while it rewrites the stream, which is a natural thing for it to do, the validation leaves the
   handler and the two arms collapse to a pointer choice.

   For reference, conditional branches are **17.6%** of Coremark's executed guest instructions:
   `beq` 5.01%, `bltu` 3.52%, `bne` 3.11%, `c.bnez` 2.74%, `c.beqz` 1.64%, `bge` 0.73%, `bgeu`
   0.56%, `blt` 0.28%. All the figures in this question come from `COREMARK_HISTOGRAM=1` at
   `COREMARK_ITERATIONS=3000`, which is exact rather than sampled, so they can be reproduced
   without a quiet machine.
6. **The ~500-variant vector composition.** The 89→147 experiment shows `match` degrading while
   `call`/`tail` do not, but 500 arms is a different regime for I-cache, and this is where the
   existing vector-extension regression lives. The most valuable case to prove.
7. **Whether `Branch` should be relative to the *next* instruction** rather than this one. It would
   remove a `- instruction_size` from every relative branch, at the cost of changing the shared
   enum's contract and every instruction that produces it.
8. ~~**`branchless` versus `zerostore` register files.**~~ **Answered: `zerostore`.** It beat `basic`
   by 6.3% with `-C target-cpu=znver4` and 8.8% on the default build, with `branchless` between them
   at −2.3% rather than level with it. Neither Xeon could separate the three. `ZeroStoreRegisters` is
   now the only one implemented; the other two are gone with the rest of the removed back ends
   (§3) and would have to be written again if the trade ever needs revisiting, which it would if
   the handler signature changes enough to alter register pressure.
9. ~~**Would explicit prefetching help?**~~ **Answered: no, and not for a reason specific to this
   workload.** `std::hint::prefetch_read`/`prefetch_write` are *data* prefetches with no
   instruction-cache counterpart in the API, both caches are already at 200 and 71 000 misses per
   billion instructions after PGO, and a prefetch spends front-end bandwidth, which §7b shows is the
   one resource the interpreter is short of. The addresses would also not be available early enough
   to help a guest program that does miss: a load's address comes out of a guest register inside
   that instruction's own handler. §7b has the numbers.
10. **PGO, and what it leaves.** PGO is now the answer to §7a point 4 — it removes the layout noise
    that source-level alignment could not — and it is what the Zen 4 numbers in §7b were taken
    under. What it does *not* remove is the front-end bound those numbers show. BOLT is the
    remaining untried tool; whether it finds anything on top of PGO for a program whose hot code is
    145 small functions reached only indirectly is an open question, and a fair one to be
    pessimistic about.
11. ~~**Does the collapsed-branch-site deoptimization that cost Stitch 30% and Wasmi 50% apply
    here?**~~ **Answered: not today, and the reason says exactly when it would.**

    The mechanism, from [Wasmi 2.0's write-up][wasmi-deopt]: a tail-threaded handler for a
    conditional branch naturally has two continuations, and if it is written as one instruction
    pointer chosen between them — `ip = if cond { target } else { fall_through }` followed by a
    single dispatch tail — the two collapse into a conditional move feeding **one** indirect jump.
    The branch predictor then has a single entry carrying the mixed history of both outcomes.
    Rust 1.92 enabling the `DestinationPropagation` MIR pass by default
    ([rust-lang/rust#142915]), which merges locals holding the same value, is what produced that
    collapse in [Stitch]: its CoreMark score fell from over 3000 to ~2200 on an M2 Pro between
    1.91 and 1.92. The fix in both projects is the same one line of shape — dispatch from inside
    each arm instead of choosing a pointer for a shared tail ([stitch@3280ff6],
    [wasmi-labs/wasmi#2027]) — and it took Wasmi 2.0 from ~2800 to over 4200, the single largest
    optimization of that release.

    [wasmi-deopt]: https://wasmi-labs.github.io/blog/posts/wasmi-v2.0/#accidental-rust-deoptimization

    [rust-lang/rust#142915]: https://github.com/rust-lang/rust/pull/142915

    [Stitch]: https://github.com/makepad/stitch

    [stitch@3280ff6]: https://github.com/Robbepop/stitch/commit/3280ff672c861a1e73107c9b1d393b06127e27ad

    [wasmi-labs/wasmi#2027]: https://github.com/wasmi-labs/wasmi/pull/2027

    **On paper this interpreter has the same shape.** A branch's arm returns
    `ExecutionResult::Branch { offset }` when taken and `ContinueNoWrite` when not, the handler
    applies whichever came back, and both then fall through to the one dispatch tail the emitter
    puts at the end (§9). Nothing in the source says the two outcomes should leave through
    different jumps.

    **In the generated code they already do, on both host architectures.** All eight conditional
    branch variants — `Beq`, `Bne`, `Blt`, `Bge`, `Bltu`, `Bgeu`, `CBeqz`, `CBnez` — come out
    with two indirect jumps and no conditional move, x86-64 and aarch64 alike. Across the Coremark
    runner's 150 generated handlers the only conditional moves anywhere are value selects that
    belong where they are: `Min`, `Max`, `Minu`, `Maxu`, `Orcb`, and on aarch64 also `Slt`, `Sltu`,
    `Slti`, `Sltiu`. `Beq` on x86-64, with the two dispatch sites marked:

    ```asm
      mov    %edi,%eax                 ; rs1 index, out of the instruction
      shr    $0xd,%eax
      and    $0xf8,%eax
      mov    (%rcx,%rax,1),%rax        ; read the guest register
      mov    %edi,%r10d                ; rs2, the same way
      shr    $0x15,%r10d
      and    $0xf8,%r10d
      cmp    (%rcx,%r10,1),%rax
      jne    .not_taken
      shl    $0x10,%rdi                ; taken: extract and scale the offset
      sar    $0x2e,%rdi
      add    %rdi,%rsi                 ; move within the decoded stream
      mov    %rsi,%rax
      sub    %rdx,%rax
      add    $0xffffffffffffffe8,%rax
      test   $0x7,%al                  ; is the target aligned?
      jne    <branch_failed>
      mov    (%rdx),%rdi               ; is it inside the decoded stream?
      shl    $0x3,%rdi
      cmp    %rdi,%rax
      jae    <branch_failed>
      mov    (%rsi),%rdi
      movzwl %di,%eax
      lea    <table>(%rip),%r11
      jmp    *(%r11,%rax,8)            ; dispatch site, taken
    .not_taken:
      add    $0x10,%rsi
      mov    (%rsi),%rdi
      movzwl %di,%eax
      lea    <table>(%rip),%r11
      jmp    *(%r11,%rax,8)            ; dispatch site, not taken
    ```

    **Why it holds, which is the part worth carrying.** The two arms are not a choice between two
    pointers. The taken one resolves the offset into a stream position and validates it — aligned,
    and inside the decoded stream — and a refused target leaves the function for
    `..._threaded_branch_failed` (§7a point 2). The other is `add $0x10,%rsi`. There is no value
    both arms produce for a pass to merge, and no way to if-convert a path that conditionally
    tail-calls somewhere else. Stitch and Wasmi collapsed precisely because their two arms *were*
    only a pointer choice, with nothing else between the comparison and the dispatch. So this is
    structural here rather than a heuristic that happened to go the right way, and a compiler
    upgrade on its own should not be able to take it away.

    **What would take it away is a change already on this list.** The moment a branch target stops
    being validated inside the handler — decode-time target resolution, or the packed
    direct-threaded slot of question 3, either of which turns the taken arm into a pointer the
    slot already holds — the two arms become exactly the pointer choice that collapses. Apply the
    fix in the same change rather than before it, and re-check afterwards.

    **The fix, when it is needed**, is in `generate_threaded_fns.rs`: interpolate the dispatch tail
    into the success path of the `ExecutionResult::Branch` arm instead of letting it join the
    fall-through. Built that way today it produces the same instruction count per handler and the
    same `.text` size on the Coremark runner, and costs about 10% more time to compile
    `ab-riscv-interpreter`, so there is nothing to gain by carrying it early.

    **How to re-check** — a build and one command, no benchmark and no Zen 4 needed, since this is
    a structural question of the kind §6 says a development machine can settle:

    ```bash
    objdump -d target/release/ab-riscv-coremark-runner \
        | awk '/^[0-9a-f]+ </ {fn = $2} /\tcmov/ {c[fn]++} /\tjmp +\*/ {j[fn]++} \
               END {for (f in j) if (j[f] == 1 && c[f]) print f}' \
        | grep -o 'instruction_[a-z0-9_]*_threaded' | sort -u
    ```

    It lists the generated handlers that have a conditional move and only one dispatch site. Today
    it prints `min`, `minu`, `max`, `maxu` and `orcb` and nothing else; a branch instruction
    appearing in it is the regression. On aarch64 the same check reads `csel` and `br` instead.

    **One instantiation does collapse today**, and it is worth knowing about because it is the
    working demonstration rather than a problem: the interpreter's own test binary, where
    `TestInstructionFetcher` keeps the program counter in memory so that both arms are "program
    counter += delta, store it", identical apart from the delta. That merges to a `cmove` and one
    dispatch site, exactly as described above. Nothing measures that instantiation, but it is what
    to look at when reasoning about a fetcher that does not validate.

    **And the ceiling, if it ever does become live here.** Indirect mispredicts are 0.0108 per
    guest instruction on Zen 4 (question 2) — roughly 3% of runtime at ~18 cycles each. This
    workload is nothing like Wasmi's 50% case; even a perfect fix would be worth a few per cent.

## 9. What was built, and where it diverged from the plan

What the generated code actually looks like, the places the build had to depart from what was
planned and why, and the alternatives that were deliberately left out so that they are not
rediscovered as ideas. There was a separate `DISPATCH-PLAN.md`; it was implemented and deleted, and
this is what outlived it.

### The shape of it

Both emitters run, by default, and the two paths coexist: `ExecutableInstruction::execute()` and
its `match` where code size matters, generated handlers where throughput does. Not one of the ~500
instruction arms was rewritten — since `fb1eaaa` they are already extracted into standalone
per-variant functions (§1), so the handler emitter is a third consumer of work that exists rather
than something that re-derives it.

Nothing new was needed from the shared traits. `Instruction`, `RegisterFile`, `VirtualMemory`,
`InstructionFetcher` and `ProgramCounter` are reused unchanged, and `ExecutionResult` is
load-bearing rather than merely reused: it is what lets the arms stay untouched, because an arm
*describes* where execution goes next and the handler epilogue is what acts on it. The
`ExecutionResult` in the middle costs nothing — it is a local that never escapes, so it scalarises
away; that was checked by building enum-mediated and hand-written versions of three handlers and
comparing the disassembly.

Default-on generation is safe because unused generated code costs nothing: handlers are generic
over the register file, memory, fetcher and environment, so one is only instantiated when something
calls it. A crate that only ever calls `execute()` never monomorphizes a handler.

Generated code is read post-expansion in `$OUT_DIR/{EnumName}_execution_impl.rs`; errors reported
against those files are errors in the emitter, not in the instruction sources.

### The four divergences that matter

**1. It is a new trait, not a method on `ExecutableInstruction`.** That was the largest divergence,
and the reason is hard rather than stylistic: `ExecutableInstruction` is a `const trait`, and calls
through a function pointer are not allowed in `const fn`. Dispatch is a table of function pointers,
so `execute_threaded()` can never be const. Hence `ThreadedExecutableInstruction`.

**2. `become` requires an exact signature match, and that reshaped dispatch.** The plan had each
handler fetch its own next instruction and `become dispatch(…)`, with `dispatch` doing
`become handler(…)`. That cannot be built: a handler necessarily takes the instruction it is about
to execute and dispatch necessarily does not, so written that way it produces thousands of
mismatched-signature errors. What was built splits the roles:

* **The instruction is an argument.** A handler's first parameter is the already-decoded
  instruction; it never fetches for itself. Every handler has exactly the same signature, which is
  what makes `become` legal between any two of them.
* **`dispatch_{enum}` is `#[inline(always)]` and returns.** It fetches, selects, and hands back both
  halves through a private `{Enum}ThreadedDispatchResult` — `Next { instruction, handler }`,
  `Break`, or `Err(…)`. The caller does the `become` itself. Being inlined and non-escaping, that
  enum never materializes.
* **`FetchInstructionResult::Continue` is a loop inside dispatch**, not a re-entry into the chain,
  which is what lets dispatch return a value rather than being a tail call.

This is the risk the plan's own risk list missed, and it is the one that cost the most.

**3. The fetcher is not carried back, and the outcome comes home in registers.** The plan had the
result type return the fetcher, since the fetcher travels into the chain by value and an address
alone would not be enough to resume from. That is not what was built, and the reason is that it was
solving a problem nobody had: the callers that run the same program repeatedly — the Coremark
runner and the interpreter benchmarks, which are the two that care — keep their own copy of the
fetcher and set the program counter on it before each run, so what execution has to hand back is
just *where* it stopped. `ThreadedExecutionResult` is therefore
`{ program_counter: Address<I>, outcome: Result<(), ExecutionError<Address<I>>> }` and nothing more.

Handlers do not return even that. They return `OpaqueThreadedExecutionResult`, the same information
flattened into a shape the target returns in registers rather than through a hidden out-pointer —
a 256-bit vector on x86-64, a homogeneous aggregate on aarch64, and still memory anywhere else,
where nothing special is needed for it — which is what keeps an argument register free for the whole
chain and costs no per-instruction stores. The x86-64 form executes AVX
instructions to build, so the handlers carry `target_feature(enable = "avx")` and
`execute_threaded()` checks `platform_supported()` once before entering the chain rather than per
handler. Miri emulates a CPU without AVX and refuses to call a function requiring it, so the
attribute is `cfg_attr`-gated off under Miri unless the build already targets AVX.

**4. Handlers are pinned to an explicit ABI everywhere, not only on Windows.** The plan pinned
`extern "sysv64"` on Windows x86-64 and left every other target on `extern "Rust"`, because the
default Rust ABI on Windows gives four argument registers rather than six (§4) and Windows x86-64 is
in this project's CI. What shipped pins `extern "sysv64"` on **all** x86-64 and `extern "C"`
everywhere else, and the reason is divergence 3 rather than the arguments: `extern "Rust"` returns
`OpaqueThreadedExecutionResult` through memory even where the platform's own convention returns it
in a register. That is a rustc bug ([rust-lang/rust#161381]), and there is a `TODO` on
`handler_abi()` to drop the pinning everywhere except Windows x86-64 once it is fixed. `become` does
accept an explicitly pinned ABI — verified by pinning to `extern "win64"` on Linux x86-64, a
genuinely non-default convention there: it compiles, works through generics and non-FFI-safe types,
and dispatch still lowers to an indirect `jmp` through the table.

[rust-lang/rust#161381]: https://github.com/rust-lang/rust/issues/161381

**The decision is made in the build script, and that is the only place it can be made correctly.**
`CARGO_CFG_TARGET_ARCH` describes the target being compiled for, and `OUT_DIR` is per-target so
nothing goes stale. A proc macro could not do it: proc macros run on the **host**, so a `cfg!()`
inside one describes the wrong machine under cross-compilation, and the only sound proc-macro
alternative is emitting every handler twice behind `#[cfg]`. A `macro_rules!` wrapper would work but
has to expand whole items — the ABI is part of an item's syntax, not an attribute — and would have
to be exported, putting an implementation detail of generated code into the crate's public API.

Two costs worth knowing. Pinning makes the handlers `extern` in the eyes of
`improper_ctypes_definitions`, which objects to the instruction enum, the result type and the
generic parameters; the generated items carry an `allow` for it rather than the `expect` the
workspace lints normally demand, and it is sound because every caller and callee is generated into
the same crate. And an explicitly pinned ABI is **not free even where it coincides
with the platform default**: forcing `sysv64` on Linux, where it is already the convention, moved
154 instructions across the binary, because `extern "Rust"` carries parameter optimization
attributes an explicit ABI does not. That cost is now paid unconditionally on x86-64 and bought
back with interest by the register return (divergence 3), which is the trade the `TODO` on
`handler_abi()` would revisit if rust-lang/rust#161381 is fixed. On Windows the same pinning is
against a baseline of four argument registers instead of six, so it should be a clear win there
regardless, but it is unmeasured on that platform.

### Smaller things the plan did not anticipate

- **`ExecutableInstructionOperands` is used by handlers after all.** Each handler opens with the
  whole-enum `get_rs1_rs2_operands()` and reads both source registers, exactly as the `match` loop
  does. That is not a regression to the shared preamble: the `let … else` destructure that follows
  ends in `unreachable_unchecked()`, so the whole-enum match folds to the one variant's arm and dead
  code elimination removes whichever reads the instruction does not use. Verified on release
  disassembly — `Lui`, `Auipc`, `Jal`, `Fence`, `Ecall`, `Ebreak` and `Unimp` contain no
  register-file read at all, while `Add` contains two.
- **The generated items are free items, not nested ones.** Nested items cannot be referred to from a
  sibling item and the handlers have to name each other, so they are emitted as private free items
  alongside the per-variant execution functions. Each repeats the full generic parameter list and
  every call site uses an explicit turbofish, because nested or not, generated items do not inherit
  enclosing generics.
- **A caller that owns its state passes `&mut` as the owned value**, which only works if `&mut T`
  implements the same traits `T` does. Blanket forwarding implementations were added for that.
  `VectorRegisters` could not be forwarded that way: its `read_vregs()` returns
  `&VectorRegisterFile<{ Self::VLEN }>`, and the compiler does not normalize
  `<&mut T as VectorRegisters>::VLEN` to `T::VLEN` while `Self` is generic. The fallback is
  `impl_vector_registers_for_mut_ref!`, a macro that writes the forwarding impl for a concrete
  type, filed upstream as
  [rust-lang/rust#161264](https://github.com/rust-lang/rust/issues/161264).
- **`explicit_tail_calls` is a crate-level feature**, so every crate that *instantiates* the
  generated code has to enable it, not just the crate that generates it. Several crates carry it
  and the matching `expect(incomplete_features)` for that reason alone, without using the threaded
  path.
- **The generated code has to be warning-clean, and that is a real design constraint.** Suppressing
  lints wholesale over generated code would hide problems in the instruction implementations that
  get inlined into it, so every suppression sits at the narrowest site that needs it.
- **`execute()` is never an ABI boundary in an optimized build.** The interpreter's release test
  binary contains threaded handler symbols and zero `ExecutableInstruction::execute` or per-variant
  execution function symbols — all of the latter are inlined away. That matters when reading
  disassembly: there is nothing to compare a handler against, only the handler itself.
- **A `#[cfg_attr(feature = "no-panic", …)]` on `execute()` moves to the per-variant execution
  functions**, and the threaded handlers do not carry it. They wrap those functions rather than
  containing the logic, so the check still applies where the logic is.
- **The gate used throughout implementation was release-disassembly comparison**, not a benchmark:
  every cleanup round was checked function-by-function against the previous build and required to
  come out byte-identical.

### Where to look

| | |
|---|---|
| emitter | `crates/execution/ab-riscv-macros/src/build/execution_impl/generate_threaded_fns.rs` |
| signature normalization, call into the emitter | `…/build/execution_impl.rs` |
| arm extraction | `…/build/extract_matches.rs` |
| types and the trait | `crates/execution/ab-riscv-interpreter/src/lib.rs` |
| entry point for the basic interpreter | `…/src/basic.rs` |
| cross-path tests | `…/src/rv64/threaded_tests.rs` |
| generated output for any enum | `$OUT_DIR/{EnumName}_execution_impl.rs` |

### Deliberately left out, and still out

- **`#[loop_match]` / `#[const_continue]`** (rust-lang/rust#132306, RFC 3720) as a way to keep the
  loop and get handler-like codegen. It does not work for this: `const_continue` requires the target
  arm to be **statically known**, and an interpreter's next state is a runtime value read from
  memory, namely the next instruction's discriminant, so the mechanism the feature is built around
  never fires here. What is left is the `#[loop_match]` structure itself, which gives replicated
  dispatch — one jump site per arm instead of a shared loop header. That is a real threaded-
  interpreter technique, and it targets **the one cost this workload measured as small**: indirect
  mispredicts are ~0.01 per guest instruction on Zen 4, about 3% of runtime. Meanwhile it leaves all
  three costs §2 identifies, because they are properties of being one large function and
  `loop_match` keeps it one large function. It is cheap to try on the `match` path, which is being
  kept anyway, and it is an independent experiment rather than an alternative to any of this.
- **Superinstructions.** 922 distinct adjacent pairs with the most common at 2.13%; no small hot set
  to exploit (question 5).
- **`extern "rust-preserve-none"`.** Ties the best `extern "Rust"` configuration, so it is an
  unstable feature for nothing — until the signature outgrows six arguments, at which point it is
  the answer (question 1).
- **Arms that compute positions directly, instead of going through `ExecutionResult`.** This was the
  fallback if the enum turned out not to fold. It does fold, so the fallback is not needed, and
  taking it anyway would cost the "bodies do not change" property for nothing.
- **A 33-slot register file with decode-time `x0` remapping.** The single largest identified win,
  but it changes the shared register types, so it is a separate decision (question 2).
