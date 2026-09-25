# [RFC] RISC-V: a tuning target for code that gets interpreted rather than executed

A lot of RISC-V code never reaches silicon. It gets decoded and dispatched by an
interpreter, or handed to a simple JIT. For that kind of consumer the cost of a program is
roughly how many instructions it retires — there is no pipeline to keep busy, no branch
predictor to please, no cache hierarchy to schedule around.

LLVM has no way to say this. Every RISC-V `-mcpu`/`-mtune` describes hardware, so projects
in this space either take a tuning aimed at a machine they are not, or build one by hand out
of flags meant for real cores. Both are happening right now:

- PolkaVM's rustc target spec sets `+auipc-addi-fusion,+ld-add-fusion,+lui-addi-fusion,+xtheadcondmov`
  on `generic-rv64` — LLVM's macro-fusion tuning features, plus a T-Head conditional-move
  extension for branchless selects, on a target with no T-Head core within a mile of it.
  ([riscv64emac-unknown-none-polkavm.json @ 28d4606](https://github.com/paritytech/polkavm/blob/28d460607f46c8609d644d8a84060fb92d94a272/crates/polkavm-linker/targets/1_91/riscv64emac-unknown-none-polkavm.json))
  Those three fusion features have since been renamed to `fusion-auipc-addi`, `fusion-ld-add`
  and `fusion-lui-addi`, so a current LLVM ignores all three with a warning — the hand-assembled
  tuning has quietly stopped doing anything, which is the failure mode this whole approach has.
- SP1 passes `-C llvm-args=-misched-prera-direction=bottomup` and
  `-misched-postra-direction=bottomup` to steer the machine scheduler directly.
  ([crates/build/src/command/utils.rs @ 9c94078](https://github.com/succinctlabs/sp1/blob/9c94078055b9c1a1201b0636eb2990539fce8f11/crates/build/src/command/utils.rs#L67-L81))
- libriscv tells users to rebuild their toolchain without the C extension for a 20-25%
  interpreter speedup, and to unroll loops in the guest.
  ([README @ e121610](https://github.com/libriscv/libriscv/blob/e121610a7b88bb902e9d2a33b4cdfc14847003e5/README.md?plain=1#L392))

These are reasonable workarounds for something missing, and they are fragile: they lean on
internal flags and on features whose meaning is tied to specific hardware.

## Who runs RISC-V in software

**Sandboxing and embedding.** libriscv sandboxes game scripts and request handlers, with a
pre-decoding threaded interpreter and optional binary translation; RVVM is a desktop-class
emulator with a tracing JIT.

**Blockchain VMs.** PolkaVM (Polkadot) recompiles RISC-V at link time into its own bytecode,
run by an interpreter or an AOT recompiler. CKB-VM (Nervos) runs RV64IMC+B in a pre-decoding
assembly interpreter. Cartesi runs RV64GC well enough to boot Linux.

**zkVMs.** RISC Zero (RV32IM), SP1 (RV64IM), ZisK (RV64IMA), Jolt (RV64IMAC), OpenVM
(RV32IM). The Ethereum Foundation's zkvm-standards work proposes a shared guest target,
`riscv64im_zicclsm-unknown-none-elf`, with the compressed extension explicitly excluded.

Two things recur. The cost unit is the dispatch, not the cycle — several of these meter gas
per instruction from an explicit cost table, so for them it is not even an approximation.
And several actively want *adjacent instruction pairs*, because their decoder folds them into
one dispatch: CKB-VM's macro-op fusion (RFC-0033/0049) covers `auipc+jalr`, `lui+addiw` and
`add`/`sltu` carry chains. A scheduler tuned for hardware will happily split exactly those
pairs apart.

## What I have working

One tuning and an `interpreter-target` subtarget feature, on LLVM main, exposed under the same
three names upstream already uses for `generic`: `generic-interpreter-rv32` and
`generic-interpreter-rv64` mirror `generic-rv32`/`generic-rv64`, and `generic-interpreter` mirrors
`generic` as a tune-only entry, so `-mtune=generic-interpreter` pairs with any `-march`. The RV32
and RV64 names have to be separate because a `-mcpu` carries the base ISA width and a mismatch is
an error, but all three take the identical tune-feature list. None carries any extension of its
own, so `-march` still decides the ISA. Two things differ from `generic-rv64`:

- **Branches are priced as unpredictable.** A guest conditional branch becomes a host
  indirect jump whose target depends on guest data, and the host predictor cannot learn it.
  So `getPredictableBranchThreshold()` is 1 and jumps are marked expensive, and a compound
  condition gets computed instead of split into a chain of branches.
- **The macro-fusion tuning features are on** — not because any hardware will fuse, but
  because the scheduling edges make fusible pairs more likely to come out adjacent.

That is all. No scheduling model of its own, inlining and unrolling untouched.

### CoreMark

Same LLVM for both mid-level and codegen, same guest ISA (`rv64imc_zba_zbb_zbs`), 1000
iterations, run on a pre-decoding interpreter that fuses adjacent pairs.

| | instructions retired | retired, interpreter fuses | static insts | fusable pairs | score | score, interpreter fuses |
|---|---|---|---|---|---|---|
| GCC 14.2 | 301,590,885 | 297,906,065 | 8387 | 30 | 2879 | 2898 |
| LLVM `generic-rv64` | 262,393,574 | 261,145,777 | **5077** | 79 | 3257 | 3215 |
| LLVM `generic-interpreter-rv64` | **250,983,705** | **232,314,852** | 5110 | **147** | 3413 | **3842** |

`generic-interpreter-rv64` dispatches **4.35% fewer instructions** than `generic-rv64`, and
**11.5% fewer** once the interpreter's fusion pass runs on top, for 0.65% more code than
`generic-rv64` and 39% less than GCC.

Scores are iterations/sec, higher is better; best against best that is +17.9% over `generic-rv64`
and +32.6% over GCC. Treat those as the optimistic end: run-to-run spread on identical binaries is
~3%, and alternating the two builds round by round on a pinned core instead gives **+8.9%** and
**+26.5%** on a second machine, with the rounds disjoint. The dispatch columns are the ones that
reproduce exactly.

Fusion is the other half. Running the interpreter's fusion pass removes 7.44% of dispatches from
the interpreter-targeted build but only 0.48% from the hardware-tuned one — below roughly 1%
fused, its per-instruction overhead is not repaid at all. So the fusion tuning features are not a
bonus on top of the instruction-count reduction; they are what makes running a fusion pass worth
it in the first place.

Concretely, what my interpreter fuses is any adjacent pair matching one of its own rules, whether
or not LLVM has a corresponding `Fusion` record. Both builds, side by side, with the LLVM tune
feature that puts each pair next to its partner:

| pair | `generic-rv64` | `generic-interpreter-rv64` | LLVM tune feature |
|---|---|---|---|
| `auipc`+`addi` | 28 | 55 | `auipc-addi` |
| `sh[123]add(.uw)`+load | 15 | 27 | `shxadd-load` |
| `add`+load/store | 12 | 15 | `add-load`; the store half has no feature |
| branch+`c.mv` | 0 | 24 | `conditional-cmv-fusion` |
| `slli`+`srli`(`.uw`) | 9 | 10 | `bfext`, `shifted-zext` |
| `addi`+`addi` | 9 | 9 | **none** |
| `auipc`+`ld` | 3 | 3 | `auipc-load` |
| `and`/`andi`+`or`/`andi` | 3 | 4 | `logic-reg-reg`, `logic-reg-imm` |
| **total** | **79** | **147** | |

Two things in there are the argument. Branch+`c.mv` goes from zero to 24 — the category does not
exist in the hardware-tuned build — and it comes entirely from `conditional-cmv-fusion`, which is
not a scheduling feature at all: it gates select lowering in `RISCVISelLowering.cpp`. And
`addi`+`addi` fires nine times in both builds with no LLVM feature behind it, because none
exists; the same is true of `addi`+`sh[123]add` and `xor`+`rori(w)`, which do not appear in
CoreMark but are the two most common adjacent pairs in the contract code I actually care about.
Those are three `SimpleFusion` records that would cost nothing on hardware with the feature off.

### Two real workloads

Interpreted, same setup, lower is better:

| | `generic-rv64` | `generic-interpreter-rv64` | |
|---|---|---|---|
| BLAKE3, hash one chunk | 16.6 µs | 14.5 µs | **12% faster** |
| ed25519, verify a signature | 687 µs | 690 µs | unchanged |

BLAKE3 gains, ed25519 does not lose. I would rather show a flat result than leave it out.

<details>
<summary>Two things I tried that made it worse, which may be more useful than the above</summary>

Both of these seemed obviously right and were not. I worked through them with an LLM as a
research assistant, which is also how most of the measurement plumbing got written.

**A scheduling model that described an interpreter honestly.** One serial dispatch slot,
occupancy equal to the per-dispatch cost (a load pays for address arithmetic, a bounds check
and a memory access; a multiply runs a longer handler), latency equal to that cost because a
handler finishes before the next dispatch starts. It produced code about **10% slower than
turning the machine scheduler off entirely**. When occupancy equals latency there is no
slack, so the scheduler has no gap to fill and no reason to keep a producer near its
consumer — while still being free to stretch live ranges. On a 5×5-limb field multiply from
curve25519-dalek it left all fifty partial products live at once: 27 spill stores, where the
generic hardware model interleaves each product with its consumer and spills nothing. A spill
is a guest memory access, one of the most expensive dispatches there is, so the model lost on
its own terms.

**Raising the inlining and unrolling thresholds**, on the grounds that an interpreter has no
instruction cache to protect. That made an ed25519 field inversion **64% slower at 3.7× the
code**: inlining a callee whose loop had high register demand made its trip count constant,
the loop was then fully unrolled, and a body that already needed every register spilled hard.

Both came from the same assumption — that an interpreter's cost is just its dispatch count —
and both were caught only by timing, never by counting. If anyone can see how to express
"reordering is free, register pressure is not" without lying about latency, I would like to
hear it.

</details>

## Prior art

CPU names in LLVM already describe things that are not silicon — WebAssembly's `lime1`,
x86-64-v2/v3/v4, AMDGPU's `gfxN-generic`, RISC-V's own `generic-ooo` — and `RISCVUsage.md`
already anticipates more than one generic model: *"Right now, we simply assign a scheduling
model that is widely used by the community to `generic`. But in the future, we can create a
standalone scheduling model for `generic`, or even create a generic model for each of the
individual sectors. For example, a `generic-embedded` for embedded processors and a
`generic-server` for server workloads."*
([RISCVUsage.md](https://github.com/llvm/llvm-project/blob/main/llvm/docs/RISCVUsage.md#scheduling-model-and-tuning),
added by [#167008](https://github.com/llvm/llvm-project/pull/167008), which also drew a report
of a ~2.3% CoreMark regression on an embedded core.) This would be another of those sectors.

## A second consumer, where it does nothing, and a third, where it does

PolkaVM was the obvious independent check: it hand-assembles a version of this tuning already,
and it has both an interpreter and a recompiler. Only the `cpu` field of its target spec differs
between the runs. Execution time, excluding compilation:

| benchmark / backend | PolkaVM's tuning | + `generic-interpreter-rv{32,64}` | |
|---|---|---|---|
| pinky, 64-bit interpreter | 72.05 ms | 75.19 ms | +4.4% |
| pinky, 64-bit recompiler | 7127 µs | 7191 µs | +0.9% |
| prime-sieve, 64-bit interpreter | 42.94 ms | 42.93 ms | −0.0% |
| prime-sieve, 64-bit recompiler | 2918 µs | 2885 µs | −1.1% |

**Nothing.** Small, inconsistent in sign, and on single measurements that demonstrates no effect —
except possibly pinky's interpreter, which is worse.

The reason is worth more than the result. **PolkaVM does not execute RISC-V.** Its linker lowers
the RISC-V ELF into its own bytecode and runs its own optimization pipeline doing it —
`perform_inlining`, `perform_dead_code_elimination`, `perform_constant_propagation`,
`perform_nop_elimination`, `perform_load_address_and_jump_fusion` in
[`program_from_elf.rs`](https://github.com/paritytech/polkavm/blob/28d460607f46c8609d644d8a84060fb92d94a272/crates/polkavm-linker/src/program_from_elf.rs).
Whatever LLVM decided about selection and adjacency gets re-decided there, and both backends run
that bytecode, so neither moves.

So this is narrower than "software execution targets". It is for consumers that **dispatch
RISC-V instructions directly**. Where a consumer re-compiles RISC-V into something else first,
its own middle-end decides and the RISC-V tuning washes out. PolkaVM is still evidence for the
*problem* — it is hand-assembling hardware flags and they have silently broken — but not for
this solution.

So I ran a third one that does dispatch RISC-V directly: libriscv, which pre-decodes an ELF into
a threaded dispatch table with no bytecode of its own in between. CoreMark again, but a hosted
build of it against picolibc rather than the freestanding one above, 40000 iterations, CRCs
validated, nine pinned runs each on two machines, interpreter mode, `-mcpu` the only difference:

| | `generic-rv64` | `generic-interpreter-rv64` | |
|---|---|---|---|
| guest instructions dispatched | 10 492 964 537 | 10 036 546 491 | −4.35% |
| host wall clock, median of 9, machine A | 8455.8 ms | 8223.2 ms | **−2.75%** |
| host wall clock, median of 9, machine B | 3577.5 ms | 3485.9 ms | **−2.56%** |
| `.text` | 22 040 B | 22 144 B | +0.47% |

Two things about this. The instruction count is a property of the codegen, not of the VM, and it
lands on the same −4.35% as my own interpreter on a different CoreMark port — so the first table
is not an artifact of my dispatcher. And roughly half of that converts into wall-clock time on
someone else's mature interpreter, from a change that is one `-mcpu` flag.

Across both machines, median, trimmed mean and fastest-run all fall between −1.8% and −2.8%, so
−2.5% is the honest figure rather than any single one of them. Shorter runs are not worth quoting:
at 0.7 s per run the same three statistics spanned −1.2% to −3.1% purely on host noise.

## What would settle it

Two interpreters is not a case for a target in the backend; four or five would be. CKB-VM also
pre-decodes and dispatches RISC-V as-is, and so do the zkVM emulators. If you maintain one of
those, or anything else that dispatches RISC-V directly, I would like to hand you the patch and
be told whether it reproduces — including if it does not, because the PolkaVM result says the
answer depends on what the consumer does with the instructions after it reads them.
