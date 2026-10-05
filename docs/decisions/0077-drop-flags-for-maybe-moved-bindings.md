# ADR 0077: A moved binding's drop is decided per exit, by walk order and a run-time flag

Status: **ACCEPTED** (2026-10-01) with the proposed answers to Q1–Q5; Q6, the version, was
answered on 2026-10-05 (0.2.0, after the landing). Drafted 2026-09-25. Closes register items
**D93**, **D64** and **D22**; see *Implementation* for what landing it settled. Amends
[ADR 0017](0017-phase-c2-kickoff-and-region-plan.md) D8, [ADR 0046](0046-partial-move-field-soundness.md)
D3/D4, [ADR 0065](0065-early-return.md) D4/D7, [ADR 0036](0036-loops.md) C1 and
[ADR 0075](0075-a-bubbling-resume-leaves-its-arm.md) D1/D5 (the full list is under *Docs to
amend*).

## Related

- [ADR 0017](0017-phase-c2-kickoff-and-region-plan.md) D8 — drop at scope exit; a moved-from
  value is not dropped where it was bound.
- [ADR 0046](0046-partial-move-field-soundness.md) — partial moves of a single-level field of a
  directly-named binding, recorded apart from whole moves.
- [ADR 0065](0065-early-return.md) D4, [ADR 0036](0036-loops.md) C1/D9,
  [ADR 0075](0075-a-bubbling-resume-leaves-its-arm.md) D1 — the drains: `return`,
  `break` / `continue`, and a bubbling `k(v)` drop the frames they leave before branching.
- [ADR 0074](0074-handler-arm-owns-its-continuation.md) D1/D2/D4 — the precedent: a handler
  arm's continuation slot is a run-time ownership record, cleared at the resume and
  null-tested in the emitted code at every exit.
- [ADR 0036](0036-loops.md) D8/A5 and [ADR 0075](0075-a-bubbling-resume-leaves-its-arm.md)
  D3 — an outer binding may not be moved inside a `while` or a handler arm. D2 below rests on
  them.
- Register **D64** (the Rust back ends leak a binding on an early exit that comes before its
  move, and `scg` does not), **D93** (the same skip list at the drains), **D22** (a `?Guard`
  moved on one branch leaves the lock held on the other).

## Context

Every drop site in the oracle and in inkwell decides whether to drop a binding by asking one
question of the `DropPlan`: is this binding in the function's moved-source set? That set is
the union of every move anywhere in the body, computed once per function (the `DropPlan` doc
comment: "for C2.4 the per-fn set is sufficient because every let-binding lives in some block
and codegen emits drops at every block-exit"). A binding moved on any path is therefore never
dropped on any path. Two classes of program get the wrong answer.

**(i) An exit that comes before the move.** `return`, `break`, `continue` and a bubbling
`k(v)` drain the frames they leave. A binding whose only move lies after the exit is in the
set, so the exit skips it and it is never freed on that path:

```sentinel
fn once(n: i64) -> i64 {
    let v: [i64] = [1, 2, 3, 4];
    if n >= 0 { return 0 } else { 0 };
    consume(v)
}
```

This is register D64, and D93 is the same finding at the bubble drain. `scg` does not have it:
its drop sites consult the moves recorded so far in its single walk, so the early `return`
above frees `v` in `scg`'s IR and not in the oracle's (constructed for this ADR: one
`sentinel_free` in `scg`'s `@once`, none in the oracle's). The codegen differential is green
only because no corpus program has the shape. The program D93 measured has since become a
class (A) bubble under ADR 0075 D6 and aborts before any drain, but the class is still reachable
there: an arm `{ let s: [u8] = "ab"; let r: i64 = k(1); r + consume(s) }` reifies its remainder
(the resumer re-runs the literal and consumes its own `s`), and at the bubble `scg` frees the
arm's `s` while the oracle does not.

**(ii) A binding MAYBE moved where its scope ends.** Moved in one arm of an `if` or a
`match`, or in the right-hand side of `&&` / `||`, and then the scope ends:

```sentinel
fn once(n: i64) -> i64 {
    let v: [i64] = [1, 2, 3, 4];
    if n < 0 { consume(v) } else { 0 }
}
```

No static answer at the single exit is right on both paths: skip it and the non-moving path
never frees it; drop it and the moving path frees it a second time. All three back ends skip
it today, and `scg`'s IR for this shape is byte-identical to the oracle's.

**Measured for this ADR** (`snc build`, which is inkwell, with a `--profile test` `snc` built
from `5c46d4c`'s code; `once` called 600,000 and 1,200,000 times from `main`; kernel peak
working set read after exit, in MiB):

| Shape | 600k | 1.2M | Control (moving path, or never moved) |
|---|---|---|---|
| (ii) `if`, the non-moving path | 36.5 | 64.1 | 8.8 (moving path), 9.0 (never moved) |
| (ii) `match` arm | 36.5 | — | 8.6 |
| (ii) `&&` right-hand side not evaluated | 36.6 | — | 8.6 |
| (ii) ADR 0046 field, `if n < 0 { consume(s.a) } else { 0 }` | 36.5 | — | 8.8 (fields read, not moved) |
| (i) `return` before the move | 36.5 | 64.2 | 8.8 (the path that moves) |
| (i) a drain AFTER a join: `let r = if n < 0 { consume(v) } else { 0 }; if m >= 0 { return r } else { 0 }; r + 1`, run with `n >= 0`, `m >= 0` | 36.5 | 64.1 | 8.8 (`n < 0`, `m >= 0`: moved, through the same `return`) |

Re-measured warm in inkwell when this ADR landed (`2fdc3dd`, the second of two runs of each),
the same shapes read 36.0–36.1 at 600,000 calls, 63.7–63.8 at 1,200,000, and 8.3–8.5 for the
controls.
About 48 bytes a call (27.6 MiB over the second 600,000 calls), unbounded in the call count. The
last row is why a smarter STATIC per-exit set would not be enough: `n` and `m` are independent,
so the `return` is reached both with `v` moved and with it not. Dropping `v` there would drop it
a second time on the path the control column measures, and skipping it leaks on the other, so
that exit too needs an answer at run time. The 2026-09-23 investigation behind D93 also
constructed the same class for `continue`, the bubble drain, a by-value parameter, `Vec` and
enum bindings, structs holding a `Shared` or `Mutex` (their release skipped on the
non-moving path), and D22's `?Guard`; those rows are not re-measured here.

**Why the drop cannot move to the non-moving edge.** Dropping `v` at the end of the arm that
did not move it looks equivalent, since the checker rejects any use of `v` after the join.
It is not: a reference into `v` can leave that arm through the `if`'s value.

```sentinel
fn once(i: i64) -> i64 {
    let x: i64 = 5;
    let v: [i64] = [42, 2, 3, 4];
    let r: &i64 = if i < 0 { let z: i64 = consume(v); &x } else { &v[0] };
    *r
}
```

This borrow-checks and exits 42. In the oracle's IR the else arm stores a pointer into `v`'s
buffer and the join loads through it; an edge drop would run between the two. It is correct
today only because `v` is never dropped on that path. A drop kept at scope exit runs after
the tail has been read, so it needs no new borrow rule.

## Decisions

### D1. The invariant.

Each owned binding is dropped exactly once on every path from where it is bound to the end of
its scope on which it was not moved, and not at all on a path on which it was. This replaces
"skip every binding in the function's moved-source set" as the rule every drop site applies:
fall-through block ends, function epilogues, and the `return`, `break` / `continue` and bubble
drains.

### D2. "Not moved yet" is decided by walk order, statically.

At a drop site, a binding with no move earlier in the emitter's walk of its function is dropped
unconditionally. This is the rule `scg`'s `cg_is_moved` already applies, because `scg` records
moves in the same walk that emits code; the oracle and inkwell adopt it.

"Walk" means each back end's OWN emission order, which is the order its code runs in. It is not
source order everywhere: inkwell evaluates the value of `a[i] = e` before the index, and the
oracle and `scg` the index first. So a back end decides "earlier" by what it has itself already
emitted, never by comparing source spans.

It is sound because, within one entry into a binding's scope, the program runs its moves and
its drop sites in the order the back end emitted them. The only constructs that run code again
without leaving the scope are a `while` loop's condition and body and a handler arm, and none of
them may move a binding declared outside it (ADR 0036 D8 and A5, ADR 0075 D3). A `handle`'s
`return` arm runs out of source order, but it is emitted where it runs — at each resume's pure
path and in the dispatch — so each copy is walked where it runs. So a move that runs before a
drop site was emitted before it.

D2 alone closes class (i) where the exit precedes every move of the binding (D64's shape,
D93's shape), and it removes the oracle/`scg` divergence at those drains.

### D3. After a binding's first move, its drops are guarded by a run-time flag.

A binding that is moved somewhere gets one `i1` alloca, its MOVED flag:

- stored `false` in the function's entry block, with the allocas;
- stored `true` at each of the binding's move sites;
- at each drop site walked after the binding's first move: loaded, and the drop runs only on
  `false`; then `false` is stored back.

The store back is what re-arms the flag for the next entry into the scope: the next loop
iteration, or the next entry into a handler arm, rebinds the name with the flag already
`false`. The flag is not armed at the `let` because the `let` is walked before the move, so a
single-pass emitter does not yet know there that the binding will need one. The drop sites
after the move do. The flag is `false` whenever the binding is freshly bound: it starts `false`;
within one entry only a move of that entry sets it (D2's argument); and every exit of that
entry that runs after the move was walked after it (the same argument), so it resets it.

This is ADR 0074's pattern — a per-binding ownership record written by the path taken and
tested in the emitted code at every exit — for an ordinary binding. It closes class (ii) and
the drains of class (i) that follow a move on some other path, including the drain after a
join. The drop stays at scope exit, so the reference case above is unaffected.

An arm-remainder resumer (ADR 0075 D6) is a function of its own for D2 and D3: it allocates its
own flags and stores `false` to them in its own entry block, and its D2 prefix is its own walk.
The oracle builds that walk fresh, and so does `scg`, which re-parses the remainder for its
resumer and numbers its bindings afresh (see *Implementation*). The two agree because a
remainder has no exit but its end — a remainder that leaves the arm is class (A) — so no drop
site of a remainder binding precedes its move there.

### D4. The Rust back ends learn move sites from the borrow checker.

A `Var` read is a bare load in both Rust back ends; neither knows that a read is a move. The
borrow checker already decides it (`check_and_record_move`, `check_and_record_field_move`), so
the `DropPlan` gains the move sites: each consuming read of a Move-typed binding or of an ADR
0046 field, a read the checker refuses as a use after a move included, as the binding (and
field) and the read's span, per function and per method key. The plan stays `Hash + Eq`
(BTree containers). A back end sets the flag when it emits a read whose span is a move site —
a membership test; D2's "earlier" still comes from the back end's own emission order, never
from comparing the spans. `scg` sets it where `record_move` / `record_field_move` already run.

The flag is set at the read. If a later operand of the same call leaves the scope first
(`f(v, return 1)`), the value in flight is not dropped by the exit: it leaks, as it does today,
and is never dropped twice. ADR 0074 D1 instead evaluates the arguments before clearing its
slot; see Q2.

The whole-function moved-source sets stay as they are. The `snc borrow` dump reads them (so its
golden and seed do not change), and so do inkwell's arena routing, which keeps excluding every
binding moved anywhere, ADR 0071's clone decision (see *Implementation*), and each drop site, to
tell a binding or field the checker records as moved, whose drop D2 or D3 then decides, from
one it does not.

### D5. Fields.

An ADR 0046 field move gets a flag of its own, keyed by (binding, field). At a drop site the
binding's whole flag, if it has one, is tested first; if it is `false`, the binding is dropped
field by field and each moved field is elided by its own flag. The shape that moves the whole
binding on one arm and a field on the other is then exact on both paths; today it skips the
binding wholesale on both.

### D6. Assignment into a flagged binding.

`x = e` where `x` has a flag drops the old value if the flag is `false`, stores the new one and
stores `false`. Without this, the new value of a binding assigned after a maybe-move is never
dropped even on the path that moved the old one: `let r = if n < 0 { consume(v) } else { 0 };
v = [5, 6, 7, 8]; r`, run down the moving path, peaked at 36.1 MiB at 600,000 calls before this
ADR, against 8.4 for the same join with no assignment. The checker still rejects a later READ
of `v`, as today. An assignment into a binding with no flag (one walked before any move of
it, into a binding never moved, or into one whose only moved fields are fields its drop does
nothing with) keeps today's behaviour, which does not drop the old value either
(`v = [5, 6, 7, 8]` after `let mut v = [1, 2, 3, 4]` measures 36.1 against 8.4, and so does
the same assignment followed by `consume(v)`); so does an assignment into a field. That is a
separate leak, register D120 (Q3).

### D7. Every drop action is guarded, guards included.

The flag guards the binding's whole drop: `sentinel_free` of an array, `Vec`, `?Struct` or enum
payload, a struct's recursive field drop, `sentinel_shared_release`, `sentinel_mutex_release`,
and the unlock of a `Guard` or `?Guard`. So D22 closes: a `?Guard` moved on one branch is
unlocked at scope exit on the other, at the point it is unlocked today when no move exists.
`tests/pass/c16_mono_key_handle_tags.sentinel` moves a guard on one branch; its IR changes and
its warning comment is rewritten. `Shared` and `Mutex` handles themselves are Copy and are never
moved, so they get no flag — except in a generic body instantiated at one, where the checker
treats the type parameter as Move: there the oracle and inkwell flag the handle binding, and
its release, skipped on every path before, runs on the path that does not move it (`fn g<T>(x:
T, c: bool)` moving `x` only when `c`, called with a `Shared<i64>` and `false` 2,000,000 times,
peaked at 69.9 MB in inkwell and the oracle and now at 8.4). `scg` lowers that body
differently: it clones the read that moves `x` and releases `x` on every path, so it did not
leak before (8.3), and its IR differs from the oracle's (register D36). A struct that holds a
handle is flagged, and so, since ADR 0071 A4, is a class that holds one.

### D8. Constant time: no new sink.

A flag is written by a store on the path taken and read by a branch at a drop site. Every
branch that chooses a path is already public — `secret_leak` rejects a secret-dependent branch
— so the flag's value is public and the branch on it is a branch on public data. Flags exist
only in codegen, never in MIR, so this argument is carried here, as ADR 0065 D5 carried its own
for `return`. No secret value is stored into a flag.

### D9. `abi-v1`, back ends, bootstrap, version.

- No symbol, signature or layout change: a flag is a local alloca, like ADR 0074's slot.
- All three back ends. The emitted IR moves wherever a binding whose drop does something is
  moved (see *Implementation*), so this is oracle-moving: `scg` mirrors it, both bootstrap fixed points must stay green, and it is at
  least a MINOR version under ADR 0076 D2. The version never enters the IR (ADR 0076 D6).
- `scg` needs the least change of the three: it already computes D2's prefix (`mvf` / `mvv`
  are filled during the walk), and its allocas are emitted at the function's teardown, where
  the flag allocas and their entry-block stores can go.

### D10. What this does not cover.

- The handles a method's or an init's parameter frame holds in inkwell. Since ADR 0071 A2
  (register D153) inkwell drains those frames on every exit, an init's statement locals
  included, and a binding moved on one path there is flagged like any other, but the frame
  releases no handle (register D154).
- The effecting-fn shape frames, which no back end drains (register D139).
- `match` payload bindings, which are never framed; ADR 0032 defers the payload model.
- Drops that do not depend on a move at all, such as a class's heap fields (register D137) or
  an enum's heap payloads (D138). Since ADR 0071 A4 a class's drop releases the handles its
  fields hold, and a moved field is guarded by its flag there as in a struct's drop.
- An assignment into a binding with no flag (walked before any move of it, into a binding
  never moved, or into one whose only moved fields are fields its drop does nothing with), or
  into a field (D6; register D120). So an array moved out of a class holding a `Shared`, then
  `c = C::init(..)`, keeps the old instance's handle: 54.6 MB over 600,000 calls in all three
  back ends, against 36.1 for the class's array leak alone (D137). D6 does not count a field
  moved without a flag because the back ends' records of field moves differ there — at
  register D117's positions (a field compared with `null` or discarded) and at a type
  parameter's field at a Copy instance, the oracle and inkwell record a move that `scg` does
  not — and a D6 keyed on those records made the oracle's and `scg`'s IR differ.
- ADR 0046 D5's deferred projections.

### D11. Later, optional: skip the test where a binding is moved on every path.

After a binding's first move, D3 tests the flag at every drop site, including the ones every
path reaches with the binding moved, where the test always skips. A path-sensitive must-moved
analysis (divergence-aware: a diverging arm's moves do not reach the join; the right-hand side
of `&&` / `||` is a branch) could drop those tests. It changes no behaviour, only the IR, so it
is an optimization with its own oracle-moving landing, not part of this ADR's first slice.

## Implementation (2026-10-05)

Landed with the proposed answers to Q1–Q5 (D2 + D3; the flag set at the read; D6 for a flagged
binding only, the general assignment leak staying register D120; D22 closed by the flag; move
sites exported in the `DropPlan`). Q6: the version went to 0.2.0 in its own commit right after
this landed. What the implementation settled that the decisions above left open:

- **A move site is keyed by its binding as well as its span** — `(VarId, start, end)`, and
  `(VarId, field, start, end)` for an ADR 0046 field — so two reads that happen to share a span
  cannot set each other's flag. A consuming read of a Move-typed binding is recorded as a site
  even where the checker reports it as a use after a move: the program is refused either way,
  and `scg`, which records every such read, sets the flag there too, so the two emit the same
  bytes.
- **A flag is numbered, not named.** `%mf<n>` is the `n`-th flag the walk of that `define`
  needed, and the flags' `alloca`s and `false` stores follow the hoisted allocas in that order.
  A flag named for its binding's VarId diverged: `scg` re-parses a replayed arm remainder and
  numbers its bindings afresh, so the oracle's `%mv9` was `scg`'s `%mv12`.
- **In the text back ends, only a binding a drop site can drop gets a flag**: one in an open
  scope frame whose type needs a drop. A move already walked of any other binding (a
  unit-only enum, a struct of scalars) is skipped at its drop sites, as every moved binding was
  before, so the emitted IR of those drops does not move. inkwell flags every moved binding in
  an open frame, so some of its flags guard a drop that does nothing.
- **D6 counts a field's flag as a flag.** An assignment into a binding with a whole or a field
  flag drops the old value as a drop site does, which stores every one of its flags back
  `false`.
- **Fail closed.** D2's unconditional drop of a binding the checker records as moved is sound
  only if every move of it in the code the walk emits was walked. The oracle and inkwell check
  that at the end of each body and refuse the program ("a move of binding #n … was not
  lowered", in inkwell `MoveNotLowered`) rather than risk a double free. No corpus program
  trips it.
- **ADR 0071's clone decision keeps the whole-function set.** `moved_out` (a `Shared` / `Mutex`
  read that moves its place is not cloned) still asks whether the binding is moved anywhere. It
  agrees with the per-site answer on every accepted program: a read in an owning context of a
  binding the checker treats as Move is a consuming read, so it is itself a move site.
- inkwell keeps its `tail_returned` skip beside the flag, and its arena routing, which excludes
  every binding moved anywhere, is unchanged.
- **It landed after ADR 0071 A2 and A4.** inkwell's method and init parameter frames, which A2
  drains but lets release no handle, pass that on to every drop in them: a drop site's, and
  D6's drop of an overwritten binding, which takes it from the frame that declares the binding.
  A class's drop, which A4 makes release the handles its fields hold, guards a moved field by
  its flag as a struct's drop does, in all three back ends.
- **A `return` arm's copies get flags of their own.** The oracle emits a `handle`'s `return` arm
  once per pure-drain site from one typed tree, and `scg` re-parses it at each, so the bindings
  it declares are new there each time. Once a copy is emitted the oracle forgets its flags for
  the bindings that copy declared, so the next copy makes its own, as `scg`'s do.
- **A moved field gets a flag only if its binding's drop does something with it.** A struct's
  drop drops each field whose type needs a drop; a class's releases the handles its fields hold
  and nothing else (ADR 0071 A4). So a field that is a struct of scalars, a unit-only enum or
  (in the text back ends) a compared `?Node` gets no flag when it is moved, nor does an array
  moved out of a class, while a `secret`-qualified struct holding a `Shared` moved out of a
  class does. Each back end applies the rule to its own drops: inkwell's struct drop also frees
  a `?Struct`'s box and drops a `secret`-qualified field, so it flags those.
- **A field's base is a binding read directly**, as the oracle's test for a `Var` target says.
  `scg` had taken the binding the last `Var` in a compound target read (`s` in `{ s }.a`; `t`
  in `(if c { s } else { t }).a`, whichever arm ran) and recorded a field move of it the checker
  does not; with drops decided by flags that elided a drop the oracle emits. It now takes only a
  target that is a single node, and its borrow dump, which had listed that field move since the
  ADR 0046 mirror (no corpus program has the shape), now matches the oracle's.

Measured (kernel peak working set after exit, the second of two runs, `once` called 600,000
times; `snc build`, and the oracle's and `scg`'s IR, each linked against the same runtime):
every shape in the table above, and a loop body moving on some iterations, a `continue` before
the move, a whole-or-field move and an assignment after a maybe-move, fell from 22–64 MiB to
8.3–8.4 MiB in all three back ends, what a binding that is never moved measures (8.3–8.5).
`scg`'s `return` and `continue` shapes were at 8.3–8.4 before, since its drops already went by
its walk (D64). The moving-path controls stayed at 8.3–8.4, except the assignment's, whose
assigned value had leaked (36.1) and is now dropped.

The oracle's IR, before and after, over the 523 `.sentinel` files in the tree: 87 files' IR
moved, and every one of them gained flags; 267 are byte-identical, and 169 the oracle refuses
both times. `snc build` and a run of the same files and 47 library wrapper programs (570
entries) changed one result: `c77_guard_moved_on_one_branch`, which timed out before (the guard
moved on one branch stayed locked on the other) and exits 42 now.

## Consequences

### Positive

- D93, D64 and D22 close together, and the class (ii) leak, which was not in the register as
  filed, closes with them.
- The oracle's and `scg`'s IR agree at a drain that precedes a move, where they disagree today.
- Drops stay at scope exit, so no borrow rule changes and no program that compiles today stops
  compiling.

### Negative

- IR growth where bindings are moved. Measured with `snc borrow` when this ADR was drafted: in
  `tests/pass`, 50 of 496 functions moved a binding (66 whole-binding moved sources, 3 field
  ones, across 35 of 215 files); in the merged self-hosted compiler (`snc merge
  selfhost/codegen.sentinel`), 229 of 642 functions did (932 whole-binding moved sources). The
  dump lists free functions only, so method, class-init and impl bodies are not in these counts.
  Each such binding whose type needs a drop (in inkwell, each such binding in an open frame)
  gets an alloca, an entry-block store, a store per move site, and a load, a branch and a store
  at each drop site after its first move. The self-compile time of `scg` under this change is
  not measured.
- The Rust back ends gain a walk-order "moved so far" set per function, which they do not
  keep today.

### Neutral

- The `snc borrow` dump, its golden `borrow_dump_conservative_if_branch_merge` and the
  `selfhost_borrow.rs` seed keep their text; the golden's comment ("the analysis is
  conservative … so both appear") and the seed list's ("the conservative if-branch union") are
  amended to say what the set is now used for.

## Verification plan (for the implementing slice)

- `tests/pass` fixtures, one per shape: `if`, `match` and `&&` joins; `return`, `break`,
  `continue` and a bubble before the move; the drain after a join; a field and the mixed
  whole/field shape; an assignment after a maybe-move; a `?Guard` moved on one branch, relocked
  on the other; the reification shape above (a literal bound before `k(1)` and consumed in the
  remainder) and a remainder that moves a binding on one path only; a `return` inside the index
  of `a[i] = consume(v)`, where D2's answer differs by back end (inkwell has already emitted the
  move there, the oracle and `scg` have not); and the reference case, which must still exit 42.
- A leak probe over the measured shapes: flat at 600k and 1.2M calls against the same-batch
  control (kernel peak after exit), and the moving paths unchanged.
- IR pins in the oracle (`tests/llvm.rs`) and inkwell (the `LAST_VERIFIED_IR` capture), in
  ADR 0074's style: each flagged drop is the `false` edge of a branch on its flag, and each drop
  site before a binding's first move drops it unconditionally.
- `scg`: new seeds in the codegen differential; both bootstrap fixed points.
- Mutations, each to be caught: omit the store back (a loop-body binding moved on one iteration
  must still be dropped on the next); set the flag at the drop site instead of the move; drop
  unconditionally after the first move (the moving path drops a second time); skip by the
  function-wide set at a drain before the move (D64 returns).
- A matched pre/post IR sweep of the corpus, to measure the churn D9 predicts, and the behaviour
  sweep.

## Docs to amend when this lands

The `DropPlan` and `FnCtx` doc comments in `sentinel-borrow-check`; ADR 0017 D8; ADR 0018 D2
("Codegen — entirely unaffected"); ADR 0043 D4 (the union needs no per-path state); ADR 0045's
"`cg_is_moved` ⟺ `moved_sources_for`"; ADR 0046 D3/D4; ADR 0065 D4 (each binding freed exactly
once) and D7 (which says the remainder after a `return` is treated as unreachable, where the
checker walks it); ADR 0036 C1 and D5; ADR 0075 D1, D5 and its Consequences; ADR 0028 D3; the
"exactly once" comments at the drains in inkwell and the oracle and the "Mirrors
`DropPlan::moved_sources_for`" comment in `scg`; the golden's and the seed's comments; the
"drop attached to the move SITE" wording in `STATE.md`, `docs/inbound-requests.md` and
`sentinel_library/std/security/aead.sentinel`; `docs/REVIEW_ACTION_PLAN.md`'s "static rewrite preferred over
dynamic drop flags" precondition, now met; the `c16_mono_key_handle_tags` comment; ADR 0071's A1 item on a binding moved on one path and A4's
list of owners never dropped; and the register: D93 cites D64 and D22 and is widened to class
(ii), and D122's sub-case of a field read on one path closes.

## Alternatives considered

- **A path-sensitive static set, with flags only where a binding is maybe moved at an exit.**
  The same run-time behaviour, with fewer flag tests. It needs a divergence-aware analysis in
  all three back ends (the checker's per-point move state today merges a returning arm's moves
  into the fall-through and records a move in the right-hand side of `&&` as definite), and in
  `scg` a pre-pass, because its single walk has not yet seen a later move when it reaches the
  `let`. D2 + D3 reach the same behaviour with the prefix `scg` already computes; the analysis
  remains available as D11's optimization.
- **Zero the moved-from slot and drop unconditionally.** No flag and no branch. It relies on
  every drop arm, at every level of a recursive drop, doing nothing for an all-zero value — an
  invariant across the runtime and all three back ends that nothing checks. The runtime's
  release functions are null-safe, but ADR 0074 D4 chose to put the test in the emitted code
  rather than leave it to the runtime, and this ADR follows it.
- **Drop on the non-moving edge.** Unsound without a new borrow rule (the reference case
  above), unlocks a guard earlier than today, and needs join pads in the two single-pass text
  emitters, which emit an `if`'s then-to-join branch before they walk its else.
- **Refuse a move on some paths only** (D22's pin, generalized). Restricts the language: it
  refuses `if n < 0 { consume(v) } else { 0 }`, and the by-value parameters consumed in one arm
  that `sentinel_library/std/security/aead.sentinel`'s open used before it switched to borrowing.
- **A flag for every binding that needs a drop.** No moved set needed at all, so the simplest
  mirror; every heap binding in every function grows a flag.

## Open questions for the maintainer

Q1–Q5 were answered as proposed (2026-09-26). Q6 was answered on 2026-10-05: the version went to
0.2.0 in its own commit right after the landing, covering every oracle-moving change since 0.1.0.

- **Q1.** Accept D2 + D3 (walk order, then a flag after the first move) in place of the order's
  framing, a path-sensitive static set with flags only where a binding is maybe moved?
- **Q2.** Set the flag at the read (D4; the `f(v, return 1)` family leaks, as today) or adopt
  ADR 0074 D1's rule (set it once the consuming operation's operands have all been evaluated)?
- **Q3.** D6 covers assignment into a flagged binding only. Fix assignment into any binding
  (the old value is never dropped) in the same slice, or separately?
- **Q4.** D22: close it with the flag (unlock at scope exit) or keep the pin D22 leaned toward?
- **Q5.** Move sites: exported in the `DropPlan` by span (D4), or re-derived in each Rust back
  end?
- **Q6.** The version: this lands as at least 0.2.0 (ADR 0076 D2). Bump before it, or with it?
