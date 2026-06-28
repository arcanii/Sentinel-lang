# ADR 0065: `return expr` — explicit early return (C-style)

Status: **PROPOSED (design).** Add an explicit `return expr` so a function can return early from
any point, instead of only by its tail expression. Owner-directed (2026-06-28): "the function
return in Sentinel is not clear; we should have a `return x;` C-style." Scope confirmed: **full
early return** (return from anywhere), coexisting with the existing tail-expression return.

This is **oracle-moving** — new surface syntax changes the parser/AST and thus `snc`'s
`ast`/`parse` dumps and emitted IR — so it follows the full rhythm: **ADR PROPOSED → Rust `snc`
+ fixtures → re-bless the per-stage differential → mirror into `selfhost/*.sentinel` → both
bootstrap fixed points green → ADR ACCEPTED.**

Date: 2026-06-28
Related: **0036** (loops — `break`/`continue`: the existing mid-block divergence + drop-to-a-floor
machinery this reuses; it already names "the fn-return early-exit drop"), **0012/0013** (surface
syntax — blocks are `stmt* tail_expr`), **0017** (refs / mutability / borrow-check / RAII drop —
the scope-drop machinery), **0008/0026** (the `secret` constant-time discipline + the
`sentinel::mir::secret_leak` pass — unaffected, see §CT), **0019/0020** (effects/handlers — the
v1 restriction in D6), **0038/0039/0044/0045** (the self-host lexer/parser/MIR/codegen this
mirrors into).

## Context

Today a function returns **only** by its body block's tail expression (Rust-style). The block
grammar is `{ stmt* tail_expr }` — every block carries a **mandatory** tail expression that is its
value (`sentinel-ast`: `Block { stmts, tail: Expr }`). So the verification examples end in a terse
`if a && b && c { 42 } else { 0 }`, and there is no way to write `if bad { return 0 } …` — the
early-exit a C/Go/Rust programmer reaches for. The owner finds the implicit tail return unclear and
wants the explicit form.

The keyword already exists: `return` is `TokenKind::Return`, reserved at C3 but used **only** for
the effect-handler `return v => body` arm — the lexer notes "no early-return statement at C3." So
no lexer change is needed; the work is parser-and-down.

The **precedent is `break`/`continue`** (ADR 0036). They are mid-block divergent control flow that
must (a) drop every live scope frame from the current point down to a *floor* before branching, and
(b) cope with the now-unreachable remainder of the statically-lowered block. ADR 0036 built exactly
this — `emit_frame_drops` per frame + `emit_loop_exit_drops(floor)` + the C3 dead-block handling —
and explicitly anticipated "the fn-return early-exit drop." `return` is the same machinery with the
floor set to the **function** instead of the loop body: *break all the way out of the function.*

## Decision (proposed)

### D1. `return expr` is a **divergent expression** (`ExprKind::Return`).

Rather than a statement (which would not fit the mandatory-tail block grammar — `if c { return 5 }
else { 10 }` needs the `then`-branch to have a tail), `return expr` is an **expression** of
*divergent* type, mirroring Rust's `return: !`. It is valid:

- as an **expression-statement**: `return x;` (the C-style spelling), and
- as a **tail**: `if bad { return 0 } else { keep_going() }` — the `then` tail is `return 0`.

`break`/`continue` stay payload-free `StmtKind`s (loop-only, tail discarded); `return` carries a
value and can be a tail, so it earns an `ExprKind`. No change to `Block`'s mandatory-tail shape.

### D2. Parsing.

`return` at expression position parses `return <expr>` (the inner expression at full precedence);
`return x;` is then an expression-statement. The inner expression is **required** in v1 (every
function has a value return type — there is no `void`/unit return surface; `return;` is a non-goal
until a unit return exists).

**Disambiguation from the handler `return v => body` arm:** the arm is parsed only inside `handle e
with { … }` (a distinct parser context that expects `=>`); the expression/statement form is parsed
in block/expression context and is `;`- or tail-terminated. The two never share a parse site.

### D3. Type-checking — divergent.

`return e` checks `e` against the **enclosing function's declared return type** (threaded as
context, the way the tail already is). The `return e` *expression itself* is **divergent**: it
unifies with whatever type its position expects (so `if c { return 0 } else { x: i64 }` types at
`i64`; `let y: T = return e;` is accepted and `y` is dead). This is a localized bottom — no full
`!`/never type is added (that would ripple through unification); `return`/`break`/`continue` are the
only divergent forms and are special-cased at their nodes.

### D4. Lowering & drops — reuse the ADR 0036 machinery, floor = the function.

`return e` lowers to: evaluate `e` → **drop every live scope frame from the current point down to
the function floor** (the ADR 0036 `emit_frame_drops` walk, floor = function instead of loop body)
→ branch to the function's single **exit block** (which stores the value and emits `ret`). The
statically-lowered remainder of the enclosing block after a `return` is **unreachable**; reuse the
ADR 0036 C3 dead-block handling (never append to a terminated block; the result-store/merge edges
are simply not taken). Each live binding is freed **exactly once** — on the early-return path *or*
the fall-through tail path, which are mutually exclusive (the same one-free invariant ADR 0036
verified leak-free under `leaks --atExit`). All three back ends (inkwell LLVM, Cranelift, self-host
`scg`) get the same shape.

### D5. Constant-time — no new sink, guarantee unchanged.

`return` is **unconditional** control flow: it does not branch on a value, index memory, or divide,
so it is **not** a new `sentinel::mir::secret_leak` sink. The dangerous shape `if secret { return a }
else { return b }` is already rejected — at the **`if`** (a secret branch condition), exactly as
today; the `return`s are irrelevant to that rejection. **Returning a `secret` value is fine** (a
function may have a `secret` return type; the secrecy is the caller's to hold) — `return secret_x`
adds no leak. The secret_leak pass is **untouched**; this is a control-flow-only feature. (A
conformance fixture pins `if secret { return … }` still rejected.)

### D6. `return` crossing a `handle` — unwind the handler frames (owner-directed).

`return` **may** exit a function from inside a handled computation (the body of a `handle e with
{ … }` or a `with`-arm). The non-local exit must tear down the heap-allocated effect-runtime state
for every `handle` region between the return point and the function floor, in addition to the
scope drops (D4). The effect runtime (ADR 0020) reifies a `perform` into a heap `SentinelKont` that
flows up to its matching `handle`, with captured eval-frames in a `SentinelFrame` linked list
(`frames_head`); the runtime already contemplates a kont "freed externally if the handler arm body
aborts without resuming." So:

- **Codegen tracks active `handle` regions** alongside scope frames. On an early `return` crossing N
  handle regions, it emits, innermost-first: that region's scope drops, then a teardown of its effect
  state.
- **A new runtime helper `sentinel_kont_free(kont)`** frees an abandoned kont + walks its
  `frames_head`, freeing each captured-state block and frame node (the inverse of `kont_push`), for
  any kont allocated-but-not-resumed on the return path. The common case — a `return` *before* any
  `perform` in the handle body — has no kont yet, so it is just scope drops + the handle-region
  bookkeeping pop.
- **The one-free invariant** (D4) extends to handler state: a kont is freed exactly once — by its
  `resume` on the normal path, or by `sentinel_kont_free` on the early-return path (mutually
  exclusive).

This is the most intricate part (the effect runtime is the youngest, most-staged subsystem — its
own header calls it "the minimum-viable runtime"), so it is implemented **after** the effect-free
path is green (Phasing stage 3), but it is **in scope for this ADR** — `return` is not restricted
from crossing handlers. **Risk note:** the handle/`perform` codegen is staged (frame reification at
arbitrary sites is itself incremental); stage 3 may surface gaps there that need closing first.

### D7. Borrow-check.

A `return` path consumes/moves its operands as any path does; the lexical checker treats the block
remainder after a `return` as unreachable (the D4 dead block). The 1.0 borrow checker is lexical and
over-rejects (`docs/borrow-check-limitations.md`); `return` does not relax it — a false rejection is
fixed by scoping, not by weakening the checker. v1 keeps it simple: a value moved on the
return-path-only is considered moved (conservative), matching `break`/`continue`.

## Self-host (oracle-moving)

**Yes, oracle-moving.** The parser/AST gain `ExprKind::Return`, so `snc`'s `parse`/`ast` dumps and
the lowered IR change for any module that uses it. Rhythm: implement in Rust `snc` + add `tests/pass`
(early return in `if`/`while`/`match`, return-before-tail, returning a value, leak-free with a heap
binding live across the return) + `tests/ui` (return type mismatch; `return` crossing a handler
rejected; `if secret { return }` still a `SecretBranch`); **re-bless** the per-stage differential;
then **mirror into `selfhost/{lexer,parser,ast,types,borrow,mir,codegen}.sentinel`** and prove `scg`
== `snc` byte-identical on every module; both bootstrap fixed points green; then ACCEPTED. The
selfhost compiler sources need not *use* `return` (they keep tail returns), but the selfhost
**parser/lowering must accept it** so the fixtures compile byte-identically under `scg`.

## Phasing

1. **Rust `snc` front end** — parser (`return <expr>`), `ExprKind::Return`, resolve, divergent typing
   (D3). `ast`/`parse` fixtures + re-bless.
2. **Rust `snc` lowering, effect-free path** — borrow-check (D7), MIR/codegen drop-to-function-floor +
   exit-block branch + dead-block (D4), both LLVM + Cranelift. `tests/pass` + `tests/ui` + the CT
   conformance fixture. End-to-end green for all non-effect code (the whole current corpus).
3. **Rust `snc` — `return` crossing a `handle`** (D6) — `sentinel_kont_free` + handle-region tracking
   + the teardown-on-early-return; `tests/pass` with effects (`return` before/after a `perform`
   inside a `handle`). Closes any handle-codegen gaps it surfaces (Risk note).
4. **Self-host mirror** — port stages 1–3 into `selfhost/*.sentinel`; `scg` == `snc` byte-identical;
   both fixed points green.
5. **ADR ACCEPTED**; update STATE/HANDOVER; (optionally) re-spell example tails as explicit
   `return`s where it reads clearer.

## Non-goals (v1)

- **`return;` (no value)** — there is no unit/void return surface; deferred with it.
- **A general `!`/never type** — D3's localized divergent typing suffices; a real bottom type is a
  separate, larger decision.
- **Warnings on dead code after `return`** — the remainder is lowered-then-unreachable (D4); a
  dead-code *lint* is a nice-to-have, not v1.
- **`return` as an arbitrary deep sub-expression** idiom (`f(return x)`) — it *parses* (D1) but is
  not idiomatic; no special support beyond the divergent typing.

## Alternatives considered

- **Tail-position `return` only** — a clearer spelling of today's tail with no control-flow change.
  Rejected: the owner asked for *full* early return; tail-only defeats the purpose.
- **`StmtKind::Return` + make `Block.tail` optional** — fits the "statement" intuition but ripples a
  `Expr → Option<Expr>` change through every block consumer (Display, types, all three codegens).
  Rejected: larger blast radius than a divergent `ExprKind` that leaves `Block` untouched.
- **A full `!`/never type** — principled but heavy (unification, subtyping of `!`). Rejected for v1
  in favor of node-local divergence (D3).

## Open questions

- **Diagnostic for D6** — name + wording for "`return` cannot cross a `handle` boundary (v1)"; and
  whether to detect it in resolve (lexical handler nesting) or effect-check.
- **Dead-code after `return`** — silently lower-and-skip (v1) vs a future lint.
- **Re-spelling examples** — once `return` lands, do we migrate example tails to `return` (and how
  much), or leave tails as-is and use `return` only for genuine early exits? (Cosmetic; owner call.)
