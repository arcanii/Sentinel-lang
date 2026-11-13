# ADR 0016: Concrete C1.7 surface syntax — generic fns, generic structs, monomorphization

Status: ACCEPTED-WITH-AMENDMENTS (A1, A2, A3) — all twelve D-decisions exercised
across the C1.7 scaffolding commit (c1e5083, AST + parser + resolve), the
C1.7.4a commit (d32a9fe, types crate + builtin re-route), the
C1.7.5 commit (ad7e10d, codegen monomorphization for generic
fns), and the C1.7.4b commit (2c6c652, generic structs end-to-end
+ the D12 phase-go). C1.7
estimated at "4-6 weeks" (ADR 0011 D6); actual elapsed was ~1
session across five commits.

**A1 (2026-09-02)** pins the mangling scheme D7 left "TBD in implementation" —
structural, id-free, exhaustive, and identical across all three back ends. See
[Amendments](#amendments) at the end of this file. (This ADR previously recorded
"no amendments — each D-decision survived implementation as written"; that stayed
true of the *syntax* decisions, but the deferred half of D7 did not.)

**A2 (2026-11-10)** makes the comparison rule hold a type parameter opaque, as the Context
says it is: a comparison takes scalar operands only (register item D185).

**A3 (2026-11-12)** keeps a type parameter in its own declaration's types: a `secret` over
one and an array of arrays over one are refused where they are written, a generic call's return
type, a generic struct literal's field and a field read are refused where the substitution keeps
one, and unification binds a callee's type parameter where an argument has its own written type
(register items D188, D182, D183).

Date: 2026-05-26
Last touched: 2026-11-12 (A3 — a type parameter stays in its own declaration's types;
register items D188, D182 and D183 closed)
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

> **Amended by A1 (2026-09-02).** "TBD in implementation" is no longer open: the
> scheme is pinned in [`docs/abi-v1.md`](../abi-v1.md) §4 and is **structural**
> (a tag names the type's shape, never an interner index) and **exhaustive**.
> Leaving it TBD let three back ends fill the gap three different ways, two of
> them producing IR no backend could read — see [Amendments](#amendments).

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

---

## Amendments

### A1 — D7's mangling scheme is pinned, structural, and exhaustive (2026-09-02)

**Status: ACCEPTED.** Closes filed-defect register item **D5**.

D7 deferred the mangling: *"separate LLVM functions with mangled names (e.g.
`_S_id_i64`, `_S_id_bool` — exact mangling scheme TBD in implementation)."* Three
implementations then grew independently — inkwell (`sentinel-codegen`), the
`snc llvm` text oracle (`sentinel-driver/src/llvm_dump.rs`), and the self-hosted
`scg` (`selfhost/types/cg.sentinel`) — and each filled the gap differently. A1
settles the scheme and makes the three agree. The table lives in
[`docs/abi-v1.md`](../abi-v1.md) §4, which A1 completes over every `Type` variant.

**The rule: a tag is derived from the type's own SHAPE, never from an interner
index.** That is not a style preference — it is the only property that lets three
independent implementations produce the same name. `scg` interns in its own order
and has no `SharedId`/`TaskId` concept at all, so any tag quoting a Rust interner
id is a name it cannot reproduce even in principle.

**What was wrong.** The oracle ended its match with
`other => format!("ty_{other:?}")`, leaking Rust's `Debug` formatting into an LLVM
identifier. `scg` ended its dispatch with `append_str(out, "ty")`, one token for
every kind it had not enumerated. Both fallbacks produced **invalid IR**, in
different ways, on the same six-line program:

```
struct Pair<A, B> { a: A }
fn main() -> i64 {
    let p: Pair<i64, Shared<i64>> = Pair { a: 1 };
    let q: Pair<i64, Mutex<i64>> = Pair { a: 2 };
    p.a + q.a
}
```

The oracle emitted `%Pair_i64_ty_Shared(SharedId(0)) = type { i64 }`, which
`llvm-as` rejects with *"expected '=' after name"* — an unquoted `(` cannot appear
in an LLVM identifier. `scg` emitted `%Pair_i64_ty = type { i64 }` **twice**:
*"redefinition of type"*. Only inkwell, the shipping back end, was right; the
program compiles and runs correctly today. That asymmetry is why the defect
survived — no exit-code test can see it, and the oracle-vs-`scg` byte comparison
cannot see a name the two get wrong *identically*.

**The scope was eleven variants, not the two on file, by two independent routes.**
The register entry described a PHANTOM generic type argument. That route needs the
type to be writable in type position, which excludes `Task`, `Process` and
`Guard` — but the **mono-key** route reaches them anyway: `idg(some_task)` on a
plain `fn idg<T>(x: T) -> T` puts the type straight into `mangle_mono_name` with no
phantom parameter anywhere, and corrupts a **function symbol** rather than a type
declaration (`define ptr @idg__ty_Task(TaskId(0))(ptr %arg0)` — `llvm-as`:
*"expected type"*). Verified reachable by construction: `Class`, `Enum`, `Channel`,
`Shared`, `Mutex`, `Guard`, `Task`, `Fn`, `Process`, `SealedChannel`, `U128`.
`Class` and `Enum` need no concurrency feature at all — a plain enum as a generic
argument reproduces the whole defect — which falsified both back ends' comments
claiming only a container kind could reach the fallback.

Verified **unreachable**, by trying every path that exists: `Kont` (a continuation
binding *"can only be called, not used as a value"*), `TraitSelf` (`Self` is not
writable in type position, and an impl body always has it substituted), and
`TypeParam` (Pass 0 filters TypeParam-bearing instances; mono args are concrete by
construction). They get real structural tags regardless — the point of an
exhaustive match is that "unreachable" is never carried by a wildcard.

**Where the scheme moved, and why that is an amendment and not `abi-v2`.**
Where inkwell was already structural (`u128`, `f64`, `ptr`, `process`,
`sealedchannel`, class/enum names, `fn_<p>_<r>`) the two printing back ends adopt
**inkwell's** spelling, because inkwell is the back end that emits objects. Where
inkwell embedded an id (`shared0`, `mutex0`, `guard0`, `chan0`, `task0`, `kont0`,
`Self_trait0`) all three move to an element-derived tag (`shared_i64`, `chan_u8`,
…). No shipped artifact can contain an old tag: these variants are reachable only
through a phantom type argument or a mono key, and nothing in `sentinel_library/`,
`examples/`, `demos/` or the fixtures instantiated a generic at any of them until
this amendment added one.

⚠ Two of the old spellings were **already agreed between the oracle and `scg`** —
`ty_F64` and `ty_Ptr`, hand-written into `cg_mangle_to` to match the oracle's
`Debug` output. They agreed with each other and both disagreed with inkwell, so
the codegen differential was green on precisely the arms that were wrong. That is
the shape to watch for: *two* implementations agreeing is not evidence when the
third is the one that ships.

**`scg` needed a second, non-mangling change.** Its `Shared<T>` / `Mutex<T>`
type-position arms resolved the type argument and then discarded it, interning
every annotation as `Shared<i64>` / `Mutex<i64>`. With the element gone,
`Tagged<i64, Shared<u8>>` and `Tagged<i64, Shared<i64>>` intern to the **same
handle**, so `scg` emitted one generic-instance declaration where the oracle
emitted two — a structural collapse no naming scheme could have separated. Making
the two arms element-generic (a two-line mirror of the existing `Channel` arm)
recovers the distinct instances. Measured on the new example: pre-change `scg`
emitted 17 declarations under 10 distinct names, post-change 20 under 20.

**`scg`'s fallback is now loud rather than plausible.** Every interner kind 1..19
has an arm; the final `else` emits `UNKNOWN_KIND_<n>`, which no oracle tag can
equal, so a future kind that forgets its arm fails the codegen differential on the
first program that reaches it. `ty` had the opposite property — plausible enough to
pass unnoticed while producing invalid IR. The Rust side gets this for free: both
`mangle_type` matches are exhaustive, so a new `Type` variant is a compile error.

**What A1 does NOT fix, stated so it is not later mistaken for closed.**

- **Tags share one flat namespace with user type names.** `struct arr_i64 {}` tags
  `arr_i64`, the same tag as `[i64]`, so `Pair<arr_i64, i64>` and `Pair<[i64], i64>`
  mangle identically — the oracle emits one `%Pair_arr_i64_i64` name for two
  **different layouts**. PRE-EXISTING, byte-identical in both printing back ends,
  and not closable by enumeration: `_` and `$` are both legal Sentinel identifier
  characters, so no separator built from them is unforgeable. A real encoding
  (length-prefixing, or `.`, which LLVM accepts and Sentinel's lexer cannot
  produce) changes every existing tag and is `abi-v2` work. The shipping back end
  is unaffected — LLVM uniquifies a duplicate name and the program runs correctly
  (verified end-to-end).

  ⚠ **A1 makes that class MEASURABLY LARGER in the two printing back ends, and the
  honest accounting is a net win rather than a clean one.** Two review lenses
  attacked this and both were right; the first draft of this paragraph said "two
  members larger" and rested on a false argument, so both are corrected here rather
  than quietly amended.

  The mechanism: before A1 the oracle's tag for every payload-bearing kind was a
  Rust `Debug` rendering containing **parentheses** — `ty_Shared(SharedId(0))`,
  `ty_Enum(EnumId(0))`. Parentheses are not legal in a Sentinel identifier, so those
  tags were *accidentally unforgeable*: no user type could be named to collide with
  one. A1 trades that accident for legality and three-way agreement, so every tag it
  introduces or renames joins the forgeable set: `f64`, `ptr`, `u128`, `process`,
  `sealedchannel`, the handle tags, `fn_…`, `kont_…`, `Self_…`, and the declared
  names now used for `Class`/`Enum`. Constructed and verified, both halves:
  `struct process { z: i64 }` beside a `Process` value makes both printing back ends
  emit `@idg__process` twice (`invalid redefinition of function`) where pre-A1 the
  oracle spelled one `idg__ty_Process`; and `struct f64 { z: i64 }` beside a genuine
  `f64` value makes `scg` emit `@idg__f64` twice, where pre-A1 it emitted
  `@idg__f64` and `@idg__ty_F64`.

  **The tempting defence does not work, and it is worth recording why.** The type
  resolver matches primitive spellings *before* the struct table, so `struct f64 {}`
  can never be **named** in type position (`Pair<i64, f64>` resolves to the
  primitive — verified). It does not follow that it cannot reach the mangler: the
  MONO-KEY route needs no type-position spelling at all, only a *value*, and
  `idg(f64 { z: 20 })` tags `f64` directly. Shadowing protects the phantom route and
  nothing else. No tag in this scheme is safe by shadowing.

  It is accepted for two reasons. inkwell — the back end that emits objects — already
  used every one of these spellings, so its exposure is unchanged and no *shipped*
  behaviour moves (LLVM uniquifies a duplicate name; the colliding program builds and
  runs correctly). And the alternative is keeping the two printing back ends
  permanently disagreeing with the one that ships. Against the cost, A1 removes a
  **ten-way unconditional** collision and every unparseable name.
- **`SealedChannel` in type position.** The tag is agreed, but `scg`'s
  `type_of_typeexpr` has no `SealedChannel` arm, so the annotation resolves to the
  `i64` handle and the tag never gets a chance. That is the pre-existing ADR 0069
  deferral already carried by three `DEFERRED_PROGRAMS` entries; folding it in
  would mean re-diagnosing those inside a mangling slice.
- **`u128` in `scg`.** `scg` models no `u128` at all, so `Pair<i64, u128>` resolved
  its argument to the `i64` handle and collapsed with `Pair<i64, i64>` (since ADR 0041
  A16 `scg` refuses `u128` as an unknown type). Both sides
  emit *valid* IR — they simply disagree — which makes it the quiet member of the
  family.
- **Non-`i64` container VALUES in `scg`.** Making the annotation element-generic
  does **not** make `Shared<u8>` work: `scg`'s codegen still emits no width
  conversion around a container's runtime call, so
  `fn take(s: Shared<u8>) -> u8 { shared_get(s) }` still yields `ret i8 %v` for an
  `i64` `%v`. Confirmed by diffing a pre-change against a post-change `scg` binary
  on that program — byte-identical output, so this is untouched ADR 0071 M1.4c
  work, not a consequence of A1.

**Pinned by** `examples/lang/phantom_type_param.sentinel`: 20 phantom markers
covering every tag form (bare scalar, nominal name, one-element wrapper,
handle-over-element, two-element signature, nested composition), asserted
byte-identical between the oracle and `scg`, `llvm-as`-clean, and running to 42
through inkwell under both build paths. The `u8` markers are load-bearing — they
fail the moment a back end tags a handle by its id or assumes `i64`. Its complement
`tests/pass/c16_mono_key_handle_tags.sentinel` covers the mono-key route, which is
the only one that reaches `task_` and `guard_`. `abi_v1_mangling_is_stable` pins the
tag strings themselves.

**Three tags have NO differential fixture**, stated rather than left to be
discovered: `u128` (the oracle emits a phantom `u128` tag but `scg` models no `u128` at all; it
resolved the argument to `i64`, and since ADR 0041 A16 refuses it as an unknown type, so no
differential fixture can carry it — register item D20), and `kont_` / `Self_`, both verified unreachable.
`abi_v1_mangling_is_stable` pins those three as unit tests, which anchors the STRING
but not the three-way agreement.

⚠ **This list said FIVE until a review lens refuted two of them.** The draft claimed
`process` and `sealedchannel` were unfixturable because "the mono-key route needs a
real subprocess, which the corpus only ever spawns behind a runtime-false guard".
That is backwards — the guard is exactly what makes them fixturable, since a tag is a
compile-time property and the IR emits whether or not the child ever runs. Both are
now covered by `c16_mono_key_handle_tags` and agree byte-for-byte. The lesson is the
one this ADR keeps running into: "no fixture exists" is a fact about the corpus, and
turning it into "no fixture is possible" is a claim that has to be tested.

Two tags are covered but **weakly**, which is worth knowing before trusting them:
`task_<elem>` cannot be distinguished from a hardcoded `i64` by any test, because
`Task<T>`'s result is always `i64` at the ADR 0024 minimum; and `guard_<elem>` is
pinned only at `i64`, because the `Mutex<u8>` that would exercise a non-`i64` guard
diverged for an unrelated reason (register D17) — now CLOSED (`6337bea`), and the pin
is written: both back ends emit `@idg__opt_guard_u8` byte-identically, so
`c16_mono_key_handle_tags` exercises the element-READING half of `guard_<elem>` and not
just `guard_i64`.

### A2 — a comparison takes scalar operands; a type parameter stays opaque (2026-11-10)

**Status: ACCEPTED.** Register item **D185**.

The Context above says a type parameter is *"opaque inside the body — you can pass it,
store it, return it, but you can't `+` it or compare it"*, and D10 defers bounds such as
`<T: Eq>` to the trait work. The arithmetic, bitwise and shift operators, unary `-` and `!`,
`&&` / `||`, `as`, indexing and an `if` condition each refuse an operand of type `T`. The
comparison rule did not. It refused a struct operand and a nullable one (bar `== null`) and
accepted every other type, a type parameter among them.

A generic body is type-checked once, as written, and codegen then builds a copy of it for
each set of type arguments, keeping the types the checker assigned (D7c). So a comparison of
two `T` values, typed `bool` in the body, stayed `bool` in every copy, and a copy compared
values a hand-written concrete program would have been refused for:

- at a struct, `snc llvm` emitted an `icmp` over the struct, which `llvm-as` rejects, and
  `snc build` stopped with an internal error;
- at a `?i64`, only the presence flags were compared, so two present values always compared
  equal;
- at a `secret` scalar, the result was typed `bool`, not `secret bool`, so nothing that
  reads secrecy from a type (the type checker's own rules, the constant-time verifier) saw it
  as secret.

The same rule let arrays, `Vec`s, enums and class instances through. Comparing two of them
produced the same invalid `icmp`, over the value's fields, and the same internal error. The rule also read a
result's `secret` qualifier from an operand's outer type only, so `[secret i64] == [secret
i64]` was typed `bool`. References, `ptr` and every handle type got through as well.

**The rule.** A comparison's left operand, once an outer `secret` is stripped, is an integer
(`i64`, `i32`, `u8`, `u128`), `bool` or `f64` (`is_comparable_scalar`). The right operand must
then have the same type, as before. `x == null` and `x != null` on a nullable keep their own
rule, in a generic body as anywhere; they test only whether a value is present (the borrow
checker still counts a Move-typed binding compared that way as moved, D117). Anything else is
`sentinel::types::mismatch` at the left operand ("expected i64, found …"), as a struct or
nullable operand already was and as the arithmetic operators report; where the right operand
had another type, that is now reported at the left operand rather than at the right one. For
these scalars an operand's outer `secret` is the whole of its type's secrecy, so the result's
type is `secret bool` exactly when an operand's type is `secret`. The list is explicit, so a
type added to the language is refused until it is added here on purpose.

**What it refuses that compiled before.** No program in the corpus: a matched sweep over the
tracked `.sentinel` files outside `selfhost/` (standalone, merged and through `snc build`; the
self-hosted sources are compiled by the bootstrap tests) and all five self-hosted drivers moved
only the new fixtures. A comparison of two references or two handles (`Shared`, `Mutex`,
`Channel`, `SealedChannel`, `Task`, `Process` and `Fn` values among them) was lowered by `snc
llvm` and `scg` to a comparison of their addresses, and `snc build` stopped on it with the same
internal error; compare the values two references point at (`*a == *b`), and test a `ptr` with
`is_null`. Comparing two results of a generic fn returning `secret T`, which the typer leaves
unsubstituted (register D183), is refused too; `snc build` could not build such a call. A
generic fn that compares values of its type parameter, such as `max<T>`, waits for bounds
(D10). When they come, the trait-bounds ADR must type a bounded comparison's result from the
instance, `secret bool` exactly when `T` is bound to a secret type, or check each instance at
its concrete types, because the body is checked once.

**Mirror.** `scg`'s checking modes make the same refusal (`tc_binary`, the scalar handles
`i64`, `i32`, `bool`, `u8`, `f64`) with the same code, so its code generator's checking pass
refuses these programs before its own walk runs. An operand it gave no type, as it types a
zero-arm `match` (D184), goes on to the same-type check as before: two such operands pass it,
as the oracle accepts them, and one against a typed operand is refused, as it was. `scg` does
not model `u128`. The message is the oracle's but where it names a type `scg` numbers in its
own order (D179): a reference, a nested array, a `secret` over other than a word scalar when
later declarations intern more. Where the oracle refuses a program earlier with a code `scg`
does not port, `scg` may now refuse it here instead (D179).

**Pins.** `comparison_operands_are_scalars` (sentinel-types) checks every comparison
operator over each kind of operand type a parameter can be written with, and the `Task` and
`Process` handles only an expression makes: a scalar is accepted and its result is `secret`
exactly when an operand's type is (both, or either of a mixed pair), and every other kind is
refused at the left operand.
Sixteen `tests/ui` fixtures (`c13_compare_*`: a type parameter bare, as a generic struct's
field, and with every call at `secret i64`; an array; an array of `secret`; a `secret` array;
a `Vec`; an enum; a class; a `ptr`; an `Fn` value; and the `Shared`, `Mutex`, `Channel`,
`Process` and `Task` handles) each carry the ported code, so every self-hosted driver must
refuse each as the oracle does. `tests/pass` `c13_generic_null_compare` holds the `null` form
in a generic body, and a codegen seed holds `scg`'s acceptance of two untyped operands.

**Versioning.** The oracle now refuses programs it accepted, so this is oracle-moving: at
least a MINOR bump by ADR 0076 D2, which the bump to 0.3.0 already owed covers. No accepted
program's emitted IR moves.

### A3 — a type parameter stays in its own declaration's types (2026-11-12)

**Status: ACCEPTED.** Register item **D188**; closes **D182** and **D183**.

A type parameter is identified by its index (`TypeParamId`), and interned types are values: a
callee's `P<T>` and a generic caller's own `P<T>` are one interned instance. So a type that
carries a type parameter out of the declaration it was written in is read, in another generic
fn, as that fn's own parameter of the same index. Three ways let one out:

- **Substitution keeps the written type where it builds no instance.** D6 makes an instance's
  types by substituting the type arguments into what the declaration wrote
  (`Type::substitute`). A `secret` over a type parameter passes through unchanged (ADR 0019
  D5); a `?T`, `[T]` or `Vec<T>` whose substituted element is not one the substitution demotes
  to keeps `?T`, `[T]` or `Vec<T>` (`?T` at `T := ?i64` or `T := secret i64`, `[T]` at
  `T := ?i64`, `Vec<T>` at `T := [i64]`, also inside a generic instance's arguments or behind a
  reference); and a nested array's element is interned as if it were concrete (ADR 0068), so
  `[[T]]` is kept whole. The type checker took the result as the type of a generic call, a
  generic struct literal's field and a field read on a generic instance; and an instance a call
  made could hold a field the substitution keeps written (`mk(n)` returning a `P<?i64>` for
  `struct P<T> { v: ?T }`), which the analyses that read an instance's fields read as written.
- **The pushdown.** A parameter type was pushed down to its argument as `try_substitute`
  answers it, which is the written type for a generic instance, a reference and a `secret`.
- **Unification's identity shortcut.** `unify_one` accepted an argument whose interned type is
  the parameter's own, for a generic instance, a reference and a nested array, without binding
  anything, so a callee's `R<U>`, already bound to `i64` by another argument, accepted a
  caller's `R<A>`.

In a fn with no type parameters the leftover parameter named nothing, and `snc build` stopped
with an internal error on most such programs (D183) and lowered those whose layout does not
depend on it (a `Vec<T>`, a `[T]` passed through). Inside another generic fn the expression was
typed at the caller's type argument instead of at a type of its own. Separately, a generic struct
that holds itself at other type arguments (`struct N<T> { next: ?N<Box<T>> }`) has instances
without end, which a walk that follows an instance's fields never finishes.

**The rule.** Four parts, each fail-closed:

1. A `secret` over a type that mentions a type parameter is refused where it is written
   (`sentinel::types::secret_over_type_param`), as ADR 0019 D5's minimum intends, in a
   signature, a field and a type argument alike.
2. An array whose element is an array that mentions a type parameter is refused where it is
   written and where an array literal builds one (`sentinel::types::generic_nested_array`),
   as ADR 0068 D6 defers generic nested arrays. With 1 and 2, `contains_type_param`, which
   looks into neither, is the whole answer everywhere else; `mentions_type_param` looks into
   both, for these two checks.
3. `Type::substitute_checked` substitutes as `substitute` does and answers no type where
   `substitute` keeps a written type that mentions a type parameter. The type checker refuses
   a generic call's return type, a generic struct literal's field and a field read on a generic
   instance where it answers none; and a generic call's result (a user fn's or a builtin's), a
   generic struct literal's field and such a field read whose type holds a generic instance, at
   any depth, one of whose fields it answers none for at the instance's arguments
   (`sentinel::types::uninstantiable_type`, "… cannot be instantiated at these type arguments").
   That walk refuses a struct it meets again, on the way down, at other type arguments, at the
   field that leads there: such a struct's instances have no end. (It refuses one met again at
   its own arguments permuted too, `?Pair<B, A>` inside `Pair<A, B>`, whose instances end.) A type
   that mentions no type parameter is complete, even where the substitution keeps it written (a
   `[secret S]` over a struct).
4. A parameter type is pushed down to its argument as the substitution makes it (as written,
   where it mentions no type parameter), and not at all where the substitution makes none.
   Unification takes the identity shortcut only for a type that mentions no type parameter, and
   otherwise unifies the arguments, which binds the callee's parameter to the caller's.

**What moves.** Refused now: each call, literal and field read above, including one that makes
an instance whose field has no form (`mk(n)` above, `vec_new()` at `Vec<P<?i64>>`) and a literal
or field read of a struct that holds itself at other type arguments (a few such programs built
and ran before, where nothing walked the field); any declaration with a `secret` over a type parameter, including a generic
nothing calls and a phantom type argument (`fn g<T>(y: T) -> P<secret T>` for
`struct P<T> { n: i64 }`), though no layout depends on the latter; any generic nested array;
and a call whose callee parameter, once bound, meets the caller's own type through an argument
of the same written instance or reference (refused as a conflicting inference; so is a `pop`
whose `?T` result a generic fn returns, `fn opt<T>(v: Vec<T>) -> ?T { let mut u = v; pop(&mut
u) }`, as the same call with a written `?i64` already was). Some refusals reach shapes whose form
the language has but the substitution does not build: `[T]` and `Vec<T>` at `T := secret S` for
a struct `S` or at a secret generic instance (`secret P<i64>`), and `[T]` at an array type
(`T := [u8]`, whose `[[u8]]` ADR 0068 has). The oracle typed these with
the callee's written `[T]` or `Vec<T>`; where the generic body only passed the value through
(`fn same<T>(xs: [T]) -> [T] { xs }` over a `[secret S]` or a `[[u8]]` of strings, a
`Ring<secret S>`'s `[T]` field, `&[T]`), `snc llvm` lowered them and `snc build` ran them, and where it built or read an element
`snc llvm` refused them as not yet ported; `scg` built them. They are refused now. Refused
otherwise: `vec_new()` at an element the substitution does not build (`Vec<secret S>`) is
`uninstantiable_type` where it was `mismatch`. Accepted now: a generic caller's own `P<T>` or
`&T` argument binds the callee's parameter, where the call was refused as an ambiguous type
argument; a value filling a `?GenericInstance<T>` or `?&T` parameter after `T` is bound is
widened to the instance's nullable, and a value already of that nullable type fills it, where
the written type refused both (`snc build` runs these; `snc llvm` lowers the reference one and
defers a generic instance's heap box, as it defers the `let`'s). Typed otherwise: a `null`
filling such a parameter takes the instance's type. Its emitted instructions do not move, but
where the program first names another instance of the same struct after the call, the order of
the named struct type definitions can (`%Bx_i64` before `%Bx_bool`); `scg` orders them the same
way. A matched sweep over the `.sentinel` files outside `selfhost/`, tracked and new
(`snc types` and `snc llvm` on each file, `snc types` on each program `snc merge` takes,
`snc build` on the rest), moved only this amendment's fixtures and
`c58_secret_float_through_generic`, now refused at its `secret T`; all five self-hosted drivers,
over every `.sentinel` file in the tree, moved only the same files.

**Not covered.** A generic fn's body is still checked once, as written: a type in it with no
form at some instance (`let n = wrap(x);` inside `g<T>`, with `g` called at `T := ?i64`) is
accepted, and so is a generic instance written in a signature, an annotation or a container
element whose field has no form at its arguments (`P<?i64>` for `struct P<T> { v: ?T }`),
where no call, literal or field read makes or reads one; `snc build` stops with an internal
error on both, and `scg`'s code generator lowers both (D189). The check that a generic struct
does not hold a reference in a field (ADR 0017 D7) is not changed here. An `impl` on a generic struct
written without type arguments types its field reads at the struct's own parameter (D190).

**Mirror.** `scg`'s checking modes make the same refusals with the same codes and messages:
`tc_secret_tparam_written` and `tc_nested_tparam` where `type_of_typeexpr` resolves a written
type and after an array literal's elements, `subst_complete` at a generic call's return type, a
generic struct literal's field and a field read, `tc_instance_fields` at a generic call's and a
generic builtin's result, a generic struct literal's field and a field read (with the oracle's
refusal of a struct met again at other type arguments), the pushdown through `subst_bound` (which fails
closed where a type parameter is unbound), and `tc_gunify`'s identity shortcut under the
oracle's condition. An unknown type name over such an argument is refused for the name first, as
the oracle refuses it. Where the two still differ, on programs both refuse unless said:
- `scg` resolves declarations in source order and the oracle resolves types first, so a program
  with another error can be refused for the other one (D145);
- `scg` checks a struct literal's whole field set first and then each field in declaration
  order, so a literal with an unknown, repeated or uninstantiable field can be refused for
  another field, or with another code, than the oracle names (D145);
- `scg` seeds a call's type arguments from an expected type where the oracle has none (D180),
  so it can name the other order of a conflicting inference, or `type_arg_inference_conflict`
  where the oracle answers `return_type_mismatch` or `uninstantiable_type`, and it accepts a
  value widened into a `?P<T>` filled before `T`'s binder and a generic fn's tail `pop` into
  `?T`, which the oracle refuses;
- a `null` argument nothing is pushed down to is typed by `scg` with no type (`?T`) rather than
  refused as the oracle's ambiguous `null` (not ported, D45);
- the return-type check at a generic builtin call is not mirrored: `vec_new()` at
  `Vec<secret S>` is refused by the oracle and typed by `scg`, as at HEAD, where the oracle
  answered `mismatch`; and `scg` checks `vec_to_array`'s element for a plain one before the
  instance walk, so an element that is both not plain and holds an instance with an unbuilt
  field is `vec_to_array_element_not_plain` in `scg` and `uninstantiable_type` in the oracle.

**Pins.** `generic_types_are_instantiated_completely` (sentinel-types) checks
`substitute_checked` against `substitute` over each kind of type that can hold a type
parameter and over a `[secret S]` and a `Vec<secret S>` that mention none, each refusing site
including the instances a call or field read makes, the declaration rules and their concrete
forms, the unification cases, regular and other recursion, and the pushdowns. `tests/ui` holds
fifteen `c17_uninstantiable_*` fixtures (`?T` at a nullable and at a `secret`, `[T]` at a
nullable, `Vec<T>` at an array, a generic instance's argument, a reference's target, a literal's
field, a field read, an instance a call, a builtin, a field read, a literal's field, another
instance's field and another instance's type argument make, and a struct that holds itself at
other type arguments), `c17_secret_over_type_param_return` and `_field`,
`c17_generic_nested_array_param` and `_literal`, `c17_unify_instance_scope` and
`c17_unify_ref_scope`, and `c58`, each with a ported code, so every self-hosted driver must
refuse each as the oracle does. `tests/pass` holds `c17_generic_param_pushdown_instance` (a
`null` typed at the instance, a widened reference), `c17_concrete_secret_array_calls` (a
`[secret S]` returned, received and held in a generic struct) and
`c17_generic_args_bind_instance` (the new binding, already-nullable arguments, and a `?&T` met
before its binder, which the oracle has always typed and on which the first commit of this
amendment made `scg`'s typer abort) and `c17_regular_recursion` (a struct that holds itself at its
own arguments), through every self-hosted differential.

**Versioning.** The oracle refuses programs it accepted, accepts programs it refused and types
some arguments differently, so this is oracle-moving: at least a MINOR bump by ADR 0076 D2,
covered by the bump to 0.3.0 already owed. No accepted program's emitted instructions move.
