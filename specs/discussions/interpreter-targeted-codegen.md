# Interpreter-targeted code generation

> Handoff note for work on LLVM. The objective is a code generator that knows its output will be
> interpreted rather than executed on silicon. Everything here is either a property of LLVM that can
> be checked in its source, or a measurement made against the interpreter in this repository, with
> the command that produced it. `riscv-interpreter-measurements.md` has the raw data those
> measurements came from and is not required reading.
>
> **Status: §5.2, §5.3 and §5.5 are built** — `generic-interpreter`,
> `generic-interpreter-rv32` and `generic-interpreter-rv64` in LLVM's RISC-V backend, worth
> **−5.0% dispatches on CoreMark against the same LLVM tuned for hardware, −12.1% with the
> interpreter's instruction fusion on top, +21.0% on the interpreted score, at slightly *less* code
> than the hardware tuning** — smaller and faster at once, and +40.5% against GCC 14.2. §8 has what
> was built and what each piece is worth; `riscv-interpreter-measurements.md` §5.6 has the numbers
> and §5.7 reproduces them in an empty directory.
>
> **It does not yet generalize.** On the contract benchmarks the same build leaves Blake3 unchanged
> and makes ed25519 **15.8% slower** at the same code size, because giving the scheduler a model
> where `generic-rv64` has none lengthens live ranges in 5-limb bignum arithmetic that already wants
> the whole register file: `FieldElement51::mul` goes from 5 spill stores to 32. Every function with
> multiplies gained spills and no function without them did. §5.6 has the evidence and the three
> runs that would isolate it.
>
> §4.1 was measured wrong and has been corrected: the toolchain it called LLVM was the system
> clang's mid-level optimizer with a newer instruction selector bolted on, and with LLVM's own
> optimizer LLVM executes fewer instructions than GCC rather than more. The objective is unchanged
> and the case for it is now made against the same compiler rather than against GCC.

## 1. The objective

LLVM optimizes for silicon. Every knob its RISC-V backend exposes targets a real pipeline:
instruction latency, issue width, port pressure, i-cache footprint, branch predictor behavior. A
processor model *is* a description of a machine, and a target without one gets `NoSchedModel` and no
tuning at all.

RISC-V is increasingly not a machine. It is a portable execution target for software: blockchain
virtual machines (this repository among them), sandboxes, deterministic replay environments,
emulators. Code compiled for those runs through an interpreter or a JIT, and the cost of running it
has almost nothing to do with the cost on hardware.

There is currently no way to say so. There is no `-mtune=interpreter`, no processor definition that
means "this will be interpreted", and — more fundamentally — no cost model hook that expresses what
an interpreter charges for. The objective is to give LLVM that notion.

Two things are in scope:

1. **A cost model that describes an interpreter**, rather than a machine that happens to be slow.
2. **A way to select it** that is reachable from ordinary build configuration, so a project can opt
   in without patching its toolchain invocation.

## 2. What an interpreter charges for

This is the substance of the whole exercise: a cost model LLVM does not have. Four terms, in rough
order of weight.

**1. Dynamic instruction count.** Every guest instruction is one dispatch: read the decoded
instruction, jump to its handler, run a handful of host instructions, jump to the next. Halving the
instructions a program executes roughly halves interpretation time. This dominates, and it is *not*
what any existing scheduling model optimizes — a model describes how instructions overlap, not how
many there are.

**2. Per-dispatch cost, which is not uniform.** A load costs more than an `addi`: address
computation, a bounds check, a memory access, often a branch on the result. So dynamic instruction
count alone does not order two programs, and a model that is only an instruction counter will be
wrong some of the time. §4.2 has a measured case where 10% fewer dispatches ran 3.4% slower.

**3. Guest branch unpredictability.** A guest conditional branch becomes a host indirect jump whose
target depends on guest data. The host branch predictor cannot learn it the way hardware learns its
own branches, so a mispredicted guest branch costs several dispatches. This is the term with no
expression anywhere in LLVM today, and the one with the most leverage: it is why branchless
selects, avoiding branch chains, and not trading a branch for two jumps all matter more here than
on hardware.

**4. Decoded-stream footprint.** A pre-decoding interpreter stores one fixed-size slot per
`ALIGNMENT` bytes of guest code — in this implementation 8 bytes of host memory per 4 bytes of guest
code. Compressed instructions halve the alignment and therefore *double* the stream per guest byte.
Smaller guest code is not automatically better, and for a JIT the equivalent term is compile time.

Two consequences worth stating explicitly, because both invert a default:

- **Static code size and dynamic instruction count are separate objectives, and optimizing the
  first can hurt the second.** §4.1 measures a compiler producing 39% less code that executes 18%
  more instructions.
- **Inlining and unrolling are more valuable here than on hardware**, because they delete dispatches
  outright rather than merely shortening a dependency chain. The i-cache pressure that normally
  bounds them is not a cost an interpreter pays in the same way.

## 3. What is reachable today, and what is not

Relevant because it constrains where a change should live.

From a rustc/clang target definition — the thing a project can check in — exactly three things reach
the backend: the **feature string**, the **CPU name**, and the code model. Everything else worth
tuning (inlining, unrolling, the machine outliner, tail duplication, scheduler and branch-merging
knobs, jump-table thresholds, vectorization) needs `RUSTFLAGS`/`CFLAGS` or a build profile, and
`-C llvm-args`/`-mllvm` reaches arbitrary `cl::opt` knobs but nothing that survives as
configuration.

**So anything put behind a CPU name or a subtarget feature is adoptable; anything put behind a pass
flag is not.** That is the main design constraint on the first patch.

Verify a feature or CPU name is recognized rather than silently ignored — the warning exists but
build systems swallow it:

```bash
rustc -Zunstable-options --print target-features --target <spec>.json
rustc -Zunstable-options --print target-cpus     --target <spec>.json
```

## 4. Evidence that the lever is real

Three measurements, each reproducible with §6. None of them is about instruction fusion.

### 4.1 LLVM already executes fewer instructions than GCC — corrected

An earlier version of this section reported the opposite, and the mistake is worth stating because
it is easy to repeat. The wrapper that built the "LLVM" row (`riscv-interpreter-measurements.md`
§5.1) replaced only `llc`, so every mid-level decision — inlining, unrolling, loop idiom recognition
— came from whichever `clang` the system happened to have, several releases older. Routing that
same clang through its own `opt` and the newer `llc` reproduces the old number to nine significant
figures, which is what pins the cause on the optimizer's version rather than on the pipeline shape.

Rebuilt so that both halves come from the LLVM under test. CoreMark, same source, same guest ISA,
1000 iterations, one batch; "dispatches" is the exact count of guest instructions executed:

| toolchain | static instructions | dispatches | interpreted score |
|---|---|---|---|
| GCC 14.2 | 8387 | 301,590,632 | 2991.81 |
| LLVM (git, `generic-rv64`) | **5077** | **262,393,554** | **3436.20** |

**LLVM emits 39% less code that executes 13% fewer instructions.** Both objectives at once, which is
not what §2's second consequence would predict, and the reason is a single transform: LLVM's
`HashRecognize` analysis recognizes CoreMark's bit-at-a-time CRC loops and `LoopIdiomRecognize`
replaces them with a 256-entry table lookup. GCC does not.

| function | GCC 14.2 | LLVM |
|---|---|---|
| `crcu8` | 66 instructions | **9** |
| `crcu16` | 131 | **14** |
| `crcu32` | 261 | **28** |
| `crc16` | 132 | **15** |

The CRC chain runs over every byte of benchmark data, so that is most of the 13%. It also means
CoreMark no longer contains the hot data-dependent branch that motivated §5.4: there is nothing left
there to compile branchlessly.

Most of the size difference remains cold, as before: `ee_printf`, `number` and `cvt` — CoreMark's
own result formatting, which runs once at the end — are 3701 instructions under GCC 14.2 and 1313
under LLVM. Benchmark code differs by much less than 39%.

**What survives is the part that matters.** Years of GCC do not move the dispatch count: 14.2 → 16.1
puts 1.0% back on it while growing static code 5.7%. Neither compiler is optimizing for the
number of instructions executed by an interpreter, and a compiler that is beating the other on that
metric by accident is not the same thing as one that is trying. §8 is what trying looks like: 13.6%
more out of the same LLVM, from nothing but telling it what the consumer is.

**What does not survive** is using GCC as the yardstick. The gap available to an interpreter-targeted
LLVM has nothing to do with the GCC/LLVM difference in either direction, and it is measured against
the same compiler tuned for hardware instead.

### 4.2 The processor model moves dispatch counts by 15% with no source change

Changing only the CPU name in the target definition, same source, same features:

| cpu | scheduling model | dispatches, hashing | dispatches, signature verify |
|---|---|---|---|
| generic-rv64 | SpacemitX60 | 14086 | **646244** |
| rocket-rv64 | Rocket | **12662** | 708340 |
| spacemit-x100 | SpacemitX100 | 12902 | 691866 |
| sifive-p670 | SiFiveP600 | 14086 | 655194 |
| tt-ascalon-x | TTAscalonX | 14518 | 655282 |
| sifive-u74 | SiFive7 | 14278 | 660343 |
| veyron-v1 | NoSchedModel | 14614 | 729426 |

Up to 15% spread, from a knob nobody thinks of as one, and no model wins both workloads. `generic-rv64`
is the rustc default for RV64 and is merely one vendor's in-order core; nothing about it is chosen
for this purpose. `NoSchedModel` is the worst row, so "turn scheduling off" is not the answer either.

**But dispatch count is not the whole cost.** Wall clock, threaded dispatch, median of three runs:

| | generic-rv64 | rocket-rv64 |
|---|---|---|
| hashing | **12.99 µs** | 13.43 µs |
| signature verify | **664.9 µs** | 765.6 µs |

`rocket-rv64` runs 10% fewer dispatches on the hashing workload and is 3.4% slower. Its instruction
mix must be more expensive per dispatch. Any model built as a pure instruction counter will mispredict
cases like this, which is §2 term 2 asserting itself. **Establishing per-instruction weights for at
least the common classes is a prerequisite for a credible cost model**, and nothing here has done it.

### 4.3 Most existing knobs are inert, so the gap is not reachable by tuning

Swept on a 32k-instruction program, static counts, against a baseline of 32181 instructions:

| knob | result |
|---|---|
| `-riscv-br-merging-base-cost` / `-likely-bias` / `-unlikely-bias` | **bit-identical output** for all three |
| SelectOptimize on/off | ±2 instructions |
| scheduler knobs (no-misched, top-down, no clustering, source order) | ≤ ±20 instructions |
| jump-table thresholds | disabling them *adds* conditional branches (3103 → 3172) while removing indirect jumps — the wrong trade, since an indirect jump is one semi-predictable dispatch and a conditional branch is an unpredictable one |
| post-RA scheduling | fewer instructions, but breaks up dependent pairs |
| machine outliner | −8.5% instructions, but unconditional jumps 827 → 1288 and indirect jumps 1552 → 1878; it pays for straight-line code in call/return dispatch pairs, the one currency an interpreter cannot absorb |
| loop unrolling off | −1160 static instructions and −74 spills, but +4.1% dispatches — **unrolling helps**, see §7 |

Nothing in the existing knob surface adds up to what §5.3 later found in the inlining and unrolling
*preferences*, which are cost-model inputs rather than pass flags. The conclusion stands even though
§4.1's 18% does not: this needs a cost model, not a configuration.

## 5. What to build

### 5.1 Where the relevant code lives

- `llvm/lib/Target/RISCV/RISCVProcessors.td` — `RISCVProcessorModel` definitions; a new CPU goes
  here, and this is where each existing one is bound to its `SchedMachineModel`.
- `llvm/lib/Target/RISCV/RISCVFeatures.td` — subtarget and tune features.
- `llvm/lib/Target/RISCV/RISCVTargetTransformInfo.{h,cpp}` — the cost hooks that drive inlining,
  unrolling and if-conversion. Most of §5.3 lives here.
- `llvm/lib/Target/RISCV/RISCVSubtarget.{h,cpp}` — `enableMachineScheduler()`,
  `enablePostRAScheduler()` and the rest of the per-subtarget switches.
- `llvm/lib/Target/RISCV/RISCVISelLowering.cpp` — where branch-versus-branchless decisions are made,
  including the `Zicond` paths.
- `llvm/lib/Target/RISCV/RISCVMacroFusion.td` and
  `llvm/include/llvm/Target/TargetMacroFusion.td` — the fusion definitions and the `Fusion` /
  `SimpleFusion` classes; relevant only to §5.4.
- `llvm/lib/CodeGen/MacroFusion.cpp` — how fusion predicates become DAG edges that hold a pair
  adjacent.

### 5.2 A `generic-interpreter` processor definition — small, and the thing that makes the rest adoptable

**Built.** `generic-interpreter-rv32`, `generic-interpreter-rv64` and a tune-only
`generic-interpreter`, over a new `InterpreterModel` in `RISCVSchedInterpreter.td`, plus an
`interpreter-target` subtarget feature so the tuning is reachable from a feature string as well as
from a CPU name. Not started from `RocketModel`: see §8.

A `RISCVProcessorModel` whose scheduling model is in-order with uniform latency. On its own this is
a modest change; its value is that it creates the **selector**. Per §3, a CPU name is reachable from
a checked-in target definition, so everything added behind it is adoptable by a project without
touching its build flags. Every later item in this section should hang off this CPU rather than off
a pass flag.

Start from `RocketModel`, which produced the fewest dispatches of anything measured on the workload
where models differed most. Do not start from `NoSchedModel`: §4.2 shows it is the worst row, because
no scheduler also means no clustering of dependent instructions.

### 5.3 Retune TTI for dynamic instruction count — where most of the measured gap is

**Built, and it is the whole measured effect.** Inlining and unrolling together account for the
entire −14.9%; see §8.

Inlining and unrolling thresholds already exist and are already reachable; they are simply tuned for
a machine with a finite i-cache and a reorder buffer. §4.1 says this is where the GCC/LLVM difference
comes from, and §2's second consequence says the direction is "more aggressive than for hardware".

This starts with a measurement, not a patch: §7's first two items are exactly the inputs needed, and
the unroll sign is genuinely unknown today.

### 5.4 Price an unpredictable branch — the new part, and the one whose evidence was retracted

**Partly built, and worth nothing measurable so far.** `getPredictableBranchThreshold()` is one and
jumps are expensive for these CPUs, which together are worth −0.05% of the dispatch count on
CoreMark. The evidence below was §4.1's CRC chain, which LLVM now compiles to a table lookup, so
the workload no longer contains the branch this item was about. See §8 for what is still missing.

Nothing upstream can say "a data-dependent branch costs several times an arithmetic instruction
here". Everything that would follow — preferring branchless selects, refusing tail duplication that
trades one branch for two jumps, not expanding jump tables into branch chains — is downstream of
that single number.

§4.1 is this item's evidence, not an argument for it: in CoreMark's CRC inner loop LLVM compiles
`if (data & 1) crc ^= poly` into a branch per bit where GCC produces branchless mask arithmetic from
the same ISA. That one decision, in the hottest code in the benchmark, is worth more than every knob
in §4.3 combined.

Note that `Zicond` being enabled is not sufficient: it was on throughout the contract measurements
and LLVM still declines to go branchless in many places, correctly, using a cost model tuned for a
real predictor. And the CRC case shows the mechanism does not even need `Zicond` — mask arithmetic
is plain base-ISA. The override is the point, not the instruction selection.

Mechanically this probably wants both a TTI hook (so mid-level passes see it) and a subtarget
predicate (so `RISCVISelLowering` sees it), gated on the CPU from §5.2.

### 5.5 Bonus: keep dependent instructions adjacent

**Built, and worth more than expected.** The fusion tune features are on for these CPUs, and take
CoreMark from 78 fusable pairs to 219, which takes what the interpreter's fusion is worth from 0.5%
to 8.3% of dispatches. The largest single contributor is `conditional-cmv-fusion`, which is on not
because anything fuses a branch with a move but because the interpreter does.

Some interpreters — this one included — fuse an adjacent pair whose intermediate value is dead into
a single dispatch. That is only possible when the compiler left the pair adjacent, and scheduling is
what pulls such pairs apart. LLVM already has machinery that holds a pair together: macro-fusion
predicates become DAG edges in `MacroFusion.cpp`, and the RISC-V `Tune*Fusion` features turn them on.

So a `generic-interpreter` model can reasonably enable the fusion tune features, not because any
hardware will fuse, but because the edges make the output friendlier to a consumer that can.

**This paragraph called that a bonus an order of magnitude below §4.1, and measurement says the
opposite.** The fusion pass is what turns +8.9% into +19.5% — and it only helps at all on code
generated this way: on GCC's and on `generic-rv64`'s output the same pass is a net *loss* of 3.7%
and 4.9%, because its per-instruction overhead is paid on every dispatch while at 0.4-1.2% of
dispatches eliminated the saving is not. The tune features are not friendlier output for a consumer
that already benefits; they are what makes the consumer's fusion pass worth running.
`riscv-interpreter-measurements.md` §5.6 has the numbers.

A related and untested hypothesis: more regular guest code should also make the *host* interpreter's
dispatch sequence more predictable, which host-side PGO and the host branch-target buffer could
exploit independently of any fusion. Measuring it needs a PGO-built interpreter, which does not exist
here.

## 6. How to validate a change

The harness that produced every number above is in this repository and is the intended way to check
whether an LLVM change helps.

**Count, do not time.** The run-to-run spread of these benchmarks on identical binaries is about
±5% — three runs of the same code disagreed on the *sign* of a 3% effect. Dispatch counts do not
move between runs at all. Use wall clock only for effects above roughly 10%, and only as a check on
a counted result, since §4.2 shows counting alone can mislead in the other direction.

```bash
# Dispatch counts for two cryptographic workloads, built from Rust with the target definition at
# crates/contracts/core/ab-contracts-tooling/src/riscv64-unknown-none-abundance.json
cargo run --release -p ab-riscv-benchmarks --example dispatch_count

# CoreMark, built from C by whatever `RISCV_CC` names, interpreted
COREMARK_DISPATCH_COUNT=1 cargo run --release -p ab-riscv-coremark-runner   # exact dispatch count
cargo run --release -p ab-riscv-coremark-runner                             # Iterations/Sec

# Wall clock for the interpreter itself, threaded dispatch over a pre-decoded stream
cargo bench -p ab-riscv-benchmarks --bench riscv -- 'interpreter/threaded/eager'
```

To try a CPU name or feature string, edit `cpu` / `features` in the target definition above and
rebuild; the guest code is rebuilt by the benchmarks crate's build script. To sweep a `rustc` or
LLVM flag against the contract workloads instead, pass it in `AB_EXTRA_RUSTFLAGS`, which reaches the
guest build specifically.

To build the C workload with a chosen LLVM instead of GCC, run
`interpreter-codegen/measure-coremark.sh` in an empty directory; it builds that LLVM, generates the
wrapper `RISCV_CC` needs, and prints a row per configuration.
`interpreter-codegen/build-patched-rustc.sh` does the same for the Rust side by building a rustc
whose own LLVM carries the change. `riscv-interpreter-measurements.md` §5.7 describes both, and §5.5
is what it cost to use a wrapper that replaced only `llc`.

The toolchain is pinned in `rust-toolchain.toml` (currently nightly-2026-08-25, LLVM 23.1.0). Do not
override it, and run the `--print` commands from the repository root so the pin applies — outside it,
a different toolchain answers and reports a different, older feature set. That trap cost a full round
of measurements once already.

## 7. Open questions

Ordered by what would most change the plan. Answered ones are kept with their answers.

- ~~**Unrolling: does it help or hurt, counted rather than sized?**~~ **It helps, up to a factor of
  about three.** Turning it off costs +4.1% dispatches for 247 fewer static instructions, but the
  factor has to be capped: letting the size budget pick it unrolls hot loops by 8 and is worse on
  both axes than capping at 3, because the spills of the wide body cost more than the backedges it
  removed. What does *not* matter is the size of the budget — raising the unroll threshold is
  bit-identical to not raising it.
- ~~**Inlining thresholds, `codegen-units = 1`, LTO, counted.**~~ **Inlining is the single best
  lever.** Doubling the threshold is −7.5% dispatches for +11% static code, and the effect plateaus
  there — tripling it changes no counts and grows the code another 9%. `codegen-units` and LTO are
  still unswept.
- **How far can this go without growing the code?** The binding constraint, and partly answered:
  the scheduling model, the fusion tune features, `conditional-cmv-fusion` and the shift-chain
  unrolling rule are together −11.7% after fusion at 0.9% *less* code, because they change the shape
  of the output rather than the amount of it. Everything beyond that is bought by duplicating code,
  and it is worth looking for more of the first kind before spending more of the second. The method
  that found the `conditional-cmv-fusion` win generalizes: take the consumer's list of pairs it can
  fuse, and ask which compiler tuning produces pairs on that list.
- **What are the per-instruction cost weights?** Still open, and now the thing blocking §5.4.
  `RISCVSchedInterpreter.td` carries a guess — 1 for ALU, 2 for a load or store, 3 for a multiply or
  an indirect jump, 8 for a divide — expressed as occupancy of a single serial dispatch resource
  rather than as latency. Nothing has measured those numbers, and until something does, the branch
  price in the model is a placeholder too.
- **Does making a branch branchless actually pay?** The new question §8 raises. Replacing a guest
  conditional branch with mask arithmetic *adds* two to four dispatches and removes one host
  mispredict, so the dispatch count — the metric this whole harness is built on — says it is a
  regression, and only wall clock can say otherwise. This needs the previous question answered
  first, and it needs a workload whose hot branches are actually unpredictable: CoreMark's are not,
  any more.
- **Does this generalize past one interpreter?** Still open, and now partly answerable from the
  inside: the contract workloads gain 2.5% from the inlining lever where CoreMark gains 7.4%, so a
  Rust front end reaches LLVM with most of those decisions already made. A second interpreter would
  still make the case substantially stronger before anything is proposed upstream.
- **Where is the boundary with JIT-targeted codegen?** Terms 1–3 apply to a JIT as well; term 4
  becomes compile time instead of memory. If the same model serves both, that is a much stronger
  argument for taking it upstream than an interpreter alone. The 2.5× static code growth is where
  the two would first disagree.
- **`-Z mir-opt-level`, `-Z merge-functions`, vectorization with no vector extension.** Unswept.
  `merge-functions` plausibly has the wrong sign, since merging creates calls where code was
  duplicated.

## 8. What was built, and what each piece is worth

In `llvm/lib/Target/RISCV`, all of it behind a CPU name or a subtarget feature so that a project can
adopt it from a checked-in target definition, per §3.

- **`RISCVSchedInterpreter.td`** — an `InterpreterModel` that describes an interpreter rather than a
  pipeline: one serial in-order issue slot, latency 1 everywhere because a handler completes before
  the next dispatch starts, and per-dispatch cost expressed as occupancy of that slot. §5.2 said to
  start from `RocketModel`; that was based on §4.2, where Rocket won one workload. A model is a
  claim about the machine, and Rocket's claim — load latency 3, multiply 4, divide 33 — is false for
  an interpreter, so it is not the right starting point even where it wins. Nothing was lost by
  ignoring the advice: the scheduling model is worth ~0% of the dispatch count on CoreMark either
  way.
- **`TuneInterpreterTarget`, spelled `interpreter-target`** — the carrier for everything below, so
  the tuning is reachable from a feature string as well as from a CPU name.
- **`generic-interpreter{,-rv32,-rv64}`** — the selector §5.2 asked for. No ISA of their own beyond
  the base integer set, exactly like `generic-rv64`, so extensions still come from `-march`.
- **`getInliningThresholdMultiplier()` = 2** — **−7.5% dispatches, +11% static code.** The best
  size-for-dispatches trade available, and it plateaus: a multiplier of 3 changes no counts and adds
  another 9% of code.
- **The tuned unrolling preferences with the factor capped at 2** — **−4.7%, for +21% more static
  code.** The cap is the interesting part. Letting the size budget pick the factor unrolls hot loops
  by 8, which is worse on *both* axes than capping at 3, because the register pressure of the wide
  body costs more in spills than the backedges it removed.
  `-mllvm -riscv-interpreter-max-unroll-count=N` moves along the frontier: 3 buys a further 1.5% for
  another 26% of code, which is where the trade stops paying.
- **Unrolling a shift chain to its period, whatever the cap says** — **−2.7% for seven static
  instructions across the whole benchmark.** A loop that does `next = list; list = list->next;`
  needs no copies inside an unrolled body but one per link at the backedge, and only the chain's
  period makes that assignment the identity. `core_list_reverse` is 10.9% of all dispatches in
  CoreMark and a cap of 2 cut its period-3 chain short, leaving a copy per node. This is the best
  dispatch-per-byte trade found anywhere in the exercise, and it was found by profiling rather than
  by reading LLVM.
- **`getPredictableBranchThreshold()` = 1, `isJumpExpensive()`, `getBranchMispredictPenalty()`, and
  the latency scheduling heuristic off** — **−0.05%.** The conceptually new part, and so far worth
  nothing that can be counted. It does what it says: a conjunction of two comparisons is computed
  and branched on once rather than split into a chain of branches. On CoreMark there is nothing left
  for it to improve, and it is size-neutral, so it stays on evidence it does not yet have.
- **The interpreter scheduling model and the fusion tune features, including
  `conditional-cmv-fusion`** — **−7.1% after fusion, at 1.2% *less* code.** Strictly
  Pareto-improving, and the piece to take first. On raw dispatch count it is nearly a wash; what it
  changes is the shape of the output, taking fusable adjacent pairs from 78 to 152 before any
  inlining or unrolling is applied. `conditional-cmv-fusion` is most of it: a select becomes
  `bcc +6; c.mv` rather than a branch diamond or a three-instruction `czero` chain, and that pair
  is one the interpreter fuses into a single dispatch. It is inert without Zca. That item came from
  reading the consumer's fusion table and asking which LLVM tune features produce pairs on it — a
  question worth asking again for the pairs that are not. §5.5 called all this a bonus an order of
  magnitude below §4.1; it is the piece that survives the size constraint.

Together, with the inlining and unrolling limits raised: **−16.6% dispatches against the same LLVM
tuned for hardware, −23.5% with fusion, +19.5% on the interpreted score, at +32% static code** —
which still leaves the binary 20% smaller than GCC's. With fusion off the score gain is only +8.9%,
about half of what the dispatch count alone predicts, because 32% more code is 32% more decoded
stream and the instruction mix changes too; `riscv-interpreter-measurements.md` §5.6 has that
arithmetic.

**That configuration is no longer the default**, because the +32% is not a size cost to be traded
against speed — on register-hungry code it *is* the speed cost. `riscv-interpreter-measurements.md`
§5.6 has the ed25519 case: 3.67× the code, 454 spill stores per call, 64% slower on a −2.5% dispatch
count. What ships is the free part below, and the two limits stay as knobs. With them neutral,
CoreMark is −5.0%/−12.1% at *less* code than `generic-rv64` — but ed25519 is still 15.8% slower from
the scheduling model alone, which is the open item.

The fusion half of that is not additive, it is conditional: the interpreter's fusion pass is a **net
loss** on hardware-tuned code (−3.7% on GCC's binary, −4.9% on `generic-rv64`'s) and a net win
(+9.7%) only on interpreter-targeted code, because below roughly 8% of dispatches eliminated its
per-instruction overhead exceeds the saving. The fusion tune features are therefore not a bonus on
top of the dispatch reduction — they are what makes running the fusion pass worth it at all.
The free part alone, with both multipliers at 1, is −5.0% raw and −11.7% after fusion at 0.9% less
code. `riscv-interpreter-measurements.md` §5.6 has the decomposition and §5.7 has how to
reproduce it.

Static code size is the second objective and the one that bounds this, since a pre-decoding
interpreter stores 8 bytes per 4 bytes of guest code and a smart contract is usually size-limited.
Everything here is on the size/dispatch frontier rather than past it: the uncapped unroll factor was
the one configuration that was not, and it was dropped for that reason.

### What is not built

- **Branchless expansion of a select on a base ISA.** §5.4's mechanism, and the one thing here that
  the dispatch count cannot be used to justify — see §7. Without Zicond, LLVM expands a select into
  a branch diamond; `matrix_sum` in CoreMark still contains one, where GCC produces mask arithmetic.
  Doing it needs a per-instruction cost function that the model does not have yet.
- **Anything at all for a JIT.** The model is named for an interpreter and the one term that differs
  — decoded-stream footprint versus compile time — is exactly the term the unrolling cap turns on.

## 9. What is left, profiled rather than guessed

Found by dispatch-profiling the interpreter build per guest PC
(`interpreter-codegen/coremark-dispatch-profile.patch` is the tooling). Percentages are of the
225.1M dispatches the current CPU executes. Where the benchmark's time actually is:
matrix 100.4M, list 71.5M, state 48.9M, CRC 4.2M.

**Ruled out, with numbers, so nobody repeats them:**

- **Spills.** All stack traffic in the interpreter build is 0.47M dispatches, 0.2%, and no hot loop
  spills at all. The register allocator is not the problem; the uncapped unroll factor was.
- **Prologues and IPRA.** The hot functions are entered 4000 times each — everything else is
  inlined — so `matrix_test`'s twelve callee-saved pairs cost 0.1M. `-enable-ipra` measured
  **−36 dispatches**.
- **Branch pricing, and why it measures −0.05%.** `getPredictableBranchThreshold()` only reaches a
  decision where `extractBranchWeights` succeeds, and CoreMark carries no branch weights at all. The
  hook is not weak; it has nothing to read. A profiled build would give it something.
- **LSR instruction-count costing.** `-lsr-insns-cost=true` is +80 dispatches: RISC-V's
  `isLSRCostLess` already ranks instruction count first.
- `-hoist-cheap-insts` +16K, `-indvars-widen-indvars=false` +1.17M,
  `-unroll-runtime-epilog=false` +0.95M and +82 instructions.

**Worth doing, largest first:**

- ~~**Unroll to a loop's register-rotation period, not to a fixed cap.**~~ **Done**, and it measured
  −2.7% of the whole program for seven static instructions — §8 has it. `core_list_reverse` was
  10.9% of all dispatches and a fixed cap of 2 left one `mv` per node in it; `mv` was 5.8% of the
  program before. Raising the global cap to three would have been the wrong lever: +1780 static
  instructions for −9.3M.
- **A second induction variable per unrolled u32-indexed loop — 3.75M, 1.7%, at zero code size.**
  The exit compare keeps a widened IV alive alongside the pointer IV; narrowing it removes one
  dispatch per iteration of every such loop, unrolled or not.
- **A loop-invariant `zext.b` left inside `core_list_find`'s comparison loop — 1.4M, 0.6%, free.**
  ISel rewrites `(x ^ inv) & 255 == 0` into `lbu` + `andi inv, 255` per unrolled copy, and
  MachineLICM does not hoist the `andi` out even though it hoists the same instruction from a
  reduced copy of the loop. GCC hoists it. Either find the rejecting check in MachineLICM or
  canonicalize the compare in InstCombine so IR LICM gets there first.
- **Two selects sharing a condition, lowered as two independent branch-over-`mv` pairs** rather than
  one — 1.3M to 3.9M unfused in `matrix_sum`, and the code shrinks.

**Worth trying, from reading LLVM rather than the profile:**

- **LoopFlatten**, registered for this target through `registerPassBuilderCallbacks`. It is the one
  mainline pass that removes a whole loop's overhead, it shrinks code, and CoreMark's matrix nests
  are exactly its shape.
- **Three new `SimpleFusion` records** for `addi`+`addi`, `addi`+`sh[123]add` and `xor`+`rori(w)` —
  the three most common fusable pairs in real contract code, none of which LLVM clusters today.
  `+fusion-logic-imm-reg` and `+fusion-add-mem` already exist, are implemented by the interpreter,
  and are worth exactly nothing on CoreMark; the contract workloads are where to measure them.
- **`SeparateConstOffsetFromGEP` + `EarlyCSE`**, the straight-line-scalar group NVPTX and PowerPC
  add, for the address arithmetic in array-of-structure code.

**On optimization level.** Everything above is `-O3`, which is what Rust builds contracts at (fat
LTO, one codegen unit). C is more often `-O2`, and the tuning holds there — −17.4% dispatches and
−23.8% fused against `generic-rv64` at the same level, slightly better than at `-O3`.

`-O2` with the interpreter CPU is 6432 instructions against 6703 at `-O3` for 1% more dispatches,
which looked like a cheap way to buy back code size. Timed, it is not quite: **1.9% slower, ±0.9%**,
over 35 alternating rounds. So it buys 4% of code for 2% of speed — worth making when size is
binding, but a trade rather than a free lunch, and a reminder that the dispatch count can mislead in
the cheap direction as well as the expensive one.

`-Os` is not on the frontier at all: it turns off the CRC recognition that is worth more than
everything in §8 put together, giving back 13% of the dispatch count to save 32% of the code.
