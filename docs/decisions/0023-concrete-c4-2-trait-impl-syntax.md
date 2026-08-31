# ADR 0023: Concrete C4.2 trait + impl surface — declarations, dispatch, named impls

Status: ACCEPTED-WITH-AMENDMENTS — flipped at C4.2 (2/N) close.
Receiver-typed dispatch (D5 Path 1) and qualified-named
dispatch (D5 Path 2) ship end-to-end via direct LLVM-call
lowering through per-impl mangled fns. Three amendments at
C4.2 close (see "Amendments" section below): A1 D5 Path 3
(bounded-generic dispatch via witness tables) DEFERRED — needs
`<W: Writer>` bounded-generic surface; A2 D9 witness-table
values not emitted (scaffolding for Path 3); A3 D7
`Type::TraitSelf(TraitId)` interner SHIPPED but unused at
runtime (params/returns at C4.2 don't reference `Self` —
positional only via self_kind, mirroring C4.1 A2).

Date: 2026-05-28
Related:
  - **0021** (Phase C4 kickoff — PROPOSED): the umbrella ADR.
    D4 picks `trait Name { sig; ... }` as the trait declaration
    form; D5 picks static-by-default dispatch + named impls
    via `impl Name as Trait for Type`; D7 picks scope-local
    coherence. This ADR fills in the surface + typing detail.
  - **0022** (Concrete C4.1 class syntax —
    ACCEPTED-WITH-AMENDMENTS): the C4.1 class surface that
    methods + init build on. Trait methods reuse the
    `self: &Self` / `self: &mut Self` receiver convention
    from ADR 0022 D3. Implementing types at C4.2 minimum are
    classes (or structs); the impl machinery composes with
    the C4.1 ClassData + TypedStructDecl tables.
  - **0016** (C1.7 generics — ACCEPTED): generic fns +
    witness-table monomorphisation. C4.2's trait-bound
    dispatch (`fn use_writer<W: Writer>(w: W)`) extends the
    C1.7 generic-fn machinery by adding a per-bound witness-
    table thread per ADR 0021 D5.
  - **0017** (Phase C2 kickoff — ACCEPTED-WITH-AMENDMENTS):
    `&Self` / `&mut Self` borrow rules apply uniformly to
    trait methods, impl methods, and class methods.

## Context

C4.1 closed with classes + methods + init shipping end-to-end.
C4.2 adds the trait + impl surface so types can declare and
satisfy named-method-set contracts. Two distinct dispatch
paths land at C4.2:

  1. **Receiver-typed dispatch**: `obj.method(args)` where
     `obj`'s class has a default `impl as Trait for Class`
     in scope. Static lookup; identical IR shape to the C4.1
     direct method-call form.
  2. **Bounded-generic dispatch**: `fn use_writer<W:
     Writer>(w: W)` where the type parameter `W` is bounded
     by `Writer`. The fn body can call any `Writer` method
     on `w`; each monomorphic instance picks the right
     `(W, impl)` pair via witness-table threading per ADR
     0016 D7.

Named impls (per ADR 0021 D5) let multiple implementations of
the same `(Trait, Type)` coexist; the call site picks which
by the qualified `ImplName::method` form. This eliminates
Rust's orphan rule at the cost of an explicit naming
discipline at the call site.

The C4.2 lexer state (from C4.0):

  - **Keywords**: `let, fn, if, else, true, false, struct,
    null, mut, effect, secret, declassify, handle, with,
    perform, return, class, trait, impl, init, delegate,
    scope, spawn, await, Self, self, as, for`.
  - **Punctuation**: unchanged from C3 + the `ColonColon`
    token added at C4.1 for `Name::init`. `ColonColon`
    composes naturally for `ImplName::method` qualified
    dispatch.

C4.2 picks up the trait-related keywords (`trait`, `impl`,
`as`, `for`) and weaves them through AST + parser + resolve
+ types + codegen.

## Decision

Twelve D-numbered sub-decisions covering surface (D1-D5),
typing (D6-D8), codegen (D9), out-of-scope items (D10),
lexer recap (D11 — no new tokens), and the C4.2 phase-go
(D12).

### D1. `trait Name { method_sigs }` declaration grammar.

The trait declaration is a top-level item alongside `fn`,
`struct`, `class`, and `effect`:

    trait Writer {
        fn write(self: &mut Self, data: i64) -> i64;
        fn flush(self: &mut Self) -> i64;
    }

Grammar:

    trait_decl    = 'trait' Ident '{' method_sig* '}'
    method_sig    = 'fn' Ident '(' self_param (',' param)* ')'
                    ('->' type_expr)? ('!' effect_row)? ';'
    self_param    = 'self' ':' '&' 'mut'? 'Self'

Each `method_sig` looks like a C4.1 method declaration
(`parse_method_decl`) but is terminated with `;` instead of
a `block`. The first parameter is the mandatory `self:
&Self` or `self: &mut Self` receiver per ADR 0021 D2 / ADR
0022 D3 — same shape as class methods. Trait methods can
declare effect rows; the row participates in the trait's
"effect contract" per ADR 0019.

At C4.2 minimum:
- **No default method bodies** in trait declarations
  (deferred per ADR 0021 D10 — `default_methods`).
- **No supertraits** (`trait Reader: Writer { ... }` is a
  follow-on).
- **No associated types or constants** (deferred).
- **Generic traits** (`trait Iter<Item> { ... }`) are
  deferred; the C4.2 minimum surface is monomorphic traits.

The parser surfaces `EmptyTraitDecl` for `trait T {}` —
empty traits are allowed structurally (they're marker
traits at the C4.2 minimum, useful for type-level grouping
without methods).

### D2. Method signature shape inside traits.

Trait method signatures mirror class method declarations
per ADR 0022 D3, with two differences:

- Body is replaced by `;` (no implementation at the
  signature site).
- `Self` refers to the implementing type at the impl site;
  inside the trait declaration it's an *abstract* type
  reference resolved at trait-check time.

The `self_kind` (Shared / Exclusive) is captured per ADR
0022's `SelfKind` enum. The same `Param` shape is reused —
parameter type annotations are mandatory per ADR 0012 D1.

Signatures are stored at type-check time on
`TypedProgram.trait_decls: Vec<TraitData>` (mirroring
`ClassData` from ADR 0022 D1):

    TraitData {
        id: TraitId,
        name: String,
        methods: Vec<TraitMethodSig {
            name, self_kind, params, return_type,
            effect_row, span,
        }>,
    }

Per-trait method-name uniqueness is enforced at resolve;
`DuplicateTraitMethod` fires on collision.

### D3. `impl as Trait for Type { ... }` (default impl).

The impl block grammar:

    impl_decl     = 'impl' impl_name? 'as' trait_ident
                    'for' type_ident '{' method_def* '}'
    impl_name     = Ident
    method_def    = (visibility?) 'fn' Ident '(' self_param
                    (',' param)* ')' ('->' type_expr)?
                    ('!' effect_row)? block

When `impl_name` is absent, the impl is the **default impl
for `(Trait, Type)` in this scope**. Default impls have no
distinguishing name; call sites use bare `obj.method(args)`
and the lookup picks the default impl by the receiver's
type.

Method `def`s inside the impl supply bodies for every
method declared in the trait. Resolve enforces
**completeness** (every trait method has an impl) and
**signature match** (impl method's signature matches the
trait's). `ImplMissingMethod` / `ImplMethodSignatureMismatch`
are the two new ResolveError variants.

The `Type` in `for Type` is restricted at C4.2 minimum to:

- Class names (the ADR 0022 `Type::Class(ClassId)`).
- Struct names (the ADR 0013 `Type::Struct(StructId)`).
- Primitive types (`i64`, `i32`, `bool`) — allowed but
  unusual; flagged with a help suggesting a class wrapper
  if it surfaces a use case.

Generic-instance types (`Pair<i64, bool>`) + `?T` /
`[T]` / `&T` are **deferred** at C4.2 — they require
generic-impl machinery (`impl<T> Writer for Box<T>`) that
overlaps with witness-table generics.

### D4. `impl Name as Trait for Type` (named impl) per ADR 0021 D5.

When `impl_name` is present, the impl is a **named impl**
for `(Trait, Type)`. Named impls coexist with the default
impl + with other named impls within the same scope, per
ADR 0021 D5 + D7. The name disambiguates at call sites:

    impl Buffered as Writer for File {
        fn write(self: &mut Self, data: i64) -> i64 {
            // buffered write path
        }
        fn flush(self: &mut Self) -> i64 { ... }
    }

Call sites:

  - `f.write(42)` — uses the default `Writer` impl for
    `File` (if one exists in scope).
  - `Buffered::write(&mut f, 42)` — uses the `Buffered`
    named impl explicitly. The receiver is passed as the
    first arg (the qualified form is uniform with the C4.1
    `Name::init(args)` pattern; the implicit-receiver
    `obj.Buffered::method(args)` form is rejected at C4.2 —
    qualified call only).

Per ADR 0021 D7, **coherence is scope-local**:

- **At most one default impl** of `(Trait, Type)` per
  scope. `DuplicateDefaultImpl` fires on collision.
- **Named impls** must have unique names within the scope.
  `DuplicateImplName` fires on collision.

Cross-scope coherence is the call site's responsibility —
the user picks which named impl to use by name.

### D5. Dispatch: static-by-default + witness tables for bounded generics.

Two dispatch paths land at C4.2 (mirroring ADR 0021 D5):

**Path 1: receiver-typed dispatch**.

    fn use_default_writer(w: &mut File) -> i64 {
        w.write(42)
    }

`w` has static type `&mut File`. The type checker looks up
the default `Writer` impl for `File` in the current scope.
If found, codegen emits a direct call to the impl's
`File__Writer__write` mangled fn. The receiver is auto-
referenced per ADR 0022 D7's pattern.

**Path 2: qualified named dispatch**.

    fn use_buffered_writer(w: &mut File) -> i64 {
        Buffered::write(w, 42)
    }

`Buffered::write` resolves to the named impl. Type-check
validates the receiver's type matches the impl's `Type`
clause; codegen emits a direct call to
`Buffered__File__Writer__write`.

**Path 3: bounded-generic dispatch (witness-table-threaded)**.

    fn use_writer<W: Writer>(w: &mut W) -> i64 {
        w.write(42)
    }

The type parameter `W` is bounded by `Writer`. At call sites,
the compiler instantiates the fn for each concrete `W` (per
the C1.7 monomorphisation pattern) AND picks a specific
`(Writer-impl)` for that `W`. The default impl is the
default pick; user can override at the call site via:

    use_writer::<File @ Buffered>(&mut f)

(turbofish-style — the `@` separator picks the named impl).
The turbofish form is **deferred** at C4.2 minimum — at
C4.2 only the default impl participates in bounded-generic
dispatch. The `@`-form lands when user testing surfaces the
need.

**At C4.2 minimum:** Path 1 + Path 2 ship. Path 3 ships for
the default-impl case only; bounded-generic + named-impl
pairing is deferred.

### D6. Method-call resolution algorithm.

Given a call site `obj.method(args)` where `obj` has static
type T:

  1. **If T is `Type::Class(class_id)`**: try class-method
     lookup first (per ADR 0022 D7's `ClassData::method_index`).
     If found, emit class-method call.
  2. **Else** (T is a struct or a class without a matching
     method): look up the default impl of `(Trait, T)` in
     scope where Trait declares a method named `method` with
     a matching signature. If exactly one match, emit impl-
     method call.
  3. **If zero or more than one match**: `MethodNotFound`
     (zero) or `AmbiguousMethodCall` (more than one — the
     user has multiple default impls of distinct traits
     each providing a `method` of the same name on T).

Given a call site `ImplName::method(receiver, args)`:

  1. Look up `ImplName` in the impl table for the current
     scope. If not found, `UndefinedImpl`.
  2. Verify `method` exists on the impl's trait
     (`UndefinedTraitMethod` if not).
  3. Verify `receiver` has the impl's `Type` (or a `&Type` /
     `&mut Type` reference). If not, `ImplMethodReceiverMismatch`.
  4. Emit direct call to the impl's mangled fn.

### D7. `Self` resolution in trait + impl contexts.

Inside a `trait Trait { ... }` body, `Self` is the abstract
implementing type. The typed AST stores it as a synthetic
`Type::TraitSelf(TraitId)` variant (the **ninth** interner-
table-style variant; preserves `Copy + Hash`). At impl
sites, `Self` resolves to the impl's `for Type` clause.
Inside an `impl as Trait for Type { ... }` body, `Self` is
`Type::Class(class_id)` or `Type::Struct(struct_id)` — the
same convention as C4.1's class-body Self resolution.

The `self` value parameter binds to a synthetic VarId at
the start of each method body (same as C4.1's class
methods). Outside a trait or impl body, `Self` and `self`
both surface the existing `SelfOutsideClassContext` /
`SelfTypeOutsideClassOrTraitContext` diagnostics from C4.1
+ a new variant for the trait/impl-scope case.

### D8. Type-check + resolve flow for traits + impls.

Adding the C4.2 pipeline pass to the existing `check` fn:

  1. Pass 0a (existing): struct name table + struct_type_param_counts.
  2. Pass 0b (existing at C4.1): class name table.
  3. **Pass 0c (new)**: trait name table. Like classes, traits
     share a name namespace with structs + classes;
     `RedefinedTrait` fires on collision.
  4. **Pass 0d (new)**: impl block table per (trait, type)
     pair. Default vs named tracked. `DuplicateDefaultImpl`
     and `DuplicateImplName` fire on collision.
  5. Pass 1 (existing): struct fields typed.
  6. Pass 2 (existing): struct cycle detection.
  7. Pass 3a (existing): effect decls typed.
  8. Pass 3b (existing): fn signatures typed.
  9. **Pass 3c (new)**: trait method signatures typed +
     interned at `TypedProgram.trait_decls`.
  10. **Pass 3d (new)**: impl signatures typed +
      completeness/signature checks against the trait.
      Bodies use stub blocks for now.
  11. Pass 4 (existing at C4.1): class signatures typed.
  12. Pass 5 (existing at C4.1): fn bodies typed.
  13. **Pass 6 (new)**: impl method bodies typed.
  14. Pass 7 (existing): class bodies typed.

Pass 0d builds the impl-table layered on the trait + class
+ struct tables; the rest of the pipeline reads it during
expression type-check.

### D9. Codegen: per-impl method emission + witness-table threading.

A trait declaration emits **no LLVM symbols** — trait
methods have no body to compile. Each impl block emits:

  1. **One LLVM fn per impl method**, mangled as
     `ImplName__Type__Trait__method` (or
     `default__Type__Trait__method` for unnamed default
     impls). Methods take a `ptr` self_ptr as the first
     param + the declared params.
  2. **A witness-table struct type** per `(Trait, Type,
     impl)` triple: `{ ptr method1, ptr method2, ... }`
     holding fn pointers in trait-method order. Used by
     bounded-generic dispatch at call sites.
  3. **A global witness-table value** (LLVM `@global_const`)
     per impl, populated with the fn-pointer entries.

Call-site lowering:

  - **Receiver-typed dispatch** (D5 Path 1): direct call to
    `default__Type__Trait__method`. Identical IR shape to
    class-method calls.
  - **Qualified named dispatch** (D5 Path 2): direct call
    to `ImplName__Type__Trait__method`.
  - **Bounded-generic dispatch** (D5 Path 3, default-only):
    monomorphic instances of the bounded-generic fn are
    emitted per the C1.7.5 pattern with the witness-table
    GEP load + indirect call replacing the direct call. At
    C4.2 minimum only default impls participate; the GEP
    is essentially a no-op since there's only one impl per
    `(Trait, Type)` in the default case.

### D10. Out-of-scope at C4.2.

The following stay deferred to follow-on ADRs or post-C4
work:

- **Default method bodies** in trait declarations. Trait
  methods at C4.2 are signatures-only; impl blocks must
  supply every method. The follow-on adds an `Option<Body>`
  to the trait-method sig.
- **Supertraits** (`trait Reader: Writer { ... }`). The
  inheritance relationship needs scope + coherence
  amendments.
- **Generic traits** (`trait Iter<Item> { ... }`). The
  trait-decl machinery would extend with type-param scoping
  similar to generic structs.
- **`dyn Trait`** (dynamic dispatch). Static dispatch via
  witness tables is the C4 minimum; dyn Trait is a
  followup ADR with vtable + fat-pointer machinery.
- **Bounded-generic + named-impl pairing** (the `@`-form
  turbofish per D5 Path 3 amendment). Defaults only at
  C4.2.
- **Impl for generic types** (`impl<T> Writer for Box<T>`).
  The C4.2 minimum restricts impl `for Type` to concrete
  types (no type params). Generic-impl machinery overlaps
  with bounded-generic dispatch and is a follow-on.
- **Coherence across modules**. C4.2 ships scope-local
  coherence per ADR 0021 D7; cross-module coherence is
  Phase C5 module-system work.
- **Associated types + associated constants**. Deferred.
- **Where-clause syntax** (`fn f<T>(x: T) where T: Writer
  { ... }`). The inline `<T: Writer>` form ships at C4.2;
  where-clause is a follow-on sugar.

### D11. Lexer (no new tokens at C4.2).

C4.0 reserved `trait`, `impl`, `as`, `for`. C4.2 activates
them at the parser layer. No lexer changes.

### D12. Phase-go program.

At `tests/pass/c42_go_no_go.sentinel`. The full C4.2
surface in one program:

    trait Writer {
        fn write(self: &mut Self, data: i64) -> i64;
    }

    class FileSink {
        let count: i64;
        pub init() {
            self.count = 0;
            0
        }
    }

    impl as Writer for FileSink {
        fn write(self: &mut Self, data: i64) -> i64 {
            self.count = self.count + data;
            self.count
        }
    }

    impl Doubling as Writer for FileSink {
        fn write(self: &mut Self, data: i64) -> i64 {
            self.count = self.count + data * 2;
            self.count
        }
    }

    fn main() -> i64 {
        let mut s: FileSink = FileSink::init();
        let a: i64 = s.write(10);
        let b: i64 = Doubling::write(&mut s, 16);
        b
    }

Expected: exit code 42 (a = 10, b = 10 + 32 = 42; b is the
return value).

Exercises:
- trait declaration with one method (`Writer.write`).
- class declaration with init constructor (per ADR 0022).
- default `impl as Writer for FileSink` — receiver-typed
  dispatch via `s.write(10)`.
- named `impl Doubling as Writer for FileSink` — qualified
  dispatch via `Doubling::write(&mut s, 16)`.
- scope-local coherence: default + named coexist for the
  same `(Writer, FileSink)` pair.
- `&mut Self` receiver in trait methods + impls reuses the
  C4.1 borrow rules.

Secondary fixtures:

- `c42_trait_basic.sentinel`: smallest trait + default impl
  + receiver-typed dispatch.
- `c42_bounded_generic.sentinel`: `fn use_writer<W:
  Writer>(w: &mut W) -> i64` with one default impl. Exercises
  D5 Path 3 default case.

UI fixtures:

- `c42_impl_missing_method.sentinel`: impl block missing a
  trait method → `ImplMissingMethod`.
- `c42_impl_method_sig_mismatch.sentinel`: impl method's
  signature doesn't match the trait's →
  `ImplMethodSignatureMismatch`.
- `c42_duplicate_default_impl.sentinel`: two `impl as
  Writer for FileSink` in the same scope →
  `DuplicateDefaultImpl`.
- `c42_duplicate_impl_name.sentinel`: two `impl Buffered
  as Writer for FileSink` → `DuplicateImplName`.

## Reasoning

The decisions cluster around three themes.

**Traits as method-set contracts.** D1-D3 + D9 keep the
underlying typed AST orthogonal to classes — trait
declarations have no runtime presence (just type-system
artifacts) and impl blocks lower to per-impl LLVM fns. The
existing class-method dispatch (ADR 0022 D7) generalises by
substituting the impl table for the class's own method
table — call-site lowering picks the same direct-call IR
shape.

**Named impls as the orphan-rule replacement.** D4 + D7
ship ADR 0021 D5's named-impl design. The trade-off is
explicit at the call site (`Buffered::write(&mut f, 42)`
instead of an implicit selection); the benefit is
scope-local coherence without a global universe rule. User
testing per SENTINEL_DESIGN §12 will validate the
ergonomics.

**Witness tables for bounded generics.** D5 Path 3 + D9
extend C1.7's monomorphic generic-fn machinery with a
per-bound witness-table thread. The default case at C4.2
minimum is essentially a degenerate witness table (one
entry per method, no choice); the machinery scales to the
full named-impl pairing when D5's `@`-form ships.

## Consequences

### Positive

- Completes the trait-based dispatch story per HANDOVER
  §6.2. Traits are the canonical contract surface for
  Sentinel.
- Named impls give a cleaner coherence story than Rust's
  orphan rule. Multiple impls coexist; the call site
  disambiguates.
- The witness-table machinery is the foundation for
  delegation (C4.3) — delegation auto-forwarders use the
  same per-impl fn-pointer pattern.
- Static dispatch keeps the call-site IR shape direct (no
  vtable indirection); perf parity with class methods.

### Negative

- ~600-900 LOC across crates (AST, parser, resolve, types,
  codegen). The largest single-sub-phase change since C4.1
  (which itself was ~1000 LOC).
- The impl table is a new top-level data structure indexed
  by (TraitId, TypeId). Adding a new dimension (scope_id)
  for scope-local coherence inflates lookup complexity.
- Bounded-generic dispatch (D5 Path 3) extends the C1.7
  monomorphisation worklist with witness-table threading.
  The first non-trivial generic-fn machinery extension
  since C1.7.5.
- The orphan-rule replacement (named impls) shifts the
  ergonomic burden to the call site. User testing may
  reveal friction.

### Neutral

- The existing class-method dispatch (ADR 0022 D7) doesn't
  change shape; trait dispatch composes with it. C4.1
  fixtures keep passing.
- `Self` resolution gains a new abstract-Self case (in
  trait declarations) but the existing concrete-Self cases
  (in class bodies, impl bodies) keep their semantics.
- Effect rows on trait methods follow ADR 0019's existing
  machinery — no new effect-system work at C4.2.

## Alternatives considered

- **Orphan rule instead of named impls** (D4 + D7).
  Rejected: ADR 0021 D5 already decided named impls. Reopens
  if user testing surfaces named-impl friction.

- **Default method bodies in trait declarations** (D2 +
  D10). Considered: useful for traits with derivable
  methods. Rejected at C4.2 minimum: adds parsing complexity
  + impl-completeness machinery overlap.

- **`dyn Trait` dynamic dispatch** (D5 + D10). Rejected at
  C4.2: static dispatch covers the C4 minimum surface;
  vtable + fat-pointer machinery is its own ADR (post-C4).

- **Generic traits** (`trait Iter<Item>`) (D10). Rejected
  at C4.2: trait-decl type-param scoping is non-trivial +
  most use cases (Iter, IntoIter, Default) can be modeled
  with monomorphic traits + classes.

- **Implicit named-impl resolution at call sites** (D4).
  Considered: `obj.method(args)` could automatically pick
  the only named impl in scope if no default exists.
  Rejected: ambiguity grows quickly; explicit
  `ImplName::method` is the safer rule.

- **Cross-module coherence enforcement** (D7). Rejected:
  Phase C5 module system is the natural place; C4.2 ships
  scope-local coherence as the minimum.

- **Path 3 bounded-generic + named-impl pairing at C4.2**
  (D5 + D10). Rejected: the `@`-form turbofish + witness-
  table substitution per call site is non-trivial. Defaults
  only at C4.2; named-impl bounded generics land in a
  follow-on.

## Amendments

At C4.2 (2/N) close, ADR 0023 flipped PROPOSED →
ACCEPTED-WITH-AMENDMENTS. The three amendments:

**A1 — D5 Path 3 (bounded-generic dispatch via witness tables)
DEFERRED.** Path 1 (receiver-typed) and Path 2 (qualified-named)
ship end-to-end at C4.2 (2/N) via direct LLVM-call dispatch
through per-impl mangled fns. Path 3 needs the `<W: Writer>`
bounded-generic syntax — a substantial new surface (parser +
AST + resolve + type-param-with-bound + types + monomorphisation
extension) that's its own sub-iteration. Defers to a Phase C4
follow-on or a Phase C5 amendment.

**A2 — D9 witness-table values not emitted.** The witness-table
struct types + global values are scaffolding for Path 3
dispatch. With Path 3 deferred (A1), there's no consumer of the
witness tables at C4.2 minimum — codegen ships direct-call
dispatch only. The mangled fn naming is forward-compatible
(adding witness tables later doesn't change per-impl symbol
names).

**A3 — D7 `Type::TraitSelf(TraitId)` interner SHIPPED but
unused at runtime.** The interner-table-style variant lands in
the typed-AST `Type` enum (preserving Copy+Hash). At C4.2
minimum, trait method sigs don't reference `Self` in their
params/returns — `self: &Self` and `self: &mut Self` are
captured positionally via the `self_kind` field (mirroring the
C4.1 A2 amendment that deferred general `Self` in type
position). The interner is in place for the eventual lift; no
substitution paths exercise it yet.

**A4 — D7's STRUCT half was never mirrored into the
self-hosted compiler.** D7 says `Self` inside an
`impl as Trait for Type` body is `Type::Class(class_id)` **or**
`Type::Struct(struct_id)`. The Rust `snc` has always done both;
`scg` did only the first. Its impl table recorded a ClassId and
had no struct counterpart, so a struct target left -1 wherever
the target was consulted, and the gap showed up twice:

- the types dump emitted the literal `" class#"` and then the
  id — and with -1 the id printed as NOTHING, because
  `append_int`'s `while v > 0` loop produces no digits for a
  negative value. `scg` said `class#` where the oracle says
  `struct#0`: a tag that lost its id rather than showing a
  wrong one.
- `self` was bound as `mk_class(c, -1)`, a type that is neither
  a struct nor a class, so a `self.<field>` read fell to
  `class_field_index(c, -1, …)` and the stage ABORTED with
  `index out of bounds: idx=-1, len=0`.
- and a THIRD, found by probing rather than reported, which is
  the only one of the three that picks a WRONG answer rather
  than an absent or crashing one: receiver-typed impl-method
  DISPATCH keyed on the class id alone. A struct receiver has
  `cid == -1`, and every struct-target impl also records
  `imcid == -1`, so the FIRST struct-target impl matched EVERY
  struct receiver. With a single struct in the program that is
  correct by accident — which is exactly why the obvious repro
  misses it — and with two, scg dispatched to the wrong impl.
  The same lookup was wrong a second, independent way: it
  returned the first matching TARGET, where D6 Path 1 routes
  receiver-typed dispatch through the DEFAULT impl only (with a
  named impl as a type's sole impl, the oracle rejects
  `x.go(..)` outright). So a named impl declared BEFORE the
  default one was dispatched to instead — and THAT half was
  pre-existing and reproduced on CLASS receivers too, untouched
  by the struct work. Re-keying the lookup fixes both.

Fixed by recording the StructId alongside the ClassId
(`imsid`, ImplId-parallel with `imcid`; AT MOST one of the pair
is >= 0 — a name resolving to neither leaves both -1, which the
oracle rejects outright so no differential sees it — class
checked first, as the oracle's resolver does).
The first two then fall out of dispatches that already existed:
the printer picks its tag from whichever id is set, and
`dump_te_field` was already testing `struct_of_handle(..) >= 0`,
so it needed no change at all. The third needed the lookup
itself re-keyed — `impl_for_class(cid, q)` becomes
`impl_for_target(cid, sid, q)`, matching on whichever id the
RECEIVER has and, importantly, matching NOTHING when it has
neither rather than falling onto the first class-less impl.
Pinned by `tests/pass/c42_impl_for_struct`, which interleaves
struct and class targets, default and named, AND uses two
DISTINCT structs — the two id vectors are parallel, so a
one-sided push shifts every later impl's target, and a single
struct would make the dispatch bug invisible.

Nothing in the corpus had a struct-target impl, which is why
every differential was green while this sat there. Note also,
found while pinning it: `snc build` (inkwell) compiles and runs
such a program, but the TEXT oracle `snc llvm` refuses it with
`impl on a non-class target` — so the two Rust back ends
disagree about whether it is compilable, and the codegen
differential skips it. That asymmetry is pre-existing and filed
separately; it is why A4 is a FRONT-END mirror only.

**A5 — the TEXT oracle refused struct-target impls that
inkwell compiled, and the refusal was self-fulfilling.**
`snc llvm` matched only `ImplTarget::Class` when binding an
impl method's `self`, erroring `impl on a non-class target` on
anything else. Inkwell had the correct two-arm form all along
(`Class(cid) => Type::Class(cid)`,
`Struct(sid) => Type::Struct(sid)`), compiled such programs and
ran them — so the two Rust back ends disagreed about whether a
struct-target impl was even compilable.

The comment justifying the refusal said "every corpus impl
targets a class", and that was TRUE only because of the refusal
itself: `selfhost_codegen` skips any fixture the oracle errors
on, so no struct-target impl could ever be compared, so none was
ever added. A4 broke the loop by adding
`tests/pass/c42_impl_for_struct` (which the front-end sweeps do
compare); A5 removes the refusal so codegen compares it too.

`self_class: ClassId` was used in exactly one place in
`dump_method`, so it generalised to the self TYPE with no other
churn, and `mangle_impl_method` already built the symbol from
`imp.type_name` — a struct target simply contributes its own
name, needing no new mangling. The emitted body is structurally
identical to the class twin, differing only in `%Struct.0` for
`%Class.0`.

**The two halves had to land together.** Teaching the oracle
un-hid an abort in `scg`: `cg_emit_method_sym`,
`cg_emit_impl_mcall` and `cg_emit_qcall` all read `imcid` /
`cgm_type_cid`, which is -1 for a struct target, and
`cg_emit_snb_clsname` then indexed `clns[-1]`. Confirmed by
building the stage and running it — exit 127,
`index out of bounds: idx=-1, len=1`. So A5 also adds
`cg_emit_snb_structname`, a `cg_emit_target_name` dispatch, and
a `cgm_type_sid` set beside every `cgm_type_cid`.

That last point is sharper than it sounds, because this field is
SET rather than pushed: a site that sets the class id and forgets
it leaves a STALE StructId from the PREVIOUS item, mangling a
symbol under the wrong type name rather than failing. Exactly one
of the three reset sites can actually change output — the
delegate forwarder's, because forwarders are synthesised in a
LATER group than user impls, so on entry the state still holds
the last user impl's target. The review pointed out that no
corpus program had both a delegate and a struct-target impl, so
that one line was unpinned. `c42_impl_for_struct` now ends with
its struct impl (making the stale id a struct's) and adds a
delegating class, and the pinning was MUTATION-TESTED: deleting
the reset makes the forwarder emit
`define i64 @default__Sink__Writer__write` — the struct's name,
colliding with the real impl — instead of
`@default__Relay__Writer__write`.

Oracle and scg are byte-identical on the fixture, and the whole
corpus stays byte-identical at codegen.

**What A5 WIDENS rather than fixes, stated so the scope is not
over-read.** Removing the refusal makes struct-target impls a
second spelling for two PRE-EXISTING holes, neither introduced
here and both reproducible without any impl at all:

- an effecting METHOD still has no `Kont*` ABI, so a
  struct-target impl method with an effect row emits
  `ret i64 %v0` on a `ptr`. The CLASS twin emits the identical
  invalid IR today, which is how we know A5 did not cause it;
  it is the hole already filed from the ADR 0072 work.
- the borrow checker accepts a value-typed copy of a borrowed
  Move-field struct, so the copy becomes a second owner. A
  struct-target impl gives that a shorter spelling because D7
  types `self` as the bare struct — but a plain free function
  with no trait, impl or class reproduces it, and it breaks both
  back ends, so it is a front-end hole. Reported through the
  private channel per CONTRIBUTING.md rather than described
  further here.

A5 does not widen either hazard's underlying reach: both were
already reachable through classes or free functions.

⚠ Note what is NOT claimed: the oracle's emitted text hardcodes
an `arm64-apple-darwin` triple for a reproducible byte-target,
so on a Windows host its IR cannot be assembled and RUN. A5
rests on the IR assembling cleanly (`llvm-as`, reading stderr),
on being structurally identical to the class lowering, on
inkwell's build of the same program exiting 42, and on scg
matching byte-for-byte — not on executing the oracle's own
output.

These amendments don't block subsequent sub-phases. C4.3
(delegation) and C4.4 (structured concurrency) compose with the
shipped Path 1 + Path 2 dispatch without depending on Path 3.

Other D-decisions all landed cleanly: D1 + D2 (trait grammar)
exercised by the c42_trait_basic + c42_go_no_go fixtures + 9
resolve/types rejection paths; D3 + D4 (default + named impl
grammar) exercised by the same fixtures; D6 (method-call
resolution algorithm) exercised by `s.write(10)` falling
through class-method lookup to default-impl lookup, and by
`Doubling::write(&mut s, 16)` routing through the named-impl
table; D8 (typing pipeline) exercised by the new Pass 0d, 3c,
3d, 6 — each producing the documented diagnostics;
D10 (out-of-scope list) honored — default method bodies,
supertraits, generic traits, `dyn Trait`, impl-for-generic,
cross-module coherence, associated types, where-clauses all
deferred; D11 (no new lexer tokens) — `trait`/`impl`/`as`/`for`
reserved at C4.0 are the activated set; D12 (phase-go fixture)
runs at exit 42.

Implementation footprint vs estimate: ADR 0021 D14 estimated
"2-3 sessions" for all of C4.2. Actual was ~2 sessions
combined (~1 session each for C4.2 (1/N) parser layer and C4.2
(2/N) resolve+types+codegen). The C4.1 → C4.2 amortisation was
real — the typed-AST parallel-tree pattern + the `Type::Class`
+ `ClassData` precedent made the trait + impl shapes
mechanical to mirror.

## Revisit

This ADR was **PROPOSED** until C4.2 close (now
ACCEPTED-WITH-AMENDMENTS). Per-D revisit
triggers:

- **D1 (trait declaration grammar)**: revisit if user
  testing surfaces a need for default method bodies or
  supertraits.
- **D4 (named impls)**: revisit if explicit
  `Buffered::method` syntax causes friction at the call
  site. The `obj.method(args)` form with named-impl
  inference is the natural alternative.
- **D5 (dispatch)**: revisit if bounded-generic + named-impl
  pairing surfaces a use case. The turbofish form per Path 3
  amendment is the planned extension.
- **D7 (Self in trait contexts)**: revisit if
  `Type::TraitSelf(TraitId)` complicates the type universe.
  An alternative is a synthetic `Type::TypeParam` scoped to
  the trait declaration — cheaper but conflates with
  generic-fn type-params.
- **D8 (typing pipeline)**: revisit if Pass 0d / Pass 3c-d
  / Pass 6 introduce ordering bugs. The conservative
  fallback is to merge impl signature + body checks into
  one pass per impl (giving up the cross-impl method-call
  optimisation).
- **D12 (phase-go program)**: revisit at C4.2 close if the
  fixture proves insufficient to pin the surface.

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; actual is
unknown until C4.2 ships.

  - **AST** (~100 LOC):
    - `TraitDecl { name, methods, span }` +
      `TraitMethodSig { name, self_kind, params,
      return_type, effect_row, span }`.
    - `ImplDecl { name: Option<String>, trait_name,
      type_name, methods, span }` +
      `ImplMethodDef { name, self_kind, params,
      return_type, effect_row, body, span }`.
    - `ExprKind::QualifiedCall { impl_name, method, args }`
      for `ImplName::method(args)` form.
    - `Program.traits: Vec<TraitDecl>` +
      `Program.impls: Vec<ImplDecl>` alongside
      `Program.classes` / `.structs` / `.effects`.

  - **Parser** (~200 LOC):
    - `parse_trait_decl` + `parse_trait_method_sig`.
    - `parse_impl_decl` (handling both default and named
      forms; the optional `Ident` before `as` distinguishes).
    - `parse_postfix` extension for the
      `Ident::method(args)` qualified-call form (the
      `ColonColon` token followed by `Ident` then `LParen`
      promotes to QualifiedCall; the existing `Name::init`
      handling distinguishes init vs method).

  - **Resolve** (~200 LOC):
    - `TraitId(u32)` + `ImplId(u32)` interners.
    - `ResolvedTraitDecl` + `ResolvedImplDecl` parallel-
      tree types.
    - Per-(scope, trait, type) impl table.
    - Method-call resolution per D6.
    - `RedefinedTrait`, `UndefinedTrait`,
      `DuplicateDefaultImpl`, `DuplicateImplName`,
      `ImplMissingMethod`, `UndefinedImpl`,
      `UndefinedTraitMethod` ResolveError variants.

  - **Types** (~250 LOC):
    - `Type::TraitSelf(TraitId)` interner extension
      (preserves Copy + Hash).
    - `TraitData` + `ImplData` typed-program tables.
    - Impl-method signature check against trait method sig
      (D3 / D4 completeness + signature-match).
    - Bounded-generic dispatch (D5 Path 3 default case)
      threading through the C1.7.5 monomorphisation
      worklist.
    - `ImplMethodSignatureMismatch`,
      `AmbiguousMethodCall`, `ImplMethodReceiverMismatch`
      TypeError variants.

  - **Codegen** (~200 LOC):
    - Per-impl LLVM fn generation (mangled name scheme).
    - Witness-table struct types + global values per
      impl.
    - Receiver-typed dispatch lowering (D5 Path 1).
    - Qualified named dispatch lowering (D5 Path 2).
    - Bounded-generic monomorphisation with default-impl
      witness-table threading (D5 Path 3 default case).

  - **Tests + fixtures** (~80 LOC):
    - c42_go_no_go.sentinel (D12).
    - c42_trait_basic.sentinel.
    - c42_bounded_generic.sentinel.
    - 4 UI fixtures (impl_missing_method,
      impl_method_sig_mismatch, duplicate_default_impl,
      duplicate_impl_name).
    - +unit tests across AST / parser / resolve / types /
      codegen.

  - **Total at C4.2 minimum**: ~750-1050 LOC across crates.
    Aligned with ADR 0021's D14 estimate of "2-3 sessions"
    for C4.2 — comparable to C4.1 in volume.

After C4.2: **ADR 0024 PROPOSED** at C4.4 open — the
Async effect surface + scheduler design. C4.3 (delegation)
will land between C4.2 close and C4.4 open per ADR 0021 D14;
delegation uses the impl-table infrastructure from C4.2 +
adds auto-forwarder codegen, so no separate detail ADR is
needed.
