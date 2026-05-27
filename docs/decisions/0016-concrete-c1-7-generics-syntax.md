# ADR 0016: Concrete C1.7 surface syntax — generic fns, generic structs, monomorphization

Status: ACCEPTED — all twelve D-decisions exercised across the
C1.7 scaffolding commit (c1e5083, AST + parser + resolve), the
C1.7.4a commit (d32a9fe, types crate + builtin re-route), the
C1.7.5 commit (ad7e10d, codegen monomorphization for generic
fns), and the C1.7.4b commit (2c6c652, generic structs end-to-end
+ the D12 phase-go). The ADR landed cleanly with no amendments —
each D-decision survived implementation as written. C1.7
estimated at "4-6 weeks" (ADR 0011 D6); actual elapsed was ~1
session across five commits.

Date: 2026-05-26
Last touched: 2026-05-27 (status flipped to ACCEPTED after the
C1.7 implementation closed; no D-decision amendments)
Related: 0011 (Phase C1 kickoff; D3 names generics as a C1 type-
system deliverable, D6 schedules C1.7 last, D12 perf discipline
becomes measurable here), 0015 (concrete C1.6 surface — D6
amendment deferred `?[T]` / `[?T]` to "alongside generics at
C1.7"; this ADR's D6 either resolves the deferral or extends it
explicitly), 0014 (concrete C1.5 surface — D9's generic builtins
`unwrap_or` / `is_some` get re-evaluated here once real generics
exist), 0013 (concrete C1.4 surface — D10's "no generics at C1.4"
is now lifted), 0012 (concrete C1 surface — D3's "primitives are
identifiers" continues to hold; generic type names are also
identifiers, distinguished from type parameters by per-fn / per-
struct in-scope tables)

## Context

C1.6 closed with a type universe of `{ I64, I32, Bool,
Struct(StructId), Nullable(NullableInner), Array(ArrayElem) }` —
primitives, nominal structs, nullables, and heap-backed arrays.
The three special-cased generic builtins (`unwrap_or`, `is_some`
from C1.5 per ADR 0014 D9; `len` from C1.6 per ADR 0015 D4) are
typed by ad-hoc branches in sentinel-types because Sentinel has
no way to *write* a function whose signature mentions an
unbound type parameter.

C1.7 fills this gap. After C1.7:

  - `fn id<T>(x: T) -> T { x }` parses, type-checks, and lowers.
  - `struct Box<T> { value: T }` parses, type-checks, and lowers.
  - `Box<i64>` in type position resolves to a concrete monomorphic
    instance of `Box`.
  - The three C1.5/C1.6 builtin generics' *typing* code-paths
    route through the same generic-fn machinery user code uses;
    their *codegen* stays special-cased (because their bodies can't
    be written in Sentinel-1.7 source — extracting the discriminator
    of `?T` requires force-unwrap / pattern matching, neither of
    which exists yet).

The 1.0 target surface (SENTINEL_DESIGN2.md §15.1) reads as
Rust-style explicit generics with possibly trait bounds. C1.7
ships the *unbounded* shape: `<T>` introduces a type parameter
that's opaque inside the body — you can pass it, store it,
return it, but you can't `+` it or compare it. Bounds (`<T:
Eq>`) wait for the Phase C4 trait/protocol work.

What C1.7 explicitly does NOT commit to: trait/protocol bounds
(`<T: Eq>`), higher-kinded types, lifetime parameters (those
wait for C2's region work), const generics (`<const N: usize>`),
turbofish call-site type args (`f::<i64>(x)`), generic methods
(methods themselves wait for C4), generic builtin source-level
*replacement* (the C1.5/C1.6 specials keep their codegen
specials), associated types, GATs. All of those land in later
ADRs without conflicting with the C1.7 grammar.

The C1.6 lexer's current token set is:

  - **Keywords**: `let`, `fn`, `if`, `else`, `true`, `false`,
    `struct`, `null`
  - **Punctuation**: `+ - * / = ( ) { } [ ] , ; : . ? ->`
    `== != < <= > >= && || !`
  - **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`
  - **Integer literals**: `[0-9]+`
  - **Skipped**: `[ \t\r\n]+`, `//[^\n]*`

C1.7's lexer additions (D5 below) are *empty* — every C1.7 syntax
construct reuses tokens that already exist. The `<` and `>` from
comparisons are repurposed to delimit type-parameter / type-arg
lists; the parser disambiguates by position.

## Decision

Twelve D-numbered sub-decisions covering syntax (D1-D4), lexer
additions (D5), the type-level representation (D6), codegen
strategy (D7), builtins retirement (D8), resolve-side
infrastructure (D9), explicit out-of-scope items (D10), the main-
fn invariant (D11), and the phase-go program (D12).

### D1. Generic fn syntax: `fn name<T1, T2, …>(params) -> ret { body }`.

A `<…>` clause between the fn name and the parameter list
introduces zero-or-more type parameters. Each type parameter is
an uppercase-starting identifier by convention (Sentinel doesn't
*enforce* the casing convention at C1.7 — that's a style-guide
concern, not a grammar one). Trailing comma permitted inside
`<…>` (consistent with fn params per ADR 0010 D5, struct fields
per ADR 0013 D1, array literals per ADR 0015 D2).

Grammar extension at the fn-def level:

    fn_def       = 'fn' Ident type_params? '(' params? ')' '->' type block
    type_params  = '<' (Ident (',' Ident)* ','? )? '>'

A fn with empty type-params (`fn f<>(...)`) parses but the
parser emits `ParseError::EmptyTypeParams` because zero generic
parameters is meaningless — the user either meant `fn f(...)` or
forgot to list a parameter. (Following the C1.4 ADR 0013 D1
precedent for empty struct decls, *those* are allowed because
empty structs have a real use — zero-byte tag values; empty
generic param lists have no such use.)

Type parameter names introduced by `<T, U>` are in scope inside
the parameter type annotations, the return type, and the fn body
(any `let x: T = …`, `let y: U = …`, etc.). They are *not* in
scope outside the fn. Resolve handles the lookup precedence: an
identifier in type position is checked against the in-scope
type-param table first, then the struct table.

Shadowing: `fn outer<T>() { fn inner<T>(...) { ... } }` is moot
because nested fns aren't supported (and aren't planned for C1).
Top-level fns can each have their own `T` independently — they're
separate scopes, no shadowing concern.

### D2. Generic struct syntax: `struct Name<T1, T2, …> { fields }`.

Same shape as D1 for fns. A `<…>` clause after the struct name
introduces type parameters. Field types may reference the
parameters.

Grammar extension at the struct-decl level:

    struct_decl  = 'struct' Ident type_params? '{' fields? '}'

Empty type-params on a struct decl is rejected by parser the same
way as fns (`ParseError::EmptyTypeParams`). Same rationale.

Inside the field-type annotations, the struct's type parameters
are in scope. Same lookup precedence as D1: type-param table
first, then struct table.

### D3. Type arguments in type position: `Name<TypeArg1, TypeArg2, …>`.

When an identifier in type position is followed by `<`, it's a
generic-instance type. The `<…>` contains one-or-more type
arguments, each a full TypeExpr (so nested generics like
`Box<Pair<i64, ?Box<bool>>>` are syntactically valid). Empty
type-args (`Foo<>`) parse but emit `ParseError::EmptyTypeArgs`.

Grammar extension at the type-expr level:

    type         = base_type | '?' type | '[' type ']'
    base_type    = Ident type_args?
    type_args    = '<' type (',' type)* ','? '>'

Arity is checked at the type-check stage, not the parser. The
parser is permissive: `Box<i64, bool>` parses (two args to a
one-param Box); `Box` parses (zero args to a one-param Box —
"raw" generic, no implicit defaulting). The type checker emits:

  - `TypeError::TypeArgCountMismatch { type_name, expected,
    found, span }` — when the number of args doesn't match the
    type's parameter count.
  - `TypeError::TypeArgsOnNonGeneric { type_name, span }` — when
    args are given for a non-generic type (e.g., `i64<bool>`).
  - `TypeError::MissingTypeArgs { type_name, expected_count,
    span }` — when a generic type is used without args (e.g.,
    `let x: Box = …`).

The parser-vs-checker split keeps the parser local — it doesn't
need to know which identifiers are generic — and surfaces clear
diagnostics from the type checker which DOES have the per-struct
type-parameter information.

### D4. No turbofish at call sites; type args inferred bidirectionally.

Call sites in expression position use the C1.5 bidirectional-
checking infrastructure to infer type arguments. The C1.5/C1.6
builtins `unwrap_or`, `is_some`, and `len` already do this via
their special-cased typing branches; C1.7 generalizes the
mechanism so user-defined generics get the same treatment.

For a call `id(x)` where `id<T>(x: T) -> T`:

  - The type checker infers T from the argument type. If `x: i64`,
    T = i64.
  - The call's result type is `T` substituted, i.e. i64.

For a call `pair(a, b)` where `pair<T>(a: T, b: T) -> T`:

  - Both arguments must type to the same T.
  - First arg synthesizes T; subsequent args are checked against
    it. If they don't match, `TypeError::TypeArgInferenceConflict
    { type_param, first_inference, conflict_at, span }`.

For a call in let-binding context: `let x: i64 = id(null)`:

  - The expected type `i64` plus the parameter type `T` give
    T = i64.
  - The argument `null` is then checked against `?T = ?i64` —
    wait, no, the argument type is `T = i64` here, not `?T`.
    `null` would fail unification with `i64`.
  - Recovery diagnostic is the existing `AmbiguousNull` /
    `Mismatch` from C1.5.

For a call where T can't be inferred: `let x = id(...)` where
the argument doesn't pin T (a future case — possible if a generic
fn takes only zero-info args like `fn f<T>() -> ?T { null }`):

  - The type checker emits `TypeError::AmbiguousTypeArg {
    type_param, span }`. The user fixes by adding annotation:
    `let x: ?i64 = f()` pushes T = i64.

The choice to skip turbofish at C1.7 is deliberate. Turbofish
exists in Rust specifically to disambiguate `f<g, h>(x)` (call
with two type args) from `f < g, h > (x)` (two comparisons —
which isn't valid Rust syntax for unrelated reasons, but the
parser had to commit to one interpretation before seeing the
trailing `(x)`). Sentinel's C1.3 grammar already rejects chained
comparisons (per ADR 0012 D6 — `a < b < c` is
`ParseError::ChainedComparison`), so the disambiguation pressure
is lower. But the *real* simplification is: with no turbofish,
the parser never sees `<…>` in expression position. The only
place `<` introduces type-args is *after an Ident in type
position*, which is unambiguous. The parser's expression-side
grammar is unchanged.

The cost: cases where T can't be inferred fall over to a clear
diagnostic asking for an annotation. Acceptable at C1.7.
Turbofish (or another explicit-args mechanism) is a future ADR
once usage shows it's needed.

### D5. Lexer additions at C1.7.

**None.** The `<` and `>` tokens already exist from C1.3
comparisons. The `,` token exists from earlier surfaces. The
`<` in type-param / type-arg position vs comparison position is
disambiguated by the parser: only `parse_type` looks for `<…>`
after an Ident.

The temptation to add `::` for turbofish is explicitly rejected
at C1.7 per D4. If turbofish becomes warranted (e.g., the
ambient inference fails on more user programs than is tolerable),
a future ADR adds `::` and revisits the parser side.

### D6. Type-level representation: interned generic instances; Type stays `Copy`.

The internal representation extends the C1.6 universe by adding
*two* new variants to `Type` and *one* new variant each to
`NullableInner` and `ArrayElem`. The non-trivial design choice
is how to represent generic instances without breaking `Type:
Copy`.

#### D6a. The interned-instance trick.

A generic instance like `Box<i64>` or `Pair<bool, ?Foo>` carries
type arguments that themselves are `Type` values. A naive
`Type::Generic { id: StructId, args: Box<[Type]> }` representation
breaks `Type: Copy` because `Box<[Type]>` is not `Copy`. The
codebase has resisted this break across three ADRs (0014's D4
amendment, 0015's D6 amendment); C1.7 continues the resistance
by interning generic instances behind an integer ID.

The interning machinery:

  - `pub struct GenericInstanceId(u32)` — `Copy + Hash` like
    `StructId` / `FnId`.
  - `TypedProgram.generic_instances: Vec<GenericInstanceData>`
    is the global table.
  - `pub struct GenericInstanceData { struct_id: StructId,
    type_args: Vec<Type> }`. The `type_args: Vec<Type>` is fine
    here — `GenericInstanceData` doesn't need to be `Copy`; only
    the *ID* needs to be `Copy`.

During type-check, when we encounter `Box<i64>`, we:

  1. Look up the struct id for `Box` — that already works.
  2. Resolve each type-arg recursively — that's parse_type
     recursion.
  3. Search `generic_instances` for an entry matching `(BoxId,
     [I64])`. If found, reuse its ID. If not, push a new entry
     and use the fresh ID.

The search is linear at first; if it becomes a bottleneck, a
`HashMap<(StructId, Vec<Type>), GenericInstanceId>` interning
table can be added. Profile-driven, not premature.

#### D6b. Type and helper enums.

    enum Type {
        I64,
        I32,
        Bool,
        Struct(StructId),
        /// Concrete generic instance, e.g. `Box<i64>` or
        /// `Pair<bool, Foo>`. The ID indexes into
        /// `TypedProgram.generic_instances`.
        GenericInstance(GenericInstanceId),
        /// Abstract type parameter in the body of a generic fn
        /// or generic struct. The ID is the position in the
        /// surrounding fn's or struct's type-param list.
        TypeParam(TypeParamId),
        Nullable(NullableInner),
        Array(ArrayElem),
    }

    enum NullableInner {
        I64, I32, Bool, Struct(StructId),
        GenericInstance(GenericInstanceId),  // ?Box<i64> etc.
        TypeParam(TypeParamId),               // ?T inside generic body
    }

    enum ArrayElem {
        I64, I32, Bool, Struct(StructId),
        GenericInstance(GenericInstanceId),  // [Box<i64>] etc.
        TypeParam(TypeParamId),               // [T] inside generic body
    }

Both `NullableInner` and `ArrayElem` gain two new variants.
Adding `GenericInstance` to them means `?Box<i64>` and
`[Box<i64>]` are now representable — *partially closing* the ADR
0015 D6 amendment's deferral. (`?[T]` and `[?T]` still aren't
representable — that would require `NullableInner::Array` and
`ArrayElem::Nullable`, which the C1.6 amendment punted to "a
future ADR." This ADR does NOT close that deferral; it's a
different shape from generics and lives behind whichever ADR
introduces a Box-based fully-recursive Type.)

#### D6c. TypeParamId scope.

`pub struct TypeParamId(u32)` is the position in the surrounding
fn's or struct's `type_params: Vec<…>`. Two distinct fns can
each have `T` as their TypeParamId(0); they don't collide because
TypeParamIds are scoped to their owner (the FnId or StructId).
The type checker tracks the current "owner" while checking each
fn body / struct decl.

A `Type::TypeParam(0)` alone is meaningless out-of-context — you
need to know which fn/struct the 0 refers to. The type checker
carries this implicitly via the current-fn / current-struct
context; codegen carries it explicitly via the FnInstance /
GenericInstance the codegen pass is currently lowering.

#### D6d. Type stays Copy.

All three new variants — `GenericInstance(GenericInstanceId)`,
`TypeParam(TypeParamId)`, plus the corresponding NullableInner /
ArrayElem additions — wrap a `u32` newtype. They're all `Copy`.
The Type enum remains `Copy + Hash` exactly as before. The
~3-month-old goal of avoiding the Box refactor survives C1.7.

The cost: `?[T]` and `[?T]` (the C1.6 deferral) stays unresolved
at C1.7. A future ADR addresses them when the right
representation lands (likely a Box-based fully-recursive Type, or
an extended GenericInstance approach where `?` and `[]` are
themselves generic — but that's the C2/C3 territory).

### D7. Codegen: monomorphization, not witness tables.

Each concrete instantiation of a generic fn or generic struct
gets its own LLVM emission. `id<i64>` and `id<bool>` are two
separate LLVM functions with mangled names (e.g.,
`_S_id_i64`, `_S_id_bool` — exact mangling scheme TBD in
implementation). Similarly, `Box<i64>` and `Box<bool>` are two
separate LLVM struct types.

#### D7a. Why monomorphization and not witness tables.

The HANDOVER §0.2 sketch for C1.7 mentioned witness tables, and
HANDOVER §14.1 names them as Sentinel's general-direction
choice. C1.7's *unbounded* generics (no trait constraints — see
D10) trivialize witness tables to "the table is empty"
because there are no T-specific operations to dispatch. The
type-erased payload would still need size + alignment metadata
(Swift-style "value witness table") to handle stack layout,
copies, etc. — but with no operations, the runtime overhead
beats nothing.

Monomorphization, by contrast, generates more code (one fn body
per instantiation, multiplied across instantiations) but
zero-cost at runtime. It's also strictly simpler to implement —
no value witness table runtime, no indirect calls, no opaque-T
ABI to design.

The right time for witness tables is when trait bounds arrive
(`<T: Eq>` requires dispatching `eq(T, T) -> bool` for an
unknown T). That's the C4 territory. C1.7 ships
monomorphization.

#### D7b. FnInstanceId — the dual of GenericInstanceId for fns.

To enumerate monomorphic instantiations cleanly, the type
checker also interns *fn* instantiations:

  - `pub struct FnInstanceId(u32)`. Same shape as
    `GenericInstanceId` but for fns.
  - `TypedProgram.fn_instances: Vec<FnInstanceData>`.
  - `pub struct FnInstanceData { fn_id: FnId, type_args: Vec<Type> }`.

For non-generic fns, the type-args list is empty; there's a
singleton entry per FnId. For generic fns, there's one entry per
call-site-discovered substitution. (Two call sites with the same
substitution share an instance.)

Generic struct field accesses, struct literals, and fn calls all
reference FnInstanceId / GenericInstanceId via the resolved /
typed AST. Codegen walks these tables to drive monomorphic
emission.

#### D7c. Codegen substitution.

When codegen emits `id::<i64>`, it walks the generic FnDef's
body, substituting any `Type::TypeParam(k)` it encounters with
`type_args[k]`. This is a straightforward AST walk + Type
rewrite, similar to how check() walks the body, but with
TypeParam resolution wired in.

Cached emissions: codegen keeps a `HashMap<FnInstanceId,
LlvmFunction>` and `HashMap<GenericInstanceId, LlvmStructType>`
to avoid re-emitting the same instantiation twice.

#### D7d. Code-size implications.

Monomorphization can lead to code bloat in pathological cases
(`fn id<T>(x: T) -> T { x }` instantiated for 50 different Ts
emits 50 copies). At C1.7's bootstrap scale (small programs, few
generic instantiations), this is fine. Profile-driven
de-duplication (per identical-after-monomorphization LLVM IR)
is a future optimization.

### D8. Generic builtins: typing routes through generics; codegen stays special.

The three C1.5/C1.6 builtins (`unwrap_or`, `is_some`, `len`) get
two changes at C1.7:

#### D8a. Typing: real generic signatures.

`unwrap_or<T>(x: ?T, default: T) -> T` becomes the canonical
signature. The special-cased typing branches in sentinel-types
collapse into the same generic-fn typing path user code uses.
This means:

  - The resolve crate registers `unwrap_or` as a generic fn with
    `type_params: vec![T]` and signature using `Type::TypeParam(0)`.
  - The type-check at the call site infers T from the actual
    args via the standard mechanism (D4).
  - `TypeError::CallArgMismatch` and similar fire if T can't be
    inferred or args don't unify.

Same for `is_some<T>(x: ?T) -> bool` and `len<T>(a: [T]) -> i64`.

#### D8b. Codegen: still special-cased.

The bodies of these builtins can't be expressed in Sentinel 1.7
source:

  - `unwrap_or<T>` needs to extract the `T` payload of an `?T`
    value when the discriminator is true. Sentinel 1.7 has no
    force-unwrap (`x!`) and no pattern matching, so the body
    can't be written.
  - `is_some<T>` needs to extract the discriminator bit. Same
    blocker.
  - `len<T>` needs to extract the length field of an array. The
    array's LLVM representation is `{ i64 len, T* data }`; there's
    no Sentinel syntax for "extract field 0 of an array's runtime
    representation."

So codegen keeps a `match fn_id { LEN_FN_ID => ..., UNWRAP_OR_FN_ID
=> ..., ... _ => ... }` dispatch that emits the special lowering
directly. The savings is in sentinel-types: the special-cased
typing branches disappear. ~50-80 LOC removed from
sentinel-types/lib.rs.

#### D8c. Full source-level retirement: deferred.

The "real source-level fn body" replacement for these builtins
waits for whichever phase adds force-unwrap / pattern matching
/ trait bounds. Sentinel-1.7 source can't express the bodies, so
the special-cased codegen stays.

### D9. Resolve-side infrastructure: type-param scopes.

Resolve gains:

  - `pub struct TypeParamId(u32)` — position-indexed inside the
    surrounding fn or struct.
  - `FnDef.type_params: Vec<Spanned<String>>` — the original
    parameter names (`T`, `U`, …) for diagnostics, indexed by
    TypeParamId.
  - `StructDecl.type_params: Vec<Spanned<String>>` — same for
    structs.
  - A per-fn / per-struct "type-param scope" passed through
    resolve. When resolving a type-expr ident, check this scope
    first; if it matches, emit `ResolvedTypeExpr::TypeParam(id)`;
    otherwise fall through to the struct-table lookup.

New `ResolveError` variants:

  - `DuplicateTypeParam { name, first_span, dup_span }` — when
    `<T, T>` lists the same name twice.
  - `UnknownTypeParam` — not strictly needed because unknown
    type-position idents already surface as
    `UndefinedStruct` (the existing C1.4 variant); but if we want
    better diagnostics for "did you mean to add `<T>` to the fn
    signature?" we can add it. **Decision**: defer to first-
    encountered diagnostic-quality issue; let `UndefinedStruct`
    cover it for C1.7.

### D10. Out of scope at C1.7.

The following generic-related features are explicitly deferred:

  - **Trait / protocol bounds** (`<T: Eq>`). The C4 trait/protocol
    work picks this up. C1.7 generics are *unbounded*.
  - **Lifetime parameters** (`<'a, T>`). Wait for C2's region work.
  - **Const generics** (`<const N: usize>`). Future ADR.
  - **Higher-kinded types** (`<F<_>>`). Future ADR; arguably
    never (Sentinel may stay first-order).
  - **Turbofish at call sites** (`f::<i64>(x)`). Per D4.
  - **Generic methods**. Methods themselves are C4.
  - **Generic builtin source-level replacement**. Per D8c —
    needs force-unwrap / pattern matching first.
  - **Associated types**. Future ADR (alongside traits at C4).
  - **`?[T]` and `[?T]`** (the ADR 0015 D6 deferral). Per D6 —
    this ADR adds `GenericInstance` to NullableInner/ArrayElem
    (partially closing the deferral for nullable-of-generic and
    array-of-generic) but does NOT add `Array`/`Nullable` to them.

### D11. `fn main() -> i64` invariant stays.

Same as ADR 0012 D11 / 0013 D11 / 0014 D11 / 0015 D13. Main
returns i64; codegen truncates to i32 for the C ABI. Main
explicitly *cannot* have type parameters — `fn main<T>() -> i64`
is a `TypeError::GenericMain` (or similar; finalize variant name
at implementation). Main is the program entry point and the C
ABI is monomorphic.

### D12. Phase-go program spec.

The C1.7 acceptance fixture at `tests/pass/c17_go_no_go.sentinel`
exercises all of D1-D8. Sketch:

    struct Pair<A, B> { first: A, second: B }

    fn make_pair<A, B>(a: A, b: B) -> Pair<A, B> {
        Pair { first: a, second: b }
    }

    fn fst<A, B>(p: Pair<A, B>) -> A { p.first }
    fn snd<A, B>(p: Pair<A, B>) -> B { p.second }

    fn pick_int(use_first: bool, p: Pair<i64, i64>) -> i64 {
        if use_first { fst(p) } else { snd(p) }
    }

    fn main() -> i64 {
        let p: Pair<i64, i64> = make_pair(7, 35);
        print(pick_int(true, p) + pick_int(false, p))
    }

Expected: stdout `42\n`, exit 0. Exercises:

  - Generic struct declaration (D2) — `Pair<A, B>`.
  - Generic fn declarations (D1) — `make_pair<A, B>`, `fst<A, B>`,
    `snd<A, B>`.
  - Generic instance in type position (D3) — `Pair<i64, i64>`,
    `Pair<A, B>`.
  - Call-site type inference (D4) — `make_pair(7, 35)` infers
    A = i64, B = i64; `fst(p)` infers A and B from p's type;
    `snd(p)` likewise.
  - Type representation (D6) — `Pair<i64, i64>` is one
    GenericInstanceId; the field types are
    `TypeParam(0)` and `TypeParam(1)` in the abstract def, then
    substituted to `I64` and `I64` after monomorphization.
  - Codegen monomorphization (D7) — one LLVM struct `Pair_i64_i64`
    emitted; one each for `make_pair`, `fst`, `snd`,
    `pick_int` (the last is non-generic so it's just a singleton
    instance).

The result `42 = 7 + 35` confirms both arms of `pick_int` work and
the struct construction / field access flows correctly.

Optional secondary fixture `c17_box.sentinel` (if implementation
budget allows): a simpler `Box<T>` example to exercise single-
parameter generics in isolation, mostly as a sanity test
distinct from the multi-parameter phase-go.

## Reasoning

The decisions cluster around four themes.

**Minimum-viable surface, generally.** D1 (Rust-style `<T>`),
D2 (same for structs), D3 (type-args at type-position), D4 (no
turbofish, inference covers call sites) all keep C1.7 scope-
tight. The C1.7 win is "the language gains user-definable
generic abstractions"; bounded generics, methods, associated
types, etc. all wait.

**Continuity over novelty in representation.** D6's interned-
instance trick keeps `Type: Copy` — the codebase has invested
substantially in this property through three ADRs (0014, 0015,
0015 amendment). Adding indirection through `GenericInstanceId`
mirrors what `StructId` already does; no novel design pattern
needed. The `TypeParam` variant is similarly trivial — a
position-index, same shape as `StructId`/`FnId`.

**Monomorphization first; witness tables later.** D7's choice
of monomorphization over witness tables is the path of least
resistance for unbounded generics: no trait bounds means no
dispatch table needed. Witness tables become essential when
operations on T need to dispatch (`T: Eq` requires per-T
implementations of `eq`); that's C4. Until then, monomorphization
is strictly simpler and strictly faster at runtime.

**Honest builtins compromise.** D8 acknowledges that the
C1.5/C1.6 builtin specials can't be fully retired at C1.7 —
their bodies need pattern matching or force-unwrap, neither of
which exists. The win is moving their *typing* through the same
mechanism user generics use, which is the cleaner half of the
retirement. The codegen specials stay, documented as known
limitations, with a clear retirement target (whatever phase
adds the missing extraction primitive).

## Consequences

### Positive

- The language gains user-definable abstraction. Pair, Box,
  Maybe (and eventually List, Map, …) become writable in
  Sentinel source. The C1 type system "finishes" with the
  first real abstraction primitive.

- ADR 0011 D6's "C1.7 = generics" milestone closes. C1 wraps up.
  C2 (regions, references, mutability) becomes the next phase.

- The sentinel-types/lib.rs special-casing for `unwrap_or`,
  `is_some`, `len` collapses into one generic-fn pathway.
  Codepath simplification of ~50-80 LOC.

- The `Type: Copy` invariant survives. Three ADRs of resistance
  pay off here — the interned-instance trick was always the
  out, and now it's vindicated.

- The parser side is bounded. No new lexer tokens (D5), no new
  expression-position grammar (D4), no turbofish disambiguation.
  Only `parse_type` and `parse_fn_def` / `parse_struct_decl`
  gain new arms.

- Phase-go program (D12) compiles to a single ~150-line
  monomorphic LLVM module — concrete proof that the
  monomorphization machinery works end-to-end.

### Negative

- D7's monomorphization can produce code-size blowup in
  pathological cases. Acceptable at C1.7's bootstrap scale;
  future profile-driven optimization may add deduplication
  passes.

- D8's "typing uses generics, codegen stays special" leaves a
  small inconsistency: `unwrap_or` *looks* like a user generic
  fn in error messages, but it can't be re-implemented in user
  code. Future-phase fixable.

- D6's interned `GenericInstanceId` table is searched linearly
  during type-check. Profile-driven HashMap interning is a
  trivial future optimization if it becomes a bottleneck.

- D4's no-turbofish stance means some programs hit
  `AmbiguousTypeArg` and require annotation. Acceptable; the
  diagnostic is clear and the fix is mechanical.

- Code review for the C1.7 bundled commit will be the largest
  C1 review yet — generic fns + generic structs + monomorphic
  codegen + builtin re-routing is a lot to land coherently.
  Mitigation: split the commit if the resolve+types+codegen
  diff exceeds ~1200 lines (rough target). C1.4 / C1.5 / C1.6
  bundled commits were 600-1000 LOC; C1.7 is likely larger.

### Neutral

- D11's "fn main not generic" is a trivial constraint but worth
  documenting — the C ABI is monomorphic, main is the only
  entry point.

- D8c's "source-level builtin retirement deferred" is a known
  followup; no new decision needed when the prerequisite
  features land.

## Alternatives considered

- **Witness tables instead of monomorphization** at C1.7. Per
  D7a: trivializes to "empty witness table" because there are
  no trait operations to dispatch in unbounded generics.
  Monomorphization is strictly simpler with no downside at this
  scope. Revisit at C4.

- **Box-based fully-recursive Type** (e.g., `Type::Generic
  { id: StructId, args: Vec<Type> }`). Rejected per D6: breaks
  `Type: Copy`, which has been the codebase's load-bearing
  invariant across three prior ADRs. The interned-instance
  alternative is strictly less invasive and achieves the same
  semantic outcome.

- **Turbofish (`f::<T>(x)`) for explicit call-site type args.**
  Rejected per D4: adds a new lexer token (`::`), expression-
  side parser ambiguity, and isn't needed for the C1.7 set of
  example programs. Future ADR adds it if usage demands.

- **Mandatory annotations at every generic call site.**
  Rejected: defeats the whole point of generics — users would
  write `id::<i64>(x: i64)` everywhere. Inference per D4 is the
  right ergonomic.

- **Trait bounds at C1.7** (`<T: Eq>`). Rejected per D10:
  protocols/traits are the C4 work. Bundling them here doubles
  the C1.7 scope and pushes the C1 close-out further out. C1
  stops at unbounded generics.

- **Lifetime parameters at C1.7**. Rejected per D10: references
  are C2's work. Generic lifetimes pile on without payoff at
  C1.7.

- **Resolve `?[T]` and `[?T]` (the ADR 0015 D6 deferral) as
  part of C1.7**. Rejected: it's a different shape from
  generics (no user-introduced T, just deeper Type nesting). A
  separate ADR with a clean design — likely a Box-based or
  fully-generic representation — handles it. C1.7's
  GenericInstance variants on NullableInner / ArrayElem
  *partially* close the deferral for nullable-of-generic and
  array-of-generic (e.g., `?Box<i64>` works at C1.7); the
  remaining cases (`?[T]`, `[?T]`) stay deferred.

- **Generic structs only, no generic fns** (or vice versa).
  Rejected: the infrastructure (TypeParamId scoping,
  monomorphization, substitution) is shared; splitting saves
  little. Land both together.

- **Defer C1.7 entirely; close C1 at C1.6 and start C2 now.**
  Rejected: ADR 0011 D6 commits generics as a C1 deliverable,
  and the C1.5/C1.6 special-cased builtins lose their
  special-case rationale if C1 ships without real generics —
  they'd remain explicit "the type checker has three hacks"
  forever. Better to land C1.7 and close C1 cleanly.

## Revisit

This ADR is **PROPOSED** until C1.7 lands the syntax decisions
herein. Per-D revisit triggers:

- **D1** (generic fn syntax): revisit at C4 when methods land —
  generic methods will share the `<T>` syntax.
- **D2** (generic struct syntax): same as D1 — revisit at C4
  when methods land.
- **D3** (type args in type position): once landed, no revisit
  unless turbofish (D4) gets added later, in which case the
  call-site syntax is the change, not the type-position
  syntax.
- **D4** (no turbofish): revisit if more than ~10% of programs
  in test fixtures hit `AmbiguousTypeArg`. Empirical, not
  ahead-of-time. A future ADR adds turbofish or another
  explicit-args mechanism.
- **D5** (no new lexer tokens): once landed, no revisit
  unless D4 changes.
- **D6** (interned generic instances): revisit if the linear
  search through `generic_instances` becomes a profile-visible
  bottleneck. HashMap interning is a trivial future swap.
- **D7** (monomorphization): revisit at C4 when trait bounds
  arrive. Bounded generics may need witness tables (or value
  witness tables for layout-only metadata). C1.7's
  monomorphization stays for unbounded generics either way.
- **D8** (builtins specials): revisit when force-unwrap or
  pattern matching lands. At that point, `unwrap_or`,
  `is_some`, `len` can be retired to source-level fns.
- **D9** (resolve type-param scopes): once landed, no revisit
  unless the lookup precedence (type-param > struct >
  primitive) needs change.
- **D10** (out-of-scope list): each item gets its own future
  ADR.
- **D11** (`fn main` not generic): once landed, no revisit.
  The C ABI doesn't change.
- **D12** (phase-go program): revisit only if the C1.7
  implementation surfaces a phase-go shape that's more
  representative of real-world use.

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; the actual
C1.7 commits will be larger if discovery surfaces
unanticipated coupling.

  - **C1.7.1**: this ADR (PROPOSED).
  - **C1.7.2**: no lexer changes per D5. May be a no-op
    commit, or skipped entirely if no other lexer-level
    cleanup wants to land alongside generics.
  - **C1.7.3**: bundled AST + parser + resolve + types + codegen
    end-to-end for generic fns and generic structs. Bundled because
    the parallel-tree pattern means each crate's enums need updated
    together. Rough LOC estimate per crate:
    - sentinel-ast: +60-100 (TypeExpr::Generic, FnDef.type_params,
      StructDecl.type_params, EmptyTypeParams / EmptyTypeArgs
      parser-side error variants)
    - sentinel-syntax (parser): +120-180 (parse_type_params,
      parse_type_args, parse_type ident-with-args branch,
      fn-def / struct-decl extensions)
    - sentinel-resolve: +200-300 (ResolvedFnDef.type_params,
      ResolvedStructDecl.type_params, ResolvedTypeExpr::TypeParam +
      ResolvedTypeExpr::Generic, in-scope tracking,
      DuplicateTypeParam diagnostic)
    - sentinel-types: +400-600 (TypeParamId, GenericInstanceId,
      FnInstanceId, instance interning tables, monomorphic
      substitution during check, generic-fn call inference,
      TypeArgCountMismatch / TypeArgsOnNonGeneric / MissingTypeArgs /
      AmbiguousTypeArg / TypeArgInferenceConflict / DuplicateTypeParam
      / GenericMain diagnostics, builtin signatures re-expressed as
      real generics)
    - sentinel-codegen: +300-500 (per-FnInstance monomorphic emission,
      per-GenericInstance struct emission, name mangling, caches,
      substitution during lowering)
    - tests/pass: +6-10 fixtures (c17_*) including c17_go_no_go.

    Total: ~1100-1700 LOC across crates, plus tests. The
    upper end is larger than C1.6's bundled commit (~1000
    LOC); splitting fns from structs into two commits is an
    option if the diff overshoots.

  - **C1.7.4** (docs): STATE.md + HANDOVER §0 refresh.

  - **Estimated session budget**: 4-6 sessions per ADR 0011
    D6. The longest single C1 sub-phase.

After C1.7: **Phase C1 closes**. ADR 0011 flips from
PROPOSED to ACCEPTED (or ACCEPTED-WITH-AMENDMENTS as actuals
warrant). Phase C2 begins with whatever ADR opens it (regions
+ references per HANDOVER §6.3).
