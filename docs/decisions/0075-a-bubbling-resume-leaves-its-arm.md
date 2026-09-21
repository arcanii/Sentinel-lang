# ADR 0075: A bubbling resume leaves the arm's entry, and must drain it

Status: **ACCEPTED for slice 1** (D1–D5, 2026-09-21), **PROPOSED for slice 2** (D6).
Addresses register item **D87**, whose leak half slice 1 closes.
Amends [ADR 0074](0074-handler-arm-owns-its-continuation.md) D2, whose bubble bullet
settles what the bubble does with the arm's *continuation* and is silent on the arm's
*scopes*.

## Related

- [ADR 0020](0020-handler-runtime-and-perform-lowering.md) D3 — deep handlers: the arm's
  body re-wraps the continuation's tail, `k := \v. handle (kont.resume v) with H`. D7 —
  the dispatch loop, and `sentinel_kont_resume` returning either a `PURE_RETURN` kont or a
  fresh op kont the caller must re-dispatch (C3.5(e)'s *bubble*).
- [ADR 0074](0074-handler-arm-owns-its-continuation.md) D1/D2 — the arm's continuation slot
  is the ownership record, and **every exit of an arm releases what its slot still holds**.
  D2 enumerates four exits — fall-through, `return`, `break`/`continue`, bubble — and says
  of the last: "nothing: the resume cleared the slot". That is right about the kont and
  says nothing about the arm's heap bindings, which is this ADR.
- [ADR 0036](0036-loops.md) D9 — `break`/`continue` drain the scope frames down to the
  loop's floor before branching. This ADR is that mechanism, with the floor at the arm.
- [ADR 0017](0017-phase-c2-kickoff-and-region-plan.md) D8 / C2.4 — scope-exit drops.
- [ADR 0065](0065-early-return.md) — `return` drains every live frame.

## Context

A handler arm is left by four paths (ADR 0074 D2 calls them exits). Three of them drain
the arm's scopes:

| exit | scope drops | emitted by |
|---|---|---|
| fall-through | yes | `lower_block`'s end-of-block `emit_scope_drops` |
| `return` | yes | `emit_return_drops` (floor 0) |
| `break` / `continue` | yes | `emit_loop_exit_drops(loop floor)` |
| **bubble** | **no** | — |

The bubble is the odd one out in a second way that D1 turns on: the other three leave the
arm for good, while the bubble branches back to the top of the `handle`'s dispatch loop,
which RE-ENTERS the arm. It is a loop body's `continue`, not a `break`.

The bubble is the path taken when `k(v)`'s resume returns a kont that is not
`PURE_RETURN` — the resumed computation performed again. All three back ends lower it the
same way: **store the new kont into the handle's dispatch slot and branch to the top of the
dispatch loop.** Nothing else. Two things follow from that single branch.

**(1) The arm's heap bindings are not freed on that path.** The branch leaves the arm's
block without reaching its end-of-block drops, and there is no floor drain on the way out,
so every bubble abandons whatever the arm's scopes hold.

**(2) The rest of the arm never runs.** Control does not come back to the `k(v)` site, so
the arm's remainder after `k(v)` is dropped, and the value of the *next* arm the loop
dispatches becomes the value of the whole `handle` rather than the value of `k(v)`. That is
ADR 0020 D3's deep re-wrap only when `k(v)` is what the arm evaluates to.

The two are separable, and they are not equally reachable. **(2) needs a `k(v)` that is not
in the arm's tail position; (1) does not** — it hits the tail idiom the entire corpus is
written in.

### Measured

Probes built with `snc build` (`--profile test` driver), run on Windows. `two()` is
`{ let a: i64 = perform Io.read(); let b: i64 = perform Io.read(); a + b }`, so the first
resume of every program below bubbles exactly once.

| arm over `two()` | answers | ADR 0020 D3 | |
|---|---|---|---|
| `k(21)` | 42 | 42 | tail |
| `if 1 > 0 { k(21) } else { 0 }` | 42 | 42 | tail, through an `if` |
| `match E::B(21) { E::A => 0, E::B(x) => k(x) }` | 42 | 42 | tail, through a `match` arm |
| `{ let v: [i64] = [1,2,3,4]; k(v[0] + 20) }` | 42 | 42 | tail — **and leaks `v`** |
| `{ let r: i64 = k(21); r }` | 42 | 42 | non-tail, remainder is the identity |
| `{ let mut s = 0; let mut i = 0; while i < 1 { s = s + k(21); i = i + 1 } s }` | 42 | 42 | non-tail, remainder is the identity |
| `return k(1) + 10` | 12 | 12 | non-tail; the re-entered arm's own `return` wins |
| `k(1) + 10` | **12** | **22** | non-tail |
| `{ let r: i64 = k(1); r * 3 }` | **6** | **18** | non-tail |
| `{ let v: [i64] = [1,2,3,4]; k(1) + v[0] }` | **3** | **4** | non-tail — **and leaks `v`** |

So a non-tail `k(v)` is not *always* a wrong answer: when the abandoned remainder happens to
be the identity on the resumed value, or when the re-entered activation exits the function
itself, the two agree. What is always true is that the remainder is abandoned.

The leak, kernel peak working set read after the process exits, with the `handle` in a
helper fn called from a loop:

| arm | 600,000 calls | 3,000,000 calls |
|---|---|---|
| `{ let v: i64 = 1; k(v + 20) }` (control — no heap) | 8.8 MB | 9.0 MB |
| `{ let v: [i64] = [1,2,3,4]; k(v[0] + 20) }` — **tail** | 36.5 MB | 147.2 MB |
| `{ let v: [i64] = [1,2,3,4]; k(1) + v[0] }` — non-tail | 36.5 MB | 147.2 MB |

About 48 bytes a call at both counts and in both shapes: one four-element array (32 bytes)
plus allocator overhead, once per call — two arm activations run per call and the first of
them bubbles. The tail shape and the non-tail shape leak **identically**, which is the
register entry's reach corrected: D87 illustrates the leak with a non-tail arm, and the
mechanism it names ("the same branch skips the arm's scope drops") is not confined to one.

The emitted IR says it directly. The tail probe's `main`, from the `snc llvm` oracle, has
the arm's array freed on the pure path and nothing on the bubble path:

```llvm
bb7:                                    ; the bubble
  store ptr %v27, ptr %v1               ; dispatch slot
  br label %bb0                         ; back to the loop — %v9 is gone
bb6:                                    ; the pure path
  %v30 = call i64 @sentinel_kont_consume_pure(ptr %v27)
  %v31 = load { i64, ptr }, ptr %v16
  %v32 = extractvalue { i64, ptr } %v31, 1
  call void @sentinel_free(ptr %v32)    ; the array
  ...
```

`sentinel-codegen`'s `lower_resume_kont`, the oracle's `lower_resume_kont` in
`llvm_dump.rs` and `scg`'s `cg_emit_resume_tail` in `selfhost/types/cg_effects.sentinel`
each emit that store-and-branch and nothing else.

### What the corpus is written in

Of the 466 `.sentinel` files tracked when this ADR was written, every handler arm resumes
in tail position — as the whole arm body, as the tail of a block, or as the tail of an `if`
or `match` arm — with one exception: `after` in
`crates/sentinel-driver/tests/fixtures/handler_arm_exits/`
`c74_arm_return_leaves_the_arm.sentinel`,

```sentinel
handle perform Io.read() with { Io.read(k) => { let a: i64 = k(i); return a + 1 } }
```

whose `handle` body is a bare `perform`. A kont straight from `sentinel_perform_op` carries
no frames, so its resume always drains pure and the bubble path is unreachable — that
program is correct today and would stay correct under (2) unfixed.

So the leak, (1), is live in the shape every program in the tree uses, and the wrong value,
(2), is not reachable from any of them. (Slice 1 adds a second non-tail resume, `looped` in
`tests/pass/c75_bubble_drains_the_arm.sentinel`, deliberately: it pins the drain of a
`while` body inside an arm. Its remainder is the identity on the resumed value, so its
answer does not move when slice 2 lands.)

## Decisions

### D1. A bubble leaves the arm's ENTRY, and drains its scopes on the way out.

Before storing the bubbled kont and branching to the dispatch loop, a `k(v)` drops every
live scope frame from the current top down to and including the **arm floor**, innermost
frame first, without popping them — mechanically ADR 0036 D9's `break` / `continue` drain
with the floor at the arm instead of the loop, and the same non-truncating shape
`emit_loop_exit_drops` / `cg_drop_range` already have.

**It is not the same KIND of exit, and that distinction is the whole soundness argument.**
A `break` branches out of its loop and never comes back, so its drain and the body's
end-of-block drops sit in mutually exclusive blocks. The bubble branches back to the top of
the `handle`'s own dispatch loop, which re-enters the arm — so the bubble block *does*
reach the pure path's drops, by a path through the loop (in the fixture's `simple`:
`kv_bubble` → `handle_loop` → `handle_arm` → `kv_pure`). What is mutually exclusive is not
the blocks but the paths through ONE ENTRY of the arm: an entry either bubbles, and drains
here, or completes, and drops at its block's end. Never both. So the rule is

> **an arm's entry frees exactly what that entry allocated, before it leaves.**

which holds only while nothing an arm binding owns outlives the entry that bound it — the
arm is a loop body, and this is a loop body's per-iteration drop. **D3 is what makes that
true.** Without it an arm binding could take over an allocation made outside the arm, and
every entry would release memory that entry did not acquire. An earlier draft of this ADR
argued instead from "mutually exclusive blocks" and from "the re-entered arm binds fresh
slots of its own": both are false — the slot is a single `entry:` alloca that every entry
reuses, and the value in it is not re-created when it was moved in from an outer scope.

Within an entry, nothing else escapes: the remainder that could have read those bindings is
what the branch abandons, and an op's resume argument is an `i64` (C3.5(a)), so no heap
value leaves through `k(v)`. A binding moved into the `k(v)` argument is skipped, as on
every other exit, by the drop plan's moved-source set.

### D2. The floor is the arm body's frame index, recorded when the arm is entered.

Each back end records `scope_stack.len()` (inkwell), the handle-stack frame's fourth
element (the oracle) and a `cg_h_floor` beside `cg_h_cks` / `cg_h_loop` (`scg`) at the
instant before the arm's body is lowered, so the frame the arm's block pushes is the floor
itself. Draining to that floor covers every frame between the `k(v)` and the arm body —
nested blocks, and the body of a `while` written *inside* the arm, whose per-iteration
drops the branch out of the loop would otherwise skip as well.

Frames *below* the floor belong to the function around the `handle` and are not drained:
the bubble does not leave them.

### D3. A handler arm is a loop body, so ADR 0036 D8's move rule applies to it.

A `while` body is walked once by the borrow checker and run many times, so ADR 0036 D8
(`MovedInLoopBody`) rejects a move out of a binding declared OUTSIDE the loop: it would be
a use-after-move on the next iteration. **A handler arm has exactly that shape and had no
such rule.** The dispatch loop re-enters the arm for every operation the handled
computation performs, and a bubbling `k(v)` branches straight back into it, but the checker
walks the arm body once.

So the same rule now applies to an arm body, as `MovedInHandlerArm`
(`sentinel::borrow::moved_in_handler_arm`): a Move-classified binding declared outside the
arm that the arm moves is rejected. It is flagged by the ROOT of the moved place, so ADR
0046's partial moves (`bag.items`) are covered as well as whole bindings — the root is what
the outer scope still owns. A binding declared INSIDE the arm is fresh on each entry and
may be moved freely.

This is conservative in exactly the way the loop rule is: it does not ask whether the arm is
in fact entered twice, because that is a property of the handled computation at run time. An
arm over a `handle` body that performs exactly once is rejected too, as a move in a loop
that runs once is. The remedy is the same — bind inside the arm, or borrow. No tracked
program is affected: no arm in the tree moves anything at all.

D1 depends on this. The drain is per-entry, so it is sound only while an arm binding's
allocation belongs to the entry that bound it, and that is what this rule establishes.

### D4. What the bubble still does not release.

Its own continuation: `k(v)` cleared the slot before resuming and `sentinel_kont_resume`
freed the kont (ADR 0074 D1). Nor any enclosing arm's: a bubble re-enters the innermost
handle's dispatch loop and leaves no outer arm. ADR 0074 D2's bubble bullet stands as
written for the kont, and gains the scope drains above.

### D5. `abi-v1` is untouched.

No new symbol, signature or layout. The drain is `emit_frame_drops` / `cg_drop_frame` at a
new call site, so what it can emit is whatever a scope exit emits for the types the arm
binds: `sentinel_free` for an array, string, `Vec` or a struct's heap fields;
`sentinel_shared_release` for a `Shared<T>`, `sentinel_mutex_release` for a `Mutex<T>` and
`sentinel_mutex_unlock` for a `Guard<T>` (ADR 0071); and, in inkwell only, `sentinel_arena_exit`
for a scope that lazily created a broker arena (ADR 0028 — the text back ends have no arena
path at all). ADR 0046's partially-moved fields are elided, as everywhere else. Every one is
already emitted on the other exits of the same arms, so no program gains a symbol it did not
already reference; what changes is the set of programs that reach them, which now includes
one whose arm holds a container across a bubbling `k(v)`.

### D6. A `k(v)` whose remainder is observable reifies that remainder onto the bubbled kont.

The route is **reify the remainder as a frame**, of the three surveyed below. The resume
site keeps the store-and-branch; what it adds, when there is something after the `k(v)`
that D3 says must still run, is a `sentinel_kont_push` of a resumer for that remainder onto
the bubbled kont *before* the store. The runtime already does the rest:
`sentinel_kont_push` appends at the chain's tail, and `sentinel_kont_resume` splices the
frames that survive a bubble onto the next one's tail, so the remainder replays after the
computation under it drains — which is exactly ADR 0020 D3's re-wrap.

Worked over `handle two() with { Io.read(k) => k(1) + 10 }`:

1. the body performs; `K1` carries `two`'s own resumer frame `Ra`;
2. the arm runs with `k = K1`; `k(1)` resumes, `Ra` sets `a = 1` and performs again,
   yielding `K2` with `two`'s second frame `Rb`;
3. **the bubble pushes `Rrem: 	. t + 10` onto `K2`**, so `K2` carries `[Rb, Rrem]`, and
   stores and branches as today;
4. the loop dispatches `K2`; that arm's `k(1)` resumes it — `Rb` gives `b = 1`, `a + b = 2`
   drains pure, and `Rrem` then runs on 2, giving 12;
5. so the inner `k(1)` is 12, that arm's value is `12 + 10 = 22`, and the `handle`'s is 22.

Which is D3's answer. No runtime change, no new resumer *mechanism* — the capture struct,
the `ptr fn(i64, ptr)` resumer signature and the frame chain are C3.5(c)/(d)/(e)'s, already
emitted by all three back ends — and the arms stay inline in the enclosing function, so
ADR 0065 D6's `return` and ADR 0074 D2's `break` / `continue` out of an arm keep working
wherever they run *before* the resume.

The other two routes were rejected on the same wall. **Outlining the dispatcher** — compile
the arms into a function taking the kont and a static link to the enclosing frame's slots,
and make the bubble a recursive call — gives per-activation storage for free, but an arm's
`return`, `break` and `continue` reach out of the `handle` into the enclosing function,
which a call frame cannot do without an escape protocol, and it needs an environment of
pointers that reification does not. **Trampolining in place** — spill the state live across
the `k(v)` to a heap stack with a site id and switch back when an arm produces a value —
preserves every exit, but what is live across the branch includes the SSA operands of the
expression the `k(v)` sits in (the left operand of `lhs + k(v)`), which no back end reifies;
reaching it means A-normalising every arm body, in all three.

#### The classification, as an explicit list

A `k(v)` in an arm is exactly one of these, and the list is closed by construction — a
shape not matched is (R), and a (R) the gates refuse is (A):

- **(T) tail** — the arm's value *is* `k(v)`'s value, reached from the arm body through
  only: the body itself, a block's tail, an `if` branch's tail, a `match` arm's tail.
  Lowered as today: store and branch, plus D1's drain. The remainder is the identity, so
  there is nothing to reify. Every tracked arm but one is here; the exception is (X).
- **(X) diverging remainder** — everything after the `k(v)` in the arm always `return`s
  (`expr_diverges` / `block_diverges` of ADR 0065, which is `Return` plus a `Block` / `If` /
  `Match` all of whose paths do). Lowered as today. Sound because the re-entered activation
  runs the same arm and takes the same `return` first, so under D3 the outer remainder is
  unreachable as well: `return k(21)` answers 42 and `return k(1) + 10` answers 12, both
  D3's answers, measured. `c74_arm_return_leaves_the_arm`'s `after` is here.
- **(R) observable remainder** — anything else. The bubble pushes a resumer for the
  remainder, which is the arm body with the `k(v)` node replaced by a fresh `i64` binding,
  and whose captures are the free variables it reads from the enclosing frame.
- **(A) refused** — an (R) whose captures or remainder the gates below reject. The bubble
  path aborts with a clean runtime diagnostic instead of branching.

#### The gates on (R)

Two, both fail-closed, and both already load-bearing elsewhere:

- **The captures cross the `i64[N]` seam**, so ADR 0072 D3/D4's `FITS` allow-list governs
  them unchanged: `i64` and `secret i64` only. A remainder that reads a `[i64]`, a `bool`,
  a `?i64` or a struct across the `k(v)` is (A), not a widened seam. This is also what
  keeps D1 consistent: the arm's heap bindings are dropped at the bubble and can never be
  captured, so nothing is both dropped there and read later.
- **The remainder must not leave the arm** — no `return`, `break` or `continue` in it, and
  no second `k(...)`: it runs inside a resumer, which cannot unwind the enclosing function,
  branch to its loops, or reach the arm's continuation slot. An unconditional `return` is
  (X) and never gets here; a conditional one, `{ let a: i64 = k(1); if a > 100 { return 7 }
  else { a + 10 } }`, is (A).

#### The (A) diagnostic

A new runtime symbol, aborting the way `sentinel_kont_panic_resumed` does, called on the
bubble path with `unreachable` after it. It is an `abi-v1` **addition** — no existing
symbol, signature or layout moves — and it is reached only where the program's answer is
wrong today, so nothing that works now regresses: `after`'s `handle` body is a bare
`perform`, whose kont carries no frames, so its bubble path is emitted and never taken.
Reusing an existing abort was rejected: both `sentinel_kont_panic_resumed` and
`sentinel_panic_oob` would print a diagnostic naming the wrong fault.

## Slices

D1–D5 and D6 are independent changes to the same site and land separately. D1 is first: it
is the memory leak, it reaches the idiom every tracked program uses, and D6 depends on it
(the heap bindings D6's captures may not carry are the ones D1 drops).

- **Slice 1 — D1–D5.** The bubble drains the arm's scopes in all three back ends (D1, D2),
  under the borrow rule that makes the drain sound (D3). Not `abi`-moving. Status below.
- **Slice 2 — D6.** The remainder reification, its two gates and the (A) abort, in all three
  back ends, plus the runtime symbol.

## Consequences

### Positive

- The bubble drains the arm's scopes, as the other three exits of an arm already did, in
  all three back ends (slice 1). It was the only one that did not.
- What it does NOT close is a property every drain site shares: the skip list is the drop
  plan's PER-FUNCTION moved-source set, so a binding whose only move lies AFTER the drain
  is skipped there too and is freed only on the path that reaches the move. Measured
  identical before and after this change, and identical at the `break` drain this one is
  modelled on (about 36.5 MB at 600,000 calls either way, against a control at 8.8) — so
  it is inherited, not introduced. Registered, not fixed here.
- `k(v)` means what ADR 0020 D3 says it means wherever it is written, not only in tail
  position (slice 2), and where the seam cannot carry it the program says so instead of
  answering wrongly.
- Slice 1 is the `break` / `continue` drain with a different floor, and slice 2 is
  C3.5(c)/(d)/(e)'s resumer machinery at a new site: both inherit mechanisms already pinned
  in all three back ends rather than adding one.

### Negative

- A `k(v)` in an arm that holds heap bindings emits their drops twice in the IR — once on
  the bubble path, once on the pure path — as `break` and `continue` already do.
- A `k(v)` in class (R) costs a resumer function, a capture struct and a
  `sentinel_kont_push` per bubble. No tracked program is in (R), so this is new cost on new
  code only.
- Class (A) is a runtime abort for a shape the type checker accepts. It is reached only on
  executions that are wrong today, but it is reached late.
- `abi-v1` gains one symbol (slice 2).

### Neutral

- Programs whose arms bind nothing heap-backed are unchanged by slice 1 — which is every
  tracked program, so the drain emits nothing, their IR is byte-identical and the bootstrap
  fixed points do not move. Under slice 2 the same holds for every arm in class (T) or (X),
  which is every tracked arm.

## Verification

**Slice 1 — done.**

- **Memory**, matched pre/post binaries, each smoke-tested on a shape whose answer differs
  before anything was measured with it (`snc_pre` 147.2 MB, `snc_post` 8.8 MB on the same
  program). Every leaking row of the table above is flat afterwards, level with the `i64`
  control measured in the same batch: the tail shape 36.5 → 8.8 MB at 600,000 calls and
  147.2 → 9.0 at 3,000,000, the non-tail one 147.2 → 8.9 at 3,000,000, and the control
  unmoved at 9.0.
- **Values**: every row of the Measured table answers exactly what it answered before.
  D6 is a later slice, so the three that disagree with ADR 0020 D3 still do.
- **The gate (D3)**: `tests/ui/c75_move_into_handler_arm.sentinel` is rejected with
  `sentinel::borrow::moved_in_handler_arm`, snapshotted whole. Five spellings of the move
  were built and all five are refused — a whole binding from a fn-local `let`, a by-value
  fn PARAM, a move through a CALL, a struct FIELD path (ADR 0046's partial move, flagged by
  the root), and any of them inside a nested block of the arm. Over the whole suite the gate
  rejects nothing else: 2,005 passed with exactly the 18 known Windows failures, and the
  corpus sweep below accepts and refuses identically on every program it builds.
- **inkwell**: `d87_a_bubbling_resume_drains_the_arms_scopes` in `sentinel-codegen` reads
  the IR the shipping back end verified. In `drains` the reachable `kv_bubble` block must
  free the arm's two frames, innermost first, traced from each `@sentinel_free` back
  through the `extractvalue` / `load` to the alloca it came out of; it must NOT contain a
  `sentinel_arena_exit`, which is how a floor set any lower reaches `outer`, the array
  belonging to the function around the `handle`; and the blocks that are NOT the bubble
  must still free the same two. In `looped`, the same for the `while` body's binding. Each
  assertion is guarded by a non-vacuity check — that the probe still builds the slot it
  names, and that `drains` really does route `outer` through an arena, without which the
  arena assertion could not see a floor set too low.
- **Text oracle**: `llvm_a_bubbling_resume_drains_the_arms_scopes` in `tests/llvm.rs`
  finds each fn's dispatch slot, then its one reachable bubble block — the block that ends
  by storing a kont into that slot and branching back — and counts the frees inside it and
  outside it: (1,1), (2,2), (1,1) and (1,2) over `simple`, `nested`, `looped` and
  `enclosing`. `enclosing` is the floor's other end: it holds an array below the arm floor,
  so a floor at the function makes its first number 2.
  ⚠ **The second number is not decoration.** A first draft checked only the bubble block,
  and an implementation that MOVES the drops onto the bubble rather than ADDING them passes
  that, the fixture, the corpus differential and both bootstrap fixed points — while leaking
  on the pure path at exactly the unfixed rate. A first draft also asserted that the
  oracle's bubble contains no `sentinel_arena_exit`; the text back ends have no arena path
  at all, so that assertion was vacuous on every input and is gone.
- **`scg`**: `tests/pass/c75_bubble_drains_the_arm.sentinel` is swept by the codegen
  differential's corpus, which compares `snc llvm` and `scg` byte for byte — so the
  mirror is held to the oracle on exactly these blocks. (The `handler_arm_exits/`
  directory was not needed: the fixture has no `if` in an arm, so the MIR differential
  takes it and `tests/pass` is the simpler home.)
- **End to end**: `pass_c75_bubble_drains_the_arm` in `tests/pass.rs` builds and runs it
  for exit 42. Its exit code is the same before and after — the drops are memory, not a
  value — which is why the pins above, not the fixture, are what catch a regression.
  (`tests/pass.rs` is a list of hand-written tests — 205 after this change — and there are
  more `.sentinel` files under `tests/pass/` than tests naming them, with no directory
  sweep to notice a missing one: the fixture sat unregistered until the four-check's total
  came back +2 rather than +3.)
- **Mutation**, each caught: the drain removed in inkwell, in the oracle and in `scg`; the
  floor moved to the function in all three; the floor moved one frame too high in inkwell;
  the drops moved onto the bubble rather than added, in inkwell and the oracle; and D3's
  gate removed, which fails the `tests/ui` snapshot. The inkwell mutations fail the
  `sentinel-codegen` pin, the oracle's fail `tests/llvm.rs` **and** the corpus differential,
  and `scg`'s fail the corpus differential — which is what shows the new fixture is
  load-bearing there rather than decorative.
- **Corpus**, matched pre/post binaries, each smoke-tested on a shape whose answer differs
  before anything was measured with it: every tracked `.sentinel` file (466) through `snc
  llvm` with both, and every one with a `main` (393) through `snc build`. **Byte-identical**:
  the same exit code, stdout and stderr for all 466, and the same acceptance for all 393 —
  the `build` half matters on its own, because `snc llvm` does not run the borrow checker, so
  it is the only half that sees D3. Which is what the arm survey predicts: no tracked arm
  binds anything heap-backed, so the drain emits nothing, and none moves anything, so the
  gate refuses nothing. The two new fixtures are untracked at the time of the sweep and so
  are not among the 466.
  ⚠ **The first run of this sweep reported 202 IR differences, and every one was the
  tool.** It compared `& $exe … 2>&1 | Out-String`, and Windows PowerShell renders a
  stderr record prefixed with the command name — so `snc_pre.exe : snc: module … not
  found` differed from `snc_post.exe : …` for every file that writes anything to stderr.
  The same binary twice compared unequal as well, because the rendering also carries the
  call site. The sweep now redirects both streams to files and compares their hashes, and
  it is smoke-tested in BOTH directions before the result is believed: it must report a
  difference on the new fixture (7 `sentinel_free` lines before, 12 after) and none for
  one binary run twice.

**Slice 2.**

- **Value**: every row of the Measured table, re-run; the three that disagree with D3 must
  answer 22, 18 and 4, and the seven that agree must not move.
- **Classification**: a fixture per class — (T), (X) in both its `return k(v)` and
  `return k(v) + n` forms, (R) with a capture and without, (A) for a refused capture and
  for a conditional `return` in the remainder — each mutated into the neighbouring class and
  caught.
- **`abi-v1`**: the new symbol added to `docs/abi-v1.md` §5, declared by all three back
  ends and threaded through `RuntimeSyms::merge` — a symbol called but not declared is
  invalid IR the oracle emits silently (the M1.4c-1 finding), so the pin is a build of a
  program that reaches it, not a reading.
- **Corpus**: as slice 1. Every tracked arm is (T) but `c74_arm_return_leaves_the_arm`'s
  `after`, which is (X), and `c75_bubble_drains_the_arm`'s `looped`, which is (R) — so
  `looped` is the one file whose IR is expected to move, and every other is expected
  byte-identical.
