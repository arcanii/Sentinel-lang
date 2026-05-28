# ADR 0021: Phase C4 kickoff — classes, traits, delegation, structured concurrency

Status: PROPOSED — to flip to ACCEPTED (or ACCEPTED-WITH-
AMENDMENTS) as Phase C4's sub-phases land. This ADR opens
Phase C4 per HANDOVER §6.2's month-12-15 budget: "classes,
traits with named implementations, delegation, structured
concurrency, actors. Most of this is 'reasonable language
design plumbing' rather than novel work, but the volume is
significant." Phase C3 closed at C3.7 with ADR 0020 ACCEPTED-
WITH-AMENDMENTS; the handler runtime is the foundation for
async-as-effect (deferred at ADR 0019 D9) which C4 picks up.

Date: 2026-05-28
Related:
  - **0011** (Phase C1 kickoff — ACCEPTED): the parallel-tree
    pattern + interner-table-with-Copy-Type invariant + ADR-
    first norm carry forward. Witness-table generics from
    C1.7 underlie trait dispatch (D5).
  - **0017** (Phase C2 kickoff — ACCEPTED-WITH-AMENDMENTS):
    refs + mutability + lexical borrow check + RAII drop set
    the foundation for `self: &Self` / `self: &mut Self`
    method dispatch (D2). DropPlan integrates with init's
    definite-assignment check (D3).
  - **0019** (Phase C3 kickoff — ACCEPTED-WITH-AMENDMENTS):
    D9 deferred async-as-effect indefinitely. C4 reopens this
    via the C3.4-C3.7 handler-runtime foundation (D9 here).
  - **0020** (handler runtime — ACCEPTED-WITH-AMENDMENTS): the
    free-monad lowering + deep-handler one-shot continuations
    are the runtime substrate for `spawn`/`await` via async-
    as-effect (D9).
  - **0016** (C1.7 generics — ACCEPTED): monomorphisation +
    interned generic-instance tables transfer to trait dispatch
    when traits are monomorphic-bound (D5).
  - **0013** (C1.4 struct syntax — ACCEPTED): the recursive-
    struct cycle detector + nominal type equality + field-
    access lowering carry into class methods (D2).
  - **0007** (Phase B effect handlers — ACCEPTED): the async-
    as-effect formulation Phase B validated maps onto C4's D9
    structured-concurrency surface.

## Context

Phase C3 closed at C3.7 with the full memory-safety + secret-
typing + effect-system trifecta. The bootstrap language now
covers:

  - Primitive scalars + nominal structs + nullable + heap-
    backed arrays + witness-table generics (C1).
  - References + mutability + lexical borrow check + RAII
    drop (C2; ADR 0018's Polonius migration plan PROPOSED).
  - Effect rows + secret typing + handler runtime with deep-
    handler one-shot continuations + nested handles + non-
    identity return arms (C3).

The remaining HANDOVER §6.2 §6 surface for the bootstrap-
language target:

  - **Classes**: structs plus methods plus init plus trait
    impls — no inheritance. The "class" keyword groups them.
  - **Methods**: `fn name(self: &Self, ...) -> T { ... }`
    inside class blocks. Method dispatch is by static type
    unless via a `dyn Trait`.
  - **Initializers**: `init(args)` constructors with definite-
    assignment check — every field must be assigned before
    init returns. No half-constructed objects.
  - **Traits**: declared with `trait Name { ... }`; named
    implementations `impl X as ImplName for Type { ... }`.
    Two impls of the same trait for the same type coexist by
    name; call sites disambiguate.
  - **Delegation**: `delegate inner: T to Trait;` inside a
    class body — methods of `Trait` forward to `inner`'s
    implementation. Replaces inheritance.
  - **Structured concurrency**: `scope concurrent { ... }`
    blocks, `spawn fn_call(...)`, `await handle`. Tasks
    cannot outlive their scope; cancellation flows down.
    Async-as-effect per ADR 0019 D9.
  - **Actors**: declared message protocols + mailbox routing.
    Cross-process actors via the broker (Phase A).

This ADR scopes Phase C4 with deliberately reduced surface
relative to the full HANDOVER §6.2 vision: **actors are
deferred to Phase C5** (D10), and **cross-process / GPU /
`@numa` location qualifiers are out of scope** (D12). Phase
C4's core deliverable is the *single-process* trait + class
+ structured-concurrency surface.

The C3.7 lexer state going into ADR 0021:

  - **Keywords**: `let, fn, if, else, true, false, struct,
    null, mut, effect, secret, declassify, handle, with,
    perform, return`.
  - **Punctuation**: `+ - * / = == != < <= > >= && || ! ? : .
    , ; ( ) { } [ ] -> => & |`.
  - The Phase B prototype used `class`, `trait`, `impl`, etc.;
    none reserved at C3.7. C4.0 lexer adds them per D11.

## Decision

Fourteen D-numbered sub-decisions covering classes (D1-D3),
traits (D4-D7), delegation (D6 subset), structured concurrency
(D8-D9), out-of-scope items (D10, D12), lexer (D11), phase-go
(D13), and the sub-phase split (D14).

### D1. `class Name { ... }` syntax.

Sentinel classes are syntactic sugar over `struct` + methods
+ trait impls. The class block contains:

    class Name {
        let field_1: T1;
        let field_2: T2;
        pub init(args) { ... }
        pub fn method(self: &Self, ...) -> R { ... }
        impl as Trait { fn op(...) { ... } }
        delegate inner: U to OtherTrait;
    }

The desugaring is straightforward — a class declaration
generates an underlying `struct Name { fields }` plus a
collection of methods + trait impls keyed against `Name`.
Visibility (`pub`) becomes part of the export surface. Existing
`struct` declarations continue to work; `class` is the
canonical form for richer types.

**Decision: ship `class` as the user-facing keyword + desugar
to existing struct machinery + new method/impl tables.**
Rationale: classes are how users will primarily declare types
at and beyond C4; preserving struct keeps the C1.4 surface
working without rewrite.

### D2. Methods: `fn name(self: &Self, ...) -> R` inside class blocks.

Method declarations look like fn declarations except:
- They live inside a class body.
- The first param is `self: &Self` (shared borrow) or `self:
  &mut Self` (exclusive borrow) per ADR 0017 D6's borrow rules.
- The receiver type `&Self` / `&mut Self` is a sugar for
  `&Name` / `&mut Name` resolved at codegen.

Method call syntax: `obj.method(args)` or `Class::method(&obj,
args)` (UFCS form). The dot form auto-references — `obj.method(args)`
where method takes `&Self` implicitly takes `&obj` per usual
borrow rules.

**Decision: methods are first-class fns under a class-mangled
namespace + the dot-syntax desugars to a static call.**
Dispatch is statically resolved against the call's receiver
type (until `dyn Trait` arrives at D5). Self-recursive
methods + mutual recursion within a class work the same way
as free fns.

### D3. `init` constructor with definite-assignment check.

Construction goes through `init(args) { ... }`:

    class Point {
        let x: f64;
        let y: f64;
        pub init(x: f64, y: f64) {
            self.x = x;
            self.y = y;
        }
    }

Inside init, `self` is the *partially-initialised* class —
fields may be unassigned. The body must definitely-assign
every field before return on every path; type-check rejects
init bodies that skip a field or read a field before assign.
The "definitely assigned on every branch" rule mirrors Rust's
local-init-check + Java's definite-assignment for ctors.

`init` calls inside class methods use `Name::init(args)` form
(or a sugar like `new Name(args)` — TBD at C4.1). The call
returns a value of type `Name`.

**Decision: `init` is mandatory for classes with any field;
type-check enforces definite-assignment; the borrow checker
treats `self` as a special LocalAnonymous source until init
returns.** No partial-construction observation. Construction
goes through `init` only — direct struct-lit syntax (`Name {
field: value }` per C1.4) is REPLACED by `Name::init(args)`
form for classes; existing structs still allow struct-lit
syntax.

### D4. `trait Name { ... }` declarations + method signatures.

Trait declarations:

    trait Writer {
        fn write(self: &mut Self, data: &[u8]) -> i64;
        fn flush(self: &mut Self) -> i64;
    }

Methods declare signatures only (no default bodies at C4
minimum; default-method-bodies are a follow-on). The `Self`
type refers to the implementing class — concrete at impl
sites, abstract at trait-decl sites.

Type-check stores traits in an interner table parallel to
the existing struct table: `TypedProgram.traits:
Vec<TraitData>` with `TraitId(u32)`. A trait carries:
- name
- methods: `Vec<TraitMethod { name, params, return_ty }>`.

**Decision: trait declarations parse + type-check at C4.2;
no default method bodies at C4 minimum.** Method bodies live
in `impl` blocks (D5). Effects on trait methods follow ADR
0019's effect-row machinery — methods can declare `! { Op }`
rows just like free fns.

### D5. Trait dispatch: static-by-default + named impls.

Two implementations of the same trait for the same type
coexist by name:

    impl as Default for File {
        fn write(self: &mut Self, data: &[u8]) -> i64 { ... }
        fn flush(self: &mut Self) -> i64 { ... }
    }

    impl Logged as Writer for File {
        fn write(self: &mut Self, data: &[u8]) -> i64 {
            print(len(data));
            self.write_default(data)
        }
        fn flush(self: &mut Self) -> i64 { ... }
    }

The impl block introduces a named implementation `Logged` of
`Writer` for `File`. Default impls (with no name) bind to the
type-default scope. Call sites:
- `obj.write(data)` → uses the type-default impl.
- `Logged::write(&obj, data)` → uses the named `Logged` impl.

**Decision: trait dispatch is static-by-default using witness-
table generics from C1.7 + ADR 0016 D7.** Witness-table
dispatch means: for a generic fn `fn use_writer<W: Writer>(w:
W)`, the compiler emits one monomorphic instance per (W,
named-impl) pair. Witness tables are computed at impl-table
construction time; method calls go through the table.

**Dynamic dispatch via `dyn Trait`**: a separate ADR (post-C4
minimum). At C4 the surface ships static + monomorphic
generics dispatch only; `dyn Trait` is the followup ADR.

The orphan rule (Rust's restriction on implementing foreign
traits for foreign types) does not apply because named impls
disambiguate. ADR 0021 D7 amends if user testing surfaces
coherence issues.

### D6. Delegation: `delegate inner: T to Trait;`.

Composition replaces inheritance:

    class LoggingWriter {
        delegate inner: FileWriter to Writer;
        let log: Logger;
        pub fn write(self: &mut Self, data: &[u8]) -> i64 {
            self.log.info(...);
            self.inner.write(data)
        }
    }

The `delegate` statement inside a class body declares that the
class implements `Writer` by forwarding all `Writer` methods
to the field `inner: FileWriter`. The class can override
individual methods (`fn write(...)` above), and the un-
overridden ones (`flush`) auto-generate a forwarder:

    fn flush(self: &mut Self) -> i64 {
        self.inner.flush()
    }

**Decision: codegen emits the auto-forwarder at C4.3.** The
typed AST gets a `DelegationDecl { field_id: FieldId,
trait_id: TraitId }` per delegation site; codegen iterates
the trait's methods + emits forwarders for each not-otherwise-
overridden method. Conflicts (two `delegate` sites declaring
the same trait, or a delegated method colliding with a manual
impl) surface as a type-check error.

Multi-trait delegation (one class delegating to multiple
traits) is supported by emitting independent delegation
declarations. Diamond-shape conflicts (two delegates for the
same trait) are rejected at type-check.

### D7. Coherence within scope, not globally.

The Rust orphan rule keeps trait coherence — no two impls of
the same trait for the same type can exist in the universe.
Sentinel's named impls (D5) relax this: multiple impls can
coexist, distinguished by name. Within a SINGLE scope, two
impls of the same trait for the same type must have different
names; the type-check error is `DuplicateImplName`. Across
scopes (modules importing each other), naming serves as
disambiguation — the call site picks which impl by name.

Default impls (un-named) follow the orphan-rule precedent:
only one default impl per (trait, type) per scope. A scope
that imports two modules each defining its own default-impl
must disambiguate at the call site.

**Decision: coherence is scope-local; named impls eliminate
the global orphan rule.** Type-check carries `(scope_id,
trait_id, type_id) → ImplSet` tables, where `ImplSet` is a
Vec of `(ImplName, ImplBody)`. Lookups resolve by name + trait
+ type.

### D8. Structured concurrency: `scope`, `spawn`, `await`.

Tasks live inside scope blocks:

    scope concurrent {
        let a = spawn fetch_user(id);
        let b = spawn fetch_orders(id);
        return Profile { user: a.await, orders: b.await };
    }

The `scope` block creates a task-cancellation boundary. Tasks
spawned inside cannot outlive the block. On scope exit:
- If all tasks completed: their values are available (await
  may have been called inline).
- If the scope returned early (or panicked): outstanding
  tasks are cancelled.

`spawn expr` creates a task. The expression `expr` is
typically a fn call; spawn's return value is a
`Task<T>`-typed handle. `task.await` blocks until the task
produces a value of T.

**Decision: structured concurrency is exposed via three
keywords (`scope`, `spawn`, `await`) + a `Task<T>` builtin
type.** Implementation is via async-as-effect (D9).

### D9. Async-as-effect (closes ADR 0019 D9).

`spawn` and `await` lower to effect operations:

    effect Async {
        spawn<T>(body: () -> T) -> Task<T>;
        await<T>(t: Task<T>) -> T;
    }

The `scope concurrent { ... }` block wraps its body in a
matching `handle ... with { ... Async.spawn(body, k) => ...
}` per ADR 0020. The handler implementation (a runtime
scheduler) lives in `sentinel-runtime`'s async substrate (new
in C4).

**Decision: async is a built-in effect (`std::Async`) handled
by a runtime scheduler.** The scheduler is single-process at
C4 minimum (Phase A's broker integration deferred to C5).
Scheduling strategy: cooperative work-stealing with one OS
thread per CPU core; tasks are heap-allocated continuations
(reuses ADR 0020 D7's SentinelKont). Cancellation propagates
via a `Task<T>::cancelled: bool` flag the scheduler reads
between awaits.

The Async effect's typing is **polymorphic**: `spawn<T>` is
generic over the task's result type. This is the first
generic effect operation in the bootstrap — ADR 0019 D12 had
operation-level type parameters out of scope; C4 partially
lifts that restriction for the Async effect only (a special
case).

### D10. Out-of-scope at C4: actors + cross-process + GPU.

  - **Actors**: declared message protocols + mailbox routing.
    The full HANDOVER §6.2 vision includes actors with cross-
    process variants via the broker. Single-process actors
    are a natural follow-on to scope/spawn/await but require
    additional channel + protocol-typing machinery. Deferred
    to **Phase C5**.
  - **Cross-process safety (`@shared` region)**: Phase A's
    broker has the runtime substrate; surfacing it at the
    language layer is C5 work.
  - **GPU / `@numa` / heterogeneous compute**: post-1.0
    language-evolution territory. Not C4.
  - **Dynamic dispatch (`dyn Trait`)**: a follow-on ADR after
    C4 minimum. Witness-table static dispatch covers the
    common cases.
  - **Default trait method bodies**: a follow-on ADR.
  - **Trait inheritance** (a trait extending another):
    deferred. Composition via multiple `impl ... for ...`
    blocks covers most use cases.
  - **Effect polymorphism in trait methods**: ADR 0019 D9's
    "interaction between effects and traits" question.
    SENTINEL_DESIGN.md §12 flags this as a genuinely open
    design problem. C4 ships trait methods with concrete
    (non-row-polymorphic) effect annotations only.

### D11. Lexer additions.

C4.0 lexer adds the following keywords:

    class
    trait
    impl
    init
    delegate
    Self
    self
    scope
    spawn
    await
    new           (TBD per D3 — only if we adopt `new Name(args)` sugar)

Plus the contextual `as` for impl naming (`impl X as Trait
for Type`).

The `self` / `Self` keywords are reserved BUT only allowed in
method/trait contexts. `Self` refers to the implementing
type; `self` is the receiver binding.

Lexer changes: ~10 new TokenKind variants. logos's longest-
match handles them straightforwardly.

### D12. Out-of-scope items.

In addition to D10, the following stay deferred:

  - **`@shared` region** + broker integration (Phase C5).
  - **`@gpu`, `@simd`, `@numa` location qualifiers** (post-1.0).
  - **Stable ABI** (Phase C5).
  - **`extern` declarations + foreign-fn linkage** (Phase C5).
  - **Plugin sandboxing** (Phase C5).
  - **Time-travel recording** (Phase C5).
  - **Authenticated values (`authenticated T`)** (post-1.0).

### D13. Phase-go program spec.

At `tests/pass/c4_go_no_go.sentinel`. The full C4 surface in
one program (~30 lines):

    trait Writer {
        fn write(self: &mut Self, n: i64) -> i64;
    }

    class Buffer {
        let total: i64;
        pub init() {
            self.total = 0;
        }
    }

    impl as Writer for Buffer {
        fn write(self: &mut Self, n: i64) -> i64 {
            self.total = self.total + n;
            self.total
        }
    }

    class LoggingBuffer {
        delegate inner: Buffer to Writer;
        let log_count: i64;
        pub init() {
            self.inner = Buffer::init();
            self.log_count = 0;
        }
        pub fn count(self: &Self) -> i64 {
            self.log_count
        }
    }

    fn main() -> i64 {
        scope concurrent {
            let lb: LoggingBuffer = LoggingBuffer::init();
            let t = spawn lb.write(42);
            t.await
        }
    }

Expected: exit code 42, no stdout.

Exercises:
  - class declaration (D1)
  - methods (D2)
  - init constructor (D3)
  - trait + named impl (D4 + D5)
  - delegation (D6)
  - scope + spawn + await (D8)
  - async-as-effect handler runtime (D9)

A secondary fixture `tests/pass/c4_traits_named_impl.sentinel`
exercises the named-impl form with two `impl Foo as Writer for
File` + `impl Bar as Writer for File` co-existing.

### D14. Sub-phase split.

Rough split per the C4 surface volume:

| Sub  | Title                                                          | Estimate     | Status |
|------|----------------------------------------------------------------|--------------|--------|
| C4.0 | Lexer: class / trait / impl / init / delegate / scope / spawn  | 1 session    | **DONE** |
|      | / await / Self / self / as / for keywords (D11).               |              |        |
| C4.1 | class declarations + methods + init + self per D1-D3.          | 2-3 sessions |        |
|      | AST + parser + resolve + types + codegen (parallel-tree).      |              |        |
| C4.2 | trait declarations + impl blocks (default + named) + dispatch  | 2-3 sessions |        |
|      | per D4-D5. Witness-table generics extension.                   |              |        |
| C4.3 | delegation per D6 — type-check + auto-forwarder codegen.       | 1 session    |        |
| C4.4 | structured concurrency surface (scope / spawn / await) per     | 2-3 sessions |        |
|      | D8-D9 + the Async effect + a runtime scheduler.                |              |        |
| C4.5 | phase-go program + ADR PROPOSED → ACCEPTED + STATE/HANDOVER    | 0-1 sessions |        |
|      | close-out.                                                     |              |        |

Total: 8-12 sessions across 6 sub-phases. C4.4 (structured
concurrency + scheduler) carries the most risk; the scheduler
is the largest new runtime component since the Phase A broker.

Per-sub-phase ADRs (mirroring ADR 0013/0014/0015/0016 for C1.4
through C1.7) land alongside their implementations:
- ADR 0022 (PROPOSED at C4.1 open): concrete class + method
  surface syntax.
- ADR 0023 (PROPOSED at C4.2 open): concrete trait + impl
  surface + dispatch resolution.
- ADR 0024 (PROPOSED at C4.4 open): the Async effect surface
  + scheduler design (substantive runtime work; warrants its
  own ADR like ADR 0020 for handlers).

## Reasoning

The decisions cluster around four themes.

**Classes as struct + methods + impls + delegation.** D1's
desugaring keeps the C1.4 struct surface working while
introducing classes as the canonical declaration form for
richer types. Init (D3) enforces no half-constructed objects;
methods (D2) reuse the existing fn-decl machinery + the C2
borrow check.

**Named impls eliminate the orphan rule.** D5 + D7 pick named
implementations as the answer to Rust's coherence-at-the-cost-
of-orphan-rule trade-off. Multiple impls coexist; call sites
disambiguate. Within a scope, named-impl uniqueness gives
coherence; across scopes, names compose.

**Async-as-effect closes the ADR 0019 D9 deferral.** D9 here
ships the Async built-in effect handled by a runtime scheduler.
The scheduler is the substantive new runtime piece — ADR 0024
(at C4.4 open) carries the design call. Phase B validated the
async-as-effect typing approach across 30+ handler tests + the
async demo; C4 implements it production-side.

**Deferred items concentrate on cross-process + heterogeneous.**
D10 + D12 push actors (single-process is C4-feasible but the
cross-process variant via Phase A broker is C5), GPU /
`@shared` / `@numa`, stable ABI, and extern linkage to Phase
C5. The C4 minimum is the *single-process language surface*
that completes the Sentinel-1.0 bootstrap arc.

## Consequences

### Positive

- Completes HANDOVER §6.2's bootstrap-language surface: every
  feature from §1-§9 is reachable from C4.
- Async-as-effect closes the longstanding ADR 0019 D9 deferral.
  Phase B's validation pattern transfers directly.
- Named impls give a cleaner coherence story than Rust's
  orphan rule; user testing at SENTINEL_DESIGN §12 will
  confirm the ergonomics.
- The runtime scheduler (C4.4) is the foundation for cross-
  process actors at Phase C5 — investment compounds.

### Negative

- ~2000-3500 LOC across crates (rough; the actual is unknown
  until C4.1 ships). Larger than ADR 0020's handler-runtime
  layer (~1500 LOC). The scheduler alone is probably ~500-
  700 LOC.
- The single largest new runtime component since Phase A's
  broker. Concurrency bugs are notoriously subtle; budget
  pessimistically.
- Trait dispatch resolution is the most invasive type-check
  change since C1.7's generics. Name-based lookup tables grow
  the typed AST footprint.
- Definite-assignment for init (D3) is a new dataflow analysis
  in the type checker. The existing borrow check has CFG
  machinery (per ADR 0017); reuse is plausible but not
  free.

### Neutral

- D5's witness-table dispatch matches C1.7's generic-fn
  monomorphisation; the pattern is established. Dynamic
  dispatch via `dyn Trait` stays deferred but its addition
  is mechanical (existing witness tables become runtime
  values).
- The lexer surface grows by ~10 keywords; precedence-aware
  longest-match copes.

## Alternatives considered

- **Inheritance instead of delegation** (D6). Rejected: SENTINEL_DESIGN.md
  §7 explicitly excludes class-based inheritance. Composition with
  explicit delegation replaces it. The auto-forwarder codegen makes
  delegation almost as ergonomic as inheritance without the diamond-
  problem complexity.

- **Single-impl-per-(trait, type)** (D5). Rejected: the orphan rule
  costs more than it saves for a systems-language target where
  cross-module trait impls matter. Named impls + scope-local
  coherence (D7) preserve disambiguation without the global
  restriction.

- **Async runtime as a separate phase** (D9). Considered but rejected:
  the handler runtime from ADR 0020 already exists; bolting `spawn`/
  `await` on top is a natural extension. Separating into Phase D would
  fragment the bootstrap-language story.

- **Actors at C4** (D10). Rejected: cross-process actors need the
  Phase A broker integration that's C5 work. Single-process actors
  are an additive surface on top of scope/spawn/await; deferring
  cleanly delineates the C4/C5 boundary.

- **`new Name(args)` sugar for `Name::init(args)`** (D3 TBD).
  Considered: aligns with most OOP languages' surface ergonomics.
  Deferred to the ADR 0022 detail layer; either form is workable.

- **Default trait method bodies at C4 minimum** (D4). Considered
  but rejected: adds typing-rule complexity (method-body type-
  checks against the trait's `Self` abstract type) that doesn't
  ship signature-level features. A follow-on ADR can add this.

- **Effect polymorphism in trait methods at C4** (D10). Rejected:
  this is genuinely open design space (SENTINEL_DESIGN §12). Phase
  B didn't model trait-method effect rows; the production compiler
  ships concrete annotations only at C4.

## Revisit

This ADR is **PROPOSED** until C4's sub-phases land. Per-D
revisit triggers:

- **D1 (class as struct sugar)**: revisit if `class` and
  `struct` end up structurally identical at the typed AST —
  in that case we might collapse them into a single
  `TypedClassDecl` with an "is-record-only" flag.
- **D3 (init definite-assignment)**: revisit if the dataflow
  analysis surfaces existing borrow-check overlap that
  suggests merging the two passes.
- **D5 (named impls)**: revisit at first user-reported
  ergonomic friction. Alternative: scope-default-impls only,
  with named impls behind an opt-in flag.
- **D6 (delegation auto-forwarder)**: revisit if class
  hierarchies get deep enough that multi-level forwarder
  emission overhead matters; LLVM should inline most cases.
- **D8/D9 (structured concurrency + Async effect)**: revisit
  at ADR 0024 open — the scheduler design call is the
  substantive runtime work for C4.4.
- **D10 (actors at C5)**: revisit at C4 close. If actors
  surface as natural extensions of scope/spawn/await, can be
  pulled forward.
- **D14 (sub-phase split)**: revisit at C4.1 close. The C4.2
  (traits) and C4.4 (concurrency) sub-phases are the largest;
  they may split further.

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; actual is
unknown until sub-phases ship.

  - **C4.0** (lexer, ~50 LOC):
    - sentinel-syntax (lexer): +50 (10 keywords + ~5 tests
      per keyword).

  - **C4.1** (class + method + init, ~600-900 LOC):
    - sentinel-ast: +150 (ClassDecl, MethodDecl, InitDecl).
    - sentinel-syntax (parser): +250 (parse_class_body etc.).
    - sentinel-resolve: +100 (method-table population +
      class-ScopeId tracking).
    - sentinel-types: +250-300 (definite-assignment analysis
      + method signature checking + `Self` resolution).
    - sentinel-codegen: +150 (class struct lowering + method
      monomorphisation).
    - tests + fixtures: +80.

  - **C4.2** (traits + impls + dispatch, ~700-1000 LOC):
    - sentinel-ast: +180 (TraitDecl, ImplBlock, ImplName).
    - sentinel-syntax (parser): +250 (parse_trait_block,
      parse_impl_block).
    - sentinel-resolve: +200 (impl-table population, scope-
      local coherence enforcement).
    - sentinel-types: +250 (trait-method check, named-impl
      resolution).
    - sentinel-codegen: +200 (witness-table + monomorphisation
      per named impl).
    - tests + fixtures: +100.

  - **C4.3** (delegation, ~300-400 LOC):
    - sentinel-ast: +50 (DelegationDecl).
    - sentinel-syntax (parser): +60.
    - sentinel-resolve: +50.
    - sentinel-types: +100 (conflict detection: delegation +
      manual impl collisions).
    - sentinel-codegen: +100 (auto-forwarder emission).
    - tests: +50.

  - **C4.4** (structured concurrency + scheduler, ~800-1200
    LOC):
    - sentinel-ast: +100 (ScopeBlock, SpawnExpr, AwaitExpr).
    - sentinel-syntax (parser): +150.
    - sentinel-resolve: +50.
    - sentinel-types: +200 (Async effect + Task<T> typing).
    - sentinel-codegen: +200 (spawn/await lowering — extends
      ADR 0020 D7's runtime symbols).
    - sentinel-runtime: +300-500 (scheduler: work-stealing
      queue + OS-thread pool + task struct + cancellation
      propagation).
    - tests + fixtures: +100.

  - **C4.5** (phase-go + close-out, ~100-200 LOC):
    - phase-go fixtures (c4_go_no_go + named-impl + delegation
      smokes).
    - STATE.md + HANDOVER §0 close-out.
    - ADR 0021 PROPOSED → ACCEPTED flip.
    - +5-8 driver pass-tests for phase-go.

  - **Total at C4 minimum**: ~2400-3700 LOC across crates.
    In line with Phase C2's investment but concentrated in
    type-check + codegen + a new runtime substrate.

  - **Estimated session budget at C4.0-C4.5**: 8-12 sessions
    across 6 sub-phases. Compare ADR 0017's "6-13 sessions"
    Phase C2 estimate — C4's surface volume is comparable but
    the scheduler (C4.4) is the wildcard.

After C4.5: **Phase C5** per HANDOVER §6.2 — broker
integration, cross-process safety, reproducible-build
guarantees, stable ABI definition, LSP/tooling polish.
Sentinel's 1.0 release is at C5 close.
