# STATE.md — Sentinel Implementation Status

This document tracks what is actually built. When it disagrees with
HANDOVER.md, STATE.md is the source of truth. New contributors (or
new chat sessions) should be able to read this file and understand
the current state of the workspace without re-reading every commit.

The workspace now has two non-stub crates with very different
purposes: a production-shape memory subsystem (broker) and a
research-grade interpreter (effects-proto). They are tracked in
separate sections below. The remaining workspace members listed in
HANDOVER §3.2 are scaffold-only.

Last updated: phase B2.3b2 landed (Perform fully wired through
effect environment; row generalization in Scheme; UnknownEffect
type error; placeholder EffectNotYetSupported retired from
TypeError). See ADRs 0003 (B1 retrospective), 0004 (row
representation), 0005 (effect-inference judgment; D9 closed), 0006
(default-close, amended; D4 row polymorphism now implemented).

---

## Section A — sentinel-broker

Production-shape crate. The broker provides generational arenas,
two allocation strategies, scoped budgets, secret memory, recording,
and diagnostics. Intended for adoption beyond the Sentinel compiler
itself per HANDOVER §4.

### A.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| A0    | Dev dependencies (thiserror, tracing, proptest)    | Done   | 9c7474d |
| A1    | Foundation types (ArenaId, Generation, ...)        | Done   | 9c7474d |
| A2    | Bump arena + generational handles + destroy_arena  | Done   | 9c7474d |
| A3    | Pluggable AllocStrategy (Bump + Slab), builder     | Done   | f606d19 |
| A3.5  | Per-slot generations + slab recycling              | Done   | 37ab02b |
| A4    | Scoped allocation budgets                          | Done   | 493ee7b |
| A5    | Stats, list_arenas, where_is                       | Done   | 15d751c |
| A6    | Recording mode (event log, ring buffer)            | Done   | 2e8fb8b |
| A7    | Secret-memory policy (mlock + zero-on-free)        | Done   | f3170bf |
| A8    | Validation examples / integration demos            | Done   | 683981d |
| A9    | Fallible builders + BrokerError carries OS detail  | Done   | 755e710 |

Test coverage as of A9: 69 tests (62 lib + 5 integration + 2
proptest). The count dropped from 70 → 69 between A8 and A9 because A9
incidentally removed `strategy::slab::tests::slab_free_returns_not_implemented`,
an obsolete test that survived A3.5 (slab recycling) and asserted the
*opposite* of the correct slab behavior — slab DOES support free as of A3.5.
The correctly-named `bump_free_returns_not_implemented` (which matches
invariant #3) is retained.

Doctests: 1 passing + 6 ignored. Clippy clean under `-D warnings`
across crate and examples.

### A.2 Crate Layout

    crates/sentinel-broker/
      Cargo.toml
      benches/arena_bench.rs
      src/
        lib.rs            crate root + re-exports
        arena.rs          Arena (strategy + recorder + counters)
        broker.rs         Broker, ArenaHandle, within_budget
        budget.rs         Budget, BudgetScope, BudgetArenaBuilder
        builder.rs        ArenaBuilder (bump/slab + try_bump/try_slab as of A9)
        error.rs          BrokerError enum (no longer Copy as of A9)
        handle.rs         Handle<T>, HandleRef<'a, T>
        ids.rs            ArenaId, BudgetId, SlotIndex, Generation,
                          SlotGeneration, monotonic counters
        recording.rs      Recorder, Event (A6)
        secret.rs         SecretPolicy, SecretStrategy (A7)
        stats.rs          BrokerStats, ArenaSummary, HandleLocation (A5)
        strategy/
          mod.rs          AllocStrategy trait + AllocOk/SlotPtr/StrategyKind
          bump.rs         BumpStrategy
          slab.rs         SlabStrategy (freelist + per-slot generations)
      examples/
        token_bucket.rs       high-frequency slab allocation
        request_pipeline.rs   bump-per-request under budgets, with recorder
        credential_store.rs   secret slab with STRICT/LENIENT, raw zero-on-free verification
      tests/
        integration.rs    end-to-end API tests
        proptest.rs       property-based isolation/invalidation tests

### A.3 Public API Surface

All re-exports from `sentinel_broker`:

- Core: `ArenaHandle`, `Broker`, `Arena`, `Handle`, `ArenaBuilder`
- IDs: `ArenaId`, `BudgetId`, `Generation`, `SlotGeneration`, `SlotIndex`
- Strategies: `AllocStrategy`, `StrategyKind`
- Budgets (A4): `Budget`, `BudgetScope`, `BudgetArenaBuilder`
- Stats (A5): `ArenaSummary`, `BrokerStats`, `HandleLocation`
- Recording (A6): `Event`, `Recorder`
- Secret (A7): `SecretPolicy`, `SecretStrategy`
- Errors: `BrokerError`

#### A.3.1 Broker

- `Broker::new()`
- `Broker::with_recorder(Arc<Recorder>)` (A6)
- `broker.create_arena(name, capacity) -> ArenaHandle`
- `broker.arena(name).capacity(n).bump() -> ArenaHandle` (panics on
  misuse; see also `try_bump`)
- `broker.arena(name).slab(slot_size, slot_align, slot_count) -> ArenaHandle`
- `broker.arena(name).secret(policy).bump()/.slab(...)` (A7)
- `broker.arena(name).try_bump() -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.arena(name).try_slab(...) -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.destroy_arena(id)` invalidates all handles
- `broker.live_arena_count()`
- `broker.stats() -> BrokerStats` (A5)
- `broker.list_arenas() -> Vec<ArenaSummary>` (A5, sorted by id)
- `broker.where_is(&handle) -> Option<HandleLocation>` (A5)
- `broker.recorder() -> Option<&Arc<Recorder>>` (A6)
- `broker.within_budget(cap, |scope| { ... })?` (A4, nestable)

#### A.3.2 Arena / ArenaHandle

- `arena.alloc(value) -> Result<Handle<T>, BrokerError>`
- `arena.free(&handle)` (slab only; bump returns `NotImplemented`)
- `handle.get() -> Result<&T, BrokerError>`
- `handle.is_live()`, `arena_id()`, `slot()`, `slot_generation()`
- `arena.__raw_slot_bytes_for_diagnostics(slot)` (A8, doc-hidden,
  forensic tooling only)

#### A.3.3 BrokerError variants

`UseAfterFree`, `UseAfterFreeSlot`, `OutOfMemory`, `InvalidSlot`,
`UnknownArena`, `BrokerPoisoned`, `BudgetExceeded`, `NotImplemented`,
`SecretMemory { reason: String, os_errno: Option<i32> }` (A9 shape),
`BuilderMisuse { reason: &'static str }` (A9).

As of A9, `BrokerError` no longer derives `Copy`. `SecretMemory`
carries the underlying OS error number where available (previously
the OS errno was only logged via `tracing::warn!`).

### A.4 Design Invariants

These are properties the test suite enforces; future changes must
preserve them.

1. Arena destruction invalidates every handle. `destroy_arena(id)`
   removes the broker's `Arc<Arena>` from its map and calls
   `Arena::invalidate()`, which advances generation atomically.
   `Handle::get()` then returns `BrokerError::UseAfterFree`.

2. Per-slot generations defeat ABA. Each slab slot has its own
   generation counter. Reusing a slot increments it, so a handle to
   the prior occupant returns `BrokerError::UseAfterFreeSlot`.

3. Bump strategy never recycles. `BumpStrategy::free` returns
   `NotImplemented`; only `SlabStrategy` supports free. Bump's whole
   point is O(1) bulk free via arena destruction.

4. Budgets pre-charge reserved capacity. `arena("a").capacity(N).bump()`
   inside `within_budget(cap, ...)` charges N to the budget chain
   BEFORE the arena exists. Reservation, not usage, is what counts.
   Nested budgets charge both inner and every ancestor.

5. Budget refunds are atomic. If `try_charge` walks the chain and
   exceeds any cap, all prior charges in that walk are refunded
   before returning `BudgetExceeded`.

6. Recording never affects behaviour. If no recorder is attached,
   the hot path is an `Option::None` branch. If recording fails
   (mutex poisoned), the event is dropped silently — recording is
   observation, not enforcement.

7. All counters use `Ordering::Relaxed`. Snapshots from `stats()`
   may show momentary inconsistency across fields under concurrent
   load. This is expected and acceptable.

8. `Broker::with_recorder` is construction-time only. No runtime
   swap, no `AtomicPtr`. The recorder is set once and read on every
   event-emitting path via `&Option<Arc<Recorder>>`.

9. (A9) Panicking and fallible builders coexist. `.bump()` and
   `.slab()` panic on misuse and are kept for tests and demos that
   want construction-time failure surfacing as a panic. `.try_bump()`
   and `.try_slab()` are exact structural twins returning `Result`.
   New code prefers the fallible variants.

### A.5 Known Limitations / Tech Debt

- `Arena::with_strategy` is `#[allow(dead_code)]` — kept as a
  convenience wrapper but only the recorder-aware variant
  `with_strategy_and_recorder` is used.
- `Recorder` uses `Mutex<Vec<Event>>`; under very high concurrent
  allocation it serializes through one mutex. Acceptable for now.
- Bounded-ring `Recorder::record` uses `Vec::remove(0)` (O(n)) on
  overflow. A `VecDeque` would be better for larger caps.
- No benchmark gate in CI. `benches/arena_bench.rs` exists but is
  not exercised on every PR.
- Doctests are ignored to avoid pulling test-only types into the
  public examples. Should be tagged `no_run` and fleshed out before
  publishing.
- (BACKLOG §0.1 remaining) Bump `slot_size_hint` is `None`; the
  `SlotInfo.size` field exists but is dead-code. Either return it
  from a bump-side override or document diagnostics as slab-only.
- (BACKLOG §0.1 remaining) `Event` variants are not `#[non_exhaustive]`.
  Consumers serializing events would see field additions as
  breaking changes.

---

## Section B — sentinel-effects-proto (Sentinel-Mini)

Research-grade tree-walking interpreter. Built to validate
Sentinel's effect-system design before committing to the Phase C
production compiler per HANDOVER §5. The crate is explicitly
expected to be thrown away or rewritten once its lessons are
absorbed.

### B.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| B0    | Scaffold: lex + parse + eval, no types or effects  | Done   | d090ca1 |
| B1    | HM type inference, letrec, span-tracked errors     | Done   | e6b06cd |
| B2.0  | Row scaffold: Ty::Fun carries empty Row            | Done   | 2cd81a7 |
| B2.1  | Remy-style row unification                         | Done   | 323ab33 |
| B2.2a | Effect-surface lexer/AST/parser                    | Done   | a3fd3cc |
| B2.2b | Wire Perform through pipeline as type error        | Done   | 62405f2 |
| B2.3a | Effect-inference judgment refactor (no semantics)  | Done   | fd8eef6 |
| B2.3b1 | Row mechanics (Lambda mints ρ, App via arrow_with); default-close residual rows | Done   | f2a17d9 |
| B2.3b2-a | Perform inference + UnknownEffect; eff_env from prog.effects | Done   | 4c69ed7 |
| B2.3b2-b | Row generalization in Scheme; instantiate freshens row vars; drop EffectNotYetSupported | Done   | 47cc5a1 |
| B2    | Effect rows and effect declarations                | In progress |   |
| B3    | Effect handlers (handle .. with ..)                | Planned |        |
| B4    | Secret T qualifier and constant-time check         | Planned |        |
| B?    | Broker-as-value-heap integration (bonus)           | Planned |        |

Test coverage as of B1: 95 tests (8 lexer + 11 parser + 11 eval +
4 span + 7 types + 41 infer + 7 diag + 6 integration). Clippy
clean under `-D warnings`. No doctests yet.

Test coverage as of B2.0: 98 tests (B1 carry-over 95 + 3 new in
`types.rs`: `b20_empty_row_renders_as_empty_string`,
`b20_arrow_with_empty_row_is_unchanged_from_b1`,
`b20_rowvar_display_uses_r_prefix`). Clippy clean under
`-D warnings`; `clippy::result_large_err` allowed crate-wide in
`lib.rs` because widening `Ty::Fun` with `Row` pushed
`TypeError::Mismatch` over the 128-byte threshold. See B.5
design decision 12.

Test coverage as of B2.1: 111 tests (B2.0 carry-over 98 + 13 new
in `infer.rs`: ~10 `b21_unify_row_*` cases covering empty rows,
var binding, matching labels, mismatched payloads, label
rewriting, occurs-check, disjoint open tails, and closed-row
failures; ~3 row display tests). Clippy clean under
`-D warnings`. See B.5 design decision 13.

Test coverage as of B2.2: 134 tests (B2.1 carry-over 111 + 23
new: 6 lexer tokens, 11 parser (effect decls, do-perform,
uppercase-label rule, paren ty-expr, missing-semicolon,
arrow-required signature, right-associative arrows, do
inside arithmetic), 3 infer (Perform rejected with
EffectNotYetSupported, span targets label, pure-body
infers), 1 eval (direct-eval defence in depth), 2
integration (pure body evaluates, do is rejected with
rendered caret). Clippy clean under `-D warnings`. See
B.5 design decisions 14 and 15.

Test coverage as of B2.3a: 134 tests (B2.2 carry-over 134 + 0
new). B2.3a is a pure behavior-identical refactor per ADR 0005
D9: the inference judgment changes from `(Subst, Ty)` to
`(Subst, Ty, Row)`, every arm returns `Row::Empty`, every
recursive `infer` call site is threaded with a new `EffectEnv`
parameter that is unused at this phase, `Scheme` gains a
`row_vars: Vec<RowVar>` field (empty by default), and `infer_top`
gains a strict residual-row check that is unreachable in B2.3a.
All 134 existing test assertions pass unchanged; only the four
`Scheme { .. }` struct-literal call sites in tests were updated
to include the new (empty) `row_vars` field. Clippy clean under
`-D warnings`. See B.5 design decision 16 for the ADR 0005 D9
divergence (`TypeError::UnhandledEffects` and the strict
`infer_top` check were pulled forward from the D9 B2.3b list).

B1 landed across five commits: spans + Spanned AST + `let rec`
(abfb3d9), types scaffold (b3589ea), inference driver wired into
`run` (24a3db8), proper HM let-rec typing (72c0996), and
hand-rolled caret diagnostics (e6b06cd).

### B.2 Crate Layout

    crates/sentinel-effects-proto/
      Cargo.toml
      src/
        lib.rs            re-exports + `run()` convenience + `MiniError`
                          (now incl. `MiniError::render(src) -> String`)
        lexer.rs          logos tokeniser, `Token` (incl. `Rec`), `LexError`
        ast.rs            `Expr = Spanned<ExprKind>`, `BinOp`, `LetRec` variant
        parser.rs         hand-written recursive descent, precedence climbing,
                          `ParseError`, span-threading
        eval.rs           tree-walking interpreter, persistent `Env`,
                          `Value`, `EvalError`, `let rec` via `OnceLock`
        span.rs           `Span { start: u32, end: u32 }`, `Spanned<T>` (B1.1)
        types.rs          `Ty`, `TyVar`, `Scheme`, free-var sets (B1.4)
        infer.rs          HM Algorithm W: `Subst`, `unify`, `instantiate`,
                          `generalize`, `TypeEnv`, `infer`, `infer_top`,
                          `TypeError` (B1.4-B1.6)
        diag.rs           `LineCol`, `locate`, `render` -- hand-rolled
                          rustc-style caret diagnostics (B1.7)
      tests/
        integration.rs    end-to-end pipeline tests

### B.3 Language Surface (B0)

Pure expression calculus with HM type inference. Everything is an
expression. No statements, no effects, no `secret` yet.
Recursion is supported via `let rec` (B1.3); types are inferred
with let-polymorphism and let-rec generalization (B1.5/B1.6).

Grammar (informal):

    expr      := let | letrec | if | lambda | compare
    let       := "let" IDENT "=" expr "in" expr
    letrec    := "let" "rec" IDENT "=" lambda "in" expr
    if        := "if" expr "then" expr "else" expr
    lambda    := "fn" "(" IDENT ")" "=>" expr
    compare   := add ( ("==" | "<" | ">") add )?
    add       := mul ( ("+" | "-") mul )*
    mul       := app ( ("*" | "/") app )*
    app       := atom ( "(" expr ")" )*
    atom      := INT | BOOL | IDENT | "(" expr ")"

Comments are `// to end of line`.

Single-parameter lambdas only. Multi-parameter functions are written
curried (`fn(x) => fn(y) => ...`).

### B.4 Public API Surface

All re-exports from `sentinel_effects_proto`:

- AST: `Expr = Spanned<ExprKind>`, `ExprKind`, `BinOp`, `expr` constructor helper
- Spans: `Span`, `Spanned<T>`
- Lexer: `Token`, `LexError`,
  `lex(source) -> Result<Vec<(Token, Span)>, LexError>`
- Parser: `ParseError`,
  `parse(&[(Token, Span)]) -> Result<Expr, ParseError>`
- Eval: `Value`, `EvalError`, `Env`,
  `eval(&Expr, &Env) -> Result<Value, EvalError>`
- Types: `Ty`, `TyVar`, `Scheme`
- Inference: `TypeError`, `TypeEnv`, `EffectEnv`, `Subst`,
  `TyVarSupply`, `unify`, `instantiate`, `generalize`, `infer`,
  `infer_top`, `infer_program`
- Top-level: `MiniError`,
  `run(source) -> Result<Value, MiniError>` (lex+parse+infer+eval),
  `MiniError::render(&self, source) -> String` for caret diagnostics

The `diag` module is `pub mod diag` but its items are reached
through `MiniError::render`; they are not re-exported at the
crate root in B1.

### B.5 Design Decisions (B0)

1. Hand-written recursive descent over `chumsky` / `lalrpop`. Per
   HANDOVER §3.3 (production compiler) and our B0 reasoning: the
   grammar will churn as effects and qualifiers are added; hand-written
   parsers absorb grammar changes more cheaply than combinator chains.
2. Plain `Box`-allocated AST. No `bumpalo`. The language is small
   enough that heap traffic is irrelevant for a research artifact.
3. Persistent `Arc`-cons-list environment for closures. Standard
   Crafting-Interpreters shape. May be revisited if broker-as-value-heap
   integration lands.
4. Errors are not span-tracked at B0. Spans land with B1 alongside
   the type checker so error highlighting can be meaningful from
   the start.
5. `BrokerError`-style two-flavour API (panicking + fallible) is
   NOT adopted here. Effects-proto is throwaway research code;
   panicking-only is acceptable and simpler. If a panic-free API
   becomes useful for embedding, it lands then.
6. (B1.1/B1.2) AST nodes carry spans via a `Spanned<T>` wrapper,
   not an inline `span` field on each variant. Confirmed cheap;
   parser pattern is `Spanned::new(kind, start_span.merge(end_span))`.
7. (B1.3) `rec` is a reserved keyword (`Token::Rec`), not a
   contextual one. `let rec` is the only place it can appear in B1.
8. (B1.4) Substitutions are eager (`apply` on `bind`), not
   union-find. Idempotency is maintained by `compose`. Fine at
   B1 scale; revisit only if profiling demands.
9. (B1.5/B1.6) Inference is Algorithm W in the textbook shape.
   `let rec` uses the standard HM treatment: monomorphic recursive
   occurrence inside the RHS, generalized scheme in the body.
   Polymorphic recursion is therefore unavailable without
   annotations -- this is intentional and matches ML/Haskell
   without explicit type signatures.
10. (ADR 0002) Function arrows are bare `Fun(Ty, Ty)`. Effect rows
    are deferred to B2 to keep B1 focused.
11. (B1.7) Diagnostics are hand-rolled (`diag.rs`, ~110 LoC, no
    `miette` dependency). Phase C will likely adopt miette; the
    prototype validates the shape (line/col header, source-line
    excerpt, caret underline) cheaply. `Display` for `MiniError`
    stays terse; pretty rendering is opt-in via `.render(src)`.
12. (B2.0) `Ty::Fun` now carries a `Row` per ADR 0002 / ADR 0004.
    `Row` is a distinct enum (`Empty | Var(RowVar) | Cons { .. }`),
    `RowVar` is a distinct kind from `TyVar`, `Subst` carries
    parallel `map` / `row_map` fields. B2.0 ships behaviour-
    preserving: every arrow gets `Row::Empty`, `unify_row` is a
    stub handling only the empty-vs-empty case (B2.1 fills in
    Remy-style row unification). Clippy `result_large_err` allowed
    crate-wide because `Row` pushed `TypeError` past the lint's
    128-byte threshold; STATE.md decision 5 already documented
    that effects-proto does not optimise error shape.
13. (B2.1) Row unification follows Remy 1989 / Leijen's
    extensible records: `unify_row` handles empty-vs-empty,
    var binding (with `row_occurs` check), matching `Cons`
    heads by recursing on arg/ret/tail, and label rewriting
    via `rewrite_row` when heads differ. Two new
    `TypeError` variants `RowMismatch` and `RowOccursCheck`
    carry the offending row and span. `unify` now threads
    `&mut TyVarSupply` to mint fresh row tails during
    rewriting; all call sites (App, If, BinOp, LetRec) were
    updated. Unit tests use `RowVar` IDs >= 100 to avoid
    collisions with the fresh-supply counter (which starts
    at 0). Inference still mints only `Row::Empty` at
    lambda introduction; user-visible effect behaviour
    arrives in B2.3.
14. (B2.2a) Surface for effect declarations and operations.
    Five new tokens (`Colon`, `Semicolon`, `Arrow`, `Effect`,
    `Do`) make `effect` and `do` reserved keywords; neither
    was used as an identifier in B1. Grammar:
    `effect Label : TyExpr ;` where TyExpr is required to be
    an arrow at the top level, and `do Label(arg)` at the
    atom level. Effect labels must start with an uppercase
    ASCII letter (parser-enforced via
    `ParseError::EffectLabelNotUpper`). A new surface-level
    `TyExpr` enum lives in `ast.rs` deliberately distinct
    from the inference-time `Ty` so the parser does not
    depend on the type system. Fix-A grammar (single
    `TyExpr` then split-on-arrow) was chosen over
    `ArgTy '->' RetTy` because the latter is ambiguous when
    `ArgTy` itself contains `->`. ADR 0004 will be amended
    in B2.5 to reflect the actual grammar production.
15. (B2.2b) `Perform` is parseable but rejected by inference
    with `TypeError::EffectNotYetSupported { label, span }`
    where span targets the label identifier (not the `do`
    keyword) so diagnostics caret the meaningful token.
    `EvalError::EffectNotYetSupported(String)` exists for
    defence in depth (callers bypassing inference) and is
    span-less to match the B1 `EvalError` precedent
    (decision 5 lineage; full span enrichment is a backlog
    item). `run()` now pipelines through `parse_program` +
    `infer_program`; effect declarations are parsed but
    inert (typing environment is unchanged). Real effect
    rows wire in B2.3.

16. (B2.3a, ADR 0005 D9) Effect-inference judgment refactored
    behavior-identical: `infer` returns `(Subst, Ty, Row)` and
    takes a new `eff_env: &EffectEnv` parameter; `Scheme` gains
    a `row_vars: Vec<RowVar>` field (empty default, source-
    compatible with `Scheme::mono`); every arm returns
    `Row::Empty`; `Perform` keeps its B2.2b `EffectNotYetSupported`
    behavior. Divergence from ADR 0005 D9 B2.3a list, deliberate:
    `TypeError::UnhandledEffects { row, span }` and the strict
    residual-row check in `infer_top` were pulled forward from
    the D9 B2.3b list. Both are unreachable in B2.3a (every arm
    returns `Row::Empty`, so `s.apply_row(&Row::Empty)` is always
    `Row::Empty`) and land here to isolate B2.3b's diff to
    semantics only -- this strengthens the bisection-point property
    ADR 0005 D9 Consequences sec.1 calls for. `UnknownEffect` is
    *not* pulled forward; it remains strictly B2.3b because it
    would be genuinely dead (never constructed) in B2.3a. Commit
    fd8eef6.

17. (B2.3b1, ADR 0005 D2 + ADR 0006 amended) Row mechanics landed
    behavior-extending: `Lambda` mints fresh `ρ` and embeds it in
    the arrow via `Ty::arrow_with`; the lambda's own row
    contribution stays `Row::Empty`. `App` mints `ρ_call`, unifies
    callee against `arrow_with(t_arg, ρ_call, result)`, and unions
    `r_callee`, `r_arg`, and resolved `ρ_call` into its
    contribution. `Let`, `LetRec`, `If`, `BinOp` union
    subexpression contributions. `Perform` still rejects with
    `EffectNotYetSupported` (B2.3b2 wires it). Default-close (ADR
    0006 D1) extended: `Row::Var` in *contributions* is treated as
    `Row::Empty` (in `row_union` and at `infer_top`'s residual
    check), because `App` legitimately produces free `ρ_call`s
    whenever the callee is a `Ty::Var` (recursive calls,
    higher-order parameters). Net +7 tests (one earlier B2.3b1
    test with a wrong premise was deleted). Commit f2a17d9.

18. (B2.3b2, ADR 0005 D9 closed) Perform inference + row
    generalization landed as a two-commit split. **b2.3b2-a**
    (4c69ed7) wires `do Label(arg)` through the effect environment:
    `infer_program` populates `EffectEnv` from `prog.effects`; the
    `Perform` arm looks up the label, infers and unifies the arg
    against the declared arg type, and contributes a closed
    single-label `Cons` row union'd with the arg's row. Unknown
    labels surface as a new `TypeError::UnknownEffect { label,
    span }` targeting the label token. **b2.3b2-b** (47cc5a1)
    extends `generalize` to quantify free row variables in the
    type (minus those free in the env) into `Scheme.row_vars`;
    `instantiate` freshens them via `fresh_row_var`. Row
    *contributions* are intentionally excluded from generalization
    — they describe the RHS's latent effect, not its binding
    scheme; conflating them would make every let-binding look
    effectful from the outside, breaking the default-close
    presentation contract (ADR 0006 D3). The placeholder
    `TypeError::EffectNotYetSupported` is removed entirely now
    that `Perform` is fully typed; `EvalError::EffectNotYetSupported`
    stays until B3 handlers land. Split rationale: semantics
    first, generalization second, each independently bisectable
    and each under ~300 LOC. Net +14 tests (10 b23b2_* unit + 5
    b23b2b_* unit + 2 integration − 2 obsolete b22b_perform_*
    rewritten − 1 integration test repurposed).

### B.6 Known Limitations (intentional at B1)

- No effects. The whole reason this crate exists. (B2 onward.)
- No `secret` qualifier or constant-time check. (B4.)
- No REPL, no driver binary. Library-only.
- `let rec` RHS must be a syntactic lambda. Parser enforces with
  `ParseError::LetRecNotLambda`. Relaxing this in B3 (when handlers
  arrive) is an open question; see ADR 0003.
- Polymorphic recursion is rejected, as in ML/Haskell without an
  explicit type signature. Test
  `b16_letrec_recursive_occurrence_is_monomorphic_inside_body`
  locks this.
- Equality (`==`) is polymorphic at the type level (`forall a. a -> a -> Bool`).
  The evaluator still rejects equality on closures; B2/B3 may
  refine via type-class-style constraints.
- `EvalError` variants carry no spans. Eval errors are rare
  post-type-check (div-by-zero, non-function application on a
  closure-typed value, the letrec uninitialised internal error)
  but they render without carets. B2 backlog item.
- Multi-line spans in `diag::render` clip to the first line. Sentinel-Mini
  programs are usually one-liners; Phase C diagnostics will handle
  multi-line ranges properly.
- Closures `clone()` the body `Expr` into an `Arc<Expr>`. Acceptable
  for a research artifact; revisit if body-clone becomes a hot path
  or if the broker becomes the value heap.
- `Value` does not derive `Eq` (closures aren't comparable); the
  error types also drop `Eq` to keep them embeddable in each other.
  They remain `Debug + Clone + PartialEq`, which is what tests use.

---

## Conventions

### Build & Test Commands

The standard check suite for a clean tree, applied per-crate:

    cargo build -p <crate>
    cargo clippy -p <crate> --all-targets -- -D warnings
    cargo nextest run -p <crate>           # or `cargo test -p <crate>`
    cargo test -p <crate> --doc

All four must pass for any commit on `main`. Current expected counts:

  - sentinel-broker:        69 tests + 1 doctest
  - sentinel-effects-proto: 155 tests (146 lib + 9 integration) + 0 doctests

### Script Convention

Every code change lands via a script under `scripts/`:

  - `NN-<phase>.sh`          primary generator/patch for a milestone
  - `NNa-`, `NNb-`, ...      follow-up patches (lint fixes, etc.)
  - `NNz-commit-<phase>.sh`  pre-commit checks + commit creation

Scripts are committed alongside source changes so contributors can
see exactly what was patched, in what order. They are also a useful
debugging aid: re-run the most recent `NNa-` under `set -x` to
inspect a patch.

Output convention: each script prints `======` delimited sections
(BUILD / CLIPPY / TESTS / DOC TESTS). When asking for help, paste
those sections back.

### Working Norms (from HANDOVER §0.1)

- Trust STATE.md (this file), not the git log.
- Terminal heredocs: single-level only. Use `cat > /tmp/script.py`
  then `python3 /tmp/script.py` instead of nested heredocs.
- Avoid `set -e` and bare `exit` in pasted scripts — they can close
  the user's terminal. Use `return 1 2>/dev/null || exit 1` and
  rely on `PIPESTATUS[0]` for return codes.
- Small patches, build between each. Land type/trait changes first,
  build, then implementations, build, then tests.
- Honest disclosure beats confident-but-wrong.
- Examples held to `-D warnings`.
- Check before overwriting docs (`p.exists()` and merge patterns).

---

*End of document. Update on every commit that changes phase
status, public API surface, or invariants.*
