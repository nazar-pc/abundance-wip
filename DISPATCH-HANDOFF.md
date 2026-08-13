# RISC-V interpreter: faster instruction dispatch

Working notes for replacing the interpreter's central `match` with per-instruction handler
functions. Everything stated as a measurement was measured; everything else is flagged as untested.

## 1. The interpreter as it exists today

`ab-riscv-interpreter` executes pre-decoded RISC-V. The pieces that matter here:

**Instruction execution.** `ExecutableInstruction::execute()` takes the instruction by value along
with `rs1`/`rs2` values, the register file, extension state, memory, the program counter and a
system-instruction handler, and returns:

```rust
pub enum ExecutionResult<Reg, CustomError = CustomErrorPlaceholder>
where Reg: Register
{
    Continue { rd: Reg, value: Reg::Type },   // write the register, fall through
    Branch { offset: i32 },                   // relative to *this* instruction
    Jump { target: Reg::Type },               // absolute guest address
    Break,                                    // stop
    Err(ExecutionError<Reg::Type, CustomError>),
}
```

Instructions *describe* control flow rather than performing it — they say where execution goes next
instead of moving the program counter themselves. This is what keeps instruction bodies independent
of how the program counter is represented, and it is a precondition for handler functions that
carry the program counter in a register rather than behind a `&mut`.

The type is ≤16 bytes so it comes back in two registers rather than through a hidden out-pointer.
That constraint drives several design details; see §4.

**Instruction fetch.** `InstructionFetcher::fetch_instruction()` returns
`FetchInstructionResult<I, CustomError>` with `Instruction(I)`, `Continue`, `Break` and `Err`.
Also ≤16 bytes, same reasoning.

**Program counter.** `ProgramCounter` provides `get_pc`, `set_pc`, `old_pc` and `set_pc_relative`.
The last exists so a pre-decoded fetcher can move *within the decoded stream* rather than resolving
a branch offset into a guest address and converting that back into a stream position — the round
trip measured ~30% slower on the eager path.

**Eager fetchers** decode the whole instruction stream up front into a `Box<[Instruction]>`, one
slot per guest halfword. `EagerTestInstructionFetcher` (in `ab-riscv-benchmarks/src/host_utils.rs`)
and `EagerInstructionFetcher` (in `ab-riscv-coremark-runner/src/interpreter.rs`) are the two, and
they are near-identical. The lazy fetcher decodes on demand and is useful as an experimental
control, since changes to the eager path should not move it.

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
`match` into **per-variant `(variant ident, arm)` pairs**, inserting `ExecutionResult::CONTINUE_ZERO`
as the tail of any arm that does not end in one explicitly. So the individual arm body — not the
enclosing `match` — is already the unit the build system stores and manipulates.

Those pairs are keyed by enum name in a build-time `state`, and
`collect_enum_execution_impls_from_dependencies()` pulls them across crate boundaries. The
`links = "ab-riscv-primitives"` key in `Cargo.toml` plus the `DEP_*` variables Cargo derives from it
are what let a downstream crate collect arms defined upstream. This is how a composed instruction
enum — `ContractInstruction`, the vector compositions, `CoremarkInstruction` — gets an `execute()`
assembled out of arms that live in several different crates.

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

## 3. What was prototyped, and what to build

The prototypes live in `crates/execution/ab-riscv-coremark-runner/src/threaded.rs`, on branch
**`claude/risc-v-dispatch-perf-6qtcdo`**. That branch is a scratchpad rather than something to
merge, but `threaded.rs` is the reference for what the back end looks like: a single `ops!` macro
table covers all 147 `CoremarkInstruction` variants exhaustively, with no catch-all, so adding an
extension is a compile error rather than a silent `Unsupported` at run time. An emitter macro
expands that one table into per-instruction handler functions.

The full set of five back ends that were measured is on
**`claude/riscv-interpreter-dispatch-mfxzji`** (tip `c9e38bd`), and in the history of `threaded.rs`
up to commit `f3bc9bc`. Four of them lost and have been removed, so that further work is not spent
maintaining them; §3's table below is the record of why. Go back to that history if a question needs
them again — the most likely one is re-running the table-size experiment (§2) against the vector
composition, which needs the `match` back end.

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

The last exists only to answer whether `become` is load-bearing. It is not, today — LLVM performs
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

### What to build

**Safe generic handlers, dispatched with `become`, with the register file left as a type
parameter.** It is the fastest and most consistent combination on Zen 4, the only machine where
measurement is reliable, and it is ordinary safe Rust with memory and the decoded stream borrowed
rather than owned.

Because axes 3 and 4 are just a type parameter and an ABI annotation, they do not need separate
implementations — build the handlers once, generic over `Regs` and with the ABI threaded through the
emitter as a `macro_rules!` metavariable, and swap either to compare. Neither raw pointers nor a
plain tail call is worth building: the first bought nothing, and the second is what `become` already
gives with a guarantee attached.

### How it gets built: a new emitter, not new instruction sources

The handlers are generated, not written. Instruction implementations stay as they are — a `match` on
`self` in the source — and the change happens in `ab-riscv-macros`, where the per-variant arms
already extracted (§1) are emitted as standalone functions instead of being stitched back into one
`match`. Instruction authors keep writing arms; what the build system does with them changes.

`threaded.rs` on the experiments branch is a hand-rolled model of exactly this: one `ops!` table
listing every variant with its operands and body, and several emitter macros — `emit_match`,
`emit_handlers`, `emit_tail_handlers`, `emit_plain_tail_handlers`, `emit_safe_handlers` — expanding
that single table into the different back ends. Reproducing that structure inside the build-time
macros, with the emitter choosing the shape, is the shape of the work: the arms are the input, the
back end is a choice about how to emit them, and the existing `match` emitter can stay as a
fallback while the handler emitter is brought up.

Practical notes for whoever does it:

- Anything a generated handler needs to call should reach it through a helper module re-exported
  from `prelude`, for the reason above.
- `extract_matches.rs` hardcodes the expected "no explicit tail" expression as a token-stream string
  comparison against `ExecutionResult::CONTINUE_ZERO`. Anything that changes what an arm may end
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
  it also means a store in every handler and a load in the loop.
- The default Rust ABI on **Windows x86-64 gives only 4** argument registers. If Windows ever
  matters, handlers need pinning to `extern "sysv64"`.

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

### Dispatch prototypes

On `claude/risc-v-dispatch-perf-6qtcdo`, from the workspace root:

Now that one configuration survives, there is one binary with two modes in it, and two scripts:

```bash
COREMARK_ITERATIONS=3000 ROUNDS=5 CORE=<core> \
    ./crates/execution/ab-riscv-coremark-runner/bench-dispatch.sh
COREMARK_ITERATIONS=3000 CORE=<core> \
    ./crates/execution/ab-riscv-coremark-runner/profile-dispatch.sh
```

`bench-dispatch.sh` builds and compares the tail-threaded back end against the generic loop. It
exists so that the methodology is not something to remember: it interleaves the two modes
round-robin rather than running all of one and then the other, reports best-of, and prints the
run-to-run spread next to every timing so that an effect narrower than the noise is visibly not a
result. It refuses to print a table when either mode returns wrong CRCs or exits non-zero.
`ROUNDS`, `CORE` and `COREMARK_REPEAT` control it.

`profile-dispatch.sh` runs `perf stat` over both modes and normalises every counter **per guest
instruction**, which is the only unit in which the two are comparable. The denominator is not
guessed: `COREMARK_HISTOGRAM` counts the dynamic instruction mix exactly, so the script derives a
true cycles-per-guest-instruction figure. It probes which PMU events the machine has rather than
assuming, so the AMD-specific indirect-branch counters appear when they exist and are skipped when
they do not. It finishes with a flat per-handler profile — tail calls mean there are no stack
frames, so self-cost is all there is, which is what is wanted.

Both find the binary by asking cargo where it put it, so `CARGO_TARGET_DIR`, `build.target-dir` and
`--target` all work.

Individual runs, if needed — set `COREMARK_DISPATCH` to anything for the threaded back end, leave it
unset for the generic loop.

Features: `dispatch-basic`, `dispatch-branchless`, `dispatch-zerostore` pick the register file, and
`dispatch-preserve-none` is orthogonal to all three and picks the ABI. `build-elf-required` makes a
missing RISC-V toolchain a build error rather than an empty ELF and a confusing run-time message.

Environment variables: `COREMARK_DISPATCH` picks the register file at run time (it must match the
one compiled in), `COREMARK_ITERATIONS` fixes the amount of work **at build time** (**essential** —
the default of 0 means autodetect, so the workload scales with interpreter speed and results are not
comparable), `COREMARK_REPEAT` runs the guest N times in-process and reports the best,
`COREMARK_HISTOGRAM` dumps the dynamic instruction mix.

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
taskset -c <core> cargo bench -- --sample-size 10 --baseline before eager
```

To A/B two revisions without rebuilding, build each once and run the binaries directly:

```bash
cargo bench --no-run --bench riscv -p ab-riscv-benchmarks   # prints the binary path
# target/release/build/ab-riscv-benchmarks/<hash>/out/riscv-<hash>
$BIN --bench --noplot --warm-up-time 1 --measurement-time 3 --sample-size 20 \
    "blake3_hash_chunk/interpreter/eager"
```

Interleave the two binaries round-robin across several rounds and compare best-of, rather than
running all of A then all of B.

**`RUSTFLAGS` replaces `.cargo/config.toml`'s flags rather than merging**, so any manual build must
carry the project's forward:

```bash
RUSTFLAGS="-Znext-solver=globally -Zmin-recursion-limit=256 -C target-cpu=znver4"
```

### Checks worth keeping in the loop

- **`blake3_hash_chunk/interpreter/lazy`** as a control: the lazy fetcher should not move when the
  eager path changes. If it does, the measurement is suspect.
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

## 7. Open questions

1. ~~**Does `extern "rust-preserve-none"` pay?**~~ **Answered: not today.** With the arguments
   ordered coldest first it rescues `basic` (−5.6%) and `branchless` (−8.3%), but it ties with
   `zerostore` + `extern "Rust"`, which was already at the floor. Since the feature is unstable, a
   tie is a reason not to take it. Revisit **only** when the handler signature grows past six
   arguments (§4), which is the case where `extern "Rust"` starts spilling and preserve-none's
   twelve registers actually pay for themselves.
2. **What is the loop limited by?** Narrowed, not yet answered. Profiled on Zen 4
   (`profile-dispatch.sh`), per guest instruction:

   | mode | cycles | host-insn | IPC | ind-mispred | i$-miss | d$-miss | loads | stores | stlf |
   |---|---|---|---|---|---|---|---|---|---|
   | generic | 11.35 | 44.57 | 3.93 | 0.0038 | 0.0000 | 0.0018 | 8.03 | 1.43 | 0.31 |
   | threaded | 6.23 | 11.74 | 1.88 | 0.0097 | 0.0000 | 0.0013 | 6.56 | 1.57 | 0.36 |

   Ruled out: **caches** (both miss rates ~0 — the ~10 KB of handlers sit inside Zen 4's 32 KB L1i
   and CoreMark's working set inside L1d); **misprediction** (0.0097 indirect mispredicts at ~18–20
   cycles is ~3% of 6.23, and note that nearly all mispredicts are indirect, so the guest's own
   conditional branches fold into the dispatch target and predict well); **issue width** (IPC 1.88
   on a six-wide core).

   **There is a large unexplained stall.** Memory ops are 8.13 per guest instruction, which at ~3
   per cycle is 2.7 cycles, and 11.74 instructions at six-wide is 2.0 — so the throughput floor is
   about 2.7 cycles against 6.23 actual. More than half the time is spent waiting.

   The register-file round trip is real but does not obviously account for it: `stlf` is 0.36 per
   guest instruction, so only a third of instructions forward from a recent store, and a successful
   forward costs about what an L1d hit costs anyway. `stlf-fail` — the expensive case — was added
   after this run and is not yet measured.

   Worth carrying: **roughly 60% of the loads are interpretation overhead rather than guest data.**
   Of ~6.56, about three read operand bytes out of the decoded instruction, one reads the next
   discriminant, and one reads the dispatch table; only about two are guest register reads. And
   reading a guest register is a *dependent pair* of loads — the register index has to be loaded out
   of the instruction before the register value can be loaded — which is the deepest serial
   structure in the loop.

   Also note the threaded loop issues *more* stores than the generic one (1.57 vs 1.43), which is
   `ZeroStoreRegisters` paying two stores per register write. It won anyway, so this is an
   observation rather than a complaint.

   **Next measurement, not next change:** the per-handler cycle attribution. The first attempt
   silently produced nothing — `perf annotate` was passed a shortened symbol name and needs the full
   one — which is fixed, along with preferring `cycles:pp` so that AMD IBS attributes precisely
   rather than with skid. Inside a twelve-instruction handler that is the difference between
   locating the stall and guessing at it. Nothing else here should be decided before it is run.

   One consequence already: question 3 is dropped, see below.

3. ~~**How handlers are reached.**~~ **Dropped**, see question 2: an inline handler pointer removes
   a dependent load from a chain the branch predictor already hides, and the entire misprediction
   budget it could recover is 3.5%. It would also double the decoded stream's footprint.
4. **How `ExecutableInstruction` exposes its handlers.** An associated *slice* — no `COUNT` constant
   is needed. The open part is how that composes across extension traits.
5. **Superinstructions, and the case against them here.** Fusing an adjacent pair removes one
   dispatch *and* one register-file round trip, which is exactly what question 2 says is expensive.
   `COREMARK_HISTOGRAM` now reports adjacent executed pairs, counting only those where the first
   instruction fell through — the condition under which the two are also stream-adjacent, so a pair
   it counts is one that could actually be fused. The distribution is flat: **922 distinct pairs,
   the most common at 2.13%, and the top 20 together only 37%**. 88.7% of instructions have a
   fusable successor, so the opportunity is real, but capturing it needs tens of superinstructions
   for a fraction of the gain each. Worth knowing before anyone assumes fusion is the obvious next
   move.
6. **The ~500-variant vector composition.** The 89→147 experiment shows `match` degrading while
   `call`/`tail` do not, but 500 arms is a different regime for I-cache, and this is where the
   existing vector-extension regression lives. The most valuable case to prove.
7. **Whether `Branch` should be relative to the *next* instruction** rather than this one. It would
   remove a `- instruction_size` from every relative branch, at the cost of changing the shared
   enum's contract and every instruction that produces it.
8. ~~**`branchless` versus `zerostore` register files.**~~ **Answered: `zerostore`.** It beat `basic`
   by 6.3% with `-C target-cpu=znver4` and 8.8% on the default build, with `branchless` between them
   at −2.3% rather than level with it. Neither Xeon could separate the three. `ZeroStoreRegisters` is
   now the only one implemented; the other two are in the history of `threaded.rs` up to `e023d69`
   if the trade ever needs revisiting, which it would if the handler signature changes enough to
   alter register pressure.
