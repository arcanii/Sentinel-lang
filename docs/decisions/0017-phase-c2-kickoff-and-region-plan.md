# ADR 0017: Phase C2 kickoff — references, mutability, regions, borrow checking

Status: **ACCEPTED-WITH-AMENDMENTS.** All 14 D-decisions
exercised across C2.0.1 → C2.5. Amendments captured below in
the "Amendments at C2.5 close" section: a recursive-field-drop
implementation gap closed at C2.5(a); a known
soundness gap (partial move through field projection + drop ⇒
double-free) documented for closure in a follow-on sub-phase;
the Polonius migration plan landed as separate ADR 0018.

D-decision progress: **D1-D14 all exercised.** C2.0.1 (d7b18c2)
+ C2.0.2 (9516ebb) + C2.1 (64edf3d) + C2.2 (4a0ca92) + C2.3
(50c826b) + C2.4 (8d72679) + C2.5 (this session). D1-D5 + D11 +
D12 cover the refs + mut + deref + assignment surface. D10
covers the lexer additions. D6 fully at C2.2 — shared-only at
C2.1 + `&mut T` + shared-XOR-mutable rule at C2.2. D7's second-
class-refs rule enforced via `BorrowError::ReturnsLocalRef`
(C2.1). **D8 exercised at C2.4** — RAII / drop at scope exit
+ `sentinel_free` runtime symbol, closing the C1.6+ heap-leak
deferral. **C2.5(a) closure**: recursive field drop for
`Type::Struct` + `Type::GenericInstance` — codegen's
`emit_drop_struct_fields` now iterates declared fields, GEPs
into each, and recursively drops heap-backed fields (Array,
?Struct, nested struct). Threading is `emit_scope_drops →
emit_drop_for_binding → emit_drop_struct_fields` with
`&TypedProgram` plumbed through. **D9 exercised at C2.3** —
move semantics + use-after-move + branch-aware merge. Eighth
`BorrowError` variant `UseAfterMove` with three-label miette
diagnostic. **D14 exercised at C2.5** — the c25_go_no_go fixture
exercises shared + mut borrows, move, and recursive field drop
in one program (struct with `[i64]` field, shared sum, move-
into-consume, `&mut` add-into-accumulator); prints `190`, exits
0.

Date: 2026-05-27
Last touched: 2026-05-28 (C2.5 close — D14 exercised: full
phase-go program combining XOR + move + drop; ADR
PROPOSED → ACCEPTED-WITH-AMENDMENTS; Polonius migration plan
shipped as ADR 0018; partial-move-through-field-projection
soundness gap documented in
docs/borrow-check-limitations.md for closure in a follow-on
sub-phase. Phase C2 closes.)
Related: 0011 (Phase C1 kickoff — now ACCEPTED, with the
parallel-tree pattern + ADR-first norm + interned-instance
trick all inherited here), 0016 (C1.7 generics — the interned
`GenericInstanceId` pattern is the precedent for C2's
`RefId` proposal in D11), 0015 (C1.6 arrays + heap runtime —
its D9 `sentinel_alloc` machinery is reused at C2 with the new
`sentinel_free` symbol per D8 of this ADR; the C1.6+ "no free,
arrays leak" deferral closes here), 0014 (C1.5 `?T` — its heap-
indirect `?Struct` payload from ADR 0015 D11 needs free at
region exit, also closing here)

## Context

C1 closed with a type system covering primitive scalars +
nominal structs + nullable types + heap-backed arrays + witness-
table generics. Every value is owned by its declaring scope and
copied freely (no move semantics; everything is implicitly
`Copy` at runtime, which means a struct passed to a fn is bit-
copied into the callee's frame). Heap allocations from C1.6
(arrays + `?Struct` payloads) **leak** at end of program — there
is no `sentinel_free`, no Drop, no resource management.

C2 introduces the substrate for Sentinel's memory-safety
guarantees at the language layer:

  - **References** (`&T` shared, `&mut T` exclusive) per the
    SENTINEL_DESIGN2 §4.2 ownership model.
  - **Mutability** distinct from binding (`let mut x = ...`,
    `&mut T` for exclusive borrow, immutable-by-default).
  - **Regions** — initially lexical (scope = region implicitly);
    named regions (`@stack`, `@heap`, `@arena<A>`) per
    SENTINEL_DESIGN2 §4.1 deferred to C2.last or C3.
  - **Borrow checking** — shared-XOR-mutable rule, use-after-
    move detection.
  - **Drop / RAII** — values dropped at end of their region;
    heap allocations freed via `sentinel_free`. Closes the C1.6+
    heap-leak deferral.

The 1.0 target surface (SENTINEL_DESIGN2 §4.1-§4.2) is named
regions with second-class references by default and first-class
refs via explicit `'esc` binding. C2 ships a **subset** — lexical
regions only, second-class everywhere — leaving named regions
and `'esc` for a later phase (C2.last or a follow-on C2.x ADR).

The HANDOVER §6.2 budget is "month 6-9" of the original 18-month
plan — three months. Per ADR 0011's "honest" retrospective
(estimated 22-28 weeks for C1, actual ~10-12 sessions), the
real C2 estimate is roughly 6-10 sessions across 5-6 sub-phases.
Borrow checking is genuinely new territory (compare Rust's
multi-year NLL development); the C1 infrastructure investment
(Salsa + per-pass crates + parallel-tree pattern + monomorphic
codegen) carries forward but doesn't compound on the borrow-
checker design itself.

What this ADR explicitly does NOT commit to at C2: named
regions (`@stack`, `@heap`, `@arena<A>`, `@static`, `@shared`,
`@gpu`, `@numa(n)`), first-class refs via `'esc` binding,
lifetime parameters on fns / structs / traits (`<'a, T>`), `rc
T` / `arc T` reference counting, NLL / Polonius precision
(C2 ships lexical borrow checking first; precision refinements
land later), `unsafe` blocks, raw pointers, interior mutability
(`Cell` / `RefCell`-style), inout / borrow-on-call semantics
(Swift's `inout` rejected in favour of explicit `&mut`),
multi-region inference, escape analysis for region promotion.

The C1.7 lexer's current token set is:

  - **Keywords**: `let`, `fn`, `if`, `else`, `true`, `false`,
    `struct`, `null`
  - **Punctuation**: `+ - * / = ( ) { } [ ] , ; : . ? ->`
    `== != < <= > >= && || !`
  - **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`
  - **Integer literals**: `[0-9]+`
  - **Skipped**: `[ \t\r\n]+`, `//[^\n]*`

C2 extends this with the additions in D10 below.

## Decision

Fourteen D-numbered sub-decisions covering syntax (D1-D5),
borrow-checker formulation (D6), region representation (D7),
drop semantics (D8), the sub-phase split (D9), lexer additions
(D10), type representation (D11), out-of-scope items (D12), the
fn main invariant (D13), and the phase-go program (D14).

### D1. Reference syntax: `&T` shared, `&mut T` exclusive.

Reference types use Rust-style notation: `&T` for a shared
(immutable) borrow, `&mut T` for an exclusive (mutable) borrow.
Grammar extension at the type-expr level:

    type         = base_type | '?' type | '[' type ']'
                 | '&' 'mut'? type
                 | Ident type_args?

Whitespace tolerance matches the rest of the grammar: `& T`,
`&mut T`, and `& mut T` all parse identically.

Nested references (`& &T`) parse but are rejected at the type-
check stage as `TypeError::NestedRef` per the standard
"depth-1 only" pattern Sentinel has held since C1.5 (`?(?T)`
rejected, `[[T]]` rejected). Future ADRs may relax.

`&` and `&mut` compose with `?T` and `[T]`:
  - `?&T` (nullable ref) — valid. Parses; type-checks. Nullable
    has refs as a payload.
  - `?&mut T` — valid.
  - `&?T` (ref to nullable) — valid.
  - `[&T]` (array of refs) — valid; second-class refs in array
    payload have a subtle scope issue: the array can outlive the
    refs. C2 handles this by requiring array elements to be
    `Sized + 'static`-equivalent; refs in arrays are rejected
    with `TypeError::RefInArray` until first-class refs land.
  - `&[T]` (ref to array) — valid; the array's len + data
    pointer flow through.

### D2. Mutability: `let mut x` for mutable bindings; `mut` param prefix.

A binding declared with `let mut x = ...` is mutable —
re-assignment (`x = new_value;`) and exclusive borrow (`&mut x`)
are allowed. Bare `let x = ...` produces an immutable binding;
mutation surfaces as `TypeError::AssignToImmutable` or
`TypeError::ExclusiveBorrowOfImmutable`.

Grammar at the stmt level:

    let_stmt     = 'let' 'mut'? Ident (':' type)? '=' expr ';'

Param mutability uses the same `mut` prefix:

    param        = 'mut'? Ident ':' type

Mutable params let the callee re-assign the local copy without
affecting the caller (Rust semantics — `mut` on params is
binding-local, not borrow-related). This is distinct from the
caller passing a `&mut T` reference, which IS observable to the
caller.

Assignment statements use the existing `=` token:

    assign_stmt  = (Ident | postfix_expr) '=' expr ';'

LHS can be a bare ident (variable), `*ref` (deref-assign), or
`expr.field` (field-assign). Indexing assign (`a[i] = ...`) is
**out of scope at C2** per D12.

### D3. Reference-take: `&expr` and `&mut expr` prefix operators.

Reference-take is a prefix unary operator at the same precedence
as `-` (unary negate) and `!` (logical not):

    unary        = ('-' | '!' | '&' | '&' 'mut') unary | atom

`&x` produces `&T` where `x: T`. `&mut x` produces `&mut T`.
The expression `x` must be an lvalue — bare ident, field
access, deref, or index. R-value borrowing (`&5`, `&(a + b)`)
is rejected with `TypeError::BorrowOfRvalue` — the C2 minimum
viable rejects this case; later ADRs may add temporary-promotion
(Rust-style) for ergonomics.

The double-borrow rule (shared-XOR-mutable) is enforced at the
borrow checker per D6.

### D4. Dereference: `*expr` prefix operator.

`*r` where `r: &T` or `r: &mut T` produces an lvalue of type T.
Grammar:

    unary        = ('-' | '!' | '&' | '&' 'mut' | '*') unary | atom

Auto-deref (Rust's `r.field` working when `r: &Struct`) is
**out of scope at C2** — the user writes `(*r).field` or just
`r.field` syntactic-sugar may be a future ADR. C2 requires
explicit `*r` to dereference.

Implicit deref-coercion at call sites (`fn f(x: T); f(&x)` —
NO; the user must write `f(*ref)` if they have a ref) is
likewise out of scope.

### D5. Lvalue vs rvalue distinction.

C2 introduces the lvalue / rvalue distinction at the type-check
stage. Lvalues:

  - Bare variable (`x`)
  - Field access on lvalue (`p.field` where `p` is an lvalue)
  - Dereference of any ref (`*r`)
  - Index of array (`a[i]` — out of scope at C2 for mutation per
    D12; but `a[i]` as a read-lvalue works)

Rvalues are everything else (literals, arithmetic, calls, etc.).

The rule:

  - Assignment LHS must be an lvalue.
  - `&` and `&mut` operands must be lvalues.
  - All other uses accept both (rvalues are implicitly read).

New `TypeError` variants: `AssignToRvalue`, `BorrowOfRvalue`.

### D6. Borrow-checker formulation: lexical first, Polonius later.

The substantive design call. The two ends of the spectrum:

  - **Lexical**: a borrow's lifetime is "from creation to the
    end of the enclosing block". Simple to implement (~few
    hundred LOC). Less precise — rejects programs that should
    work (the classic "borrow lives past last use" case). What
    Rust shipped before NLL (2018).
  - **Polonius**: dataflow-based, origins / loans tracked
    through the control-flow graph. Precise — accepts most
    programs a programmer would expect to compile. Substantially
    more complex (multi-year Rust dev cycle); HANDOVER §6.2
    names this as the long-term target.

**C2 ships lexical first, with explicit migration to NLL /
Polonius as a later sub-phase (C2.5) or post-C2 ADR.** Reasoning:

  - Lexical is "wrong but useful": the rejected programs all
    have a polite local workaround (introduce a new scope; copy
    the value). The C2 minimum-viable can ship without choking
    on edge cases.
  - The lexical implementation is bounded (~few-hundred LOC) —
    fits the per-sub-phase rhythm Sentinel has held since C1.4.
    Polonius alone could take 4-6 sessions; bundling it with C2
    would overshoot ADR 0011's "honest" budget.
  - Polonius's design has stabilised over the last several years.
    Migrating from lexical to Polonius is a known-shape
    refactor: replace the lexical analysis with the dataflow
    pass, keep the borrow-checker's diagnostic surface stable.
  - HANDOVER §6.2 says "Use the Polonius formulation" but
    that's a long-term commitment, not a "must ship in C2.0"
    constraint. The phase ends with both lexical + a documented
    Polonius migration plan.

The borrow checker runs as a new pass between the type-checker
and codegen:

    parse_query → resolve_query → check_query → borrow_check_query → codegen

It consumes a `TypedProgram` and produces a `BorrowCheckedProgram`
(parallel tree, just adding lifetime + move-state metadata) or
a `BorrowError`. New error variants: `BorrowConflict`,
`UseAfterMove`, `BorrowOutlivesScope`, `MutableBorrowOfShared`,
`SharedBorrowOfMutable`.

### D7. Regions: lexical at C2 minimum; named regions deferred.

C2 ships **lexical regions only** — every scope is implicitly a
region, with refs valid for that scope's extent. There is NO
named-region syntax (`@stack`, `@heap`, `@arena<A>`, `@shared`)
at C2 minimum.

This is a deliberate gap with SENTINEL_DESIGN2 §4.1's full
design (named, visible regions). The full design is the long-
term target; C2's minimum-viable surface is a stepping stone.

Implications:

  - The `Type::Ref` variant (per D11) doesn't carry a region tag
    explicitly. Lifetime is determined positionally by the
    enclosing scope at borrow-check time.
  - Error messages reference SCOPE BOUNDARIES (`"this borrow
    must end at the closing brace of fn foo"`) rather than
    region names. The user-facing diagnostic stays
    comprehensible.
  - The first-class-ref problem (refs escaping their source via
    storage in heap structures or return-past-source) is
    handled by D6's "second-class everywhere" rule: arrays /
    struct fields can NOT hold refs at C2 (per D1's
    `RefInArray` rejection; same for struct fields holding
    refs — `TypeError::RefInStructField`). This punts first-
    class refs to the named-region ADR.

The migration plan: when named regions arrive (call it ADR
0019 or 0020), `Type::Ref` extends with a `RegionId` field;
lexical regions become a special case (`RegionId(0) = "current
scope"`); explicit `'esc` becomes a separate RegionId class. The
borrow checker's lifetime logic gets a region-graph extension.

### D8. Drop / RAII: closes the C1.6+ heap-leak deferral.

Heap-allocated values from C1.6 (arrays, `?Struct` payloads
per ADR 0015 D11) currently leak. C2 closes this by introducing
RAII drop tied to lexical region exit:

  - At the end of every scope, owned values are dropped.
  - For heap-backed values (`[T]`, `?Struct`), drop calls
    `sentinel_free(ptr)` — a new runtime symbol added to
    sentinel-runtime alongside the C1.6 `sentinel_alloc` and
    `sentinel_panic_oob`.
  - For struct-by-value, drop recursively drops each owned
    field.
  - References are NOT dropped (they don't own).
  - Move semantics (D9 below) interact with drop: a moved-from
    value is not dropped at its original scope's exit because it
    was moved (state-tracked in the borrow checker).

The `sentinel_free(ptr: ptr) -> void` runtime symbol:

  - Takes the pointer returned by an earlier `sentinel_alloc`.
  - Calls `libc::free` (the matching pair).
  - Returns void.

No epoch tracking; no generational handles; no broker
integration at C2. Those are C5 territory.

Drop order within a scope: reverse declaration order (Rust
convention).

`fn drop(&mut self)` user-defined destructors are **out of scope
at C2** — the auto-generated drop is sufficient for the C1
type universe. User Drop trait lands at C4 (alongside other
traits).

### D9. Move semantics + `own T` defaults.

Bindings of compound types (`struct`, `[T]`, `Pair<...>`)
implicitly own their data. Re-assignment or pass-by-value
**moves** the data:

    let p1: Point = Point { x: 1, y: 2 };
    let p2: Point = p1;        // p1 moved into p2
    print(p1.x);               // ERROR: use-after-move

Primitives (`i64`, `i32`, `bool`) are `Copy` and pass freely
without consumption.

Move detection is part of the borrow checker (D6): track each
binding's move state (`Live` vs `Moved { at_span }`). On any
read of a moved binding, surface `TypeError::UseAfterMove
{ binding, original_span, move_span, use_span }`.

The `own T` keyword from SENTINEL_DESIGN2 §4.2 is **implicit**
at C2 — every owned binding is conceptually `own T` but the
keyword isn't surfaced in the C2 grammar. (Future ADR may
introduce `own T` explicitly for ergonomics with `rc` / `arc`.)

References (`&T`, `&mut T`) are `Copy`-like (sharing is free)
but constrained by the borrow checker.

### D10. Lexer additions at C2.

Two new tokens, one new keyword:

| Token / Keyword | Notes                                       |
| --------------- | ------------------------------------------- |
| `&`             | Ampersand — borrow-take + ref-type prefix    |
| `*`             | Star — already exists for multiplication;    |
|                 | parser disambiguates dereference vs multiply |
|                 | by position (prefix unary vs infix binary)   |
| `mut`           | Keyword — `let mut`, `&mut T`, `mut x: T`    |

`*` already exists from C0 arithmetic — no new lexer token.
The parser handles the dereference role positionally (unary
prefix). Same disambiguation strategy as C1.3's `-` (unary
negate vs subtract).

`'a` lifetime syntax (e.g., `'static`, `'a`) is NOT added at
C2 — D6's lexical-only borrow checker doesn't need it. Named
regions / explicit lifetimes will need a separate lexer
addition (`'` prefix + Ident lex pattern) in a later ADR.

### D11. Type representation: interned RefId following C1.7.4b pattern.

The substantive design call for the type system. C2 adds a
reference type that fundamentally needs a recursive `Type` (a
`&T` where T can itself be any Type, including another ref).
Two representations:

  - **Box-based**: `Type::Ref { mutable: bool, inner: Box<Type> }`.
    Breaks `Type: Copy`. ~30-50 sites in the codebase need
    `.clone()` updates. Mechanical refactor.
  - **Interned**: `Type::Ref(RefId)` with `RefData { mutable,
    inner: Type }` stored in a program-level
    `TypedProgram.refs: Vec<RefData>` table. Preserves `Type:
    Copy`. Same pattern as C1.7.4b's `GenericInstanceId`.

**Decision: interned, following the C1.7.4b precedent.**
Reasoning:

  - The codebase has resisted breaking `Type: Copy` for four
    ADRs (0014 D4 amendment, 0015 D6 amendment, 0016 D6a
    interned-instance, and this ADR). The interned pattern is
    proven, mechanical, and consistent across the type-system
    extensions.
  - Sites that consume `Type` by value (codegen,
    type-display, comparisons) all benefit from `Copy`. Switching
    to non-Copy would force a sweep across sentinel-codegen
    (~1400 LOC) + sentinel-types (~2500 LOC).
  - The interned-RefId table has a natural lookup helper
    (`TypedProgram::ref_data(id) -> &RefData`), mirroring the
    existing `signature(id)` / `struct_decl(id)` / `generic_instance(id)`
    accessors.

The internment scheme:

  - `pub struct RefId(pub u32)` — `Copy + Hash`.
  - `pub struct RefData { pub mutable: bool, pub inner: Type }`.
  - `TypedProgram.refs: Vec<RefData>`.
  - `pub fn intern_ref(refs: &mut Vec<RefData>, mutable: bool,
    inner: Type) -> RefId` — linear search through the table;
    HashMap interning is a future profile-driven optimization
    (same as ADR 0016 D6a's GenericInstance).

`Type::Ref(RefId)` joins the existing variants:

    enum Type {
        I64, I32, Bool, Struct(StructId),
        Nullable(NullableInner),
        Array(ArrayElem),
        TypeParam(TypeParamId),
        GenericInstance(GenericInstanceId),
        Ref(RefId),          // C2 / ADR 0017 D11
    }

`NullableInner` and `ArrayElem` each gain a `Ref(RefId)` variant
for `?&T` and `[&T]` (though `[&T]` is rejected at C2 per D1's
`RefInArray` rule; the variant exists for future extensibility).

`Type::substitute` extends to recurse through `Ref` args.
`Type::is_ref(self) -> bool` helper added. The C1 invariants
(`Type::Copy + Hash`) survive C2.

### D12. Out of scope at C2.

The following reference / region features are explicitly
deferred:

  - **Named regions** (`@stack`, `@heap`, `@arena<A>`,
    `@static`, `@shared`, `@gpu`, `@numa(n)`). Deferred to a
    later ADR (call it ADR 0019 or 0020).
  - **First-class refs** via `'esc` binding (refs storable in
    heap structures or returnable past source). Deferred
    alongside named regions.
  - **Lifetime parameters** on fns / structs / traits (`<'a>`,
    `<'a, T>`). Deferred.
  - **`rc T` / `arc T`** reference counting. Deferred to C2.5+
    or a follow-on phase.
  - **Polonius / NLL precision**. C2 ships lexical-only borrow
    checking per D6. Polonius is the long-term target.
  - **`unsafe` blocks + raw pointers**. C5 territory.
  - **Interior mutability** (`Cell`, `RefCell`-style).
    Deferred.
  - **`inout` / call-side mutability** (Swift's `inout`).
    Rejected in favour of explicit `&mut` per D1.
  - **User-defined `Drop`**. Auto-drop only at C2 per D8;
    `fn drop(&mut self)` lands with traits at C4.
  - **Auto-deref** (`r.field` working transparently when `r:
    &Struct`). C2 requires explicit `*r` per D4.
  - **Implicit deref-coercion at call sites**. Out of scope
    per D4.
  - **Mutable indexing** (`a[i] = v`). Sentinel has no
    assignable-index semantics yet; deferred.
  - **R-value borrowing** (`&5`, `&(a + b)`). C2 rejects with
    `BorrowOfRvalue`; later ADR may add temporary promotion.
  - **Refs in arrays + struct fields** (`[&T]`, `struct { f:
    &T }`). Deferred until named regions arrive — these are
    the "first-class refs" case.

### D13. `fn main() -> i64` invariant stays.

Same as ADRs 0012 D11 / 0013 D11 / 0014 D11 / 0015 D13 / 0016
D11. Main returns i64; codegen truncates to i32 for the C ABI.
`fn main()` cannot have ref params (it's the entry point; no
caller).

### D14. Phase-go program spec.

The C2 acceptance fixture at `tests/pass/c20_go_no_go.sentinel`
(named c20 for "C2.0 go/no-go" to distinguish from the c-prefix
C0 fixtures) exercises D1-D9 in a single program:

    fn add(a: &i64, b: &i64) -> i64 {
        *a + *b
    }

    fn increment(x: &mut i64) -> i64 {
        let new_val: i64 = *x + 1;
        *x = new_val;
        *x
    }

    fn main() -> i64 {
        let mut a: i64 = 10;
        let b: i64 = 32;
        let sum: i64 = add(&a, &b);
        let inc: i64 = increment(&mut a);
        print(sum + inc)
    }

Expected: stdout `53\n`, exit 0 (sum = 10 + 32 = 42; inc = 11
after the increment; sum + inc = 42 + 11 = 53). Exercises:

  - Shared ref param (`&i64`) (D1 + D3).
  - Mutable ref param (`&mut i64`) (D1 + D3).
  - `let mut` binding (D2).
  - Reference-take `&a` and `&mut a` (D3).
  - Dereference `*x` (D4).
  - Deref-assignment `*x = new_val` (D4 + D2).
  - The borrow checker (D6): `&a` and `&b` are simultaneous
    shared borrows (XOR rule says both shared = OK); `&mut a`
    comes later, after `sum` is computed (sequential borrows
    don't overlap).
  - Codegen: refs lower as LLVM pointers; deref as load;
    deref-assign as store.

A secondary fixture `c20_drop_arrays.sentinel` exercises D8
(RAII closes the C1.6+ heap leak):

    fn main() -> i64 {
        let xs: [i64] = [1, 2, 3, 4, 5];
        let total: i64 = sum_array(&xs);
        print(total)
        // At end of main, xs is dropped → sentinel_free called.
    }

    fn sum_array(arr: &[i64]) -> i64 {
        // ... (implementation; len + iteration)
    }

Expected: stdout `15\n`, exit 0. The drop semantics aren't
visible in the program output but are testable via valgrind /
leak sanitizer — `c20_drop_arrays.sentinel` runs clean under
leak detection where C1.6's c16 fixtures would have leaked.

## Sub-phase split

A rough split into 5-6 sub-phases. Each sub-phase ADR-first per
the C1 rhythm; concrete D-decisions refined in the sub-phase
ADRs.

| Sub  | Title                                                        | Estimate     | Status |
|------|--------------------------------------------------------------|--------------|--------|
| C2.0 | Infrastructure: `Type::Ref(RefId)` + AST + parser + lexer;   | 1-2 sessions | DONE   |
|      | resolve passes through; types accepts; codegen lowers refs   |              | d7b18c2|
|      | as LLVM pointers. NO borrow checking yet.                    |              | +      |
|      |                                                              |              | 9516ebb|
| C2.1 | Shared-only lexical borrow checker. `&T` only. Lifetime      | 1-2 sessions | DONE   |
|      | tracking. Rejects use-after-scope but no `&mut` yet.         |              | 64edf3d|
| C2.2 | `&mut T` + shared-XOR-mutable rule. The interesting half.    | 2-3 sessions | DONE   |
|      | This is where most borrow-checker complexity lives.          |              | 4a0ca92|
| C2.3 | Move semantics + `own T` defaults + use-after-move.          | 1-2 sessions | DONE   |
|      | Struct + array bindings consume on re-assignment / call.     |              | 50c826b|
| C2.4 | RAII / drop + `sentinel_free`. Closes the C1.6+ heap         | 1-2 sessions | DONE   |
|      | leak. Auto-drop on scope exit.                               |              | 8d72679|
| C2.5 | Polish: NLL/Polonius migration plan documented; consolidation | 0-2 sessions | DONE   |
|      | of corner cases; phase-go program; STATE.md + HANDOVER       |              | this   |
|      | close-out. Recursive field drop closure (C2.5(a)).           |              | session|

Honest total: 6-13 sessions estimated across 5-6 sub-phases.
**Actual: ~6 sessions** (C2.0.1 + C2.0.2 + C2.1 + C2.2 + C2.3
+ C2.4 + C2.5 — six feat commits plus the C2.0 infrastructure
split into .1 and .2 to keep the lexer-only and refs-
infrastructure commits coherent). Lower end of the estimate
range. Compare ADR 0011 D6's "C1.7 estimate 4-6 weeks; actual
~1 session" — the C1 retrospective's pattern (ADR-first +
parallel-tree + interned-pattern rhythm compresses estimates
significantly) held for C2 as well, though less dramatically:
borrow checking IS novel machinery and the C2.2 + C2.3 sub-
phases used most of their per-sub-phase budget. The
infrastructure investment did compound on the surrounding
pieces (lexer + AST + parser changes were trivial; codegen
plumbing for refs / drops was bounded). The substantive new
work was concentrated in the borrow checker itself, as the
ADR predicted.

## Reasoning

The decisions cluster around four themes.

**Minimum-viable C2 surface.** D1-D5 (refs + mutability + deref +
lvalue/rvalue) ship the syntactic core. D6 (lexical borrow
checker) ships the simplest correct-enough checker. D7 (lexical
regions only) defers the visible-region work that
SENTINEL_DESIGN2 §4.1 calls for. D12 documents what's
deliberately punted. The C2 win is "the language gains the
substrate for memory safety"; the long-term named-region story
follows.

**Continue the C1 design patterns.** D11 (interned RefId)
continues the four-ADR streak of preserving `Type: Copy` via
internment. D8's `sentinel_free` reuses the C1.6 runtime-
symbols pattern. The borrow-check pass slots into the existing
salsa pipeline as a new query between check_query and codegen.
Phase-go fixtures continue the cNN_go_no_go.sentinel naming
(c20 for C2.0).

**Lexical first, Polonius later.** D6's lexical-first
formulation is the calculated bet: most Sentinel programs at
the bootstrap stage will satisfy the lexical rules; rejected
programs have local workarounds; the implementation is
bounded; the migration to Polonius is a known-shape refactor.
HANDOVER §6.2 names Polonius as the long-term target — C2 is
the stepping stone.

**Drop closes the leak.** D8's auto-drop + `sentinel_free`
closes the C1.6+ deferral. Heap-allocated values get cleaned
up at scope exit. This is the smallest correct resource-
management story; user-defined Drop trait waits for C4.

## Consequences

### Positive

- The language gains memory safety primitives. References,
  mutability, borrow checking, drop. This is the core of
  Sentinel's value proposition at the type-system layer.
- The C1.6+ heap-leak deferral closes. Programs no longer
  leak arrays + `?Struct` payloads at end of scope.
- The C1 invariants survive — `Type: Copy + Hash`, the
  parallel-tree pattern, the salsa pipeline, ADR-first per
  sub-phase. C2 inherits all of them.
- The borrow checker is a NEW pass between check_query and
  codegen. Pipeline becomes: `parse_query → resolve_query →
  check_query → borrow_check_query → codegen`. LSP / tooling
  benefits from the additional query.
- D6's lexical-first design lets us ship in 5-6 sub-phases vs
  the multi-year Polonius dev cycle. Sentinel still gets to
  Polonius — it just gets there in a separate ADR with C2's
  surface stable.
- The interned RefId continues the proven C1.7.4b pattern. No
  new design risk.

### Negative

- Lexical borrow checking rejects programs that Polonius would
  accept. Users will hit the "borrow lives past last use" case
  and need to add explicit scopes. Documented as a known
  limitation with the migration plan; same shape as Rust pre-
  2018.
- Named regions deferred means SENTINEL_DESIGN2 §4.1's visible-
  region story doesn't ship at C2. The diagnostic-quality
  advantage of "this borrow must escape its arena" isn't yet
  available. Lexical-scope error messages are still
  comprehensible (see HANDOVER §6.3) but less rich than
  Sentinel's long-term plan.
- D8's auto-drop is positional + recursive. User-defined Drop
  semantics (`fn drop(&mut self)`) wait for C4 traits. Some
  resource-management patterns (e.g., file handles) need a
  workaround in the meantime.
- D11's interned RefId adds a fourth interner table to
  `TypedProgram` (after `fn_signatures`, `generic_instances`,
  and the implicit struct/var/fn-id ranges). Profile-driven
  HashMap interning becomes more relevant as the type table
  grows.
- D9's move semantics + use-after-move detection is a
  significant borrow-checker complexity. C2.3 will be one of
  the harder sub-phases.

### Neutral

- D7's "no named regions" is a deliberate scope-cut, not a
  permanent decision. The migration plan is documented.
- D10's "no new keyword except `mut`" is small; the `*`
  re-use for dereference is a precedent from C1.3's `-` re-use
  for unary-negate-vs-subtract.

## Alternatives considered

- **Ship Polonius at C2.** Rejected per D6: too large a single-
  phase commitment given Sentinel's session budget. Polonius is
  a multi-year endeavour in Rust; Sentinel's path is lexical
  first, Polonius later.

- **Ship named regions at C2.** Rejected per D7: the design
  space is wide (region polymorphism, escape analysis, region
  inference) and SENTINEL_DESIGN2 §4.1's full design isn't
  yet stable enough to commit to a concrete grammar. Lexical
  regions cover the C2 ergonomic needs; named regions follow.

- **Use Rust's NLL formulation as the C2 borrow checker.**
  Rejected as an interim choice: NLL is between lexical and
  Polonius. The migration from lexical → Polonius doesn't
  require NLL as a waypoint. Going lexical → NLL → Polonius is
  three refactors; lexical → Polonius is one. The lexical
  formulation's diagnostic surface stays roughly stable across
  the migration.

- **Box-based `Type::Ref { mutable, inner: Box<Type> }`.**
  Rejected per D11: continues the four-ADR streak of breaking
  `Type: Copy` would force a sweep across codegen + types.
  The interned approach is mechanical and consistent.

- **Refs at C2 without mutability or borrow checking** (just
  `&T` shared as a transparent wrapper). Rejected: defeats the
  purpose of C2. Without enforcement, `&T` is just an alias.
  Sentinel's value proposition requires the borrow checker.

- **Reference counting (`rc T` / `arc T`) at C2 instead of
  refs + borrow checking.** Rejected: `rc` / `arc` are
  ergonomic shortcuts on top of borrow checking; building them
  first inverts the dependency. SENTINEL_DESIGN2 §4.2 lists
  both; C2 builds the foundation, `rc` / `arc` follow.

- **Inout semantics at call sites instead of `&mut`** (Swift's
  approach: caller writes `inc(&x)` and callee receives a copy
  it can mutate, with copy-back on return). Rejected per D1: a
  heavier surface than `&mut`, hides the aliasing cost, and
  fights the broker's region model. Sentinel sticks with
  Rust's explicit `&mut`.

- **Auto-deref at C2** (`r.field` working transparently when
  `r: &Struct`). Rejected per D4: adds a coercion rule that
  complicates the type checker for a small ergonomic gain.
  `*r` is explicit and self-documenting.

- **Auto-`fn drop(&mut self)` at C2.** Rejected per D8: drop
  customisation belongs with traits at C4. The auto-generated
  drop is sufficient for the C1 type universe.

- **Refs in arrays / struct fields without first-class
  region tracking.** Rejected per D7: this is the canonical
  first-class-ref case, which requires named regions or `'esc`
  binding to be sound. Sentinel rejects with `RefInArray` /
  `RefInStructField` at C2 to keep the lexical-only checker
  sound.

## Amendments at C2.5 close

Three amendments to the original ADR, recorded as the ADR
flipped from PROPOSED to ACCEPTED-WITH-AMENDMENTS:

**A1. D8's "recursive drop for struct fields" was deferred from
C2.4 to C2.5(a).** The C2.4 commit shipped `emit_drop_struct_fields`
as a no-op stub — direct array bindings and `?Struct` bindings
dropped correctly, but a struct containing an array field would
leak the inner array. C2.5(a) closed this via threading
`&TypedProgram` through the drop helpers and iterating
`program.struct_decl(id).fields` (with substitution for
`Type::GenericInstance`). Three c25 fixtures + one c25_go_no_go
pin the closure end-to-end. No D-decision text needed updating;
the implementation gap was per-commit, not per-decision.

**A2. The Polonius migration plan shipped as a standalone ADR
0018, not as an appendix to this ADR.** D6's "explicit migration
to NLL / Polonius as a later sub-phase (C2.5) or post-C2 ADR"
language allowed either shape. Standalone won because (a) ADR
0018 is the canonical document for the migration, with its own
D-decisions about trigger / fact generator / sub-phase shape,
and (b) keeping ADR 0017 stable as the C2 design record is
cleaner than appending the migration plan inline. ADR 0018
references back here for the lexical-checker context.

**A3. A previously-known soundness gap — partial moves through
field projection + drop ⇒ double-free — was empirically confirmed
at C2.5(c) and documented in `docs/borrow-check-limitations.md`.**
The C2.3 docstring noted the issue as "slightly unsound for
non-Copy fields but benign at C2.3 since drop hasn't shipped."
With drop shipping at C2.4 + C2.5(a), the gap is no longer
benign: a program like
`consume_arr(p.items)` (where `p: Pair { items: [i64], ... }`)
double-frees the array at main's drop. Closure: a follow-on
sub-phase (provisionally C2.6 or a separate ADR 0019) adds
per-(VarId, FieldPath) move-state. This is genuine post-C2
work that the ADR did not originally call out; flagged here
for visibility. Until it lands, programs that pass Move-typed
struct fields by value to drop-eligible consumers are unsound.

## Retrospective at C2.5 close

Six sub-phases shipped across ~6 effective sessions (split into
seven commits because C2.0 was bundled as C2.0.1 + C2.0.2 for
coherence). The original estimate was 6-13 sessions across 5-6
sub-phases. Came in at the low end.

What worked:

- **ADR-first per sub-phase**. Each C2.x had its design
  decisions written before the code. C2.2 was the largest sub-
  phase by complexity and benefited the most — the XOR rule's
  diagnostic surface needed the most thinking before the
  implementation could ship coherently.
- **The interned-RefId pattern (D11)**. The fifth ADR in a row
  preserving `Type: Copy` via internment paid off again — no
  clone-cascade refactor through codegen or types.
- **The salsa-pipeline shape**. `borrow_check_query` slotted
  into the existing pipeline cleanly. No new infrastructure
  needed; diagnostics flow through the accumulator.
- **The parallel-tree pattern**. C2.0.2's refs additions to
  AST / parser / resolve / types / codegen all followed the
  existing per-pass cadence.

What surprised us:

- **C2.4 split into two sub-phases (C2.4 + C2.5(a)).** The
  recursive-field-drop closure was originally bundled in the
  C2.4 estimate; in practice it slipped to C2.5 because the
  C2.4 commit was already large (DropPlan + sentinel_free +
  three drop fixtures + scope-stack codegen). Splitting kept
  each commit reviewable.
- **The partial-move-through-field-projection unsoundness**
  (A3 above) was known at C2.3 but its severity wasn't
  appreciated until C2.5(c)'s empirical scan. Drop is what
  weaponised the gap. Documenting it instead of fixing now is
  the right call; the fix's scope (per-FieldPath move state)
  approaches half the Polonius migration's fact-generator
  work.
- **The c25 go/no-go fixture surfaced no new issues.** The
  combined-surface program (`consume(b)` after `&b` then
  `&mut acc` + `add_into`) just worked. The C2.x sub-phases
  exercised the relevant integration points individually.

What didn't compound from C1:

- **Borrow-checker complexity** was genuinely new ground. The
  ~1500 LOC of `sentinel-borrow-check` is comparable to
  `sentinel-types` (~3000 LOC) and `sentinel-codegen` (~2500
  LOC) at a per-sub-phase delta. C1's "1-session per sub-
  phase" rhythm didn't carry to C2.2 + C2.3 + C2.4 — each
  took a full session.

Pipeline at C2 close:

    parse_query → resolve_query → check_query
                → borrow_check_query → codegen

Returns `(DropPlan, Vec<BorrowError>)` from borrow_check_query;
codegen consumes DropPlan to skip dropping moved bindings + to
recursively drop heap-backed struct fields per C2.5(a).

## Revisit

This ADR is **ACCEPTED-WITH-AMENDMENTS** after C2's sub-phases
landed. Per-D revisit triggers:

- **D1** (ref syntax): revisit once named regions arrive
  (later ADR adds the `@region` suffix); `&T` syntax stable
  otherwise.
- **D2** (mutability): revisit if param-mut vs binding-mut
  ergonomics surface as confusing in user testing.
- **D6** (lexical first): the explicit revisit trigger.
  Polonius migration is the planned post-C2 work.
- **D7** (lexical regions only): revisit at the named-region
  ADR.
- **D8** (auto-drop): revisit at C4 when traits land — user-
  defined Drop becomes available then.
- **D9** (move semantics): revisit if the C2.3 move-state
  tracking surfaces diagnostic-quality issues. Use-after-move
  errors should reference the original move site clearly.
- **D11** (interned RefId): revisit if profiling shows the
  linear-search interner becomes a bottleneck. HashMap upgrade
  is mechanical.
- **D12** (out-of-scope list): each item gets its own future
  ADR. The named-region ADR is the next-largest single chunk.

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; the actual C2
sub-phase commits will be larger if borrow-checker design
surfaces unanticipated coupling.

  - **C2.0** (infrastructure, ~600-900 LOC):
    - sentinel-ast: +50-80 (TypeExprKind::Ref variant + parser-
      side helpers; let-mut + param-mut + assignment +
      reference-take + dereference AST nodes)
    - sentinel-syntax (parser): +150-200 (ref-type parse, ref-
      take prefix, deref prefix, let-mut, assignment, the
      lvalue grammar)
    - sentinel-resolve: +50-80 (pass-through; resolve doesn't
      need to know about refs at C2.0; just carries them)
    - sentinel-types: +200-300 (`Type::Ref(RefId)` variant +
      `RefId` + `RefData` + `intern_ref` + display + substitute
      threading; lvalue/rvalue check; ref-type-expr resolution;
      D1's `?&T`/`&?T`/`[&T]` rejections; `BorrowOfRvalue` /
      `AssignToRvalue` / `RefInArray` / `RefInStructField`
      errors)
    - sentinel-codegen: +100-150 (refs as LLVM pointers, &expr
      lowers to alloca pointer, *r lowers to load/store, no
      borrow checking yet)
    - tests/pass: +3-5 fixtures (c20_ref_basic, c20_mut_basic,
      c20_deref_basic — no borrow-violation tests yet)

  - **C2.1** (shared borrow checker, ~400-700 LOC):
    - sentinel-borrow-check (new crate): +400-700 (lifetime
      tracking, shared-borrow rules, error variants)
    - sentinel-driver: +20 (wire borrow_check_query into the
      pipeline)
    - tests/pass + tests/ui: +5-10 fixtures (positive + negative
      shared-borrow patterns)

  - **C2.2** (&mut + XOR, ~500-900 LOC): the largest C2 sub-
    phase. Mutable-borrow rules; conflict detection;
    diagnostic-quality work for borrow conflicts.

  - **C2.3** (move semantics, ~300-500 LOC): per-binding move-
    state in the borrow checker; use-after-move detection.

  - **C2.4** (RAII + drop, ~200-400 LOC): drop emission at
    scope exit; `sentinel_free` runtime symbol; recursive drop
    for struct fields; integration with move-state.

  - **C2.5** (polish, ~100-300 LOC): NLL/Polonius migration
    plan doc; consolidation; STATE.md + HANDOVER close-out;
    ADR 0017 flip PROPOSED → ACCEPTED.

  - **Total**: ~2100-3700 LOC across crates. The upper end is
    larger than C1.6's bundled ~1000 LOC commit; consistent
    with C2 being "the hardest single phase" per HANDOVER §6.2.

  - **Estimated session budget**: 6-13 sessions across 5-6 sub-
    phases. Wide range because the borrow checker is novel
    machinery. Compare ADR 0011 D6's "C1.7 estimate 4-6 weeks;
    actual ~1 session" — Sentinel's rhythm compresses
    estimates, but borrow checking is the test case.

After C2: Phase C3 (effect-system integration from Phase B
Sentinel-Mini) per HANDOVER §6.2. The effects + region
combination is where Sentinel's full security thesis at the
language layer comes together.
