# ADR 0065: `return expr` — explicit early return (C-style)

Status: **PROPOSED → IMPLEMENTING.** The **entire effect-free path is implemented and
self-hosted** (Windows-verified, four-check green, both bootstrap fixed points hold): stages
**1–2** (front end + effect-free lowering, snc-side), the **stage-4 front end** (selfhost
parser + resolver mirror), and the **stage-4 codegen** (real `return` text-IR, byte-identical in
the `snc llvm` oracle + the self-hosted `scg`) — `return` is in the typing/mir/borrow/effects/codegen
differential corpora via `tests/pass/c65_return*.sentinel`. The two deferred v1 typing limitations are
**CLOSED**: the join-site **divergence acceptance** (a divergent `return` is no longer coerced to the
expected type — `coerce_to_expected` skips it) and the **match-arm divergence** (a returning arm no
longer over-rejects the arm-type join). Stage **3 (the cross-handler unwind, D6)**: the functional behaviour already worked, and the
remaining **kont LEAK is now FIXED** in the runtime (`sentinel_kont_free`) + the inkwell back end
(the `Return` arm frees an abandoned handle kont). What remains is snc-only faithfulness (the
text-IR + selfhost-MIR mirror) and an orthogonal, separately-tracked gap (a handle body whose
control flow reaches a `perform` silently miscompiles) — see Phasing stage 3. Add an explicit
`return expr` so a function can return early from any point, instead of only by its tail
expression. Owner-directed (2026-06-28): "the function return in Sentinel is not clear; we should
have a `return x;` C-style." Scope confirmed: **full early return** (return from anywhere),
coexisting with the existing tail-expression return.

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

1. ✅ **Rust `snc` front end** (DONE) — parser (`return <expr>`), `ExprKind::Return` threaded through
   resolve/types, divergent typing (D3), the `expr_diverges`/`block_diverges` join-site integration
   (if/body/method-body). `current_return_type` stashed on the per-fn `VarTypeEnv`.
2. ✅ **Rust `snc` lowering, effect-free path** (DONE) — borrow-check (D7, passthrough + the
   path-insensitive over-rejection note), MIR (Opaque-carry the inner — control flow is codegen's),
   codegen `emit_return_drops` (floor = fn) + `build_fn_return` (the main i64→i32 / effecting-kont
   ABI shared with the epilogue) + the dead-block parking (D4). Demonstrator
   `examples/lang/early_return.sentinel` (heap binding live across the return; both back ends) +
   `tests/ui/c65_return_type_mismatch`. Kept in `examples/` (snc-only, OUT of the scg differential)
   so both fixed points stay byte-identical without the stage-4 mirror — the u128/f64 pattern.
   **(Both former v1 limitations — the `match`-arm divergence and the `snc llvm` text-IR stub — are
   now CLOSED; see stage 4.)**
3. **`return` crossing a `handle`** (D6) — **REMAINS; assessed 2026-06-29.** Intended work:
   `sentinel_kont_free` + handle-region tracking + the teardown-on-early-return. **Assessment +
   leak fix (2026-06-29):** the FUNCTIONAL behaviour already worked — a `return` from a handle BODY
   (pure rest) and a `return` from a handler ARM (instead of resuming `k`) both produce the CORRECT
   value today (verified: exit 42 on both paths). The only D6 gap was a **memory LEAK** — the arm/body
   `return` abandoned the in-flight kont without freeing it (the IR `ret`s without a free). **FIXED in
   the runtime + the inkwell back end:** a new `sentinel_kont_free(kont)` (frees the kont + walks
   `frames_head` freeing each captured block/frame, the inverse of `kont_push`; 2 runtime unit tests),
   and the inkwell `Return` arm frees each active handle region's in-flight kont (innermost first,
   `handle_stack`) before the `ret` — the one-free invariant (resume frees on the normal path, this on
   the early-return path, mutually exclusive). Demonstrator `examples/lang/early_return_handle.sentinel`
   (arm-return + body-return; exit 42, both paths leak-free, no double-free; the 23 effecting pass
   tests unaffected). **Deferred (snc-only, the demonstrator stays OUT of the differential — the
   early_return / u128 / f64 pattern):** the **text-IR mirror** of `kont_free` (the `snc llvm` oracle +
   selfhost `cg` mode still omit it — invisible to the exit code; the self-hosted `scg` compiles
   handle-free sources so its bootstrap is unaffected) and the **selfhost MIR collapse** (snc MIR
   collapses a `handle` to opaques without lowering the arm's `return`; the selfhost MIR lowers it, so
   they diverge — a separate selfhost-MIR faithfulness item). **The orthogonal gap is being LIFTED (stage 3a,
   2026-06-29):** a handle body that performs through control flow used to silently miscompile (the
   perform's `Kont*` stored into the `i64`-typed merge slot and `kont_pure`-wrapped). First it was
   made a clean rejection; now the **common case is SUPPORTED** in the inkwell back end — a `perform`
   in TAIL position of an `if`/`else` branch OR a `match` arm (incl. nested `if`s, and a pure sibling
   branch/arm) is NORMALIZED to a continuation: `lower_body_as_kont` / `lower_if_as_kont` /
   `lower_match_as_kont` / `lower_block_as_kont` make the result slot a `ptr` and each leaf a `Kont*`
   (a direct `perform` as-is, a pure value `kont_pure`-wrapped), so the handle dispatches the merged
   continuation. Demonstrator `examples/lang/handle_control_flow.sentinel` (snc-only). **Still
   rejected** (needs per-eval-site frame reification — `CodegenError::HandleBodyNotDirectPerform`,
   pinned by `tests/ui/c65_handle_perform_in_control_flow.sentinel`): a NON-tail perform
   (`perform Op() + 1`) and a `let`-bound perform inside the body. **Deferred (snc-only):** the
   `snc llvm`
   oracle + selfhost still reject ALL control-flow performs (not yet 3a-aware), so the demonstrator is
   out of the differential (the text-emitter mirror is the follow-up). No over-rejection (the 23
   effecting pass tests + c36b's literal nested `handle` still compile).
4. ✅ **Stage-4 codegen + self-host + typing acceptance** (DONE 2026-06-29). The **real `return`
   text-IR** lands byte-identical in both text emitters: the `snc llvm` oracle
   (`crates/sentinel-driver/src/llvm_dump.rs`) and the self-hosted `scg` (the `cg` mode of
   `dump_texpr` in `selfhost/types.sentinel`) — eval inner → drop every live binding to the fn floor
   (`emit_loop_exit_drops(0)` / `cg_drop_range(c, 0)`) → `ret` with the epilogue ABI (main i64→i32;
   effecting → `sentinel_kont_pure`; ordinary) → park a dead block. Selfhost MIR mirrors the Rust
   `Return(inner) => Opaque(vec![v])`. `tests/pass/c65_return.sentinel` (return in `if` + a heap
   binding live across the return + statement-position) and `tests/pass/c65_return_match.sentinel`
   (return in a `match` arm) bring `return` INTO every differential corpus; both fixed points hold.
   The two **typing limitations are CLOSED** (sentinel-types): the **coerce-skip** (a divergent node
   is not coerced to the expected type, so `return e` is valid in any context — `check_expr` returns
   early on `expr_diverges`) and the **match-arm divergence** (skip diverging arms in the result-type
   join). Demonstrators `examples/lang/early_return*.sentinel` (snc-only) cover the mismatched-divergent
   cases (out of the differential, the u128/f64 pattern). The **selfhost typer needs no `expr_diverges`
   mirror** — it is a pure dumper (never rejects), and the only observable difference is the node type
   for a *mismatched*-divergent join, which is snc-only.
5. **ADR ACCEPTED** — pending stage 3 + a cross-platform (macOS) confirmation; update STATE/HANDOVER;
   (optionally) re-spell example tails as explicit `return`s where it reads clearer.

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
