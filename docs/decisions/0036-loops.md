# ADR 0036: Phase D.5 — loops (`while`)

Status: ACCEPTED-WITH-AMENDMENTS (D.5 **(1/N)** the `while` loop + **(2/N)**
`break` / `continue` both landed end to end; the 3 OPEN DESIGN POINTS were
resolved at the proposed defaults; see Amendments). The fifth Phase D sub-phase
ADR under ADR 0031 (Phase D kickoff) D4 item 6. After sum types (D.1), strings + a
byte type (D.2), growable collections (D.3), and file I/O (D.4), **loops** are
next: the surface has been **recursion-only by design** since 1.0, but a
compiler's iteration-heavy passes (scanning a byte buffer, walking a token `Vec`,
draining a work-list) are awkward + stack-bounded as recursion (deep recursion,
no early break — now fixed by `break`).

Date: 2026-05-31
Related:
  - **0031** (Phase D kickoff): D4 item 6 names "loops — `while`/`for` (the
    surface has been recursion-only by design); likely wanted for the compiler's
    iteration-heavy passes." Chosen as D.5 ahead of #5 modules (the smaller,
    higher-leverage of the two remaining prerequisites).
  - **C1.3 `if`** (the control-flow precedent): `if` is an *expression*
    (`ExprKind::If`) lowered to basic blocks (`lower_if`) — a cond block + then /
    else blocks reconciling **forward** at a merge block. A `while` reuses the
    block machinery but introduces the **first backward branch** (a CFG
    back-edge), so it is a **statement**, not an expression (D3).
  - **0017** (refs / mutability / borrow check / RAII drop): the loop body is a
    scope; its bindings drop **per iteration** (D5), and the borrow checker's
    move tracking must account for the back-edge — a binding moved in the body is
    moved on the *next* iteration (D8, the key risk).
  - **0028** (broker scope arenas): a loop body that allocates (e.g. `let w =
    read_word();` each pass) frees per iteration via the existing scope-exit
    drop; no arena change.

## Context

Of the remaining Phase D prerequisites (ADR 0031 D4), **loops** are the smaller,
higher-leverage one (vs. #5 modules). The 1.0 + D.1–D.4 language expresses
iteration only as **recursion** (verified at 1.0): a lexer scanning bytes, a
parser draining a token `Vec`, a symbol-table walk — all recurse. That is
stack-bounded (deep inputs overflow), has no early break, and is awkward for the
mutate-a-counter / drain-a-work-list patterns a compiler is built from.

This sub-phase adds a **`while` loop** — a condition-gated, statement-level loop
with a per-iteration body scope — and defers `for` (D8: `for` needs ranges or
iterators, and the `for` keyword is already taken by `impl Trait for Type`, so a
for-loop is contextual). `while` + a manual index (`let mut i = 0; while i <
len(v) { …; i = i + 1; }`) covers the compiler's iteration needs.

The unifying decision is **a loop is a statement with a backward branch** — it
reuses the `if` basic-block + the block-scope drop machinery, adding only the
back-edge (cond → body → cond) and the borrow-checker's loop-carried move rule.
The genuinely new pieces are the **back-edge codegen** (the first non-forward
CFG) and the **loop-carried move check** (D8).

## Decision

### D1. Goal.

Add a `while` loop, end to end (lexer → parser → AST → resolve → types →
borrow-check → codegen), executing a body block repeatedly while a `bool`
condition holds, with **per-iteration drop** of the body's bindings. Enough to
write the iteration-heavy passes a self-hosted lexer/parser need.

### D2. Surface syntax.

```sentinel
fn sum_to(n: i64) -> i64 {
    let mut total: i64 = 0;
    let mut i: i64 = 1;
    while i <= n {            // condition is any bool expression
        total = total + i;   // mutate loop-carried state (Assign, C2.2)
        i = i + 1;
    }
    total                     // 1 + 2 + ... + n
}

fn count_bytes(src: [u8]) -> i64 {
    let mut i: i64 = 0;
    while i < len(src) {
        // a per-iteration binding: dropped each pass (D5)
        let b: u8 = src[i];
        i = i + 1;
    }
    i
}
```

  - **`while <cond> { <body> }`** — a **statement** (not an expression; D3).
    `cond` is any `bool`-typed expression (the C1.3 bool machinery); `body` is a
    block, run repeatedly while `cond` is true, its tail value **discarded** each
    iteration (like a `StmtKind::Expr`).
  - **Loop-carried state** is a `let mut` declared **outside** the loop, mutated
    by `Assign` (`i = i + 1`) — the existing C2.2 assignment machinery drives
    termination. A `let` **inside** the body is per-iteration (D5).
  - **`for` is deferred** (D8): it needs ranges/iterators, and the `for` token is
    already `impl … for …`.

### D3. A loop is a statement, not an expression.

`if` is an *expression* (it yields the chosen branch's value, reconciled at a
forward merge). A `while` loop has **no meaningful value** — it runs zero-or-more
times for effect. Sentinel has no unit type, so making `while` an expression
would force a synthetic value (e.g. `i64` 0), awkward as a block tail. Instead,
`while` is a new **`StmtKind::While { cond, body }`** alongside `Let` / `Assign`
/ `Expr` (statements already exist). The body block's tail value is discarded
each iteration (exactly as `StmtKind::Expr` discards its expression's value).
**Loop-as-expression / `break`-with-value is deferred** (D8).

### D4. Codegen — the first backward branch.

`while` lowers to three basic blocks (reusing the `if`/`lower_block` machinery,
adding the back-edge):

  - **`loop_cond`** — evaluate `cond` (an `i1`); conditional-branch to `loop_body`
    (true) or `loop_after` (false). The entry block unconditionally branches here.
  - **`loop_body`** — lower the body as a **scoped block** (`lower_block`: push
    scope, lower stmts + tail, **emit scope drops**, pop), then an **unconditional
    branch back to `loop_cond`** — the **back-edge**. This is the first
    non-forward branch in codegen (all prior control flow — `if` / `match` —
    merges forward into a tree-shaped CFG).
  - **`loop_after`** — control continues after the loop.

Per-binding allocas are created once (in the fn entry, the existing convention),
so a loop-body `let`'s slot is reused each iteration; the *value* is recreated +
dropped each pass.

### D5. Per-iteration drop (load-bearing).

The body is lowered via `lower_block`, which pushes a scope and calls
`emit_scope_drops` at the body's end. Those drop calls are emitted **once** in
`loop_body`'s IR but **execute every iteration** (control re-enters `loop_body`
via the back-edge). So a body that allocates — `let w: [u8] = read_file(p);`, a
`let v: Vec<i64> = …; push(…)`, a string literal — **frees its heap each
iteration**, with no accumulation: leak-free under `leaks --atExit`. This is the
load-bearing correctness property and the primary thing the phase-go verifies (a
loop body that allocates N times leaks nothing). Loop-carried bindings (declared
*outside* the loop) are **not** dropped per iteration — they drop at their own
(outer) scope exit, as today.

### D6. Termination via mutation.

A `while` reuses C2.2 `Assign`: the loop var is a `let mut` outside the loop, and
the body mutates it (`i = i + 1`) so `cond` eventually goes false. No new
mutation surface. An infinite loop (`while true { }`) is well-formed (it just
never terminates) — the type checker does not prove termination.

### D7. Types.

`cond` must be `bool` (else a `Mismatch` / a focused `WhileCondNotBool`,
mirroring `if`'s condition rule). The body block type-checks normally; its value
type is **discarded** (no constraint — unlike `if`, whose arms must reconcile).
The `while` statement itself has no type (it is a `StmtKind`, not an expr). Adds
no new `Type` variant and no cascade.

### D8. Borrow check — the loop-carried move rule (the key risk).

A loop body is a scope, so borrows taken inside it are transient (cleared at
statement boundaries, C2.2) — no change there. The **subtle** interaction is
**moves across the back-edge**: the borrow checker walks the body **once**
(linearly), but the body runs **repeatedly**. A body that **moves** a binding
declared *outside* the loop is sound on iteration 1 but a **use-after-move** on
iteration 2 (the binding was consumed last pass). A single linear walk catches a
move-then-use *within* one body pass, but **not** a move on a late line +
re-entry. So the borrow checker must treat **a move of an outer binding inside a
loop body as a use-after-move** — conservatively, **reject moving an
outer-scope (Move-typed) binding inside a `while` body** (it would be consumed
on re-entry), surfacing `UseAfterMove` / a focused `MoveInLoopBody`. Bindings
declared *inside* the body are fine (fresh each iteration). This rule is the
genuinely new borrow-check surface and the main correctness risk — the phase-go
+ UI fixtures must cover both the rejected (move-outer-in-loop) and accepted
(copy reads, inner-binding moves, loop-carried `Assign`) cases.

### D9. Pipeline / sub-phase split.

| Sub        | Title                                                          | Risk   |
|------------|----------------------------------------------------------------|--------|
| D.5 (1/N)  | `while` — `while` lexer token + parser (statement position) +  | medium |
|            | `StmtKind::While` (AST + resolved + typed) + the `bool`-cond   |        |
|            | type rule + the borrow-check loop-carried-move rule (D8) +     |        |
|            | the back-edge codegen + per-iteration drop. End to end.        |        |
| D.5 (2/N)  | `break` / `continue` — branch to the current loop's            | medium |
|            | `loop_after` / `loop_cond`; a loop-target stack for nesting.   |        |

### D10. Phase-go + fixture.

`tests/pass/c5d5_loops.sentinel`: a `while` loop computing a result via a mutated
counter (e.g. sum 1..=N, or `push` into a `Vec` in a loop then `len`/index it),
returning a computed exit code; **plus a body that allocates each iteration**
(e.g. builds a `[u8]`/`Vec` per pass) **verified leak-free via `leaks --atExit`**
(D5 — the heap is freed each iteration, not accumulated). Plus a type-layer unit
corpus (`while` with a non-`bool` cond → reject; a loop-carried `Assign`
type-checks) and a borrow UI fixture (moving an outer binding inside a `while`
body → the D8 rejection).

## Reasoning

**Why `while`-as-a-statement, not an expression.** `if` yields a value because
both arms produce one and they reconcile; a loop runs 0..N times and has no
natural value (no unit type to give it). A statement avoids inventing one and
matches the existing `Let`/`Assign`/`Expr` statement forms. Loop-as-expression /
`break`-with-value is a later refinement gated on need.

**Why `while` only (defer `for`).** `for` needs ranges (no range type) or
iterators (deferred, ADR 0034 D8), and the `for` keyword already means `impl
Trait for Type`, so a for-loop is contextual to parse. `while` + a manual index
expresses every iteration the compiler needs; `for` is sugar on top, added once
ranges/iterators exist.

**Why the loop-carried move rule matters.** It is the one place the
single-pass borrow checker's assumptions break under a back-edge: a value moved
"after" its use textually is moved "before" it on the next iteration. Rejecting
moves of outer bindings inside a loop body is the conservative, sound, minimal
rule (Rust's borrow checker reaches the same conclusion via a more precise
dataflow; the conservative rule is the right MVP).

## Consequences

### Positive
- Bounded, early-exitable (with (2/N) `break`) iteration lands — the lexer's
  byte scan + the parser's token drain become natural + stack-safe, with maximal
  reuse of the `if` basic-block + block-scope-drop machinery.
- The first backward CFG branch, cleanly scoped to one construct.

### Negative
- The first non-tree CFG (a back-edge) + the loop-carried move rule (a real,
  if conservative, new borrow-check surface) — the medium risk.
- `for` / loop-as-expression / labeled break deferred; `while true {}` is a
  non-terminating-but-well-formed program (no termination check).

### Neutral
- No new `Type` variant, no `Type`-match cascade (a `StmtKind`, a bool cond, a
  discarded body value). No `FnId`-shift (no new builtin).

## Amendments

D.5 landed in two sub-phases: **(1/N)** the `while` loop, **(2/N)** `break` /
`continue`. With (2/N) closed the ADR is **ACCEPTED-WITH-AMENDMENTS**.

### D.5 (1/N) — `while` (landed)

End to end through the whole pipeline; phase-go `tests/pass/c5d5_loops.sentinel`
(a counter loop, a `Vec` built by `push`ing in a loop, and a body allocating each
iteration) runs at exit 67, leak-free. The 3 OPEN DESIGN POINTS were **resolved at
the proposed defaults**: the loop-carried move rule is the conservative reject
(`MovedInLoopBody`); `break`/`continue` are (2/N); a loop is a `StmtKind::While`.
Deviations / details discovered during implementation:

- **A1 — a statement-only body synthesises a discarded unit tail.** Sentinel
  blocks require a trailing expression (ADR 0010 D6), but a loop body runs for
  effect and usually has none (`while c { i = i + 1; }`). The `while`-body parser
  (`parse_loop_body`, a `parse_block_inner(allow_stmt_only=true)`) accepts a body
  with no trailing expression and synthesises a unit tail (`0`, discarded each
  iteration), so the body stays a regular `Block` reusing `check_block` /
  `lower_block`. A body that *does* end with an expression keeps it (still
  discarded).
- **A2 — stack-safety via entry-block alloca hoisting (the load-bearing codegen
  fix).** D4's "the body alloca is reused each iteration" only holds if the
  `alloca` is in the function **entry block**. A body `let`'s `alloca` emitted
  *inline* in `loop_body` executes every iteration, allocating a fresh stack slot
  each pass (LLVM `alloca` is not freed until function return) — at large N the
  stack overflows (empirically: a 2,000,000-iteration body-`let` loop SIGSEGVs).
  Fix: a `loop_depth` counter (bumped around `lower_block(while-body)`); when
  `> 0`, per-binding allocas (`let`, `if`-result, `match`-result) are placed at
  the top of the entry block (executed once, the slot reused) via a
  `binding_alloca` helper. Non-loop codegen (`loop_depth == 0`) keeps the inline
  `alloca`, so it is **byte-identical** to pre-D.5 (the c51 repro bar holds).
  Loop-carried-state loops (no body `let`) never had the issue; with the hoist,
  body-allocating loops are stack-safe too.
- **B (the D8 rule) — `MovedInLoopBody`.** A new `BorrowError`: moving an outer
  Move-typed binding inside a `while` cond/body is rejected (the conservative
  loop-carried move rule). Implemented by snapshotting the in-scope bindings +
  the moved set before the loop and flagging any outer binding newly moved in the
  cond/body. Verified: rejects `consume(p)` for an outer `p`, accepts an
  inner-binding move + loop-carried `Assign` / `push(&mut v)`.

### D.5 (2/N) — `break` / `continue` (landed)

End to end through the whole pipeline; phase-go `tests/pass/c5d5_break_continue.sentinel`
(a `break`-terminated sum, a `continue`-filtered sum, and two loops that allocate
a `[u8]` each iteration and break / continue with it live) runs at exit 115,
leak-free under `leaks --atExit`. `break` / `continue` are payload-free
**statements** (`StmtKind::Break` / `Continue`, new `break` / `continue` lexer
keywords) branching to the innermost enclosing loop's `loop_after` / `loop_cond`.
No new `Type`, no `FnId`-shift. Deviations / details discovered during
implementation:

- **C1 — drains-before-branch (the load-bearing (2/N) property).** A `break` /
  `continue` branches *out of the middle* of the body block, skipping
  `lower_block`'s end-of-body `emit_scope_drops`. So before the branch, codegen
  drops **every scope frame from the current top down to the loop body** (the body
  scope plus any nested `if` / block scopes between it and the branch), innermost
  first — exactly the body-end drop, emitted early (the fn-return early-exit
  shape, ADR 0017). Without it a body-scope heap binding live at the `break` leaks.
  Implemented by splitting `emit_scope_drops` into a per-frame `emit_frame_drops`
  and adding `emit_loop_exit_drops(scope_floor)`; each runtime path frees a given
  binding exactly once (the early-exit drop, or the body-end drop on fall-through —
  mutually exclusive blocks). Verified leak-free with a heap binding live across
  both a `break` and a `continue`, including in a nested inner loop.
- **C2 — the loop-target stack + `scope_floor`.** A `LoopTarget { cond_bb,
  after_bb, scope_floor }` is pushed onto `CodegenCtx::loop_targets` entering a
  `while` body and popped on exit; `break` / `continue` read the top (the innermost
  loop — no labels at D.5). `scope_floor` is `scope_stack.len()` captured the
  instant before the body scope is pushed, so the drain bounds `[scope_floor ..]`
  cover exactly this loop's body + nested scopes and never the enclosing loop's
  (verified: an inner `break` in a nested loop drops only the inner scope).
- **C3 — the first mid-block divergence needs a dead block.** Sentinel has no early
  `return`, so `break` / `continue` is the first construct that terminates a block
  mid-stream. After the branch, codegen parks the builder on a fresh
  `after_loopctl` block (no live predecessors; LLVM discards it) so the
  statically-lowered, now-unreachable remainder of the block — a statement-only
  body's synthesised unit tail, a dead trailing statement, `lower_if`'s
  store-to-result + merge branch — never appends to a terminated block.
- **C4 — out-of-loop rejection via the type env's loop depth.** `break` /
  `continue` outside any loop is a `TypeError::LoopControlOutsideLoop` (naming the
  keyword). Tracked by a `loop_depth: u32` on `VarTypeEnv` (already threaded
  through every checker, incl. nested `if` / `match` blocks), bumped around a
  `while` body; legal iff `> 0`. A fresh env per fn resets it at every fn boundary
  (no `break` across a fn). The borrow checker is unaffected (loop control moves
  nothing); the mir / effect-check / resolve cascades are no-ops.
- **C5 — usage ergonomics (a noted limitation, not a (2/N) blocker).** Because
  `if` is an expression that requires an `else` and a tail (ADR 0010 D6 / 0013
  D3a), a *conditional* `break` / `continue` uses the tail idiom `if c { break; 0 }
  else { 0 };` — the `0`s are discarded. This is a pre-existing property of
  Sentinel's statement-position `if` (any conditional side-effect needs it), not
  something `break` introduces. A cleaner ergonomics — a statement-level `if`
  without `else`, or `break` / `continue` as a tail (diverging) expression — is a
  Revisit, gated on a self-host pass finding the idiom too noisy.

Still deferred (D8): `for` / ranges / iterators, labeled break, `break`-with-value
/ loop-as-expression, a termination check.

### A3 (2026-09-20) — the hoist reaches every slot a loop body can (registers D62, D77, D88)

A2 hoisted the slots it named — a body `let`, an `if`-result, a `match`-result — and
left **fifteen** others on `builder.build_alloca` at the insertion point: the handle
lowering's five (`current_kont_slot`, `kont_var`, `arm_param` and both return-arm value
slots), `classinit`, the `match` payload binding, and the out-slots of `recv`, `lock`,
`read_file`, TCP read, process read, process recv, stdin recv and `arg`. Inside a loop
body each of those is a dynamic alloca, so D4's "the slot is reused each iteration" did
not hold for them: the stack grew by the slot's size every iteration and the loop
exhausted the 16 MB main-thread stack. Thirteen are scalar or pointer slots and cost 16
bytes (8, rounded up to x86-64's stack alignment); the other two are the size of the value
they hold — `classinit` is the class struct and the `match` payload binding is the
binding's type — so a wider value overflows proportionally sooner. Measured: a performing
`handle` between 520,000 and 530,000 iterations, and a `recv`, `lock`, one-binding `match`
or one-field class loop (register D62) between 1,040,000 and 1,060,000, against 250,000 to
270,000 for an eight-`i64` class. All fifteen now go through `binding_alloca`.

Reusing one slot per site is sound because no slot's ADDRESS outlives the iteration that
took it: the out-slots are C-ABI out-params the runtime writes through and does not
retain, `classinit`'s buffer is loaded into a value at the call, and a reference to a
binding is second class (ADR 0017 D7). Twelve of the fifteen also store before they load.
The three that do not are `recv`, `process_recv` and `stdin_recv`: their runtime writes
`*out` only on the success status, so on a failure the site loads a word this iteration
never wrote — the previous iteration's value now, fresh stack garbage before. It is
unreachable either way, because that word becomes the payload of a `?T` whose `valid` bit
is the status, and every consumer reads the payload only under that bit.

What is left on `builder.build_alloca` is prologue-only: a function's or a resumer's
parameters, a resumer's `resumed_value` and captured-state slots, and the `slice` in the
`export "C"` wrapper — each built into the entry block it belongs to and executed once per
call, never per iteration.

The trade-off is A2's: a function containing a loop now reserves those slots in its entry
block even on a call where the loop body never runs, so every frame of such a function can
be larger, which deep recursion pays for. The constant is program-dependent and scales with
the SIZE of the hoisted slots rather than their number. Measured at `-O0` with a zero-trip
loop inside a recursive fn: a handle-only body, a handle + `classinit` + `let` body, and a
body with eight handle sites each cost ZERO extra frame bytes — the existing frame absorbs
the slots as the dynamic-alloca bookkeeping goes away — while four constructions of a
16-field class, whose `classinit` buffer is 128 bytes apiece, cost about 464 bytes a frame
and some 40% of the recursion headroom (a zero-trip loop at depth 20,000 completes before
the hoist and overflows after it). From one iteration on, the hoist is the cheaper side, and
the deepest user of this compiler — `scg` compiling the whole merged compiler, in both
bootstrap fixed points — stays green.

**One position was still outside the hoist**, and this amendment did not change it: a
`while` CONDITION was lowered before `loop_depth` was bumped (the increment bracketed
`lower_block(body)` only), so for an OUTERMOST loop every slot the condition allocated —
A3's fifteen and A2's three alike — was still built inline in `loop_cond`, which the
back-edge re-enters. Measured on a matched pre/post pair: `while i < (match E::B(n) { … })`
completes at 500,000 iterations and overflows at 560,000, identically before and after this
amendment, while the same loop nested inside another loop (so the condition is lowered at
depth ≥ 1) completes. That was register D90, **closed by A4 below**. `lower_handle`'s own
dispatch loop is invisible to `loop_depth` in the same way and is not; see A4.

The fix is inkwell-only and **not oracle-moving**: the `snc llvm` oracle and `scg` already
hoist every alloca to `entry:`, so no stage dump, differential or `selfhost/` file moves.
Pinned by `d88_every_loop_body_slot_is_allocated_in_the_entry_block`, which reads the IR
inkwell verified for a program whose loops reach all fifteen and asserts that no `alloca`
sits outside the entry block; every site was mutated back to prove the pin sees it. A slot
added later belongs on `binding_alloca` too, and in that probe: the pin sees only the sites
the probe's loops reach, so a sixteenth slot left inline is invisible until it is added.

### A4 (2026-09-21) — the CONDITION is part of the loop (register D90)

A2 and A3 raised `loop_depth` around `lower_block(body)` alone, so the slots a `while`
CONDITION allocates were hoisted only when the loop was nested inside another one. For an
OUTERMOST loop they were built inline in `loop_cond` — the block the back-edge re-enters —
and grew the stack every iteration exactly as a body slot did: `while i < (match E::B(n) {
… })` (a `match`-result and a payload binding, 32 bytes an iteration) completes at 500,000
iterations and overflows the 16 MB stack at 560,000, while `while i < (if n > 0 { n } else
{ 0 })` (one 16-byte slot) completes at 1,040,000 and overflows at 1,060,000. `loop_depth` is now raised around the
condition too, and the decrement is unconditional: the result is captured and `?`-ed after
it, as the body's already was.

Soundness is A2's argument unchanged — the condition runs once per iteration and its slots
are consumed inside that evaluation, so one slot per site is enough. The hoist stays
confined to loops: a binding written after a loop is still built where it stands, which
keeps A2's "outside any loop, byte-identical to pre-D.5 codegen" true. Both halves are
pinned, in `sentinel-codegen` by `d90_a_loop_condition_allocates_in_the_entry_block_too`
and `d90_a_binding_after_a_loop_is_not_hoisted`, and end to end by
`tests/pass/c5d5_loop_cond_slot_reuse.sentinel`; reverting the bump fails the first, and
dropping the decrement — which would silently hoist every later binding in the module —
fails the second.

The trade-off is narrower than A3's but real. A condition is evaluated at least once on
every call that REACHES its loop, so for a slot on the condition's unconditional path the
hoist is free or better — measured, an always-evaluated allocating condition gives MORE
recursion headroom after it (about 12,900 frames against 11,600), because the
dynamic-alloca bookkeeping goes away. The exception is any slot the condition allocates on
a path a given call does not execute, and there are TWO shapes of that: (a) a branch of the
condition that is not taken — the RHS of a short-circuited `&&` or `||`, an untaken `if`
arm, an unreached `match` arm; and (b) a loop that is not reached at all, whose condition is
unconditional but whose enclosing arm is not taken — the ordinary `if c { while … { … } }`.
Before, those allocas never executed; now their slots are reserved in the entry block of
every call. Both shapes measure the same: four 16-field class constructions in a
short-circuited RHS take a recursive frame from 376 to 1,240 bytes and the recursion depth
from about 38,700 to about 12,900, and four 32-field ones give 888 → 2,776 bytes and
15,000–18,000 → 3,000–6,000 frames for the short-circuited RHS and the unreached loop
alike — a legal program that completed before can overflow. Neither shape is new in kind:
A3 already reserves an untaken loop BODY's slots the same way, and that half measures
identically before and after this amendment; what A4 adds is the condition's slots. The
reach is small: no corpus program changes — the only `while` condition in the repo whose
sub-expression allocates is this amendment's own fixture — and every self-host top-level
module object (`lexer`, `parser`, `codegen`, `borrow`, `mir`, `effects`, `ctverify`,
`types`, `resolve`, `merge`) is byte-identical before and after, so the bootstrap is
untouched. A per-branch hoist would avoid shape (a) and is not worth the machinery today;
shape (b) would want the loop's preheader instead, which is not a drop-in — an inner loop's
preheader is re-executed by the outer loop. If a real program meets either, those are the
fixes to reach for.

inkwell-only again, and the oracle's IR does not move: over the 465 tracked `.sentinel`
files, `snc llvm` output is byte-identical and all 286 programs that build run the same.
What A3 left open and A4 does not close is the other invisible loop: `lower_handle` emits
its own dispatch loop, which `loop_depth` — a count of SOURCE `while` nesting — cannot see,
so a `handle` with no enclosing `while` still builds its arm slots inside that loop. It is
bounded today only because ADR 0072 refuses a `perform` outside tail position, so the
number of bubbles is the number of syntactic `perform`s on the path; widening that gate
must come with hoisting those slots. Register D91 keeps that half.


### A5 (2026-09-24) — a FIELD moved out of an outer binding is carried too (register D110)

D8's rule snapshots the outer bindings and the already-moved set, and flags a binding the body
newly moves. A move of one FIELD of an outer binding — ADR 0046's partial move, which that ADR's
D5 scopes to a single-level field of a directly-named binding — is recorded apart from whole
moves, in `moved_fields`, and the rule did not read that set: a body that moved `s.a` out of an
outer `s` was accepted, although the next iteration moves the same field again, which is the
use-after-move D8 exists to reject. The rule now reads both sets and flags a field move by its
ROOT, the binding the enclosing scope still owns. ADR 0075 D3's handler-arm rule already read
both; the two now share one helper, `newly_moved_outer`. A root the body or arm newly moved whole
is reported at that whole move's span. A root it newly moved only by fields is reported at the
lowest-offset span among those new field moves (a field already moved before the loop or arm is
skipped), so the diagnostic no longer depends on hash-map order, as the arm rule's did (the same
file answered three different spans across ten runs before A5, and one after). In a straight-line
body that is the first new field move in source order; the set keeps one span per field, so after
a branch that moved a field on more than one path it holds only one of those paths' spans.

The rule is as conservative for a field as D8 is for a whole binding. It refuses a `while` whose
condition or body moves a field of an outer binding even where the field is moved at most once at
run time — the move is followed by `break`, sits under a guard that fires once, or is in a
condition that is false on its first check — and where the field is assigned before or after the
move, since a reassignment does not un-move it. The whole-binding rule has always refused the same
shapes. Each such field loop passed the borrow checker before A5, so A5 is at least a minor
version (ADR 0076 D2). `docs/borrow-check-limitations.md` now lists the ones a flow-sensitive
checker would accept (the reassignments and the `break`); how often a guard fires, or whether a
condition holds at once, is a run-time property the rule does not ask.

Pinned by `tests/ui/c5d5_move_outer_field_in_loop.sentinel` and four unit tests in
`sentinel-borrow-check`: `while_loop_carried_field_move_rejected`,
`while_loop_carried_field_moves_report_the_first_in_source_order`,
`while_inner_binding_field_move_ok` and `handler_arm_field_moves_rejected_by_the_root`, the first
test of the arm rule's field path. `tests/ui/c5d5_move_outer_in_loop.sentinel` is the
whole-binding UI fixture D10's phase-go lists; only unit tests held D8 until now. Four mutations,
each caught: the loop rule reverted to whole moves fails the field fixture and the first two unit
tests; reporting the last field move instead of the first fails the source-order test and the arm
test; the arm rule reverted to whole moves fails the arm test; and flagging field moves of
bindings declared inside the body fails `while_inner_binding_field_move_ok`.

It is an `snc` rule, and the borrow differential skips a program the oracle rejects. A matched
in-place `snc borrow` sweep of all 475 `.sentinel` files changes one verdict, the new field
fixture's. That single-file command does not load 141 files outside `tests/ui` — 96 with `use`
imports, 24 with no `main`, and 21 parts of the self-hosted compiler's multi-file modules — so
those were checked by a matched `snc build`, which does load imports: of each program or
exporting library among them, of each self-hosted module root, and of a two-line program
importing each remaining library module (a build checks every function of a module it imports,
an unused private one included). No verdict changed, and every one of those builds reached the
borrow stage; the only failures, identical on both sides, were links of POSIX and socket symbols.

## Revisit

ACCEPTED-WITH-AMENDMENTS (D.5 closed). Triggers:
- **D8**: if the conservative loop-carried-move rule rejects a real self-host
  pattern, refine toward a dataflow (move-state fixpoint over the body) check.
- **(2/N) C5**: if the conditional-`break` tail idiom (`if c { break; 0 } else {
  0 };`) proves too noisy in a self-host pass, add a statement-level `if` (no
  `else`) or make `break` / `continue` a tail (diverging) expression.
- **labeled break / `break`-with-value**: when a self-host pass needs to exit an
  *outer* loop or carry a value out (loop-as-expression).
- **D2**: `for` (+ ranges / iterators) once the iterator protocol is designed.

## OPEN DESIGN POINTS — RESOLVED (at the proposed defaults)

1. **Loop-carried move rule (D8).** → **reject moving an outer-scope Move-typed
   binding inside a `while` body** (conservative, sound; `MovedInLoopBody`, D.5
   (1/N)). A precise dataflow check is a future refinement.
2. **`break` / `continue` scope (D9).** → **(2/N)** (landed): payload-free
   statements branching to the innermost loop's `loop_after` / `loop_cond` via a
   loop-target stack; rejected outside a loop; drops-before-branch keeps the body
   scope leak-free.
3. **`while`-as-statement (D3).** → a `StmtKind::While` (no loop value).
