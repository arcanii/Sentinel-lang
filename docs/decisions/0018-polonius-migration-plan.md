# ADR 0018: Polonius migration plan — from lexical to flow-sensitive borrow checking

Status: PROPOSED — to flip to ACCEPTED-WITH-AMENDMENTS once a
concrete migration sub-phase opens. This ADR records the *plan*;
no code changes ship with it. It satisfies ADR 0017 D6's
"the phase ends with both lexical + a documented Polonius
migration plan" commitment.

Date: 2026-05-28
Related: 0017 (Phase C2 kickoff — lexical-first call in D6 is what
this ADR plans the migration off of), 0011 (Phase C1 kickoff —
the salsa query graph + parallel-tree pattern that the migration
preserves), HANDOVER §6.2 (names Polonius as the long-term
borrow-checker target)

## Context

ADR 0017 D6 committed to a **lexical** borrow checker for the
C2 minimum-viable surface. A borrow's lifetime is "from creation
to the end of the enclosing block." This is what shipped across
C2.1 (shared-only), C2.2 (`&mut` + XOR), C2.3 (move semantics),
and C2.4 (RAII / drop). The implementation lives in
`crates/sentinel-borrow-check/src/lib.rs` — ~1500 LOC including
tests — and runs as a salsa-tracked query between `check_query`
and `codegen`:

    parse_query → resolve_query → check_query → borrow_check_query → codegen

Per-fn state lives in `FnCtx`:

  - `places: HashMap<VarId, PlaceState>` — per-source-binding
    active borrow set with `Vec<BorrowInstance>` of shared loans
    plus `Option<BorrowInstance>` for the exclusive loan.
  - `moved: HashMap<VarId, MoveState>` — `Live` / `Moved { at_span }`
    per binding, with snapshot/restore around if-else for branch-
    aware merging.
  - `scope_stack: Vec<ScopeFrame>` — push on `{`, pop on `}`;
    rooted borrows die when their declaring scope pops.
  - `moved_sources_union: BTreeSet<VarId>` — cross-branch union
    feeding the `DropPlan` returned to codegen.

This is "wrong but useful" by construction. The lexical formulation
rejects programs that a flow-sensitive checker (NLL / Polonius)
would accept. The canonical case:

    let mut v: i64 = 5;
    let r: &i64 = &v;
    let snapshot: i64 = *r;   // last use of r
    v = 10;                   // ERROR (lexical): write while &v
                              //   still active; r is "alive" to
                              //   the end of its enclosing block
                              // OK (flow-sensitive): r's last use
                              //   was the previous line; the loan
                              //   is dead here

Rust pre-2018 had exactly this behavior. NLL (2018) introduced
flow-sensitive analysis; Polonius is the formal-dataflow reformulation
that subsumes NLL with cleaner soundness arguments.

Sentinel's bootstrap-stage workload — small programs, recursion
over scalar / heap-backed types, no closures yet — typically
satisfies the lexical rules. Rejected programs all have a polite
local workaround (introduce a narrower scope; copy the value;
bind by value instead of borrowing). Polonius would relax the
checker without changing the diagnostic surface for the programs
the lexical checker accepts.

## Decision

Six D-numbered sub-decisions covering trigger (D1), preserved
surface (D2), the polonius-engine adoption (D3), required
representation changes (D4), the migration sub-phase shape (D5),
and out-of-scope items (D6).

### D1. Migration trigger: friction, not principle.

The lexical checker is the long-term target ONLY if no real
Sentinel program ever needs flow-sensitive precision. That's
empirically false at any non-trivial scale: any codebase that
uses `&mut` heavily will hit the "borrow lives past last use"
case routinely.

The trigger for opening a migration sub-phase is **any of**:

  - **User-reported friction**. A program where the user clearly
    expects the borrow checker to accept and a polite local
    workaround changes the program's shape (not just adds a
    scope brace).
  - **Phase D rest-of-language work demands it**. Closures,
    iterators, async, and Sentinel-Mini's effect handlers all
    have known interactions with flow-sensitive borrow checking
    that lexical can't model. Whichever lands first becomes the
    forcing function.
  - **A clear "the lexical rules surface as confusing" pattern
    in C2 diagnostics**. Tracked via the C2.5(c) corner-case
    notes (see Appendix below).

No migration sub-phase opens before then. The cost of Polonius
adoption is real (~4-6 sessions per ADR 0017 D6 estimate); the
benefit is bounded by how often the lexical rule rejects
programs we care about. Empirical signal beats speculation.

### D2. Preserved surface.

The Polonius migration **does not change**:

  - The `Type::Ref(RefId)` representation. Refs are still interned;
    `Type: Copy` is preserved.
  - The `BorrowError` variants (eight at C2.4 close —
    `OutlivesSource`, `ReturnsLocalRef`, `MutableBorrowOfShared`,
    `SharedBorrowOfMutable`, `BorrowConflict`, `WriteWhileBorrowed`,
    `ReadWhileMutBorrowed`, `UseAfterMove`). Variant names and
    semantic intent stay. **Spans may become more precise** — a
    flow-sensitive checker can point to "the last use of the
    borrow that conflicts" rather than "somewhere in this
    enclosing block."
  - The `DropPlan` artifact handed to codegen. Flow-sensitive
    move tracking is strictly more precise than the per-binding
    branch-merge that C2.3 ships; the union-of-moved-sources
    summary surface stays the same.
  - The salsa pipeline shape. `borrow_check_query` still consumes
    `TypedProgram` and returns `(DropPlan, Vec<BorrowError>)`.
    Polonius is an *internal* reformulation of the analysis.
  - Codegen — entirely unaffected. The `DropPlan` consumer doesn't
    care how the moved-source set was computed.

### D3. Adopt polonius-engine; don't reinvent.

Two implementation options for the dataflow:

  - **Build from scratch** — write the origin / loan tracking
    against Sentinel's CFG. ~600-1000 LOC plus a soundness review
    we don't have the expertise to self-grade.
  - **Adopt `polonius-engine`** — the same crate Rust's compiler
    uses to drive its Polonius prototype. Stable since ~2020,
    embedded as a library, takes facts in / produces analysis
    results out. The fact-schema is what the embedder fills in.

**Decision: adopt `polonius-engine`.** The crate is unmaintained
since 2024-09 (last `polonius-engine` 0.13.0 release on
crates.io) but is feature-complete for the NLL-equivalent
analysis Sentinel needs. The Datafrog-based engine doesn't
require runtime nightly. If the crate goes stale enough to need
a fork, that's a single internal vendor copy — the fact-schema is
small (~10 relations) and the inference rules are documented in
the Polonius book (rust-lang.github.io/polonius).

We are NOT betting on Polonius shipping into rustc proper as the
default borrow checker. Sentinel uses the engine as a library for
its own analysis; rustc's adoption status is irrelevant to ours.

If `polonius-engine` becomes truly unmaintained (no security
fixes for a stale dependency tree, fact-schema needs evolve past
what 0.13 supports), the fallback is the **build-from-scratch**
option — keep the lexical checker, ship NLL-equivalent precision
incrementally without the Datafrog dependency.

### D4. Representation changes the migration requires.

The lexical checker tracks state per-binding-VarId. Polonius
tracks state per-(origin, point-in-CFG). The migration introduces:

  - **A control-flow graph for each fn body**. The TypedAST is
    structured (block / if / call); flattening to a CFG is
    mechanical. Each statement / sub-expression boundary becomes a
    `Point`. Already-named in polonius-engine as `Point`.
  - **Origins**. Each `&` / `&mut` expression introduces an origin
    variable. Origins flow through assignments + calls + returns;
    the lexical "borrow source" enum
    (`Local` / `Incoming` / `LocalAnonymous`) is replaced by
    origin variables with subset constraints.
  - **Loans**. Each `&x` / `&mut x` is a loan keyed by
    (place, kind). Existing `PlaceState` becomes Polonius's
    `loan_killed_at` + `subset` relations.
  - **Liveness**. Computed by a separate dataflow pass; the input
    to Polonius's "loan in scope at point" derivation.

The C2.3 `MoveState` survives mostly unchanged — Polonius doesn't
do move tracking directly; move semantics is layered on top of
the borrow analysis via "place initialized at point" relations.
We compute those from the existing move-state pass; if a place is
"definitely moved" at a point and is read at that point, surface
`UseAfterMove`.

### D5. Migration sub-phase shape.

Three-step rollout, each independently shippable:

  - **C3.x.a — Fact generator + Polonius input**. Add a new
    `polonius_facts(program) -> AllFacts` pass that builds the
    fact set from `TypedProgram`. Don't wire it into the
    pipeline yet; produce facts in parallel, log them in a debug
    mode, compare against the lexical analysis's results on a
    fixture corpus. Goal: confidence that the fact generator
    captures Sentinel's semantics correctly. ~400-600 LOC.
  - **C3.x.b — Polonius output → BorrowError lowering**. Take
    polonius-engine's `Output<...>` (errors per origin per point)
    and lower to existing `BorrowError` variants with precise
    spans from the fact generator's `Point` ↔ `Span` map.
    Ship in `borrow_check_query` behind a feature flag
    (`SENTINEL_USE_POLONIUS=1`). Lexical stays default. ~200-400
    LOC.
  - **C3.x.c — Flip Polonius default + remove lexical**. After
    confidence on the fixture corpus, flip the default. Keep
    the lexical implementation as a runtime-toggleable fallback
    for one release. Delete on the release after. ~100 LOC of
    deletions.

ADR 0017 D6's "4-6 sessions for Polonius" estimate covers the
sum of these three steps. The three-step shape lets us bail at
step .a if the fact generator surfaces unanticipated complexity
(e.g., generic fns + traits require additional fact relations).

### D6. Out of scope at the migration ADR.

The following remain deferred to *later* ADRs even after the
Polonius migration lands:

  - **Field-precise borrows** (`&p.x` and `&mut p.y` non-
    conflicting). Polonius supports field-precise places, but the
    Sentinel fact generator can start with binding-precise places
    (matching C2's surface) and refine later. Tracked as a follow-
    on ADR.
  - **First-class refs** via `'esc` binding + named regions.
    These are ADR 0017 D7 territory; Polonius's origin system can
    model them but the surface design is independent. The named-
    region ADR (call it 0019 or 0020) ships alongside or before
    this migration.
  - **`unsafe` blocks + raw pointers + interior mutability**. C5
    territory per ADR 0017 D12.
  - **Trait-driven borrow rules** (e.g., `Send` / `Sync`-style
    auto-trait analysis). C4 traits + then a follow-on.
  - **Closures**. Captures introduce origin constraints between
    the capturing closure and the captured place; Polonius
    handles this but the Sentinel surface for closures isn't
    yet designed (Phase D / E territory).

## Reasoning

**Why migrate at all.** The lexical checker rejects valid
programs. Every rejection is a workaround the user has to write
explicitly. Workarounds are noise that obscures the program's
intent. Flow-sensitive precision is the same property Rust's
2018 NLL gave Rust users; Sentinel should match.

**Why not migrate now.** ADR 0017 D6's reasoning still stands.
Lexical ships in ~1500 LOC + a 5-session arc; Polonius adds 4-6
sessions of work plus a soundness review. The lexical formulation
is sufficient for the bootstrap-stage workload. Open a migration
sub-phase when D1's trigger fires, not before.

**Why polonius-engine over rolling our own.** Sentinel's
infrastructure investment compounds best when we adopt proven
external dependencies for the load-bearing pieces (LLVM for
codegen, logos for lexing, miette for diagnostics, salsa for
queries). polonius-engine is the proven external dep for
flow-sensitive borrow checking. Rolling our own would burn
~600-1000 LOC + soundness review for nothing the existing crate
doesn't already give us. The maintenance risk (D3's "unmaintained
since 2024-09") is real but bounded — the fact schema is small
enough to vendor in a fork if needed.

**Why a three-step rollout.** Step .a is the unknown — building
the fact generator is where unanticipated coupling can surface.
Step .b is mechanical given .a. Step .c is risk-managed because
the feature flag lets us roll back. If .a surfaces a deal-breaker
(e.g., Sentinel's effect-system interactions need fact relations
polonius-engine doesn't model), we know before committing to .b.

## Consequences

### Positive

- Programs that the user expects to compile actually compile.
  Eliminates the "introduce-a-scope-brace" workaround as the
  default fix for borrow-check rejections.
- Diagnostic quality improves. "Borrow conflicts with last use
  at line N" beats "borrow conflicts somewhere in this enclosing
  block."
- The surface (BorrowError variants, DropPlan, pipeline shape)
  is preserved. Codegen + downstream tooling unchanged.
- Aligns with HANDOVER §6.2's long-term plan.
- Sets up first-class refs + named regions. Origin variables are
  the substrate both work need.

### Negative

- New ~600-1000 LOC dependency on polonius-engine + ~600-1000
  LOC of fact-generator code. Total ~1200-2000 LOC delta.
- polonius-engine's maintenance status is a real risk. If the
  crate atrophies past 0.13, we vendor a fork — manageable but
  not free.
- Fact-generator code is harder to debug than per-binding state.
  When Polonius rejects a program, tracing why means tracing
  through origin / loan / subset relations rather than a
  per-binding state machine.
- Edge cases in the fact schema (e.g., generic fn signature
  origins, struct field borrows) may surface late.

### Neutral

- The feature flag from step .b lets us ship gradually. No big-
  bang switchover.
- Lexical implementation removal is a future ADR's call. It can
  live alongside Polonius indefinitely if there's value in
  cross-checking results (e.g., differential testing for the
  fact generator).

## Alternatives considered

- **Build a custom NLL-style flow analysis from scratch**
  (skip polonius-engine). Rejected per D3. We don't have a
  soundness-proof team; adopting the proven library is cheaper
  and lower-risk.

- **Stay lexical forever**. Rejected per the migration trigger
  (D1) — flow-sensitive precision is empirically necessary at
  non-trivial scale.

- **Build a Polonius-equivalent over Datalog without using
  polonius-engine**. Rejected — re-implements the same engine
  with no benefit. Datafrog is the standard runtime for this
  shape of analysis.

- **Migrate to NLL (rustc's pre-Polonius shipped implementation)
  as a halfway point**. Rejected. NLL → Polonius is its own
  refactor; lexical → Polonius is one refactor with the same
  endpoint. Two migrations cost more than one.

- **Defer the migration plan ADR itself until the migration
  starts**. Rejected — ADR 0017 D6 commits to documenting the
  plan at C2.5 close, and the plan is small enough to write
  once + reference forever. Writing it now while the lexical
  implementation is fresh is the right time.

## Revisit

- **D1 trigger** — revisit whenever a Sentinel program is reported
  as "should compile but doesn't" with a non-trivial workaround.
  If the rejection is a known lexical case, that's a vote toward
  opening the migration sub-phase.
- **D3 polonius-engine adoption** — revisit if the crate's
  maintenance status changes. Vendor-fork or roll-own become
  alternatives.
- **D5 sub-phase shape** — revisit at the start of step .a. If
  the fact generator's coupling exceeds the estimate, the three-
  step rollout may collapse to one or stretch to four.

## Appendix: known lexical over-rejections at C2.5

For posterity — the cases the lexical checker rejects that flow-
sensitive would accept. C2.5(c)'s corner-case scan filled this
in; any new patterns the C2.x sub-phases surface should be added
here.

1. **Borrow lives past last use** (the canonical NLL case).

       let mut x: i64 = 5;
       let r: &i64 = &x;
       let snapshot: i64 = *r;    // last use of r
       x = 10;                    // rejected: write while &x
                                  //   active (lexical: r alive
                                  //   to enclosing block end)

   Workaround: wrap the borrow + use in an inner block.

2. **Conditional move with one branch consuming, other
   reborrowing**. Not yet a documented case at C2.5 (the C2.3
   branch-merge handles the common pattern); flagged as a
   suspected friction point.

3. **Long-lived shared borrows blocking unrelated mutations**.
   Same shape as #1; arises when a shared borrow at the top of
   a fn blocks any later mutation of the source.

This appendix is non-exhaustive. The C2.5(c) corner-case scan
notes any additional cases.
