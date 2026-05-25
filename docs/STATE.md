# STATE.md — Sentinel Implementation Status

This document tracks what is actually built. When it disagrees with
HANDOVER.md, STATE.md is the source of truth. New contributors (or
new chat sessions) should be able to read this file and understand
the current state of the workspace without re-reading every commit.

The workspace has two complete Phase A/B crates plus the in-progress
Phase C bootstrap compiler. Phase A is the broker (production-shape
memory subsystem), Phase B is sentinel-effects-proto (Sentinel-Mini,
research-grade interpreter), Phase C populates the remaining
sentinel-* compiler crates per ADR 0009. As of C0.0, sentinel-syntax
has a lexer; the other nine compiler crates remain scaffold stubs.

Last updated: **C1.1 landed (C1.1.1 at 438dd16, C1.1.2 at
9374edf)** — name resolution lifts out of `sentinel-codegen` into
a populated `sentinel-resolve` crate per ADR 0011 D4. C1.1.1
scaffolds the crate: VarId/FnId/FnSignature, parallel-tree
resolved AST (ResolvedProgram/FnDef/Param/Block/Stmt/Expr),
ResolveError with the 6 variants that used to live in
CodegenError (UndefinedVariable, RedeclaredVariable,
UndefinedFunction, ArityMismatch, RedefinedFunction, MissingMain),
pure `resolve()` + `#[salsa::tracked]` `resolve_query` chaining on
parse_query. C1.1.2 rewires sentinel-codegen and sentinel-driver:
`compile_to_object` now takes `&ResolvedProgram`; codegen's vars
map is keyed by VarId, fns map by FnId; the driver pipeline
becomes parse_query → resolve_query → codegen with diagnostics
transitively accumulated across all three stages. **Phase C1.1
complete**; ADR 0011 D4 ACCEPTED. Workspace test delta: +20
(C1.1.1's resolve unit tests + salsa query smoke) -5 (codegen's
rejects-NAME tests moved to resolve, net -8 deleted + 3 new
positive codegen tests added) = net +15. 468 tests total. All
22 C0 pass-test fixtures still run end-to-end through the new
pipeline. Pre-C1.1 context: **C1.0c decision landed** — the
codegen-salsa question is resolved as "defer until C1.2+ (typed
HIR rewrite)"; ADR 0011 D1 amended with the three-option weigh-up
and rationale. The Salsa retrofit is now complete for the front-
end with codegen intentionally outside the query graph. **Phase
C1.0 is complete**. Pre-C1.0c context: **C1.0b landed at
557cc60** — the lex and parse pipeline stages
now run through `#[salsa::tracked]` queries against `SentinelDb`,
with `sentinel_base::Diagnostic`s flowing through the
`#[salsa::accumulator]` rather than rich error vectors through
tracked-struct fields (the C1.0a pause was caused by
`miette::SourceSpan` not deriving Hash; routing errors through the
accumulator side-steps the Hash bound entirely). AST types
(`Spanned<T>`, `Block`, `Param`, `FnDef`, `Program`, `StmtKind`,
`ExprKind`) gained `#[derive(Hash)]`. `sentinel-syntax::query`
exposes `lex_query` and `parse_query`; the `sentinel-driver`
binary instantiates a concrete `SentinelDatabase`, sets a
`SourceFile`, calls `parse_query`, and collects diagnostics via
`parse_query::accumulated::<Diagnostic>`. All 22 existing pass-test
fixtures still run end-to-end through the new query-based driver
path; the C0 go/no-go program at `tests/pass/c05_go_no_go.sentinel`
still produces stdout `10\n`, exit 0. Codegen is intentionally not
yet salsa-wrapped (deferred to C1.0c per HANDOVER §0.2 step 5);
LLVM context lifetimes may not fit salsa's query model cleanly and
the retrofit/driver wiring was worth landing first. ADR 0011's D1
(Salsa adoption at C1.0) is now exercised end-to-end; ADR 0011
remains PROPOSED until all of C1.0 (incl. codegen) is in. Net +7
workspace tests over C1.0a (4 lex/parse query positive + 1
diagnostic + 2 cache validation).

Phase C0 retrospective (preserved as historical context for what
came before Phase C1): the bootstrap compiler can lex, parse,
name-resolve (still in codegen), and lower fn-based programs with
let, arithmetic, if/else, blocks, and print to runnable binaries
via LLVM. The ADR 0010 appendix go/no-go program
(`double + pick + main with print`) compiles and runs at
`tests/pass/c05_go_no_go.sentinel`: stdout `10\n`, exit 0.
Programs are one-or-more `fn` definitions with an explicit `main`
entry point. Codegen is two-pass (signatures, then bodies); main
returns i32 (the C ABI shape) and other fns return i64. ADR 0009
status records Phase C0 as complete; ADR 0010 status notes all
D-decisions exercised. Workspace test count at Phase C0 close:
445 (+22 over C0.4 — 2 ast Display + 14 parser + 1 codegen net +
5 pass).

Phase C0 retrospective: six sub-phases (C0.0 lexer, C0.1 parser +
AST, C0.2 LLVM codegen + first runnable binaries, C0.3 let +
variables, C0.4 if/else + print + first stdout, C0.5 fn defs +
main) shipped across twelve commits. The compile pipeline source
-> lex -> parse -> AST -> two-pass LLVM IR -> object -> cc-linked
executable handles every C0 feature.

Phase B retrospective (preserved as historical context for what
came before C0): all three HANDOVER §5.2 validation demos landed
(supply-chain, async-as-effect, password-verify), 226 tests passing
in effects-proto. ADRs 0001-0008 ACCEPTED. See ADRs 0003 (B1
retrospective), 0004 (row representation), 0005 (effect-inference
judgment; D9 closed), 0006 (default-close, amended; D4 row
polymorphism implemented), 0007 (effect handlers; status fully
ACCEPTED, D9 fully complete — all phases B3.0 + B3.1 + B3.2 landed),
0008 (secret qualifier and constant-time check; status ACCEPTED, D1
through D7 confirmed by B4.0 + B4.1, D8 implicit via existing
free-var/free-row-var recursion, D9 amended for B4.2 landed). Phase
B is finished; Phase C began with ADR 0009 at 7a04ba1 and C0.0 at
8f37381.

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
| B2    | Effect rows and effect declarations                | Done   |        |
| B3.0  | Handler surface (lexer + parser + AST + placeholders) | Done   | 821b16a |
| B3.1a | row_split + HandlerLabelNotInRow                   | Done   | febf379 |
| B3.1b | Handler typing rule + DuplicateHandlerArm          | Done   | e7958e1 |
| B3.2a | Handler runtime scaffolding (Step/Continuation/Frame)| Done   | bdda217 |
| B3.2b | Handler runtime (Perform reifies, Handle dispatches) | Done   | a9cefb1 |
| B3.2c | Positive runtime coverage (4 integration tests)      | Done   | 8e3de20 |
| B3.2  | Handler runtime (operation reification + dispatch)   | Done   |         |
| B4.0a | Surface AST + SecretsNotYetSupported placeholder     | Done   | 1693b8c |
| B4.0b | Lexer Token::Secret + Token::Declassify              | Done   | 63cd57b |
| B4.0c | Parser secret prefix + declassify atom + DoubleSecret| Done   | 0b6b2ce |
| B4.0  | Secret/declassify surface (B4 phase 0 of 3)          | Done   |         |
| B4.1a | 4 TypeError variants + D2 unify + Declassify typing  | Done   | e760d57 |
| B4.1b | D3 If/Div + D4 comparisons; drop placeholder         | Done   | 52acc0a |
| B4.1  | Secret typing (unify, infer, four CT rejections)     | Done   |         |
| B4.2  | Three Phase B validation demos + README/STATE refresh| Done   | 9541969 |
| B4    | Secret T qualifier and constant-time check           | Done   |         |
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
- Eval: `Value`, `EvalError`, `Env`, `Step`, `Continuation`,
  `eval(&Expr, &Env) -> Result<Step, EvalError>` (B3.2a return type;
  `crate::run` bridges `Step::Value` → `Value` and surfaces
  `Step::Op` as `EvalError::UnhandledOpAtTopLevel`)
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


19. (B3.0, ADR 0007 D1/D2) Handler surface landed: `handle e with
    { L(x, k) => body, ..., return v => ret_body }` with arms
    comma-separated, trailing comma permitted, empty `{}` rejected,
    arm labels required to be uppercase (mirroring `do Label`).
    `handle` parses at `parse_expr` precedence (peer of `if`, `let`,
    `fn`). `return` becomes a globally reserved keyword — no
    existing test or code used `return` as an identifier, but the
    reservation is real and future surface work should account for
    it. AST: `ExprKind::Handle { body: Box<Expr>, arms:
    Vec<HandlerArm>, ret_arm: Option<ReturnArm> }`, with `HandlerArm`
    carrying `label`, `label_span`, `arg`, `kont`, `body`, `span` and
    `ReturnArm` carrying `var`, `body`, `span`. The return arm is
    optional; absence semantically defaults to `return v => v`, with
    the default introduced by the typer (D1 in the ADR), not the
    parser, so synthesized AST nodes with fake spans never enter the
    diagnostic path. New `ParseError` variants
    `HandlerArmLabelNotUpper` and `EmptyHandler`. Placeholder
    inference and eval errors (`TypeError::HandlersNotYetSupported`
    and `EvalError::HandlersNotYetSupported`) keep the build whole
    until B3.1 (typing) and B3.2 (runtime) replace them. Commit
    821b16a.

20. (B3.1, ADR 0007 D3/D4) Handler typing rule landed in two commits.
    **B3.1a** (febf379) adds `row_split` as the dual of the existing
    `rewrite_row`: given a row and a label, returns the discovered
    `(arg_ty, ret_ty)` signature and the residual row, with four
    cases per ADR 0007 D4 (head match, deeper match, row-var case
    minting fresh `α`/`β`/`tail` and binding the var, empty-row
    erroring with the new `HandlerLabelNotInRow`). Critically the
    row-var case mints a fresh *type* var for both arg and ret —
    this is what makes handlers compose with row-polymorphic
    callers, because the handler arm's typing of `x` and `k` later
    pins those fresh vars to concrete types. `rewrite_row` was not
    reused: it expects a known signature to unify against;
    `row_split` reads the signature out. Distinct errors, distinct
    intent, easier to diagnose. **B3.1b** (e7958e1) wires the
    typing rule per D3: infer body, reject duplicate arm labels with
    `DuplicateHandlerArm`, peel each arm's label out of the body's
    row threading substitution, capture the post-peel residual as
    `r_outer_initial`, mint `t_result`, type each arm body under
    `env + {x: arg_ty, k: ret_ty -> t_result ! r_outer_initial}` and
    unify with `t_result`, union each arm body's own row
    contribution into `r_accumulated`, handle the return arm
    (or default to `t_body = t_result` when absent). The
    continuation arrow uses `r_outer_initial` rather than the
    final `r_accumulated` — the standard Eff/Koka calculus, sound
    because `k`'s declared row is what `k` requires of its
    invocation context, not what the arm body around the `k`
    invocation may additionally perform. `TypeError::HandlersNotYetSupported`
    removed (variant + `lib.rs` span arm). Three planned items
    landed as no-ops: (a) the D7 canary collapses to
    `b31b_handle_identity_discharges_effect` because
    `infer_program`'s default-close policy (ADR 0006 D6) hides
    observable row polymorphism at the public type level — the
    canary's load-bearing property (generalize/instantiate compose
    with handler typing) is satisfied transitively by all `b31b_*`
    tests passing without modifying that machinery; (b) the D8
    positive/negative pair is unreachable through `infer_program`
    (permissive) and uncallable through `infer_top` (no effect
    env), so strictness coverage comes indirectly through
    `b31b_handle_two_arms_discharges_both` and
    `pipeline_handle_typechecks_then_runtime_is_placeholder`; (c)
    extending `TypeError::UnhandledEffects` was reviewed and
    rejected — `Row`'s `Display` already renders residuals as
    `{Label1, Label2 | tail}`, human-readable as-is. ADR 0007
    status flipped to ACCEPTED for B3.0 + B3.1; D9 amended with
    completion-marker status. Net +20 lib tests (4 b31_row_split_*
    + 7 b31b_* + 9 b30_ from B3.0) + 1 integration test rewritten
    (`pipeline_handle_typechecks_then_runtime_is_placeholder`).
    Eval-side placeholders remain pending B3.2.
Test coverage as of B3.2: 183 tests (169 lib + 14 integration). Lib
count unchanged from B3.1 because B3.2a was scaffolding (no behavior
change) and B3.2b rewrote three placeholder-asserting tests in place
rather than adding new ones (one direct-eval Perform test:
`b22b_eval_perform_directly_returns_effect_error` →
`b32b_eval_perform_directly_reifies_step_op`; one direct-eval Handle
test: `b30_eval_handle_directly_returns_handlers_not_yet_supported` →
`b32b_eval_handle_no_arms_no_ret_is_identity_on_value`; one
integration test: `pipeline_handle_typechecks_then_runtime_is_placeholder`
→ `pipeline_handle_resumes_with_arg_through_run`, the first Sentinel
program with effects to actually compute a value through `run()`).
B3.2c added four integration tests covering two-effect dispatch,
non-trivial arm body work, arm-body reading outer Let bindings via
the env bundled into `Value::Resumption`, and explicit return-arm
post-resume. Clippy clean under `-D warnings`; no doctests. See B.5
design decision 21.

21. (B3.2, ADR 0007 D5) Handler runtime landed in three commits.
    **B3.2a** (bdda217) is scaffolding: `eval`'s return type changed
    from `Result<Value, EvalError>` to `Result<Step, EvalError>` where
    `Step = Value(Value) | Op { label, arg, kont }`; every existing
    arm threaded `Step::Value` propagation (Int/Bool/Var/Lambda wrap;
    Let/LetRec/If/App/BinOp match on the recursive eval result,
    propagating Value on the happy path and pushing a `Frame` onto
    kont + re-raising Op otherwise). `Continuation` is a Vec<Frame>
    enum, not a boxed `FnOnce` — chosen for Debug/Clone ergonomics
    and for keeping the resume-point states explicit and greppable.
    Eight Frame variants enumerated by counting resume-points in the
    existing eval arms (LetBody, LetRecBody, IfBranch, AppArg,
    AppCall, BinOpRight, BinOpApply, PerformReify). `Continuation::resume`
    declared as `todo!("B3.2b")` so the API signature was fixed.
    Perform and Handle arms kept their B2.2b/B3.0 placeholder errors
    unchanged; tests stayed at 169 + 10 + 0. Two new EvalError
    variants added: `UnhandledOpAtTopLevel { label }` for
    defence-in-depth at `crate::run` (mirroring B2.2b's posture —
    the type system's default-close at `infer_program` should
    prevent open rows at the top level, but the runtime checks
    anyway), and `ContinuationAlreadyResumed` reserved for B3.2b's
    one-shot enforcement. `Step` re-exported from the crate root.

    **B3.2b** (a9cefb1) is the substantive commit. Five sub-patches
    in the working tree, single commit at the end: (1)
    `Value::Resumption { kont, arms, ret_arm, env }` variant added
    + `apply` refactored to dispatch on it (initially without the
    deep re-wrap, filled in next); (2) `Frame::HandleFwd { arms,
    ret_arm, env }` variant added — the 9th Frame, not visible in
    B3.2a because Handle didn't propagate before — and `handle_step`
    helper added as the shared dispatch hub used both by the Handle
    arm at evaluation start and by `apply` at resume re-wrap; (3)
    `Perform` arm now reifies — eval the arg, on Value produce
    `Step::Op { label, arg: v, kont: Continuation::empty() }`, on Op
    push `Frame::PerformReify` and re-raise; (4) `Handle` arm now
    dispatches — eager-Arc the arms and ret_arm once per Handle
    evaluation (so subsequent Resumption/HandleFwd clones are cheap
    refcount bumps), eval the body, route the resulting Step
    through `handle_step`; (5) cleanup —
    `EvalError::EffectNotYetSupported` and `EvalError::HandlersNotYetSupported`
    deleted now that their producing arms gained real
    implementations, `#[allow(dead_code)]` stripped from Frame.
    Three placeholder tests were rewritten in place to assert real
    behavior. The bundled-context shape of `Value::Resumption`
    (kont + arms + ret_arm + env) was chosen over a bare
    `Value::Continuation` because the deep re-wrap data must travel
    with the resumption value — `apply`'s dispatch stays in one
    place (sibling to `Value::Closure`) without an App-site lookup
    into handler state, and the alternative (synthesising a
    Lambda AST node that calls a builtin) requires a builtin-
    function mechanism the prototype does not have.

    **B3.2c** (8e3de20) adds four pipeline tests through `run()`
    along axes the B3.2b integration test did not cover:
    `pipeline_two_effect_handler_discharges_both` exercises deep
    re-wrap (apply's `handle_step` after `kont.resume`) and the
    splice mechanism (step_frame's `BinOpRight` branch producing a
    nested Op with `BinOpApply` prepended onto its inner kont);
    `pipeline_arm_body_computes_with_op_arg_before_resume` covers
    the rhs-raises BinOp path (sibling to the previous test's
    lhs-raises path) and confirms arm bodies can do non-trivial
    work with the operation's argument before invoking k;
    `pipeline_arm_body_uses_outer_let_binding` confirms the arm
    body sees outer Let bindings via the env bundled into
    `Value::Resumption` (the closest the current language reaches
    toward a state handler without CPS state-threading);
    `pipeline_return_arm_runs_on_resumed_value` exercises
    `handle_step`'s Some-ret_arm branch (the B3.2b integration test
    had no ret_arm, hitting only the None/identity branch).

    Two structural decisions worth recording inline. (a)
    `Continuation` derives `Clone` because `Value` is `Clone` and
    `Value::Resumption` holds `Continuation` by value. The
    `Cell<bool>` resumed-flag copies its bool on clone, so cloning
    a *resumed* `Continuation` produces a clone that also refuses
    to resume — the one-shot guarantee therefore holds
    per-Continuation-instance, not per-logical-resumption. Nothing
    in eval clones a Resumption today; user-level multi-shot via
    `Value::Clone` is not statically prevented. Stricter alternatives
    (move-only `Resumption` variant, or `Rc<Cell<bool>>` for a
    shared flag) are recorded inline at the `Continuation`
    definition for Sentinel proper. (b) A real CPS-state state
    handler in the Eff/Koka sense — Get/Put paired effects threading
    mutable state through resumption — was scoped *out* of B3.2c
    in favor of the simpler `pipeline_arm_body_uses_outer_let_binding`
    test. The prototype's unary lambdas preclude `k(state, value)`,
    and CPS-state via `state -> result` returns produces a test
    program ~6 lines long with a trace that obscures what is being
    tested. The load-bearing mechanic (outer Let visible inside arm
    via bundled env) is covered. Deferred to whatever phase
    introduces multi-arg functions or records. Filed in ADR 0007's
    "Considered and rejected" section.

    Net 0 lib tests, +4 integration tests, −1 lib test renamed,
    −2 lib test rewrites in place, −1 integration test rewrite in
    place. Placeholder-error variants gone:
    `EvalError::EffectNotYetSupported`,
    `EvalError::HandlersNotYetSupported`. New variants:
    `EvalError::UnhandledOpAtTopLevel`,
    `EvalError::ContinuationAlreadyResumed`. New public types: `Step`,
    `Continuation` (with `is_empty()` for tests).

Test coverage as of B4.0: 209 tests (192 lib + 17 integration). Net
+23 lib (+5 b40_ in types.rs from B4.0a, +3 b40a_ placeholder tests
in infer.rs, +6 b40b_ lexer tests, +9 b40c_ parser tests) and +3
integration (declassify rejected at infer, effect-decl-with-secret
rejected at infer, double-secret rejected at parse). Clippy under
`-D warnings` is broken by 5 pre-existing lints (Arc-not-Send/Sync,
extend-vs-append, into_iter-in-IntoIterator-context, unnecessary
map_or) that pre-date B4 and were inherited from B3.2; not a B4
regression. cargo test remains the project's green-gate. See B.5
design decision 22.

22. (B4.0, ADR 0008 D9) Secret/declassify surface landed in three
    commits. **B4.0a** (1693b8c) is the surface AST + placeholder.
    `Ty::Secret(Box<Ty>)` chosen over (a) qualifier-field-on-every-Ty
    and (c) parallel qualifier lattice per ADR 0008 D1: shape (b)
    falls out of HM unification with zero new machinery, while the
    no-α-leak unification restriction (D2) substitutes for full
    qualifier polymorphism. Idempotent smart constructor
    `Ty::secret` collapses `Secret(Secret(_))` so substitution and
    unification call sites don't worry about flattening; the parser
    separately rejects literal `secret secret T` (B4.0c) so the
    surface complains early but the inference layer is robust
    regardless. Display per D6 with arrow-parens (`secret int` vs
    `secret (a -> b)`). `Subst::apply` recurses via `Ty::secret`;
    `unify` adds a structural `(Secret, Secret)` arm. The B4.0 Var
    arm in `unify` will happily bind a TyVar to a `Ty::Secret(_)` —
    D2's no-α-leak rule is B4.1 — but this is unobservable in B4.0
    because every entry point that introduces `Ty::Secret` into
    inference is gated: `infer`'s `Declassify` arm returns
    `SecretsNotYetSupported`, and `infer_program` walks each effect
    decl's signature with a new `tyexpr_find_secret_span` helper,
    rejecting any decl that mentions `secret`. `eval` gets a
    `Declassify` arm that delegates to `inner.eval` (the
    declassification is type-level only; `Value` is qualifier-blind
    by B0 design, so no resume-point work is needed) — unreachable
    in B4.0 via the full pipeline because inference rejects first,
    but exists so eval is total over `ExprKind`. 3 placeholder tests
    in infer.rs build AST directly because the lexer/parser do not
    yet recognise the keywords; 5 `b40_*` tests in types.rs (added
    in the prior session, landed in this commit) pin display,
    idempotency, free-var recursion, close_rows recursion.

    **B4.0b** (63cd57b) adds `Token::Secret` and `Token::Declassify`
    to the lexer. Both globally reserved — cannot be used as
    identifiers, matching the policy applied to `rec` (B1.3) and the
    handler keywords (B3.0). Logos token attributes only; the
    existing Ident regex is tried after keyword matches. 6 lexer
    tests (standalone for each keyword, reserved-status in let-bind
    position for each, `secret Bytes` and `declassify(x)` full
    streams).

    **B4.0c** (0b6b2ce) adds the parser surface. `secret T` as a
    prefix on type atoms in `parse_ty_atom`, binding tighter than
    `->` per ADR 0008 D6: `secret Int -> Bool` parses as
    `(secret Int) -> Bool`, not `secret (Int -> Bool)`. This
    precedence falls out of recursing on `parse_ty_atom` (not
    `parse_ty_expr`); users wanting the arrow inside the secret
    write `secret (Int -> Bool)`. `declassify(e)` as
    atom-precedence expression in `parse_atom`, mandatory parens
    paralleling `do Label(arg)` and preserving the audit-point
    property called out in D5. `ParseError::DoubleSecret` rejects
    literal `secret secret T` (caught at the immediately-recursive
    call site) and `secret (secret T)` (caught after the paren
    collapse returns `TyExpr::Secret(_, span_with_parens)`). 9
    parser tests + 3 integration tests through `run()`. The
    integration trio confirms: full-pipeline `declassify(1)`
    rejected at inference with `SecretsNotYetSupported`;
    full-pipeline `effect ReadKey : Int -> secret Int ; 0` likewise;
    full-pipeline `effect F : Int -> secret secret Int ; 0` rejected
    at parse with `DoubleSecret` (proves the early parser rejection
    short-circuits ahead of the placeholder).

    Three structural decisions worth recording inline. (a) `secret`
    binds tighter than `->`. Considered the inverse (arrow tighter,
    so `secret Int -> Bool` parses as `secret (Int -> Bool)`),
    rejected because the former matches Rust's `&mut T -> U` reading
    that ADR 0008 D6 cites as precedent, and because `secret`
    binding loosely would force users writing single-arrow effect
    signatures `Int -> secret Bytes` to add redundant parens around
    the `secret Bytes` ret type. (b) `tyexpr_find_secret_span` walks
    the surface `TyExpr` rather than the lowered `Ty` because the
    surface tree carries human-source spans and is the right layer
    to point a diagnostic at. The walker is recursive structural —
    no row-handling needed because effect-decl signatures are pure
    `Int`/`Bool`/`Arrow`/`Secret` at this scope (no inline
    polymorphism in the surface yet). (c) Both `secret` in effect
    decls and `declassify` in expressions block at inference, not
    earlier. The parser produces well-formed AST for both surfaces;
    rejection happens at the inference layer specifically so that
    `infer_program` is the one boundary that decides "we're not
    ready to type secrets" — when B4.1 lands, the rejection sites
    delete and the typing rules replace them, no parser or lexer
    churn.

    Net +23 lib tests, +3 integration tests, no rewrites in place
    (the surface is wholly new). New `TypeError` variant:
    `SecretsNotYetSupported`. New `ParseError` variant:
    `DoubleSecret`. New `Token` variants: `Secret`, `Declassify`.
    New `Ty` variant: `Secret(Box<Ty>)` + `Ty::secret` smart
    constructor. New `ExprKind` variant: `Declassify { inner, span }`.
    New `TyExpr` variant: `Secret(Box<TyExpr>, Span)`. ADR 0008
    status flipped from PROPOSED to ACCEPTED with note that D1/D5/D6
    are confirmed by B4.0 and D2/D3/D4/D8 land in B4.1.

Test coverage as of B4.1: 222 tests (203 lib + 19 integration).
Net +13 lib / +2 integration from B4.0's 192 + 17. B4.1a landed
+5 lib (Declassify-on-non-secret-is-SecretFlow rename of the
B4.0a placeholder test, plus +5 fresh tests covering D2 direct,
Declassify positive via synthetic env, SecretFlow via catch-all,
Secret-Secret recursion sanity, Secret-Int-vs-Secret-Bool mismatch
on inners), 0 net integration (rename in place of the declassify
placeholder). B4.1b landed +8 lib (effect-decl-with-secret tests
rewritten from rejection to positive [+2 in place], +6 fresh
covering SecretBranch, SecretDivisor, D4 on three comparison shapes,
D4-Lt-Bool-rejects-on-inner) and +2 integration (password-verify
chain rejects with SecretBranch -- the HANDOVER §5.2 deliverable;
secret-in-arithmetic-is-SecretFlow). The effect-decl-rejection
integration test was rewritten as a positive type-check (net 0).
The placeholder variant `TypeError::SecretsNotYetSupported` is
gone. Clippy under `-D warnings` still has the 5 pre-existing
lints inherited from B3.2; chip filed to clean up separately.
See B.5 design decision 23.

23. (B4.1, ADR 0008 D2-D7) Secret typing landed in two commits.
    **B4.1a** (e760d57) is the foundation. Four new `TypeError`
    variants: `SecretFlow { from, to, span }` (the public/secret
    unification failure, raised by the catch-all arm of `unify`
    when either side is `Ty::Secret(_)` and the other is
    non-secret-non-Var), `SecretEscapesPolymorphism { var, span }`
    (D2 no-α-leak, raised by the Var arm of `unify` when a bare
    TyVar would bind to a secret type), `SecretBranch { span }`
    (declared; fires in B4.1b), `SecretDivisor { span }` (declared;
    fires in B4.1b). The `unify` Var arm gains a `matches!(t,
    Ty::Secret(_))` short-circuit; the catch-all Mismatch arm
    splits into SecretFlow (one side Secret) and Mismatch (neither
    side Secret). The Declassify infer arm replaces its B4.0a
    `SecretsNotYetSupported` placeholder with the real D5 rule:
    mint a fresh α, unify `t_inner` against `Ty::Secret(α)`, return
    `s.apply(α)` as the result type. Three failure cases handled
    naturally by the unify machinery: inner is concrete non-secret
    → SecretFlow via catch-all; inner is bare TyVar →
    SecretEscapesPolymorphism via Var arm; inner is Secret(t) →
    Secret-Secret arm binds α := t, result is t.

    **B4.1b** (52acc0a) is the CT-specific rejections + cleanup.
    `infer`'s If arm rejects `cond : Ty::Secret(_)` with
    SecretBranch before the Bool unify so the diagnostic is
    dedicated rather than the generic SecretFlow. `infer`'s BinOp
    arm rejects Div with a secret divisor with SecretDivisor
    (Sentinel-Mini has no Mod; ADR D3's `Div | Mod` is
    forward-looking). The Eq/Lt/Gt arms gain the D4 comparison
    rule: when either operand types as Secret(_), unwrap that side
    to its inner type, unify the other operand against the inner
    (the (Secret, Secret) arm of unify handles the both-secret case
    naturally via the same code path because both inners become
    non-Secret here), and produce Secret(Bool) as the result type.
    For Lt/Gt the inner type must additionally be Int per the
    existing binop_signature; that unify fires after the cross-side
    unify and produces a SecretFlow if the inner isn't Int (or
    Mismatch if neither side was secret).

    The `infer_program` `tyexpr_find_secret_span` walker and the
    associated SecretsNotYetSupported variant are deleted in B4.1b.
    ADR 0008 D7 (effect signatures may mention `secret`) is
    confirmed end-to-end: `effect ReadKey : Int -> secret Int ;`
    now type-checks, and `do ReadKey(0)` flows a Secret(Int) value
    into inference where D2/D3/D4 keep it safe.

    Two structural decisions worth recording inline. (a) The Var
    arm's D2 check is `matches!(t, Ty::Secret(_))` rather than a
    deeper walk that would refuse to bind any TyVar that contains a
    Secret inside a structural type (like `Fun(_, _, Secret(_))`).
    The shallow check is sufficient because Secret-inside-Fun is
    not a "bare TyVar binds to secret" violation -- the inner
    binding is fine if the function itself is concrete. Forward
    compatibility note: if a future ADR promotes the restriction
    to full qualifier polymorphism (shape (c)), the shallow check
    becomes a quantifier-restricted bind rule; the existing call
    sites already test it at the Var arm so the migration is
    contained.

    (b) The D4 comparison rule for Lt/Gt unifies the (potentially
    secret-unwrapped) lhs inner type against Int as a SECOND
    unification step, after the cross-side unify. This makes
    `Lt(Secret(Bool), Secret(Bool))` reject with Mismatch (the
    unwrapped Bool vs Int) rather than SecretFlow (which would be
    odd because both sides are secret-and-equal). The trade-off:
    diagnostic for Lt-on-secret-non-Int is "expected Int, found
    Bool" rather than something CT-specific. Acceptable because
    such a program is rare and the inner-type mismatch is what the
    user actually needs to fix.

    A positive end-to-end test for declassify-on-Secret cannot be
    written because ADR 0008 D5 intentionally omits a `classify`
    primitive (the Secret-introduction dual). The Secret-introducing
    path is restricted to typing of `do L(arg)` for effect-decls
    naming secret; resuming a continuation requires producing a
    Secret value, and the surface has no form that does so. Positive
    D5 coverage lives in lib's
    `b41a_declassify_on_secret_unwraps_the_inner_type` via a
    synthetic env. The integration tests file carries an inline note
    explaining the gap.

    D8 (generalization participation) was implicit by B4.0a's
    structural recursion of `collect_free_vars` and
    `collect_free_row_vars` into `Ty::Secret(_)`. No dedicated test
    added because no surface program exposes the generalization
    boundary with a polymorphic secret-typed value -- D2 prevents
    that by construction. Confirmed by code review of types.rs.

    Net +13 lib / +2 integration. New TypeError variants land:
    SecretFlow, SecretEscapesPolymorphism, SecretBranch,
    SecretDivisor. Variant removed: SecretsNotYetSupported.

Test coverage as of B4.2: 226 tests (203 lib + 23 integration).
B4.2 lands one commit covering the three HANDOVER §5.2 Phase B
validation demos plus README/STATE refresh. Net +4 integration
(supply-chain handler-mismatch, async-as-effect doubling-handler,
async-as-effect identity-handler, polished password-verify with the
CT-chain rationale block); 0 lib changes. The polished
password-verify is added next to the terse
`pipeline_password_verify_naive_rejects_with_secret_branch` rather
than replacing it -- the terse form is useful as a
regression-pin; the polished form is what HANDOVER §5.2 actually
calls for as the Phase B deliverable. Clippy clean under
`-D warnings`; no doctests. See B.5 design decision 24.

24. (B4.2, ADR 0008 D9 + HANDOVER §5.2) Phase B's three validation
    demos all live as integration tests under the `pipeline_b42_`
    prefix in `crates/sentinel-effects-proto/tests/integration.rs`.
    Implementation choices:

    (a) Tests, not example files. The crate has no `examples/`
    directory and remains library-only. Examples would have
    required `crate::run` and `Value` exposed as `cargo run`
    targets; the test runner already exposes both via the
    pipeline path. The trade-off accepted: examples are
    discoverable via `cargo run --example`, tests are
    discoverable via `cargo test pipeline_b42_`. The latter is
    sufficient given the prototype's "throwaway research artifact"
    framing in HANDOVER §5.

    (b) Supply-chain demo asserts `HandlerLabelNotInRow{label:
    "Storage"}`. The error diagnostic is from the handler's
    perspective ("I said I'd handle Storage but the body never
    raised it") rather than the user's ("the body raised Network
    without my permission"). Both readings are correct; the row
    machinery refuses the program either way. ADR 0007's row
    discipline doesn't carry user-intent metadata, so the
    diagnostic frame is mechanical. A comment in the test
    explains the supply-chain framing.

    (c) Async-as-effect demo is a pair of tests with the same
    `let app = fn(n) => do Tick(n) + 1 in ...` prefix; only the
    handler arm differs (`Tick(x, k) => k(x * 2)` vs
    `Tick(x, k) => k(x)`). The byte-identical-source assertion is
    pinned via test pairing, not via shared-string-constant
    machinery (which would require helper indirection and
    obscure the demo). The two tests assert different `Value::Int`
    results, which is the load-bearing observation.

    (d) Polished password-verify demo carries the full CT-chain
    rationale as a comment block above the test: the D7 effect
    signature, the D4 comparison rule producing `Secret<Bool>`,
    the D3 `SecretBranch` rejection of the surrounding `if`. The
    naive comment in the original B4.1b test is intentionally
    short; the polished version is the "publishable" form of the
    demo.

    (e) No featured `classify : T -> secret T` primitive added.
    The B4.2 task offered it as a design call (it would enable a
    positive end-to-end declassify test); rejected for B4.2 because
    Secret-introduction is exactly what ADR 0008 D5 deliberately
    omits to preserve the audit-point property. Adding `classify`
    would require its own ADR amendment justifying the exception
    and a strong-comment audit marker. Deferred indefinitely.

    Net +4 integration tests, 0 lib changes (no new variants, no
    surface changes; the demos are exercise programs over the
    existing surface). README updated to mark B1-B4 done, refresh
    the test count, and add a "What works today" paragraph for
    effects-proto with the `cargo test pipeline_b42_` invocation
    that runs the three demos. ADR 0008 D9 amendment updated to
    flip B4.2 from roadmap to landed. Phase B is now complete;
    Phase C (bootstrap compiler) per HANDOVER §6 is the next major
    phase.


### B.6 Known Limitations (intentional at B1)

- No effects. The whole reason this crate exists. (B2 onward — done.)
- `secret` qualifier and constant-time check: complete (B4.0 surface,
  B4.1 typing, B4.2 validation demos). D2 (no-α-leak), D3 (the four
  CT rejections), D4 (comparisons produce `Secret<Bool>`), D5
  (declassify typing) all wired. All three HANDOVER §5.2 Phase B
  validation deliverables exist as integration tests under the
  `pipeline_b42_` prefix. A positive end-to-end `declassify` test is
  intentionally absent because ADR 0008 D5 omits a `classify`
  primitive (audit-point property); positive D5 coverage lives in
  lib's `b41a_declassify_on_secret_unwraps_the_inner_type` via a
  synthetic env.
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

## Section C — bootstrap compiler (HANDOVER §6)

Phase C is the production Sentinel compiler in Rust per ADR 0009. C0
is the end-to-end pipeline for the smallest language subset (let,
arithmetic, if, function calls) compiling to LLVM with no type
system — everything i64 — to prove the pipeline shape works. Six
sub-phases C0.0-C0.5.

The Phase C compiler crates are populated in pipeline order across
C0-C5 per ADR 0009 D7. As of C0.0, sentinel-syntax has a lexer; the
remaining nine compiler crates (sentinel-ast, sentinel-resolve,
sentinel-types, sentinel-hir, sentinel-mir, sentinel-codegen,
sentinel-driver, sentinel-runtime, sentinel-lsp) remain 20-line
scaffold stubs.

### C.1 Phase Tracker

| Phase | Title                                                          | Status  | Commit  |
|-------|----------------------------------------------------------------|---------|---------|
| C0.0  | Tokens + lexer + tests/ui/ harness + 1 lex-error UI test       | Done    | 8f37381 |
| C0.1  | Hand-written parser + AST + `snc parse` subcommand             | Done    | 7e32e8c |
| C0.2  | LLVM codegen + `snc build` + first runnable binary + tests/pass/ | Done  | 0b07931 |
| C0.3  | let bindings + variable references (i64 everywhere)            | Done    | 80d2b6b |
| C0.4  | if/else + block expressions + `print` calls + first stdout     | Done    | baf68fc |
| C0.5  | fn definitions + main entry; **C0 go/no-go passes**            | Done    | 6ce8336 |
| C1.0a | sentinel-base crate: Salsa db trait + SourceFile input + Diagnostic accumulator | Done | 09dc8c3 |
| C1.0b | Wrap lex/parse as `#[salsa::tracked]` queries; driver uses SentinelDatabase | Done | 557cc60 |
| C1.0c | Codegen-salsa decision: defer until typed HIR (C1.2+); ADR 0011 D1 amended | Done | 8b58644 |
| C1.1.1 | Scaffold sentinel-resolve: VarId/FnId, parallel-tree resolved AST, resolve() + salsa query | Done | 438dd16 |
| C1.1.2 | Codegen consumes ResolvedProgram; driver chains resolve_query | Done | 9374edf |
| C1.2  | ADR 0012 (concrete C1 surface) + sentinel-types::check() real | Pending |         |
| C1.3+ | Multiple primitives, structs, nullability, arrays, generics    | Planned |         |

ADR 0010 (concrete C0 surface syntax) lands between C0.0 and C0.1
per ADR 0009 D8.

Test coverage as of C0.0: 13 sentinel-syntax tests (11 lexer + 1
smoke + 1 UI integration). The UI test runs
`tests/ui/lex_invalid_char.sentinel` (one-line `let x = @`) through
the lexer and snapshots the miette-formatted diagnostic at
`crates/sentinel-syntax/tests/snapshots/ui__ui_lex_invalid_char.snap`.

Test coverage as of C0.1: sentinel-syntax gains 23 parser unit
tests (six covering the precedence ladder and left-associativity
on both add and mul; three covering unary minus including the
unary-binds-tighter-than-mul rule; three covering span tracking
across full / parenthesized / unary expressions; seven covering
parse errors — unmatched open paren, unexpected close paren, EOF
after operator, EOF after unary, lex-error passthrough, int-lit
overflow, trailing garbage; plus an int_lit_zero edge case and a
nested-parens test). One new UI integration test
(`ui_parse_unbalanced_paren`, snapshotted at
`ui__ui_parse_unbalanced_paren.snap`) snapshots the
`unmatched_paren` diagnostic for the fixture `(1 + 2`. sentinel-ast
gains 6 Display tests (int literal, binary, nested precedence,
unary, plus the BinOp::symbol and UnaryOp::symbol helpers) on top
of its existing smoke. Workspace delta: +30 active tests.

Test coverage as of C0.2: sentinel-codegen lifts out of scaffold-
stub status with 1 new target-init probe (`target_init_does_not_panic`)
on top of its existing smoke. sentinel-driver gains 5 pass tests at
`crates/sentinel-driver/tests/pass.rs` (`pass_c02_arithmetic`,
`pass_c02_precedence`, `pass_c02_parens`, `pass_c02_unary`,
`pass_c02_division`) covering the four operators, precedence,
parens, and unary minus via the full pipeline. The runner uses
`CARGO_BIN_EXE_snc` to locate the snc binary that cargo builds
before integration tests run; per-fixture executables land in
`target/sentinel-pass/` (gitignored). Workspace delta: +6 active
tests (353 total).

Test coverage as of C0.3: sentinel-ast gains 5 Display tests
covering Var, StmtKind::Let, StmtKind::Expr, Program with empty
stmts (which prints just the tail — preserving C0.1/C0.2 output
verbatim), and Program with stmts (one stmt per line + tail).
sentinel-syntax lib gains 14 parser tests: 2 Var-in-expression
tests, 6 program-level happy paths (empty stmts, single let,
multiple lets, expr-statement followed by tail, let-uses-let, span
tracking on `let` covering keyword through `;`), and 6 program-
level error cases (empty input, only-let-no-tail, missing `;`,
missing `=`, missing identifier, bare `let`). sentinel-codegen
gains 4 lib tests: empty-stmts program, let program, undefined
variable rejection, redeclared variable rejection. sentinel-driver
gains 4 pass tests (`pass_c03_simple_let`,
`pass_c03_multiple_lets`, `pass_c03_let_uses_let`,
`pass_c03_expr_statement` — the last verifies that an expression-
statement is computed but its result is discarded). Workspace
delta: +27 active tests (380 total).

A UI snapshot for the `undefined_variable` codegen diagnostic was
deliberately deferred — the lib unit tests cover the error variants
and a dedicated UI runner for codegen errors lands when C0.4+
introduces more codegen-stage diagnostics worth coordinating
(unbound call target, arity mismatch, etc.).

Test coverage as of C0.4: sentinel-ast gains 7 new Display tests
covering block/if/call/expr_block and the call-zero/one/two-arg
variants. sentinel-syntax lib gains 20 new parser tests: 3 for
blocks (simple, with stmt, in arithmetic position as atom), 4 for
if/else (simple, else-if chain, with var condition, in parens
inside arithmetic), 6 for calls (zero/one/multi args, trailing
comma, in arithmetic, with complex arg), 1 for var-vs-call
disambiguation, 1 for a program with a print-statement, and 5
error cases (if missing else, if missing then-block, empty block,
call unclosed args, block unclosed after tail). sentinel-codegen
gains 6 new tests (if expression, block expression, call to
print, undefined function, arity mismatch too-many, arity
mismatch too-few). sentinel-runtime gains its first real
function (`sentinel_print`) with a smoke + return-zero test.
sentinel-driver gains 8 new pass-test fixtures (print_simple,
print_then_tail, if_true_branch, if_false_branch, if_with_var_cond,
else_if_chain, block_expression, if_with_print) — five assert
on exit codes, three assert on stdout content + exit. Workspace
delta: +43 active tests (423 total).

Test coverage as of C0.5: sentinel-ast gains 4 new FnDef-related
Display tests (display_program_one_main, display_program_two_fns,
display_fn_def_zero_params, display_fn_def_multi_params) and
retires 2 stmt+tail Program tests, net +2. sentinel-syntax lib
gains 14 new parser tests covering fn-def parsing (single fn,
fn with one/multi/trailing-comma params, multi-fn programs,
fn-def Display round-trip, span tracking) plus 6 fn-def error
cases (top-level not-fn, top-level bare expr, missing name, missing
parens, missing body, bad param name); a `parse_block_str` public
function is added so the existing C0.3-0.4 parse_program_* tests
keep working with brace-wrapped inputs. sentinel-codegen gains 1
net test (compile_main_with_int_lit, compile_main_with_let_program,
compile_rejects_missing_main, compile_rejects_redefined_function,
compile_rejects_user_redefining_print, compile_multi_fn_with_
forward_ref, compile_call_to_user_fn_arity_check — 7 new tests but
with restructuring of the C0.4 tests around the new main-required
shape, the net is +1). sentinel-driver gains 5 new pass-test
fixtures: c05_simple_fn (double + main), c05_multi_arg_fn (add),
c05_forward_ref (main calls fn defined after it), c05_call_chain
(quad = double(double(...))), and the C0 acceptance program
**c05_go_no_go** (the ADR 0010 appendix double + pick + main with
print). All 17 pre-C0.5 fixtures are mechanically rewrapped in
`fn main() { ... }` for the hard-break top-level shape change.
The UI fixture parse_unbalanced_paren rewrites to
`fn main() { (1 + 2 }` so it remains valid C0.5 program shape with
the embedded error. Workspace delta: +22 active tests (445 total).

Test coverage as of C1.0b: sentinel-syntax gains a new `query`
module hosting two `#[salsa::tracked]` queries and seven unit tests
covering positive lex / positive parse / diagnostic-accumulation on
lex error / diagnostic-accumulation on parse error / lex error
propagated through parse with lex stage preserved / cache stability
across reruns for both queries. sentinel-driver's bin gains a
concrete `SentinelDatabase` struct (Storage<Self> + salsa::Database
impl with no-op salsa_event + SentinelDb impl); `run_parse` and
`run_build` are refactored to instantiate the DB, set a SourceFile,
call `parse_query`, and collect diagnostics via the accumulator.
sentinel-ast types `Spanned<T>`, `Block`, `Param`, `FnDef`,
`Program`, `StmtKind`, `ExprKind` gain `#[derive(Hash)]`
(non-behavioral; required for salsa-friendliness of any future
tracked-struct fields, even though C1.0b avoids tracked structs by
going through the accumulator). Workspace delta: +7 active tests
(453 total: 90 syntax lib + 2 ui + 22 ast + 13 codegen + 22 pass +
2 runtime + 3 base + 5 stub + 69 broker + 226 effects-proto). All
22 C0 pass-test fixtures still run end-to-end through the new
query-based driver path; the C0 go/no-go program at
`tests/pass/c05_go_no_go.sentinel` still produces stdout `10\n`
exit 0. See C.3 design decisions 33-36.

### C.2 Crate Layout (C0.5)

    crates/sentinel-base/                 (C1.0a)
      Cargo.toml          deps: salsa, thiserror, tracing
      src/
        lib.rs            SourceFile (#[salsa::input] with `path`
                          and `text` fields); SentinelDb trait
                          (#[salsa::db], inherits salsa::Database);
                          Diagnostic accumulator (#[salsa::
                          accumulator]) with stage / severity /
                          code / message / span fields. Test-only
                          TestDb verifies the salsa machinery
                          (3 tests). Downstream pipeline crates
                          plug into SentinelDb at C1.0b (lex/parse)
                          and C1.0c (codegen).

    crates/sentinel-ast/
      Cargo.toml          deps: tracing, thiserror
      src/
        lib.rs            Span (= Range<usize>), Spanned<T>, BinOp
                          (Add|Sub|Mul|Div), UnaryOp (Neg), ExprKind
                          (IntLit | Var | Unary | Binary | Block | If
                          | Call), Expr = Spanned<ExprKind>; StmtKind
                          (Let { name, name_span, value } | Expr),
                          Stmt = Spanned<StmtKind>; Block { stmts,
                          tail, span }; Param { name, span }; FnDef
                          { name, name_span, params, body: Block,
                          span }; Program { fns: Vec<FnDef>, span }
                          (C0.5 top-level — was stmts+tail at
                          C0.3-0.4); Display impls for all (Program
                          prints fn-defs newline-separated, FnDef
                          prints `(fn name (params) body)`).
                          **C1.0b**: Spanned<T>, Block, Param, FnDef,
                          Program, StmtKind, ExprKind all derive
                          Hash; BinOp / UnaryOp already did. Required
                          for salsa-friendliness of any future
                          tracked-struct field, even though C1.0b
                          itself routes errors through the accumulator
                          and avoids tracked-struct fields.

    crates/sentinel-syntax/                (C1.0b adds query module)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path, C1.0b), logos, miette, salsa
                          (C1.0b), thiserror, tracing
                          dev-deps: insta
      src/
        lib.rs            module declarations + public re-exports
                          (lex, LexError, TokenKind from lexer;
                          parse, parse_expr, parse_block_str,
                          ParseError, Parser from parser;
                          lex_query, parse_query from query — C1.0b;
                          Program, Span, Spanned, Stmt, StmtKind
                          re-exported from sentinel-ast)
        lexer.rs          logos-based TokenKind (4 keywords + 11
                          punctuation kinds + Ident + IntLit; skip
                          patterns for whitespace and `//` line
                          comments); imports Spanned from
                          sentinel-ast; LexError (miette::Diagnostic);
                          pure lex() fn returning (tokens, errors)
        parser.rs         hand-written recursive descent. Three
                          pure entry points: parse(src) ->
                          Result<Program> at C0.5+ parses one or
                          more fn-defs (parse_program loops over
                          parse_fn_def, which eats `fn Ident
                          ( params? ) block`); parse_expr(src) ->
                          Result<Expr> retains the single-expression
                          contract for existing C0.1 tests + REPL;
                          parse_block_str(src) -> Result<Block>
                          parses a brace-wrapped block in isolation
                          (used by tests + future REPL/completion).
                          Internal: parse_block (for `{ stmt* tail
                          }` from `if` branches, atoms, and fn
                          bodies), parse_let_stmt (`let Ident =
                          expr ;`), parse_if (with else-if chain
                          via synthetic Block wrapping), parse_atom
                          dispatches IntLit / Ident-with-`(`-is-call
                          / bare-Ident-is-Var / `{`-is-Block /
                          `(`-is-paren. ParseError variants
                          unchanged since C0.3: Lex (transparent),
                          UnexpectedToken, UnexpectedEof,
                          UnmatchedParen, IntLitOverflow
        query.rs          (C1.0b) two `#[salsa::tracked]` queries:
                          lex_query(db, file) -> Vec<Spanned<TokenKind>>
                          (return_ref) and parse_query(db, file) ->
                          Option<Program> (return_ref). Each
                          accumulates `sentinel_base::Diagnostic`s
                          for errors via the salsa::Accumulator
                          trait. Private helpers
                          lex_error_to_diagnostic and
                          parse_error_to_diagnostic perform the
                          (stage, code, message, span) conversion;
                          ParseError::Lex forwards to the lex
                          converter so a lex-error-through-parse
                          still carries the lex stage/code. The
                          queries are independent — parse_query calls
                          `parse(src)` directly rather than depending
                          on lex_query, matching parse's existing
                          fail-fast-on-lex-error semantics. Test-only
                          TestDb mirrors the one in sentinel-base
                          (7 tests).
      tests/
        ui.rs             integration runner; shared themed-none
                          handler at 80 cols; ui_lex_invalid_char,
                          ui_parse_unbalanced_paren
        snapshots/
          ui__ui_lex_invalid_char.snap
          ui__ui_parse_unbalanced_paren.snap

    crates/sentinel-resolve/                (C1.1: populated)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path), sentinel-syntax (path), miette,
                          salsa, thiserror, tracing
      src/
        lib.rs            Stable identifiers: VarId(u32), FnId(u32),
                          plus the PRINT_FN_ID const (= FnId(0)).
                          FnSignature { id, name, name_span: Option<Span>,
                          arity, is_main, is_runtime }. Parallel-tree
                          resolved AST: ResolvedProgram { fns:
                          Vec<ResolvedFnDef>, fn_signatures: Vec<FnSignature>,
                          span }; ResolvedFnDef, ResolvedParam,
                          ResolvedBlock, ResolvedStmt(Kind),
                          ResolvedExpr(Kind) all mirror their AST
                          counterparts with Var/Call replaced by
                          IDs; binding sites retain their source
                          name for diagnostics + IR debug names.
                          ResolveError with 6 variants:
                          UndefinedVariable, RedeclaredVariable,
                          UndefinedFunction, ArityMismatch,
                          RedefinedFunction, MissingMain (moved from
                          CodegenError at C1.1.2). resolve(program:
                          &Program) -> Result<ResolvedProgram,
                          ResolveError> is the pure-function entry
                          point: pass 1 builds the fn table
                          (`print` as FnId(0), user fns following),
                          pass 2 resolves each fn body with a
                          per-fn vars HashMap. RHS of `let x = expr`
                          resolves BEFORE binding x, so `let x = x`
                          with no outer x is UndefinedVariable.
                          resolve_query(db, file) -> &Option<ResolvedProgram>
                          is the `#[salsa::tracked]` wrapper that
                          chains on sentinel_syntax::parse_query;
                          errors flow through the Diagnostic
                          accumulator with stage="resolve" and parse
                          errors propagate transitively. 21 tests
                          (positive paths + each error variant + 4
                          salsa query tests including parse-error
                          propagation and cache validation).

    crates/sentinel-codegen/                (C1.1.2 rewrite — consumes ResolvedProgram)
      Cargo.toml          deps: sentinel-ast (path), sentinel-resolve
                          (path, C1.1.2), inkwell (llvm18-0 feature,
                          workspace-pinned), miette, thiserror,
                          tracing
                          dev-deps: sentinel-syntax (for src-string
                          test driving via parse + resolve)
                          lints.rust: unsafe_code = "allow" (inkwell
                          uses unsafe internally for FFI)
      src/
        lib.rs            compile_to_object(program: &ResolvedProgram,
                          output_path) builds an LLVM module
                          containing all fns declared by the program.
                          **Two-pass**: pass 1 declares every fn by
                          iterating program.fn_signatures (the runtime
                          `print` maps to LLVM symbol `sentinel_print`
                          via signature.is_runtime; otherwise the
                          source name); pass 2 emits each user fn
                          body. `main` returns i32 (C ABI shape,
                          truncated from i64 body value) — gated on
                          signature.is_main; other fns return i64.
                          CodegenCtx<'ctx, 'a> threads &context +
                          builder + i64_type + HashMap<FnId,
                          FunctionValue> fns table + current_fn +
                          HashMap<VarId, PointerValue> vars map.
                          compile_fn resets vars + current_fn per fn,
                          allocates+stores each param by VarId, then
                          lowers the body. lower_call dispatches via
                          FnId; lower_expr's Var arm reads vars by
                          VarId. LLVM SSA debug names preserved via
                          find_var_name_in_block / _in_expr that walks
                          the current fn for the binding's source
                          name — purely for IR readability.
                          **CodegenError** keeps only LLVM-lowering
                          errors: VerifyFailed, TargetInit,
                          TargetMachine, WriteFailed, Builder. The
                          6 name-resolution variants migrated to
                          ResolveError at C1.1.2.

    crates/sentinel-runtime/
      Cargo.toml          deps: tracing, thiserror
                          [lib] crate-type = ["lib", "staticlib"]
                          (the staticlib output is libsentinel_
                          runtime.a, linked into Sentinel programs
                          by the driver via the system cc; the rlib
                          remains for Rust consumers)
      src/
        lib.rs            sentinel_print(i64) -> i64 via
                          `#[no_mangle] extern "C"` — writes the
                          i64 to stdout as ASCII decimal + newline
                          and returns 0 (the call expression's
                          value per ADR 0010 D11)

    crates/sentinel-driver/                (C1.1.2 chains resolve_query)
      Cargo.toml          deps: sentinel-base (path, C1.0b),
                          sentinel-codegen (path), sentinel-resolve
                          (path, C1.1.2), sentinel-syntax (path),
                          miette (with "fancy" feature), salsa
                          (C1.0b), thiserror, tracing
      src/
        main.rs           snc binary; subcommands `parse` and `build`
                          (C0.1+ shape; both lifted from Expr to
                          Program at C0.3). **C1.0b**: defines
                          `SentinelDatabase` (storage: salsa::Storage
                          <Self>) with `#[salsa::db]` impls for
                          `salsa::Database` (no-op salsa_event) and
                          `SentinelDb`. `run_parse` calls
                          sentinel_syntax::parse_query and pretty-
                          prints the AST. **C1.1.2**: `run_build`
                          chains sentinel_resolve::resolve_query
                          (which depends on parse_query in the
                          salsa graph), collects diagnostics via
                          resolve_query::accumulated::<Diagnostic>
                          — picks up parse-stage diagnostics
                          transitively — and feeds the resulting
                          &ResolvedProgram to
                          sentinel_codegen::compile_to_object.
                          Diagnostics render through miette::MietteDiagnostic
                          (constructed at runtime from
                          stage/code/message/span — drops per-variant
                          help text and label text; rough but
                          functional, refinement deferred). `build`
                          then invokes the system `cc` on the
                          emitted `.o` plus `libsentinel_runtime.a`
                          (found via current_exe().parent()) to
                          produce the executable. Output defaults
                          to <file_stem>; exit codes 0 / 1 / 2.
      tests/
        pass.rs           pass-test runner; reads workspace-root
                          tests/pass/*.sentinel; uses CARGO_BIN_EXE_snc
                          to locate the binary cargo built for the
                          integration tests; builds each fixture into
                          target/sentinel-pass/ and asserts on the
                          executable's exit code

    tests/                                (workspace root, ADR 0009 D5)
      ui/
        lex_invalid_char.sentinel         `let x = @` fixture
        parse_unbalanced_paren.sentinel   `(1 + 2` fixture
      pass/                               (all wrapped in `fn main() { ... }` at C0.5)
        c02_arithmetic.sentinel           `6 + 7` -> exit 13
        c02_precedence.sentinel           `1 + 2 * 3` -> exit 7
        c02_parens.sentinel               `(5 + 3) * 2 - 1` -> exit 15
        c02_unary.sentinel                `-(-5)` -> exit 5
        c02_division.sentinel             `12 / 3` -> exit 4
        c03_simple_let.sentinel           `let x = 5; x` -> exit 5
        c03_multiple_lets.sentinel        `let x = 3; let y = 4; x + y` -> exit 7
        c03_let_uses_let.sentinel         `let a = 2; let b = a * 3; b + 1` -> exit 7
        c03_expr_statement.sentinel       `let x = 1; x + 99; 5` -> exit 5
        c04_print_simple.sentinel         `print(42)` -> stdout "42\n", exit 0
        c04_print_then_tail.sentinel      let + 2x print + tail -> stdout "7\n14\n", exit 21
        c04_if_true_branch.sentinel       `if 1 { 42 } else { 99 }` -> exit 42
        c04_if_false_branch.sentinel      `if 0 { 42 } else { 99 }` -> exit 99
        c04_if_with_var_cond.sentinel     let x=5; if x { x*2 } else { 0 } -> exit 10
        c04_else_if_chain.sentinel        let x=0; if/else-if/else -> exit 3
        c04_block_expression.sentinel     `let r = { let y = 4; y + 1 }; r * 2` -> exit 10
        c04_if_with_print.sentinel        if/print -> stdout "100\n", exit 0
        c05_simple_fn.sentinel            double + main -> exit 14
        c05_multi_arg_fn.sentinel         add(5, 6) -> exit 11
        c05_forward_ref.sentinel          main calls triple defined later -> exit 12
        c05_call_chain.sentinel           quad(3) = double(double(3)) -> exit 12
        c05_go_no_go.sentinel             ADR 0010 appendix: double + pick + main
                                          with print -> stdout "10\n", exit 0

    .cargo/
      config.toml         workspace-local cargo config (C0.2): [env]
                          sets LLVM_SYS_180_PREFIX (non-forcing —
                          developers with the env in zshrc are
                          unaffected); target.'cfg(target_os =
                          "macos")' rustflags add /opt/homebrew/lib
                          and /usr/local/lib to the link search path
                          so the linker can find brew-installed
                          zstd/libxml2 that LLVM 18 references

Four scaffold-stub compiler crates remain at 20-line
`crate_name() + smoke` stubs per ADR 0009 D7. Updated population
schedule (sentinel-resolve populated at C1.1):

  - sentinel-types:    C1.2 (per ADR 0011 D5; arrives after
                       ADR 0012 (concrete C1 surface, annotation
                       grammar) lands)
  - sentinel-hir:      C1/C2
  - sentinel-mir:      C2
  - sentinel-lsp:      C5

### C.3 Design Decisions (C0)

ADR 0009 (D1-D8) is authoritative; in-source highlights:

1. Lexer uses `logos`. ADR 0009 D4 prescribes hand-written recursive
   descent for the parser only; lexers benefit from the regex-DFA
   payoff with no ergonomic cost.
2. `lex(src: &str) -> (Vec<Spanned<TokenKind>>, Vec<LexError>)` is
   a pure function per ADR 0009 D1a's "C0 pipeline stages are pure
   functions" discipline. No shared mutable state. No `&mut Cx`
   threading. Diagnostics accumulate via the return value.
3. No CST/AST split (ADR 0009 D4). The lexer's output is a flat
   `Vec<Spanned<TokenKind>>`; C0.1's parser will produce a direct
   AST enum.
4. Keywords (`let`, `fn`, `if`, `else`) lex via dedicated `#[token]`
   rules. logos's longest-match guarantees `letter` lexes as
   `Ident`, not `Let` + `ter`. See `lex_keyword_prefix_is_ident`.
5. Valid tokens still flow through when `LexError`s occur (the
   lexer collects errors rather than fail-fast). C0.1's parser
   decides whether to stop on lex errors or continue.
6. UI snapshot uses `GraphicalTheme::none()` + `width(80)` for
   host-independence. Terminal-width detection and ANSI colors
   would make snapshots host-dependent.
7. Workspace-root `tests/ui/` holds data files (HANDOVER §3.2); the
   integration runner lives in `crates/sentinel-syntax/tests/ui.rs`.
   insta snapshots stay at insta's default location (next to the
   runner). Centralizing snapshots under workspace-root
   `tests/snapshots/` is deferred until there's more than one
   runner crate to coordinate.
8. Parser is the pure function `parse(src: &str) -> Result<Expr,
   ParseError>` per ADR 0009 D1a. Lex errors block parsing and
   surface through a transparent `ParseError::Lex(LexError)` variant
   — the front end has a single error type for the driver to handle.
   Parse errors are fail-fast (one error per call); error recovery
   is deferred until parser ergonomics demand it.
9. `Span` and `Spanned<T>` live in `sentinel-ast` rather than
   `sentinel-syntax` because the AST is conceptually below syntax
   in the pipeline. The lexer's `Spanned<TokenKind>` and the parser's
   `Spanned<ExprKind>` are the same generic type. `sentinel-syntax`
   re-exports `Span` and `Spanned` for crates that consume tokens
   and AST nodes together.
10. Parens are syntactic only — they are not represented as a
    distinct AST node. `(1 + 2)` and `1 + 2` produce the same AST
    shape; the outer span on the parenthesized form covers the
    parens. C5+ LSP-style source-preserving formatting may revisit
    this if exact original-source round-trip becomes a requirement.
11. The driver uses miette's default (fancy color) `GraphicalReport
    Handler` for human-facing errors; UI tests use
    `GraphicalTheme::none()` + 80-col width in the test runner for
    host-independent snapshots. Two separate code paths, two
    separate concerns.
12. sentinel-codegen lowers `Expr` to an LLVM IR module via inkwell
    0.5 with the `llvm18-0` feature. The emitted module defines
    `main() -> i32` whose return value is the i64 expression value
    truncated to i32 — the temporary exit-code-is-the-answer
    convention. ADR 0010 D11's `print(x)` will replace it at C0.4
    when function calls land; the truncation goes away when stdout
    is the result channel.
13. Linking lives in the driver, not in sentinel-codegen. The
    driver's `build` subcommand invokes the system `cc` on the
    emitted `.o` to produce the executable. Linking is platform
    glue (linker flags, library search paths, dynamic loader
    conventions) rather than a compiler concern; sentinel-codegen
    stays focused on IR generation. The `cc` invocation will move
    behind a more controlled interface when cross-compilation is
    in scope (C5+).
14. `.cargo/config.toml` (workspace root) sets two things: (a)
    `LLVM_SYS_180_PREFIX` via cargo's non-forcing `[env]` so
    subprocess shells (CI, automation) see what the developer's
    interactive zshrc already provides; (b) target-conditional
    `rustflags` adding `/opt/homebrew/lib` and `/usr/local/lib`
    to the link search path so the linker finds brew-installed
    `zstd` and `libxml2` that LLVM 18 references but `llvm-sys`
    does not emit search paths for. Non-existent paths are
    silently ignored. This is a macOS-specific workaround; when
    Sentinel grows beyond macOS the configuration moves to a
    build script that probes `llvm-config --libdir`.
15. ADR 0009 D7 prescribed a `sentinel-types::check() -> Result<(),
    Diagnostic>` stub at C0.2 "so the pipeline shape is right when
    C1 fills it in." C0.2 deferred this to C0.3 (or possibly C1)
    because the stub adds no value at C0.2: arithmetic has no type
    semantics, the no-op `check()` would be threaded through the
    driver as `parse -> check (noop) -> codegen`, and the driver
    pipeline is already the right shape without it. C0.3 confirmed
    the deferral — let-binding scope tracking lives in codegen
    (see 20), not in a separate types pass. ADR 0009 status line
    records the deviation.
16. `ExprKind::Var(String)` stores the bound name as an owned
    `String` rather than an interned identifier. Interning is a C1
    concern: it lives in `sentinel-resolve` where name resolution
    formalises the symbol table. C0.3's `String` allocations are
    bounded by program size and disappear at codegen time, so the
    cost is acceptable.
17. Flat per-function scoping in C0.3. There is no notion of
    nested scope yet; redeclaring an existing name in the same
    function is `CodegenError::RedeclaredVariable` rather than
    shadowing. Block-scoped shadowing arrives with nested blocks
    at C0.4 (when `if`/`else` requires them). The choice was
    deliberate at the start of C0.3 — flat is simpler to
    implement and easier to diagnose; shadowing is a useful
    feature but not load-bearing at C0.
18. `StmtKind::Let { name, name_span, value }` carries the name's
    own span in addition to the wrapping `Stmt`'s full span. This
    is so redeclaration diagnostics can point at just the
    identifier rather than the whole `let x = expr;` statement.
    The pattern generalises: future statements with prominent
    sub-spans (struct definitions, function parameters) carry
    their own field-spans alongside the statement-level span.
19. `Program { stmts, tail, span }` is the AST top-level rather
    than an `ExprKind::Block(Vec<Stmt>, Box<Expr>)`. The Block-
    expression approach would have changed the C0.1/C0.2 parser
    tests (they would read the IntLit through a Block wrapper);
    keeping `Program` distinct lets `parse_expr(src)` keep its
    "single expression, no statements" contract that those tests
    rely on. Block arrives as an expression at C0.4 when `if`/
    `else` needs nested blocks; both representations coexist —
    Program at the top, Block inside expressions.
20. Codegen name resolution: the codegen pass maintains a
    `HashMap<String, PointerValue>` from variable names to LLVM
    alloca pointers. Each `let` enters the map; each `Var`
    reference reads from it. When `sentinel-resolve` lands at C1
    (per ADR 0009 D7), this map moves out of codegen and codegen
    becomes a pure structural lowering pass against a resolved
    AST. The C0.3 arrangement is a deliberate short-term
    architectural debt — STATE.md flags it in this list so the
    refactor at C1 is obvious.
21. `Block { stmts, tail, span }` and `Program { stmts, tail,
    span }` have the same shape. Both are AST struct types, and a
    Block can be promoted to an Expr via `ExprKind::Block(Box<Block>)`.
    Program is the top-level form (no surrounding braces in source;
    implicit body of the future `main` function); Block is the
    brace-wrapped nested form. Keeping them as distinct types lets
    a reader see at a glance which level of nesting a value
    represents, at the cost of one redundant struct. At C0.5 when
    `fn main() { … }` lands, Program may collapse into a list of
    fn-defs each containing a Block body.
22. `if`/`else` codegen uses an **alloca-based result slot** rather
    than LLVM `phi` nodes. The result alloca is created in the
    current insert position; both branches `store` the branch
    value; the merge block does `load` from the slot to produce
    the if-expression's value. At -O0 this is correct and easy to
    read in the IR; mem2reg promotes the alloca to phi when
    optimization is enabled. The C0.4 plan A pinned this choice up
    front.
23. `if` condition is `cond != 0` (C-style truthy per ADR 0010
    D9). The condition is computed as i64 and compared to zero via
    `IntPredicate::NE`. When C1 introduces `bool`, ADR 0010 D9's
    Revisit clause fires and the comparison goes away — the
    condition will be `bool`-typed by then.
24. `if` is positioned at the top of `parse_expr`, not inside the
    arithmetic precedence ladder. `if x { 1 } else { 2 } + 3` is
    a parse error ("expected end of input" after the if-expr);
    `(if x { 1 } else { 2 }) + 3` works because the parens accept
    any expression. The restriction parallels Rust's; revisited
    if a real program wants the looser form.
25. sentinel-runtime is built with `crate-type = ["lib",
    "staticlib"]` so cargo produces both `libsentinel_runtime.a`
    (linked into Sentinel programs by the system cc) and
    `libsentinel_runtime.rlib` (consumable from Rust if a future
    integration test wants to call the runtime directly). The
    driver locates the staticlib via
    `current_exe().parent().join("libsentinel_runtime.a")` because
    cargo puts the snc bin and the runtime in the same target
    directory — works for both `cargo run --bin snc` and
    `CARGO_BIN_EXE_snc`-driven integration tests.
26. `CodegenCtx<'ctx, 'a>` decouples the LLVM IR lifetime ('ctx,
    bound by Context) from the ctx struct's borrow lifetime ('a,
    bound by an inner scope in `compile_to_object`). The two
    lifetimes let the ctx be scoped to drop before `module.
    verify()` and `target_machine.write_to_file(&module, …)` run
    — the borrow checker can see that the ctx's borrows end at
    the inner block's `}` and so the later `module.verify()` is
    unaliased. C0.5 dropped the `&module` field from the ctx
    because pass 1 declares every function up-front, so pass 2
    never needs to mutate the module — it only emits IR through
    the builder against pre-existing FunctionValues.
27. C0.5 top-level shape is **`Vec<FnDef>`** with a mandatory
    `main` entry point — a hard break from the C0.3-0.4 implicit-
    main `stmt* tail_expr` form. The existing 17 C0.2-0.4 pass
    fixtures were mechanically rewrapped in `fn main() { ... }`.
    The hard break was chosen at the C0.5 start because clean
    shape going into C1 was worth the one-time fixture rewrite
    over preserving two top-level forms forever.
28. Codegen is **two-pass** per the C0.5 plan A. Pass 1 declares
    every function (including the runtime `print` mapped to
    `sentinel_print`); pass 2 emits each body. Forward references
    work because all signatures are in the module before any body
    is emitted. The cost is one extra walk of `program.fns`; the
    benefit is no defined-before-use constraint on user code.
29. `main` returns i32 while every other fn returns i64. This
    matches the C ABI's `main` signature so the system linker is
    happy with no extra glue. The i64 -> i32 truncation happens
    inside `compile_fn` only for `main`; other fns build a normal
    i64 return.
30. `print` is reserved: pre-declared in pass 1 as the runtime
    `sentinel_print` symbol, so a user-defined `fn print(x)` at
    pass-1 declaration time collides with the pre-declaration and
    surfaces as `CodegenError::RedefinedFunction`. The check is
    by name in the fns table, not a special-case in the parser.
31. Function parameters become per-fn allocas in the entry block:
    on `compile_fn` we clear `vars`, then for each param we
    allocate an i64 slot and `store` the incoming parameter value
    into it. The body then reads parameters via the same
    `vars.get` path as `let`-bindings — uniform treatment. C0.5
    arity check fires from `lower_call` via
    `fn_value.count_params()`; it covers both `print` (declared
    with one param) and user-defined fns uniformly.
32. `parse_block_str(src)` is a new C0.5 public entry point that
    parses a single brace-wrapped block. It's used by the
    parser's own tests (so the C0.3-0.4 stmt+tail tests can wrap
    their input in `{ ... }` and keep their assertions) and is
    available for any future REPL or LSP completion machinery
    that wants to parse just a block.

33. (C1.0b, ADR 0011 D1) The salsa retrofit lands lex and parse but
    not yet codegen. `parse_query` does NOT depend on `lex_query`;
    it calls the pure `parse(src)` entry point directly. Pros: the
    salsa-aware queries inherit `parse`'s existing fail-fast-on-
    lex-error semantics with zero divergence; lex errors flow
    through exactly one diagnostic path (via
    `parse_error_to_diagnostic(ParseError::Lex)`, which forwards to
    `lex_error_to_diagnostic` so the diagnostic carries the lex
    stage/code); no risk of double-emitting a lex diagnostic from
    both queries when the driver collects via parse_query's
    accumulator alone. Cons: parse_query re-lexes internally
    (wasted CPU; bounded by program size); no salsa cache benefit
    between lex and parse. C1.0c+ may revisit if codegen or
    sentinel-types want both tokens and AST in the same incremental-
    rebuild scope — a `parse_from_tokens(src, &tokens)` helper plus
    a parse_query that depends on lex_query is the obvious move,
    paired with peeking-the-accumulator from inside parse_query to
    avoid double-diagnostics (or accepting that minor cost).

34. (C1.0b) Errors flow through the `#[salsa::accumulator]`
    pattern rather than tracked-struct fields. The C1.0a session
    halted on `Vec<LexError> as tracked-struct field` because
    `miette::SourceSpan` doesn't derive Hash; the C1.0b resolution
    is that tracked-function return values carry ONLY the success
    payload (`Vec<Spanned<TokenKind>>`, `Option<Program>`) and
    errors get converted at the query boundary into
    `sentinel_base::Diagnostic`s — a Hash-friendly struct with
    `(stage, code, message, span: Range<usize>)` — and pushed via
    `Accumulator::accumulate(db)`. The conversion drops per-variant
    `#[diagnostic(help(...))]` text and per-`#[label(...)]` text
    that the lex/parse error enums carried; the driver renders
    using `miette::MietteDiagnostic` constructed at runtime, which
    produces a less ornamented but still source-pointed diagnostic.
    Refining the help/label preservation is a follow-up; the
    pipeline shape is the C1.0b deliverable, not diagnostic
    polish.

35. (C1.0b) Hash derives across sentinel-ast (Spanned<T>, Block,
    Param, FnDef, Program, StmtKind, ExprKind) land prophylactically.
    C1.0b itself routes errors through the accumulator and does
    NOT use tracked structs, so strictly speaking Hash isn't
    required for the retrofit to compile. But: (a) HANDOVER §0.2
    step 1 prescribed it based on the C1.0a investigation; (b)
    Hash is a strictly additive derive (no breakage); (c) future
    sub-phases that DO want to put AST nodes into tracked-struct
    fields (e.g., a `#[salsa::tracked]` Module struct in C1.1 with
    a resolved-program field) inherit Hash for free. Cost is one
    derive per type; benefit is "salsa will not surprise us at the
    next sub-phase."

36. (C1.0b) The concrete `SentinelDatabase` struct lives in
    sentinel-driver/src/main.rs, not in sentinel-base. ADR 0011 D1
    placed the cross-crate `SentinelDb` trait in sentinel-base
    deliberately so pipeline crates (sentinel-syntax,
    sentinel-codegen, future sentinel-resolve, sentinel-types) can
    depend on the trait without depending on the concrete database
    that wires every query. The driver is the assembly point; the
    concrete DB lives there. Tests inside individual crates (e.g.,
    `query.rs`'s tests in sentinel-syntax) declare their own
    `TestDb` rather than reaching into the driver — same pattern
    as `sentinel-base`'s test module. Repeated minimal `TestDb`
    boilerplate (~12 lines per crate) is acceptable; a shared
    test-util crate would invert the dep direction and bloat
    test-build times.

38. (C1.1.1, ADR 0011 D4) sentinel-resolve uses a **parallel-tree**
    representation rather than a side-table or generic-AST scheme.
    ADR 0011 D4 specifies that `ResolvedProgram` "is the input AST
    with name references replaced by stable identifiers." Three
    representations were considered: (a) the parallel-tree approach
    (chosen) where ResolvedProgram has its own ResolvedExprKind /
    ResolvedStmtKind etc. mirroring the AST shape; (b) a side-table
    approach where the original AST stays untouched and resolution
    state lives in HashMap<Span, ID> tables; (c) a generic-AST
    approach where ExprKind<R> takes a reference type R that
    instantiates to String pre-resolve and to VarId/FnId post-
    resolve. Reasoning: (a) wins on debuggability and ergonomics
    (each variant is concrete, no generics to chase through error
    messages or type signatures); (b) was rejected because span-
    keyed maps are fragile (duplicate spans break it; future
    macro-expanded code would too) and HashMap-of-pointers is
    ergonomically awful; (c) was rejected because the generics
    parameter propagates through every AST type's signature and
    inflates the surface that callers (codegen, future types pass,
    LSP) have to track. The cost of (a) is keeping two parallel
    type hierarchies in sync as the AST grows; the discipline is
    "every AST change at C1.3+ updates the resolved tree in the
    same commit," which matches the rhythm of the codebase already.

39. (C1.1.1) `let x = expr` resolves the RHS BEFORE binding the
    name. So `let x = x` with no outer `x` is UndefinedVariable,
    not a self-reference. This matches the C0 codegen's behavior
    (lower_expr on the RHS happens before vars.insert(name)) and
    keeps the language consistent with Rust on this point. The
    behavior is locked by the
    `let_x_equals_x_errors_when_outer_x_undefined` unit test.

40. (C1.1.2) Codegen preserves source names in LLVM SSA labels by
    walking the current fn's body for the binding's source name at
    each Var load. The walk lives in `find_var_name_in_block` /
    `find_var_name_in_expr`. This is **purely IR readability** —
    semantically the VarId is the load-bearing identifier;
    codegen never depends on the name for lookup. Cost: O(fn-body
    size) per Var reference at codegen time. Acceptable at C1 scale
    (no fn body is more than ~50 lines at C0); revisit if codegen
    becomes a profiling bottleneck. An alternative would be to
    pre-build a HashMap<VarId, &str> per fn at the start of
    compile_fn; the walk-on-each-load avoids the bookkeeping and
    is the simpler default until measurements demand otherwise.

37. (C1.0c, ADR 0011 D1 amendment) Codegen stays outside the salsa
    query graph through Phase C1.0. ADR 0011's original D1 sketch
    had "… through codegen" in the query list, suggesting
    `compile_to_object` would eventually become `#[salsa::tracked]`.
    C1.0c reconsiders and rejects that for now. Three options
    were weighed (see ADR 0011 D1 amendment for the full
    write-up); the chosen option (2: don't wrap codegen at all)
    is justified by three factors: (a) codegen gets rewritten at
    C1.2 against typed HIR anyway, so investing in a pre-types
    salsa wrapper amortises over weeks at most; (b) the LLVM
    `'ctx` lifetime woven through `Context`, `Module<'ctx>`,
    `Builder<'ctx>`, and `FunctionValue<'ctx>` doesn't trivially
    fit salsa's `'static`-ish query model — fitting it requires
    either bitcode-roundtripping or single-fn codegen, and the
    cost-benefit isn't favorable yet; (c) the C1.0b front-end
    retrofit is what LSP / `cargo check`-style tooling actually
    cares about, since those tools exit after types-but-not-
    codegen — codegen incremental rebuild has near-zero practical
    value at C0/C1 scale. The cost is a small piece of explicit
    architectural debt (driver does a direct function call from
    parse_query's Program output into non-salsa codegen). It is
    revisited automatically at C1.2 because the codegen rewrite
    for typed HIR will touch the call site. ADR 0009 D1a's pure-
    function discipline preserved through C0+C1.0 keeps the
    retrofit mechanical whenever we do choose to do it. No code
    change at C1.0c; this is a pure docs commit capturing the
    architectural decision.

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
  - sentinel-effects-proto: 226 tests (203 lib + 23 integration) + 0 doctests
  - sentinel-syntax:        92 tests (90 lib + 2 UI integration) + 0 doctests
                            (lib = 83 lexer/parser + 7 query at C1.0b)
  - sentinel-ast:           21 tests (1 smoke + 20 Display) + 0 doctests
  - sentinel-codegen:       8 tests (1 smoke + 1 target init + 6 positive compile)
                            + 0 doctests — name-resolution rejection tests
                            migrated to sentinel-resolve at C1.1.2
  - sentinel-resolve:       21 tests (positive paths + 6 error variants
                            + 4 salsa query smoke incl. parse-stage propagation
                            + cache validation) + 0 doctests
  - sentinel-driver:        22 pass integration tests + 0 doctests
  - sentinel-runtime:       2 tests (smoke + sentinel_print_returns_zero) + 0 doctests
  - sentinel-base:          3 tests (salsa query runs/caches + source file accessors) + 0 doctests
  - other compiler crates:  1 scaffold smoke test each, 0 doctests
                            (sentinel-types, -hir, -mir, -lsp)

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
