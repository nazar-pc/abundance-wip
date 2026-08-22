# Direct threading, and what the padded slot really bought

Working notes on a dispatch variant that was built, measured and then **not** taken. Everything
stated as a measurement was measured; everything else is flagged as untested. Read
`DISPATCH-HANDOFF.md` first — this assumes the threaded dispatch it describes, as it stands after
the phase-3 work on `claude/risc-v-dispatch-phase-3-tqhzyl`.

The experiment is reproducible: `DISPATCH-DIRECT-THREADING.py` applies it to a checkout of that
branch (`python3 DISPATCH-DIRECT-THREADING.py` from the repository root, `git checkout crates/` to
undo), and `DISPATCH-DIRECT-THREADING.patch` is the diff it produces, against
`45a91bd` ("Hand refused branch targets to a cold continuation with a tail call"). It is kept here
so it does not have to be written a third time.

## 0. Summary

Direct threading is worth **+5.3%** of Coremark, of which only **+3.0%** is direct threading — the
other **+2.2%** comes from the decoded slot growing to 16 bytes, and is available *without* direct
threading, from padding alone. The price is that the decoded instruction stream doubles: it is
already 8 bytes for every 2 bytes of guest code, so this makes it 16, an **8x** expansion of the
program being interpreted.

Not taken, for two reasons:

- The +2.2% that padding buys is almost certainly not about the padding. Whatever it is —
  prefetcher stride, line alignment of the slot — is worth chasing with an explicit prefetch or
  something similar, which costs no memory at all. **Untested.**
- The remaining +3.0% does not justify 8x on its own, and direct threading is the sort of thing
  that wants to be an *option* rather than a replacement, the way threaded dispatch is an option on
  top of the `match` loop today. Whoever adds it should add it as a third mode.

All numbers are single-machine (Xeon Gold 6248R, 2 load ports); see `DISPATCH-HANDOFF.md` §6 for why
that matters. Zen 4 has 3 load ports and has repeatedly ranked these variants differently.

## 1. What direct threading is here

Today the decoded slot is 8 bytes and dispatch is *token* threaded: the slot carries a 16-bit
variant tag, and every handler ends by loading the next slot, extracting the tag and indexing a
table of handler pointers.

```
mov    0x10(%rsi),%rdi     ; next instruction, tag and payload together
add    $0x10,%rsi
movzwl %di,%eax            ; tag
lea    table(%rip),%r11
jmp    *(%r11,%rax,8)
```

Direct threading widens the slot to `{ instruction: 8, handler: *const () }` and resolves each
instruction to its handler once, when the stream is decoded. The tag, the table and the load
through it all disappear:

```
lea    0x20(%rax),%rsi     ; advance
mov    0x20(%rax),%rdi     ; next instruction
jmp    *0x28(%rax)         ; its handler, straight from the stream
```

Five instructions become three, and the jump target no longer depends on a second load.

## 2. What was measured

Coremark, four builds, runs interleaved, median of four:

| build | slot | dispatch | it/s | vs base |
| --- | --- | --- | --- | --- |
| baseline (`45a91bd`) | 8 B | table lookup | 2236 | — |
| lean dispatch, no `ThreadedDispatchResult` | 8 B | table lookup | 2242 | +0.3% |
| **padded slot only** | 16 B | table lookup | 2285 | **+2.2%** |
| **direct threading** | 16 B | pointer in slot | 2354 | **+5.3%** |

Three things worth keeping:

**The 2x slot is not a speed cost — it is a speed gain.** Padding the slot to 16 bytes with a `u64`
that nothing reads bought +2.2% on its own. Doubling the decoded stream did not cost throughput at
Coremark's size; something about the resulting 32-byte stride per 4-byte guest instruction is worth
more than the extra memory traffic. This is the result that should be chased separately.

**Direct threading itself is worth ~3%** (2285 → 2354), for the two instructions and the removed
table load.

**The dispatch-result enum is free.** `ThreadedDispatchResult`, which the dispatch step constructs
and the handler immediately destructures, costs nothing: LLVM folds it into the jump table. It was
worth checking because it does *not* fold once the table is gone (see §4).

## 3. What it costs

The decoded stream holds one 8-byte slot per 2 bytes of guest code — every second slot of a 4-byte
instruction is unused, which is what buys a program-counter-to-slot mapping that is a shift. Direct
threading makes that 16 bytes per 2 bytes of guest code.

For Coremark, whose `.text` is 27,580 bytes: **108 KiB today, 215 KiB with direct threading**.

## 4. Traps for whoever picks this up

Two things make the naive version *slower* rather than faster, and both took a while to find:

- **`FetchInstructionResult` has a niche.** `InstructionFetcher::peek_instruction()` returns the
  instruction wrapped in an enum whose other variants are encoded in out-of-range values of the
  instruction's own tag. Constructing it from a raw read therefore compiles to a range check
  (`cmp $0x93,%ebx` against the variant count) plus a branch to a twelve-way switch that builds an
  `ExecutionError`. Today that check is free, because the `match instruction` that selects the
  handler absorbs it into the same jump table. Remove the table and it materializes in every
  handler. The experiment adds `peeked_instruction_raw()`, which reads the instruction without the
  wrapper — sound for the eager fetcher, whose position always holds a decoded instruction.
- **`ThreadedDispatchResult` has the same niche, for the same reason.** Same fix: the experiment has
  the dispatch step return a plain tuple.

Between them these two were the difference between 1700 it/s and 2354 it/s. Anyone measuring a
direct-threaded variant that comes out *slower* should look here first.

Also: `handler_for()` — the function that maps a variant to its handler, used once per instruction
at decode time — must be `#[inline(always)]`. Left as an ordinary call it is emitted as a real call
in the dispatch step of every handler, which costs six callee-saved pushes and takes Coremark to
980 it/s.

## 5. If it is added later

As a third mode, alongside the `match` loop and today's token-threaded dispatch, not as a
replacement for either. The shape the experiment used:

- `Slot { instruction, handler: *const () }` in the fetcher, with the handler filled in once after
  decoding.
- `InstructionFetcher::peeked_instruction_raw()` and `peeked_handler()`, both `unsafe`, valid
  between `peek_instruction()` and `advance()`.
- A generated `pub(crate) handler_for_<enum>()` that the runner calls once per decoded instruction
  to fill the slots — it needs the same generic parameters the handlers are monomorphized with, so
  the caller has to name them.
- The dispatch step returning `(instruction, handler)` rather than `ThreadedDispatchResult`.

The experiment wires this up for the Coremark fetcher only; every other fetcher gets a stub that
panics. Making it a real option means giving the fetcher a say in which mode it supports.
