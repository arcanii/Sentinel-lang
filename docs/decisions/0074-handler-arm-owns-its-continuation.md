# ADR 0074: A handler arm owns its continuation until it resumes it

Status: **ACCEPTED** (2026-09-20; proposed 2026-09-19). Closes register item **D79**.
Landed with its fixtures and pins after three rounds of adversarial review: four-check
green — 1,996 passed with exactly the 18 known Windows failures, doctests and clippy
clean, every `selfhost_*` differential green and both bootstrap fixed points
byte-identical.
Amends [ADR 0020](0020-handler-runtime-and-perform-lowering.md) D2 (where the one-shot check
is made) and [ADR 0065](0065-early-return.md) D6 (which record a `return` tears down from),
and extends D6's one-free invariant from `return` to every exit of a handler arm.

## Related

- [ADR 0020](0020-handler-runtime-and-perform-lowering.md) D2 — one-shot continuations,
  enforced by a `consumed` flag in the kont; D4 — an arm "MUST eventually call `k(v)` to
  resume, or return without calling `k` to abort". That `sentinel_kont_resume` frees the
  kont it resumes is not in D7: it is the runtime's `# Safety` contract for that symbol,
  which ADR 0065 D6's one-free invariant relies on.
- [ADR 0065](0065-early-return.md) D6 — `return` crossing a `handle` frees the abandoned kont
  (`sentinel_kont_free`), under a one-free invariant: resume frees on the normal path, the
  teardown on the early-return path.
- [ADR 0036](0036-loops.md) D9 — `break` / `continue` drain the scope
  frames down to the loop's floor before branching; this ADR adds the arms entered since.
- [`abi-v1.md`](../abi-v1.md) §3 (`SentinelKont`) and §5 (`sentinel_kont_resume`,
  `sentinel_kont_free`).

## Context

A handler arm binds its continuation `k` to the kont the dispatch loop matched. ADR 0020 D4
makes resuming optional: an arm that yields a value without calling `k` aborts the rest of
the handled computation, and that value becomes the `handle`'s. Nothing freed the kont on
that path. Only ADR 0065 D6's `return` teardown ever called `sentinel_kont_free`, and only in
the inkwell back end — the `snc llvm` oracle and the self-hosted `scg` deferred it — so an
arm like `Io.read(k) => 5` dropped the kont, its frame nodes and their captured blocks on
every `handle` it ran (register D79).

Measured before this change on the shipped compiler, kernel peak working set read after the
process exits, with the `handle` in a helper fn called from a loop:

| program | 600,000 calls | 3,000,000 calls |
|---|---|---|
| `Io.read(k) => k(5)` (control — resumes) | 8.9 MB | 8.9 MB |
| `Io.read(k) => 5` over a direct `perform` | 36.5 MB | 147.1 MB |
| `Io.read(k) => 5` over an effecting fn with one frame and no captures | 55.0 MB | 239.2 MB |
| `if (i / 2) * 2 == i { k(10) } else { 5 }` — declines on odd `i`, half the calls | 22.7 MB | 78.0 MB |
| `{ if i > 0 { continue; 0 } else { 0 }; 5 }`, the `handle` in a one-pass loop | 36.6 MB | 147.1 MB |
| `handle (handle g() with { Io.read(k) => k(40) }) with { Log.get(k2) => 2 }` | 64.3 MB | 285.7 MB |

That is about 48 bytes per call for the frameless kont (one 32-byte `SentinelKont` plus
allocator overhead) and about 80 for the framed one — a 24-byte frame node more; that effecting
fn captures nothing across its `perform`, so its frame carries no captured block. A frame that
captures one `i64` measures about 97, which is the last row: `g` carries `a` across its second
`perform`. The shape that declines half the time leaks half the frameless rate. Whether an arm
resumes can depend on its input, so a program that declines to resume on some inputs leaks at
the rate those inputs arrive.

Declining to resume is not the only exit that abandons `k`. An arm is left:

1. by **falling through** to the `handle`'s merge with its value;
2. by a **`return`** (ADR 0065 D6);
3. by a **`break` or `continue`** to a loop that encloses the `handle` — the type checker's
   loop depth is not reset at a `handle`, so this is legal today, and it too dropped the kont;
4. by a **bubble**: `k(v)` resumed, the resumed computation performed again, and the arm
   branches back to the dispatch loop with the new kont.

Exits 1 and 3 never released the kont in any back end, and exit 2 released it in inkwell
only. Wherever an exit only dropped the kont, the value the program computed was right, so no
test could see it.

ADR 0065 D6's teardown also has the wrong record to work from. It frees what the `handle`'s
**dispatch slot** holds. That slot still holds the arm's kont after `k(v)` has consumed it,
so it cannot tell a resumed kont from an abandoned one. ADR 0020 D2's `consumed` flag cannot
either: it lives inside the kont, and `sentinel_kont_resume` frees the kont it resumes, so
there is no kont left for a second `k(v)` to find the flag in.

## Decisions

### D1. The arm's continuation slot is the ownership record.

Each arm already stores its kont into a slot of its own (the `k` binding). `k(v)` now
**clears that slot** before it resumes: it evaluates the argument, loads the kont, stores
`null` back into the slot, then calls `sentinel_kont_resume`, which frees the kont as it
always has. So at every point in an arm, a non-null slot means *this arm still owns the
kont*, and a null slot means *it was resumed*. `k` is second class — the type checker refuses
any use of it but a call (`KontUsedAsValue`) — so no other binding can hold the kont; the one
other copy is the `handle`'s dispatch slot, which no teardown reads any more.

The argument is evaluated **before** the slot is read (the back ends used to read it first).
An argument that leaves the arm — `k(return 5)`, `k({ break; 0 })` — therefore leaves the
slot owned, and that exit releases it (D2); read first and cleared, the slot would have
told the exit there was nothing to release. And a `k(v)` nested in its own argument,
`k(k(1))`, is refused by D3 at the outer call, which finds the slot the inner one cleared.

### D2. Every exit of an arm releases what its slot still holds.

Each exit loads the arm's slot and, if it is not `null`, calls `sentinel_kont_free` on what it
loaded:

- **fall-through** — after the arm's body, before its value is stored to the `handle`'s
  result (a nested `handle` wraps it via `sentinel_kont_pure` after the release);
- **`return`** — for every arm open in the function, innermost first, after the scope drops
  and before the `ret`. This **replaces** ADR 0065 D6's read of the dispatch slot;
- **`break` / `continue`** — for every arm entered since the target loop began, innermost
  first, after the scope drops and before the branch. Each loop records the arm depth at
  its entry, exactly as it records its scope floor;
- **bubble** — nothing: the resume cleared the slot, and `sentinel_kont_resume` freed the
  kont and handed back a new one, which the dispatch loop binds to the next arm.

Each kont is therefore freed exactly once on every path: by `sentinel_kont_resume` if the
arm resumed it, by the arm's exit otherwise. This is ADR 0065 D6's one-free invariant, keyed
on the one record that knows which case holds.

Two arms are open at once in one function only when a `handle` is written inside another
handle's arm. A `return` from the inner arm, or a `break` / `continue` from it to a loop
outside both arms, walks both, inner then outer. The inner arm's fall-through, and a `break` /
`continue` to a loop inside the outer arm, release the inner arm only. `c74_two_open_arms`
pins the exits that leave both arms (a `break` out of both leaked about 97 bytes per call
before this ADR); its inner `handle`'s value is discarded, and the inner `handle` has no
`return` arm. Where the inner value is used, or an inner `return` arm resumes the outer `k`,
register D81 gets there first (D6).

### D3. The one-shot check is made on the slot.

`sentinel_kont_resume(null, v)` aborts with the existing diagnostic
(`sentinel_kont_panic_resumed`: "continuation already resumed (one-shot per ADR 0020 D2)").
A second `k(v)` in the same arm finds its slot clear, so the check no longer depends on the
kont that the first resume freed. ADR 0020 D2's `consumed` field stays in the layout (it is
`abi-v1` §3) and the runtime still writes it, but it no longer decides anything.

### D4. `abi-v1`: no symbol, signature or layout changes, and the `null` test is in the emitted code.

The runtime an artifact runs against is not always the one its compiler shipped with. Two
`--lib` archives built by different compilers and linked into one program resolve every
`sentinel_*` symbol from whichever archive the linker meets first; a stale runtime staticlib
can sit beside a newer `snc`; and `snc build --separate` can reuse a unit object another `snc`
compiled (register D73). So the emitted code relies on nothing this ADR adds to the runtime:
D2's `null` test is in the emitted code, and `sentinel_kont_free` is only ever called on a
kont. Against the runtime from just before this ADR, the five programs under Verification
give the same exit codes.

The emitted code does reference `sentinel_kont_free` more widely. Every handler arm's exits
now call it, where before only inkwell's `return` out of an arm did, so a program with a
handler arm needs a runtime from ADR 0065 D6 (2026-06-29) on. An `abi-v1` runtime older than
that has no such symbol, and the program fails to link against it.

The runtime still gains two widenings, as defence in depth: `sentinel_kont_free(null)`
returns without doing anything, and `sentinel_kont_resume(null, _)` aborts as in D3. Both
widen what the symbol accepts, so every call an earlier compiler emitted behaves as before.
The one place new code does hand a runtime `null` is a second `k(v)` — an illegal program;
against the runtime from just before this ADR it dereferences `null` instead of printing D3's
diagnostic.

### D5. All three back ends; oracle-moving.

inkwell, the `snc llvm` text oracle and `scg` get the same shape — the oracle and `scg`
byte-identical — which also retires ADR 0065 stage 3's deferral of the text-IR teardown.
Every `handle` arm's IR changes — on each exit a load, a `null` test and a guarded
`sentinel_kont_free` call; at each `k(v)` a `store ptr null` — so the rhythm is the
oracle-moving one. (`scg`'s known mis-lowering of a direct call on a `Fn`-typed local, ADR
0070's deferred D3-revisit and the `fn_value` entry in `DEFERRED_PROGRAMS`, goes through the
same resume path, so it now clears that local's slot too; the entry's reason still holds.)

### D6. What this does not cover.

- **D81's shapes.** A `handle` inside another handle's arm — an op arm or a `return` arm —
  whose value is USED does not work: let-bound it answers garbage; as the arm's value, a
  `k(v)` argument or an operand it fails to compile (an LLVM verification failure, or an
  inkwell panic where the value is an integer operand). D2's multi-arm walk is pinned only
  where that value is discarded.
- **An inner `handle`'s `return` arm that resumes the OUTER `k`** (also D81). `scg` recurses
  at compile time until its own stack overflows, and so do `snc build` and `snc llvm` once
  any inner op arm contains a `k2(v)`, whether or not the inner value is used: while that op
  arm is lowered, `handle_stack.last()` is the inner frame, so `k2(v)`'s pure path applies the
  inner `return` arm, whose `k(w)` applies the same arm again. Identical before and after this
  ADR.
- A `k(v)` of an **outer** arm written inside an inner `handle`'s op arm branches its bubble
  to the inner `handle`'s dispatch loop (`handle_stack.last()`), not the outer one. Same
  D81 territory; D2's bubble case assumes the resumed `k` is the innermost arm's own.
- **The rest of an arm after a bubbling `k(v)`.** The bubble exit branches straight back to
  the dispatch loop, so whatever the arm would have done with `k(v)`'s value never runs and
  the arm's scope drops are skipped — register D87. D2 is right about the kont on that exit
  (the slot is clear); the arm's other bindings are D87's.
- Heap values inside a captured block. `sentinel_kont_free` releases the block, not what it
  points to; ADR 0072 D4's `FITS` admits only `i64` and `secret i64` captures, so there is
  nothing inside to release in the shipped compiler. (Register D69's remainder — the oracle
  and `scg` not applying ADR 0072 A1 — is where a wider capture would come from.)

## Consequences

### Positive

- The leak is gone on every exit, in all three back ends.
- A second `k(v)` aborts with the one-shot diagnostic, in all three back ends.
- The oracle and `scg` now carry the teardown ADR 0065 D6 gave inkwell alone.
- Code compiled from this ADR on relies on nothing this ADR added to the runtime. Against the
  runtime from just before it, the five programs give the same exit codes, and only an
  illegal second `k(v)` fails differently (D4).

### Negative

- On each arm exit a load, a `null` test and a branch around the free, and one store per
  `k(v)`, in every program that handles an effect.
- Every program with a handler arm now references `sentinel_kont_free`, so it no longer links
  against an `abi-v1` runtime from before ADR 0065 D6 (D4).

### Neutral

- The dispatch slot (`current_kont_slot`) keeps its dispatch job — holding the kont the loop
  dispatches next: the body's first, then each bubbled one — and is no longer read by any
  teardown.

## Verification

- **Memory**, measured as in Context: after the change every shape in the table is flat at
  both call counts, level with the resuming control measured in the same batch (8.3–8.9 MB
  across batches; the baseline drifts about 0.5 MB from batch to batch).
- **inkwell**: seven `adr0074_*` unit tests in `sentinel-codegen` read the IR the shipping
  back end verified (a `#[cfg(test)]` capture, `LAST_VERIFIED_IR`). They count one release per
  exit and open arm — fall-through, `return`, `break`, `continue`, both arms for a `return`,
  `break` or `continue` that leaves two, none for a `break` inside a loop in the arm — counting
  only releases reachable from the function's entry, so a `return`, `break` or `continue`
  release moved past its exit's terminator, into the dead block that follows it, does not
  count. Each release must be the taken edge of a conditional branch on `icmp ne ptr` of the
  kont it frees, loaded from an arm's continuation slot; and the argument must precede the
  slot's load and clear.
- **Text oracle**: `tests/llvm.rs` pins a golden for a conditional resume and, over the five
  programs below, per-function counts of reachable releases, the same guard, that no release
  reads a dispatch slot, the load-clear-resume sequence and D1's argument order.
- **`scg`**: the five programs under `crates/sentinel-driver/tests/fixtures/handler_arm_exits/`
  are seeds of the codegen differential, so `scg` is held to the oracle byte-for-byte on every
  exit. They are not `tests/pass` fixtures because the MIR differential sweeps every file there
  and an `if` inside a handler arm makes the two MIR lowerers diverge (register D83,
  pre-existing — identical output before and after this ADR).
- **End to end**: `tests/handler_arm_exits.rs` runs the five programs through `snc build`
  (exit 143, 34, 11, 13, 16) and three one-shot shapes, each of which must abort with ADR 0020
  D2's diagnostic (D3). `tests/selfhost_codegen.rs` compiles the oracle's IR of the same five
  programs (with `llc` and `link.exe` on Windows, `cc` elsewhere) and runs it for the same exit
  codes; `scg` emits that IR byte-for-byte, so this runs the text back ends' releases. Running
  them also shows which slot a release reads once its arm has resumed: had `after`'s `return`
  release read the `handle`'s dispatch slot (ADR 0065 D6's old record, Context),
  `c74_arm_return_leaves_the_arm` would exit differently (measured on Windows). Until an arm
  resumes, the two slots hold the same kont in these programs, so a release that runs before
  it — every `break` / `continue` release here, for one — exits the same whichever it reads;
  `tests/llvm.rs` checks every release's slot.
- **Runtime**: `sentinel_kont_free(null)` is a no-op; `sentinel_kont_resume(null, _)` aborts
  with the one-shot diagnostic (checked in a child process).
- **Older runtime**: the five programs, built with this ADR's `snc` against the runtime from
  just before it, give the same exit codes; only the illegal double resume fails differently
  (D4).
- **Corpus**: every `.sentinel` file in the tree (464) through `snc llvm`, and where it has a
  `main` through `snc build` and a run, before and after the change. No file changes whether
  it is accepted or builds; the IR changes in 40 files, each of which contains a `handle`; 284
  of the 285 programs that build run the same (the two Win32 message-box demos time out both
  times), and the one that differs is `c74_arm_return_leaves_the_arm` — Context's `return`
  after a resume.
- **Mutation**: thirty-three mutations, each reverting or displacing one piece in one back end
  or the runtime, were each caught by a pin — sixteen in a first round, run again on the final
  code; nine in a second, on what that round missed — the oracle's slot clear, `scg`'s argument
  order, the per-loop arm floor, the `null` guard in each back end, and the runtime's `null`
  refusal end to end; and eight in a third, on the pins the second and third reviews prompted —
  inkwell's guard inverted, its branch targets swapped, or testing the slot's address instead
  of the kont; a `return` release in inkwell and a `break` / `continue` release in the oracle
  moved past the exit's terminator; a `return` release reading the dispatch slot, in each (in
  the oracle caught both by `tests/llvm.rs` and by running its IR; in inkwell by the unit tests
  and by `tests/handler_arm_exits.rs`, which could already see it); and inkwell's `break` /
  `continue` out of two arms releasing only the inner one, which no pin saw until the seventh
  unit test. Inkwell's guard pin was too weak twice over: it first counted the guard's `icmp`, which a mutant with an
  unconditional branch still emits, then only that a conditional branch existed; it now checks
  the branch's taken edge, its `icmp ne` of the freed kont, the arm slot that kont was loaded
  from, and that the release is reachable. With the slot clear removed the three one-shot tests
  still passed (the `snc build` of 2026-09-19, on Windows), so they pin the abort and the
  runtime's `null` refusal behind it, not the clear.
- Both bootstrap fixed points hold; every `selfhost_*` differential is green.
- Four-check: 1,996 passed with exactly the 18 known Windows failures; doctests and
  `clippy -D warnings` clean.
