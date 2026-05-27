# HANDOVER.md — Sentinel Bootstrap Compiler Implementation

This document is the practical handover for starting work on the Sentinel
bootstrap compiler in Rust. It assumes you have read SENTINEL_DESIGN.md
and SENTINEL_DESIGN2.md and have decided to proceed with the staged
validation approach described in Section 16.3 of the design document.

Read this top to bottom once before writing any code. Then use it as a
reference as you work through the milestones.

---

## 0. Current Implementation Status

> This section is the canonical "where the codebase is right now"
> pointer. For per-crate detail and design decisions, read
> docs/STATE.md and the ADRs under docs/decisions/.

**Phase A — sentinel-broker — complete.** Generational arenas
(bump + slab), scoped budgets, diagnostics, recording mode,
secret-memory policy. 69 active tests + 1 doctest. See STATE.md
Section A. ADR 0001 (staged validation) is the umbrella.

**Phase B — sentinel-effects-proto (Sentinel-Mini) — complete.**
Research-grade tree-walking interpreter validating Sentinel's
effect-system design before the production compiler commits.
226 tests (203 lib + 23 integration). All three HANDOVER §5.2
validation demos landed (supply-chain, async-as-effect,
password-verify). The crate is explicitly throwaway per
HANDOVER §5; deletion-eligible once C3 absorbs its lessons.
ADRs 0002-0008 are authoritative. See STATE.md Section B.

**Phase C0 — bootstrap compiler MVP — complete.** The new
production-shape crates (sentinel-syntax, sentinel-ast,
sentinel-codegen, sentinel-driver, sentinel-runtime) now ship a
full lex → parse → AST → two-pass LLVM IR → object → cc-linked
executable pipeline via the `snc` binary. The ADR 0010 appendix
go/no-go program runs:

    fn double(x) { x * 2 }
    fn pick(cond, a, b) { if cond { a } else { b } }
    fn main() {
        let x = 5;
        let y = pick(x, double(x), 0);
        print(y)
    }
    // stdout: "10\n", exit 0

Six sub-phases C0.0-C0.5 shipped across twelve feat+docs commits.
22 pass-test fixtures cover the full surface. ADRs 0009 (Phase C
kickoff) and 0010 (concrete C0 surface) are ACCEPTED. Everything
is `i64` per ADR 0009 ("no type system in C0"); `bool` arrives at
C1.3. See STATE.md Section C.

**Phase C1.0 — Salsa retrofit — complete.**
**Phase C1.1 — sentinel-resolve crate lift — complete.**
**Phase C1.2 — annotation grammar + sentinel-types::check() — complete.**
**Phase C1.3 — bool + i32 + comparison + logical operators; ADR 0010 D9 retired — complete.**
**Phase C1.4 — struct definitions + field access + struct literals — complete.**
**Phase C1.5 — nullable types `?T` + null literal + unwrap_or / is_some builtins — complete (D10 deferred; retired at C1.6).**
**Phase C1.6 — arrays `[T]` + indexing `a[i]` + `len` builtin + heap runtime + ADR 0014 D10 unlock — complete.**
**Phase C1.7 — witness-table generics (generic fns + generic structs + monomorphisation) — complete. Phase C1 closes.**
Phase C1 (type system per HANDOVER §6.2) is **complete** per ADR
0011 (now ACCEPTED, 8 sub-phases, ADR's honest 5-6 month estimate
beaten — actual elapsed across C1.0a through C1.7.4b was ~10-12
sessions, ~5-6x faster than estimated; the infrastructure
investment compounded). All eight sub-phases landed:

  - **C1.0a** (09dc8c3): foundation crate `sentinel-base` hosting
    the `#[salsa::db]` SentinelDb trait, `#[salsa::input]`
    SourceFile, and `#[salsa::accumulator]` Diagnostic. Salsa 0.18
    is in the dep graph.
  - **C1.0b** (557cc60): `sentinel-syntax::query` exposes
    `lex_query` and `parse_query` as `#[salsa::tracked]` queries.
    Errors route through the Diagnostic accumulator. sentinel-driver
    instantiates a concrete `SentinelDatabase`.
  - **C1.0c** (8b58644, decision-only commit): codegen stays
    outside the salsa query graph through Phase C1.0. Three options
    weighed in the ADR 0011 D1 amendment; chosen option is "don't
    wrap codegen at all" because (a) it gets rewritten at C1.2 for
    typed HIR, (b) LLVM `'ctx` lifetimes don't trivially fit
    salsa's query model, (c) LSP/check-only tooling that wants
    incremental rebuild exits after types-not-codegen anyway.
  - **C1.1.1** (438dd16): sentinel-resolve crate populated.
    VarId(u32) / FnId(u32) stable identifiers; FnSignature lookup
    table; parallel-tree resolved AST mirroring sentinel-ast's
    Program shape with name references replaced by IDs;
    ResolveError with the 6 name-resolution variants migrated from
    CodegenError; pure `resolve(program)` entry point;
    `resolve_query(db, file)` `#[salsa::tracked]` wrapper chaining
    on parse_query. 21 unit tests (positive paths + each error
    variant + 4 salsa query smoke).
  - **C1.1.2** (9374edf): codegen consumes &ResolvedProgram;
    driver pipeline becomes parse_query → resolve_query → codegen.
    Codegen loses ~200 lines (name resolution is gone).
  - **ADR 0012** (6ab3661, PROPOSED): concrete C1 surface syntax
    — annotation grammar (D1-D4 for C1.2), bool/comparison/logical
    operators (D5-D8 for C1.3), lexer additions (D9), hard-break
    fixture rewrite plan (D10).
  - **C1.2.1** (af16655): lexer `:` token landed per ADR 0012 D9.
  - **C1.2.2** (90965a5): AST gains `TypeExpr` (Spanned<Ident at
    C1.2>), `FnDef.return_type`, `Param.ty`, `StmtKind::Let.ty_annot`
    (Option); parser wires `fn name(p: T) -> T` and optional
    `let x: T = ...`; resolve carries TypeExpr through;
    22 pass-test fixtures + 1 UI fixture mechanically rewritten
    via committed Python script per ADR 0012 D10.
  - **C1.2.3** (ded07bc): sentinel-types crate populated.
    `Type::I64` universe (C1.3 widens), parallel-tree TypedProgram
    with `ty: Type` on every TypedExpr, 4 TypeError variants
    (UnknownType active at C1.2; Mismatch / ReturnTypeMismatch /
    CallArgMismatch dormant until C1.3 multi-type expressions),
    pure `check()` + `#[salsa::tracked]` `check_query`.
  - **C1.2.4** (c9a21ff): codegen consumes &TypedProgram; driver
    pipeline becomes parse_query → resolve_query → check_query
    → codegen. `check_query::accumulated::<Diagnostic>` picks up
    lex / parse / resolve / types diagnostics transitively — full
    four-stage front-end is now query-shaped.
  - **C1.3.1** (2801a81): lexer adds the 11 C1.3 tokens per ADR
    0012 D9 — `true` and `false` keywords, six comparison ops
    (`== != < <= > >=`), three logical ops (`&& || !`). logos's
    longest-match handles the precedence-aware lexing
    (`==` beats `=`, `!=` beats `!`, `<=` beats `<`, `>=` beats
    `>`). 9 new lexer tests.
  - **C1.3.2-4** (cd1c0d4): the bool + comparison + logical
    surface lands end-to-end as a single coordinated commit
    because the AST / resolve / types / codegen parallel-tree
    enums need their exhaustive matches updated together.
    AST gains `ExprKind::BoolLit(bool)`, `ExprKind::Cmp(CmpOp,
    l, r)`, `ExprKind::Logic(LogicOp, l, r)`, `UnaryOp::Not`,
    new `CmpOp` and `LogicOp` enums. Parser inserts the
    or → and → cmp precedence levels per ADR 0012 D7;
    comparisons are non-associative per D6 (chained cmp surfaces
    as `ParseError::ChainedComparison`). Resolve passes the new
    variants through unchanged. Types widens its universe to
    `{ I64, I32, Bool }`; operator-typing rules per ADR 0012 (arith
    rejects bool; cmp same → Bool; logic Bool, Bool → Bool;
    unary `!` Bool → Bool). Codegen drops `i64_type` from the
    ctx in favour of an `llvm_int_type(Type)` helper that picks
    between `i1` / `i32` / `i64`; vars HashMap stores
    `(PointerValue, Type)`; comparisons lower via
    `build_int_compare` with the right `IntPredicate`; logicals
    lower as PHI-based short-circuit; unary `!` is `xor x, 1`.
    Activates the dormant `Mismatch` / `ReturnTypeMismatch` /
    `CallArgMismatch` variants from C1.2. +49 unit tests
    (+7 ast / +21 parser / +18 types / +10 codegen) — clean
    pipeline change with no behavior regressions.
  - **C1.3.5** (ba5fd9d): retires ADR 0010 D9's C-style truthy.
    Type checker now requires `cond.ty == Bool` for `if`;
    codegen drops the legacy compare-NE-zero path (debug_assert
    pins the invariant). Six C0 if-using fixtures rewritten
    mechanically: `if 1` → `if true`, `if 0` → `if false`,
    `if x` (x: i64) → `if x != 0`; c05_go_no_go restructured to
    use `is_positive(x): bool` + `pick(cond: bool, ...)` per
    the ADR 0012 appendix's C1.3 phase-go shape. Seven new
    c13_* pass-test fixtures land (bool_literal, comparison,
    logical_and/or, unary_not, short_circuit_and/or). The
    short-circuit fixtures specifically pin the PHI-based
    codegen — if a future change ever regresses to eager
    evaluation, the side effect of the skipped `print(99)` will
    surface in stdout and the test fails. +8 tests over step 2
    (+1 types if_condition_rejects_non_bool + 7 c13 fixtures).
  - **ADR 0013** (e93635b, PROPOSED→ACCEPTED): concrete C1.4
    surface — struct declaration grammar (D1), postfix field
    access (D2), struct literal grammar (D3) with parser-
    disambiguation D3a (no struct lit in if-cond, parens
    escape), struct types in type position (D4) extending ADR
    0012 D3's "primitives are identifiers" pattern, nominal
    type equality (D5), struct == deferred to C1.5+ (D6),
    recursive-struct detection at type-check time (D7), lexer
    additions (D8: `struct` keyword + `.` token), tuple /
    unit / derives / methods / generics all out of scope at
    C1.4 (D9, D10), `fn main() -> i64` invariant stays (D11),
    phase-go program spec (D12).
  - **C1.4.1** (f34b401): lexer adds `struct` keyword + `.`
    token per ADR 0013 D8. Two new TokenKind variants. logos
    longest-match not relevant — no `..` / `.=` neighbours
    until ranges arrive at C2+. +4 new lexer tests including
    the `structure` / `structured` ident-prefix-vs-keyword
    regression.
  - **C1.4.2-6** (aa8f252): the struct surface lands end-to-end
    as a single coordinated commit because the parallel-tree
    pattern requires it. AST gains `StructDecl` + `StructField`
    at top-level; `Program` gains `structs: Vec<StructDecl>`
    alongside `fns`. New `ExprKind` variants: `StructLit`
    (Rust-style `Name { field: expr, ... }`), `FieldAccess`
    (postfix `expr.field`). Parser handles all of it with a
    new `allow_struct_lit: bool` mode flag for D3a (forbids
    bare struct lit in if-cond; parens escape). The parser also
    gains a `parse_postfix()` wrapper that consumes `.field`
    chains after any atom — sets up the pattern for C1.6
    arrays' `[index]` and C4 methods' `.method()`. Resolve
    adds `StructId(u32)`, struct table built in pass 0 (before
    fn signatures), `ResolveError::RedefinedStruct` +
    `UndefinedStruct`. Types widens the universe to
    `{ I64, I32, Bool, Struct(StructId) }`; `resolve_type_expr`
    looks up Idents against the struct table; check_expr handles
    StructLit (validates field set matches decl, reorders to
    decl order so codegen iterates by index) and FieldAccess
    (validates target is struct, looks up field index); cycle
    detection emits `TypeError::RecursiveStruct` for direct
    + mutual cycles; four new error variants in total
    (FieldAccessOnNonStruct, UnknownField, MissingField,
    RecursiveStruct). **Codegen's value type widens from
    `IntValue<'ctx>` to `BasicValueEnum<'ctx>`** so struct
    values flow through the same machinery; new pass 0 declares
    LLVM struct types; new `llvm_basic_type` helper replaces
    `llvm_int_type` for the storage-type path (int helper
    retained for arithmetic operand coercion); StructLit lowers
    via `build_insert_value` chain from `get_undef`; FieldAccess
    lowers via `build_extract_value`. +63 tests across all
    crates. Five new c14_* pass-test fixtures land
    (struct_basic, struct_nested, struct_in_if,
    struct_bool_field, c14_go_no_go).

  - **ADR 0014** (3cb1238, PROPOSED→ACCEPTED-WITH-AMENDMENTS):
    concrete C1.5 surface — postfix `?T` type syntax (D1),
    `null` keyword literal (D2) with bidirectional context
    inference, implicit `T → ?T` widening at expression position
    (D3), `Type::Nullable(NullableInner)` flat-subset
    representation (D4 — amended from Box<Type> for Copy
    preservation), bidirectional checking infrastructure (D5),
    no-nested-nullables (D6), `==` / `!=` against `null` (D7),
    lexer additions (D8: `null` keyword + `?` token), generic
    builtins `unwrap_or` / `is_some` (D9), recursive-struct
    cycle-check relaxation (D10 — DEFERRED to C1.6+ because
    `?T = { i1, T }` flat representation can't actually break
    cycles without heap), out-of-scope list (D11: pattern
    matching, force-unwrap `x!`, optional chaining `?.`,
    null-coalesce `??`, `?` propagation, flow typing).
  - **C1.5.1** (dff8642): lexer adds `null` keyword + `?` token
    per ADR 0014 D8. Two new TokenKind variants. The `?` token
    is reserved for type-position only at C1.5; the
    expression-position uses (`?.`, `??`, `x!`, `?`
    propagation) are deferred per D11. +6 new lexer tests.
  - **C1.5.2-6** (1d0adae): the nullable surface lands end-to-end.
    AST gains `ExprKind::NullLit` + `TypeExprKind::Nullable`.
    Parser handles the optional `?` prefix in parse_type with
    `ParseError::NestedNullable` rejection for `??T`; null
    literal recognition in parse_atom. Resolve adds
    `ResolvedExprKind::NullLit` and pre-registers the two
    generic builtins (`unwrap_or` at FnId(1), `is_some` at
    FnId(2)); user fns now start at FnId(3). Types widens the
    universe to `{ I64, I32, Bool, Struct, Nullable }` via the
    flat NullableInner subset enum (D4 amendment — keeps Type
    Copy); adds bidirectional `check_expr(expr, expected:
    Option<Type>, ...)` infrastructure; `coerce_to_expected`
    inserts `TypedExprKind::WidenToNullable` wrappers for the
    implicit T→?T widening; `TypeError::AmbiguousNull` for bare
    `let x = null;`; Cmp rule extended for `x == null` /
    `null == x` comparing discriminator bits; unwrap_or /
    is_some special-cased at the Call typing arm with
    type-from-arg inference. Codegen lowers `?T` as LLVM
    `{ i1, T }`; null lit as const `{ i1 false, T zero }`;
    WidenToNullable via build_insert_value; unwrap_or via
    build_select; is_some via build_extract_value(0); Cmp on
    nullable extracts valid bits and compares. Pass 0 splits
    into "declare opaque struct types" then "set bodies" to
    handle forward references through `?Other` fields. +42
    tests across all crates. Six new c15_* pass-test fixtures
    land (null_literal, widen, eq_null, nullable_struct_field,
    maybe_compose, c15_go_no_go).

  - **ADR 0015** (8924d38, PROPOSED→ACCEPTED-WITH-AMENDMENTS):
    concrete C1.6 surface — `[T]` array type syntax (D1),
    `[e1, e2, ...]` array literal (D2), postfix `a[i]` indexing
    (D3), `len(a) -> i64` builtin (D4), empty array needs
    annotation (D5), `Type::Array(ArrayElem)` flat subset
    representation (D6 — amended to depth-1: NullableInner and
    ArrayElem stay primitive-only, no `?[T]` / `[?T]` at C1.6
    because mutual enum recursion would force Box and break
    Type's Copy), bidirectional element typing (D7), lexer
    additions (D8: `[` + `]`), heap runtime (D9:
    `sentinel_alloc` + `sentinel_panic_oob`, no `free`),
    bounds-check semantics (D10: 0 <= idx < len; panic_oob on
    failure), ADR 0014 D10 unlock implemented (D11: `?Struct`
    codegen switches to heap-indirect `{ i1, ptr }` so
    recursive structs through `?T` work; cycle detector
    relaxes), out-of-scope list (D12: mutable indexing,
    slicing, push/pop, multi-dim, methods, free, ==), fn main
    invariant stays (D13). The ADR 0014 D10 deferral retires
    here.
  - **C1.6.1** (3cfd49f): lexer adds `[` and `]` tokens per
    ADR 0015 D8. Two new TokenKind variants disambiguated by
    the parser into three roles: array type / array literal /
    postfix index. +6 new lexer tests.
  - **C1.6.2-6** (8c5bbbe): the array surface + heap runtime
    + ADR 0014 D10 unlock land end-to-end. sentinel-runtime
    gains `sentinel_alloc` (libc malloc wrapper + abort on
    failure) and `sentinel_panic_oob` (abort with diagnostic).
    AST gains `ExprKind::ArrayLit` + `ExprKind::Index` +
    `TypeExprKind::Array`. Parser handles `[T]` in parse_type,
    `[...]` in parse_atom (with empty-needs-annotation per D5),
    `a[i]` in parse_postfix alongside `.field`. Resolve
    pre-registers `len` builtin at FnId(3); user fns now start
    at FnId(4). Types widens with `Type::Array(ArrayElem)` flat
    subset (D6 amendment — primitives only; no `?[T]` / `[?T]`),
    bidirectional element typing (D7), array literal / index /
    len typing rules (D2/D3/D4), four new TypeError variants
    (AmbiguousEmptyArray, IndexOnNonArray, IndexNotInt,
    NestedArray), and **the cycle-detector relaxation** that
    closes ADR 0014 D10: only direct struct edges contribute to
    cycles; `?Struct` edges break them via heap indirection.
    Codegen: array as `{ i64 len, ptr data }`; ArrayLit lowers
    to alloc+store+insert_value; Index lowers to bounds-check
    + GEP + load (two basic blocks idx_ok/idx_oob); len
    extract_value(0); the `?Struct` representation switches
    from inline `{ i1, T }` to heap-indirect `{ i1, ptr }`;
    `WidenToNullable` for struct types allocates+stores +
    wraps in pointer. +52 tests across all crates. Seven new
    c16_* pass-test fixtures (array_basic, empty_array,
    array_as_arg, array_of_struct, array_in_struct,
    linked_list_node, c16_go_no_go).

  - **ADR 0016** (e411ded, PROPOSED→ACCEPTED): concrete C1.7
    surface — generic fn syntax `fn name<T>(x: T) -> T` (D1),
    generic struct syntax `struct Box<T> { ... }` (D2), type args
    in type position `Box<i64>` (D3), no turbofish at call sites
    (D4) with iterative bidirectional inference, no new lexer
    tokens (D5 — `<` and `>` reused from C1.3 comparisons),
    interned-instance `Type::GenericInstance(GenericInstanceId)`
    representation preserving `Type: Copy` (D6a), monomorphic
    codegen (D7) chosen over witness tables because unbounded
    generics trivialise them to "empty", builtins typing routes
    through the unified inference path (D8a) but codegen stays
    special (D8b — no Sentinel-1.7 source bodies for force-
    unwrap / pattern-matching / runtime-metadata extraction),
    resolve-side type-param scoping with DuplicateTypeParam
    diagnostic (D9), out-of-scope list (D10: bounds, lifetimes,
    HKT, const generics, turbofish, generic methods), `fn main`
    not generic (D11), Pair<A,B> phase-go (D12).
  - **C1.7 scaffolding** (c1e5083): AST + parser + resolve
    infrastructure. AST gains `TypeParam` struct, `type_params:
    Vec<TypeParam>` on `FnDef` / `StructDecl`, and
    `TypeExprKind::Generic { name, args }`. Parser gains
    `parse_type_params` / `parse_type_args` helpers and the
    `Ident<...>` branch in `parse_type`. Resolve gains
    `TypeParamId`, `ResolvedTypeParam`, `DuplicateTypeParam`
    error, and `FnSignature.type_params_count` (builtins flagged
    as generic with count=1). +19 parser tests + 7 resolve tests
    = 777 total.
  - **C1.7.4a** (d32a9fe): types crate generic-fn typing +
    builtin re-route. New `Type::TypeParam(TypeParamId)` +
    matching variants on `NullableInner` / `ArrayElem`. New
    helpers `Type::substitute`, `try_substitute`,
    `contains_type_param`. Builtin signatures rewritten with
    real `Type::TypeParam` (`unwrap_or<T>(?T, T) -> T`,
    `is_some<T>(?T) -> bool`, `len<T>([T]) -> i64`); the
    ~75 LOC of special-cased Call branches in check_expr
    collapse into one unified `check_call`. Iterative
    bidirectional inference handles null literals via
    fixed-point typing (`unwrap_or(null, 0)` works:
    arg[1]=0→I64 binds T=I64; arg[0]=null re-checked with
    expected `?I64`). `TypedExprKind::Call` gains
    `type_args: Vec<Type>`. New error variants: GenericMain,
    AmbiguousTypeArg, TypeArgInferenceConflict,
    GenericStructNotYetSupported (placeholder for C1.7.4b).
    Codegen: skip generic user fn declarations + bodies
    + emit `CodegenError::GenericCallNotYetSupported` at
    user generic-fn call sites pending C1.7.5. +12 types
    tests = 789 total.
  - **C1.7.5** (ad7e10d): codegen monomorphization for user
    generic fns. `TypedFnDef::substitute` deep-clones a generic
    fn with TypeParams substituted to concrete types — the
    monomorphic def looks no different from a non-generic fn
    to compile_fn. Worklist algorithm
    (`collect_mono_instantiations`) walks non-generic fn bodies
    seeding instantiations, then transitively processes each
    pending instance under its substitution. Per-instance LLVM
    fn declaration with mangled name (`id__i64`, etc.) via
    `mangle_mono_name` + `mangle_type`. Builtin lowering stays
    inline per ADR 0016 D8b. Four new c17 fixtures
    (c17_id stdout "42", c17_two_instantiations "41",
    c17_generic_nullable "100", c17_generic_array "6"). +4
    pass tests = 793 total.
  - **C1.7.4b** (2c6c652): generic structs end-to-end + ADR
    0016 D12 phase-go. New `Type::GenericInstance(GenericInstanceId)`
    variant + interner table on TypedProgram. NullableInner /
    ArrayElem gain `GenericInstance` variants (partially
    closing the ADR 0015 D6 deferral: `?Box<i64>` and
    `[Box<i64>]` now work; `?[T]` and `[?T]` stay deferred).
    `Type::substitute` extended to take `&mut Vec<GenericInstanceData>`
    for interner-extending substitution. Same threading
    through TypedFnDef/Block/Stmt/Expr::substitute. New
    `check_call` extensions: unify_one recurses into
    GenericInstance args; bidirectional pushdown extended for
    generic-instance returns (so `fn make_pair<A, B>(...) -> Pair<A, B>
    { Pair { ... } }` works). Codegen pass 0 splits into
    declare-then-set-bodies passes for both regular structs and
    generic-struct instances, with abstract instances
    (TypeParam-using args) filtered out via
    `arg_contains_typeparam`. Two new fixtures (c17_box stdout
    "42", c17_go_no_go stdout "42" — the full ADR 0016 D12
    Pair<A,B> + make_pair / fst / snd / pick_int program). +5
    types tests + 2 pass tests = 798 total.

  - **C1.7 docs commit** (4028dd7): STATE.md banner refresh +
    HANDOVER §0 close-out + ADR 0011/0016 flips to ACCEPTED.

  - **ADR 0017** (ea4bcfd, PROPOSED): Phase C2 kickoff. 14
    D-decisions covering reference syntax (`&T` / `&mut T` per
    D1), mutability (`let mut` + `mut` param prefix per D2),
    borrow-take + dereference (`&expr` / `&mut expr` / `*expr`
    per D3 / D4), lvalue / rvalue distinction (D5), borrow-
    checker formulation (lexical first, Polonius later per D6),
    region representation (lexical only at C2 minimum per D7;
    named regions deferred), drop / RAII (auto-drop at scope
    exit + `sentinel_free` per D8 closes the C1.6+ heap-leak
    deferral), move semantics + use-after-move (D9), lexer
    additions (D10), interned `Type::Ref(RefId)` (D11 — the
    fifth ADR running to preserve `Type: Copy` via
    internment), out-of-scope items (D12), `fn main` invariant
    (D13), and phase-go program spec (D14). Sub-phase split
    table: C2.0 (infrastructure) → C2.1 (shared borrow
    checker) → C2.2 (`&mut` + XOR — the largest) → C2.3 (move
    semantics) → C2.4 (RAII + drop) → C2.5 (Polonius
    migration plan + STATE.md / HANDOVER close-out).

  - **C2.0.1** (d7b18c2): lexer adds `&` (Amp) token + `mut`
    keyword per ADR 0017 D10. The `*` token already exists for
    multiplication from C0; per D10 the parser disambiguates
    dereference vs multiplication positionally at C2.0.2. No
    `'a` lifetime syntax at C2 minimum per D7. logos longest-
    match handles `&&` (AmpAmp) staying a single token. +10
    new lexer tests = 808 total.

  - **C2.1** (64edf3d): shared-only lexical borrow checker per
    ADR 0017 D6. New crate `sentinel-borrow-check` (~600 LOC
    including tests). New salsa-tracked query `borrow_check_query`
    chains on `check_query`; the pipeline becomes parse_query →
    resolve_query → check_query → borrow_check_query → codegen
    with diagnostics accumulating transitively. Driver wires the
    gate: a borrow failure blocks codegen + exits with code 1.
    Two `BorrowError` variants at C2.1: `OutlivesSource`
    (canonical use-after-scope) and `ReturnsLocalRef` (a fn
    returns a `&T` whose source is fn-local — per ADR 0017 D7's
    "second-class refs" rule). Borrow-source representation is
    a bounded enum `{ Local(VarId), Incoming, LocalAnonymous }`;
    the analysis is per-fn with limited inter-procedural
    reasoning (a call returning a ref inherits the most-
    restrictive source among its ref args). Inner blocks push/
    pop scopes; let-stmts record source from RHS before declaring
    the binding; assign-stmts to ref-typed Vars update the
    recorded source. ADR 0017 D6's lexical-first formulation is
    now exercised; D7's second-class-refs rule enforced.
    +15 borrow-check unit tests + 4 driver pass-test fixtures
    (c21_borrow_local_ok / c21_pass_through_ref / c21_reborrow /
    c21_go_no_go). c21_go_no_go runs: stdout "168\n", exit 0
    (`sum_two(&a, &b) + triple(&a) + triple(&b)` = 42 + 30 + 96).

  - **C2.0.2** (9516ebb): bundled AST + parser + resolve +
    types + codegen for refs end-to-end per ADR 0017 D1-D5 +
    D11. AST gains `UnaryOp::Ref` / `UnaryOp::RefMut` /
    `UnaryOp::Deref`; `TypeExprKind::Ref { mutable, inner }`;
    `StmtKind::Let.mutable`; `StmtKind::Assign { target, value }`;
    `Param.mutable`. Parser handles `&T` / `&mut T` (with
    whitespace tolerance), `&expr` / `&mut expr` / `*expr`
    prefix unaries, `let mut`, `mut` params, and assignment
    statements (after parsing an expression at stmt position,
    a following `=` triggers Assign-statement parsing).
    Resolve passes the new variants through with `mutable`
    bits threaded on ResolvedParam / ResolvedStmtKind::Let;
    new ResolvedStmtKind::Assign. Types adds `Type::Ref(RefId)`
    + `RefData { mutable, inner }` + `intern_ref` per ADR
    0017 D11, mirroring the C1.7.4b GenericInstance interner
    pattern (keeps `Type: Copy`). `NullableInner` gains `Ref`
    for `?&T`; ArrayElem stays primitive-only (refs in arrays
    rejected at parse-array-type time with `RefInArray`).
    Type::substitute extended to recurse through Ref (clone,
    substitute inner, re-intern). unify_one extended for Ref
    (mutability match + inner recursion — enables generic+ref
    inference). VarTypeEnv becomes `HashMap<VarId, (Type,
    bool)>` to track mutability. New TypeError variants:
    NestedRef, RefInArray, RefInStructField, BorrowOfRvalue,
    AssignToRvalue, AssignToImmutable, BorrowMutOfImmutable,
    DerefOfNonRef, AssignThroughSharedRef,
    IndexAssignNotSupported. check_expr dispatches Unary
    Ref/RefMut/Deref: Ref requires lvalue; RefMut requires
    mutable lvalue; Deref requires Type::Ref operand and
    returns its inner. check_stmt's Assign arm validates LHS
    is a mutable lvalue (recursive through field-access),
    pushes target.ty down to RHS for widening, and Mismatch's
    on type disagreement. Codegen lowers refs as LLVM opaque
    pointers (LLVM 15+ no-typed-pointer). New
    `lower_lvalue_ptr` helper handles Var → alloca ptr, `*r` →
    load r's value (the ptr), `p.field` → struct_gep into the
    field; assignment and `&` / `&mut` both delegate through
    it. `*r` lowers as load-from-pointer of the inner type
    (looked up via `program.refs[id].inner`). Tests: +62 (870
    total) — +8 ast, +23 syntax, +21 types, +6 codegen, +4
    driver pass-test fixtures (c20_ref_basic / c20_mut_basic /
    c20_deref_basic / c20_go_no_go). c20_go_no_go runs:
    stdout "53\n", exit 0 (the full ADR 0017 D14 program with
    `add(&a, &b)` shared-borrows + `increment(&mut a)`
    exclusive-borrow + `let mut a` + deref-assignment + print).

**C2 next: C2.2 — `&mut T` + shared-XOR-mutable rule.** The
largest C2 sub-phase per ADR 0017 D6 / D9. C2.2 introduces
borrow conflict detection: a `&mut T` borrow can't coexist with
ANY other `&T` or `&mut T` of the same place; multiple `&T`s
remain fine. New BorrowError variants: `BorrowConflict` /
`MutableBorrowOfShared` / `SharedBorrowOfMutable`. The
sentinel-borrow-check crate landed at C2.1 is the substrate;
C2.2 extends its analysis with the place-tracking machinery
+ conflict-window detection. Estimated 2-3 sessions per ADR
0017 D9.

**Workspace test count**: 889 active across all crates (+1
doctest at sentinel-broker; +19 over C2.0.2: +15 borrow-check
unit tests in the new crate, +4 driver pass-test fixtures
across c21_*). All four check-suite checks green (cargo build
--workspace, cargo clippy --workspace --all-targets -D warnings,
cargo test --workspace, cargo test --workspace --doc). c05
go/no-go (C1.3 bool flow) runs: stdout "10", exit 0. c14
go/no-go (C1.4 struct flow) runs: stdout "7", exit 0. c15
go/no-go (C1.5 nullable flow) runs: stdout "142", exit 0. c16
go/no-go (C1.6 array flow) runs: stdout "15", exit 0. c17
go/no-go (C1.7 generics flow) runs: stdout "42", exit 0. c20
go/no-go (C2.0.2 refs+mut+assign flow) runs: stdout "53", exit
0. c21 go/no-go (C2.1 shared-borrow flow — `sum_two(&a,&b) +
triple(&a) + triple(&b)`) runs: stdout "168", exit 0. See
STATE.md "Conventions" for the per-crate breakdown.

**ADR status**:

  - 0001 staged-validation                       ACCEPTED
  - 0002 effect-rows-in-mini                     ACCEPTED
  - 0003 b1-retrospective                        ACCEPTED
  - 0004 row-representation-and-effect-surface   ACCEPTED
  - 0005 effect-inference-judgment               ACCEPTED
  - 0006 default-close-row-variables             ACCEPTED
  - 0007 effect-handlers                         ACCEPTED
  - 0008 secret-qualifier-and-constant-time      ACCEPTED
  - 0009 phase-c-kickoff-and-c0-plan             ACCEPTED (all C0
                                                 sub-phases done)
  - 0010 concrete-c0-surface-syntax              ACCEPTED (all
                                                 D-decisions exercised)
  - 0011 phase-c1-kickoff-and-type-system-plan   ACCEPTED — all 12
                                                 D-decisions exercised
                                                 across C1.0 through
                                                 C1.7. D6's eight-
                                                 sub-phase budget is
                                                 closed (every C1.x
                                                 done). D12's perf
                                                 discipline measured:
                                                 sub-second cold
                                                 builds + sub-100ms
                                                 incremental rebuilds
                                                 on the current
                                                 corpus.
  - 0012 concrete-c1-surface-syntax              ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.2-3
  - 0013 concrete-c1-4-struct-syntax             ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.4
  - 0014 concrete-c1-5-nullable-syntax           ACCEPTED — D1-D11
                                                 all fully exercised:
                                                 D1-D9 + D11 at C1.5;
                                                 D10 retires at C1.6
                                                 via ADR 0015 D11
                                                 (`?Struct` heap
                                                 indirection unlocks
                                                 the recursive-
                                                 struct relaxation).
                                                 D4 representation
                                                 stays as the flat
                                                 NullableInner subset
                                                 enum (C1.5
                                                 amendment)
  - 0015 concrete-c1-6-array-syntax              ACCEPTED-WITH-
                                                 AMENDMENTS — D1-D5
                                                 + D7-D13 all fully
                                                 exercised; D6 amended
                                                 (NullableInner +
                                                 ArrayElem stay
                                                 primitive-only,
                                                 deferring `?[T]` and
                                                 `[?T]` to a future
                                                 ADR; C1.7.4b partially
                                                 closes by adding
                                                 GenericInstance
                                                 variants — `?Box<i64>`
                                                 and `[Box<i64>]` work,
                                                 but `?[T]` and `[?T]`
                                                 stay deferred); D11
                                                 implementation
                                                 closes ADR 0014 D10
  - 0016 concrete-c1-7-generics-syntax           ACCEPTED — all 12
                                                 D-decisions exercised
                                                 cleanly across the
                                                 C1.7 scaffolding +
                                                 4a + 5 + 4b commits.
                                                 No amendments — each
                                                 D-decision survived
                                                 implementation as
                                                 drafted.
  - 0017 phase-c2-kickoff-and-region-plan        PROPOSED — D1-D5
                                                 + D6 + D7 + D10 +
                                                 D11 + D12 + D13 all
                                                 exercised at C2.0.1
                                                 (lexer additions per
                                                 D10), C2.0.2 (refs
                                                 end-to-end per D1-D5
                                                 + D11), and C2.1
                                                 (lexical-first
                                                 shared-only borrow
                                                 checker per D6 +
                                                 second-class-refs
                                                 ReturnsLocalRef
                                                 enforcement per D7).
                                                 D8's RAII+drop /
                                                 D9's move semantics
                                                 still pending
                                                 across C2.3 → C2.4;
                                                 D14's full
                                                 borrow-checking
                                                 phase-go program
                                                 lands at C2.5. ADR
                                                 flips to ACCEPTED at
                                                 C2.5 close-out per
                                                 its sub-phase split
                                                 table.

### 0.1 Working norms (carry forward into Phase C2)

Original Phase-A norms, augmented with Phase-B and Phase-C
lessons:

- **Trust STATE.md, not the git log.** Commit messages are dense
  and miss design rationale. Always read docs/STATE.md and this
  file before doing anything; never infer state from git log alone.

- **Terminal quirk: nested heredocs break.** This developer's
  terminal mangles `<<EOF ... <<INNER ... INNER ... EOF`-style
  scripts. Use one of: (a) base64-encoded python3 -c blocks,
  (b) write a script to /tmp/ via a single non-nested heredoc and
  then execute it, or (c) single non-nested heredocs only.

- **Small patches, build between each.** The session that built
  Phase A7 took four diagnostic/fix iterations because the
  initial patch was too ambitious. Better practice: land the
  type/trait changes first, build, then add the implementations,
  build, then add the tests. Same lesson held for Phase C0:
  small sub-phase commits + cargo test after each beats one big
  commit.

- **Honest disclosure beats confident-but-wrong.** This developer
  values being told when something is uncertain or guessed at
  ("I'm not sure if BudgetScope::within_budget emits BudgetClosed
  on rejection, so I included an assertion to find out") over
  patches presented as definitely-correct that turn out not to be.
  The C1.0b pause is an example: rather than land a half-working
  retrofit, the session committed the working sentinel-base alone
  and documented C1.0b's path forward.

- **Minimal ceremony.** "go", "proceed", short replies are the
  norm. Long preambles are unwelcome.

- **Examples held to -D warnings.** Don't allow lint debt in
  examples; they're educational artifacts. Same for tests/pass/
  fixtures (Phase C).

- **Check before overwriting docs.** When patching documentation
  files via Python, always check `p.exists()` and read existing
  content first. Prefer merge/append patterns for docs/. Phase A
  hard-learned lesson on BACKLOG.md.

New norms learned during Phase B and Phase C:

- **ADR-first per phase boundary.** ADR 0002 was the Phase B
  kickoff; ADR 0009 was Phase C kickoff; ADR 0011 was Phase C1
  kickoff. Each landed PROPOSED before the first feat commit,
  became ACCEPTED at sub-phase completion. Continue the pattern
  for Phase D and beyond.

- **feat + docs commit pairs per sub-phase.** Each sub-phase
  ships as a feat commit (code + tests) followed by a docs
  commit (STATE.md refresh + ADR status updates). The docs
  commit also backfills the hash that the feat commit produced.
  See the C0.0-C0.5 history for the rhythm.

- **The pure-function pipeline discipline (ADR 0009 D1a).**
  C0 held it — `lex`, `parse`, `compile_to_object` are all
  `(input) -> (output, diagnostics)`. C1.0a starts cashing in
  the payoff by retrofitting Salsa. Keep new pipeline stages
  pure-function until salsa wrapping happens at a known
  sub-phase.

- **cargo clippy --workspace --all-targets -D warnings** is
  part of the standard four-check suite alongside build / test
  / test --doc. Don't let clippy debt accumulate; it has caused
  full re-sweep commits before (4182ff6 cleared pre-B4.0 lints).

- **No pushes from the assistant.** Commits land locally; the
  dev pushes via GitHub Desktop in batches. Never run `git push`.

- **macOS-only assumption.** `.cargo/config.toml` hardcodes brew
  paths (LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18, link
  search at /opt/homebrew/lib + /usr/local/lib). Cross-platform
  is a future concern; right now LLVM 18 must be `brew install`-ed.

- **Mode B working conventions** (from Phase B onward): paste-
  direct zsh; anchor-guarded Python patches via `/tmp/`; no
  nested heredocs; when a Python script needs multi-line Rust
  text put it in a separate `cat > /tmp/foo.txt <<'RSEOF' …`
  block and `read_text()` it from Python (triple-quoted Python
  strings inside a bash heredoc can mangle terminals); cargo
  test -p <crate> after each patch.

### 0.2 Next session opening (Phase C2 — regions, references, mutability)

Resume at **Phase C2** per HANDOVER §6.3. Phase C1 closed with
C1.7. The C1 type system covers: primitive scalars, nominal
structs, nullable types (including recursive via heap), heap-
backed arrays, and generics (witness-table-style via
monomorphisation). C2 introduces references (`&T`, `&mut T`),
region inference, and the borrow checker — the substrate for
Sentinel's memory-safety guarantees at the language layer.

**Pre-flight work before any code**: write ADR 0017 *PROPOSED*
covering the concrete C2 surface decisions and the sub-phase
split. Likely Patterns to argue:

  - **Reference syntax**: `&T` for shared, `&mut T` for unique.
    Where syntactically can refs appear — fn params, let bindings,
    fn returns? Per HANDOVER §6.3.
  - **Mutability**: `let mut x = ...` for mutable bindings;
    parameter mutability (Rust's `mut` on params is local; Swift
    style with `inout` is heavier). Pick one.
  - **Region inference**: lexical regions vs NLL-style (the Rust
    pre-2018 vs post-2018 split). Lexical is simpler to implement
    + teach; NLL handles more programs correctly. Trade-off.
  - **Borrow-checker shape**: explicit lifetime parameters
    (`<'a>` on fn signatures) vs inferred-only at C2. Sentinel
    1.0 will probably want explicit when needed; C2 can ship
    inferred-only with explicit deferred.
  - **RAII / drop semantics**: arrays leaked at C1.6+ — the heap
    payload of `sentinel_alloc` is never freed. C2 introduces
    Drop (`fn drop(&mut self)`-style finalizers) tied to lexical
    region exit. The leaks close here.
  - **`?Struct` heap indirection**: now under region control —
    the heap allocation has a region lifetime, freed on region
    exit. Reuse the C1.6 `sentinel_alloc` machinery; add the
    matching `sentinel_free`.
  - **Out of scope at C2**: lifetimes-as-generic-params (`<'a,
    T>`) — defer; cell types (`Cell`, `RefCell`) — defer;
    multi-region inference (regions that span loops, etc.) —
    likely the C2 split point.

Sub-steps once ADR 0017 is in (rough sketch — refine in the
ADR):

1. **Lexer**: `&` and `mut` keyword. Possibly `'a` lifetime
   syntax (deferred if not needed at C2 minimum).
2. **AST + parser**: `TypeExprKind::Ref { mutable, inner }`;
   `mut` modifier on let bindings + params; `&expr` for
   reference-take.
3. **Resolve**: nothing new — refs flow through transparently.
4. **Types**: borrow checking; region inference; reject
   double-borrow, use-after-move, etc. New error variants:
   `BorrowConflict`, `UseAfterMove`, `LifetimeMismatch`,
   `MutableBorrowOfShared`, etc.
5. **Codegen**: refs as LLVM pointers; drop calls inserted at
   region exit; sentinel_free added.

**Estimated effort for Phase C2**: substantial — likely 6-10
sessions across 4-5 sub-phases. Borrow checking is genuinely
new territory and the design space is wide (compare Rust's
multi-year NLL development). The good news: C1's infrastructure
investment (Salsa + resolve + types parallel tree + monomorphic
codegen) all carries forward. Region inference is "just" another
analysis pass between check_query and codegen.

C1.7 retrospective (estimate vs actual): ADR 0011 D6 estimated
"4-6 weeks" (the longest single C1 sub-phase); actual was ~1
session across 5 commits (e411ded ADR 0016 PROPOSED + c1e5083
scaffolding + d32a9fe types + ad7e10d codegen + 2c6c652 generic
structs end-to-end). Faster than estimated, in line with the
C1.4/5/6 pattern. The substantive pieces:
(a) the interned-instance design choice (`Type::GenericInstance(
GenericInstanceId)` with the args in a program-level table)
preserved `Type: Copy` and avoided a ~30-site clone-cascade
refactor; (b) the eager-substitute approach for codegen
(`TypedFnDef::substitute` deep-clone + lower the substituted
def via the existing per-fn path) avoided the lazy-substitution
audit risk; (c) the unified `check_call` consolidation —
deleting the ~75 LOC of special-cased C1.5/6 builtin Call
branches — is the cleanest payoff of the C1.7 design. Notes
captured in STATE.md decisions 82-93.

C1.6 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 74-81.

C1.5 retrospective (kept for reference): "2-3 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 54-64.

C1.3 retrospective (kept for reference): "2 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 46-53.

**C1 overall retrospective**: ADR 0011 D6's honest 22-28 week
budget for all of C1 was generous; actual elapsed across C1.0a
through C1.7.4b was ~10-12 sessions (~5-6x faster). The
infrastructure investment (Salsa retrofit + per-pass crate split
+ parallel-tree pattern + per-sub-phase ADR-first discipline)
compounded heavily — each sub-phase reused the same scaffolding
and the same five-step rhythm (ADR → lexer → bundled AST/parser/
resolve/types/codegen → fixtures → docs). C2's region work
likely won't compound the same way (borrow checking is novel
machinery) but the ADR-first norm, the parallel-tree pattern,
and the salsa pipeline all carry forward intact.

C1.5 retrospective (kept for reference): "2-3 weeks" estimated;
~1 session actual. The bidirectional checking infrastructure
and the D4/D10 amendments were the highest-thought-cost pieces.
Notes in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. The codegen value-type widening from
`IntValue<'ctx>` to `BasicValueEnum<'ctx>` was the substantive
change. Notes in STATE.md decisions 54-64.

C1.3 retrospective (kept for reference): "2 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 46-53.

### 0.3 Quick-status block for session start

For pasting into a fresh chat to bootstrap context:

    Continuing Sentinel-lang work. Repo: https://github.com/arcanii/Sentinel-lang
    Local HEAD: 64edf3d (feat(c2.1): shared-only lexical borrow
    checker).
    Branch in sync with origin/main (verify with `git status` at session start).
    Working tree clean.

    Phase A (broker) + Phase B (effects-proto) + Phase C0
    (bootstrap compiler MVP) + Phase C1 (full type system — all
    8 sub-phases C1.0 through C1.7) complete. Phase C2 in flight
    per ADR 0017 PROPOSED — refs / mutability / regions /
    borrow checking. C2.0.1 (lexer) + C2.0.2 (refs infrastructure)
    + C2.1 (shared-only lexical borrow checker) all landed.
    889 active workspace tests + 1 doctest. **Seven go/no-go
    programs run end-to-end:** tests/pass/c05_go_no_go.sentinel
    (C1.3 bool flow): stdout "10", exit 0; c14_go_no_go (C1.4
    struct flow): "7"; c15_go_no_go (C1.5 nullable flow):
    "142"; c16_go_no_go (C1.6 array flow): "15";
    c17_go_no_go (C1.7 generics flow): "42", exit 0;
    c20_go_no_go (C2.0.2 refs+mut+assign flow): "53", exit 0;
    c21_go_no_go (C2.1 shared-borrow flow — sum_two(&a,&b) +
    triple(&a) + triple(&b)): "168", exit 0.

    Pipeline at C2.1: **parse_query → resolve_query → check_query
    → borrow_check_query → codegen** with diagnostics
    transitively accumulated through the chain (borrow_check_query
    is the final pre-codegen query; its `accumulated::<Diagnostic>`
    picks up parse + resolve + types + borrow stages). The driver
    gates codegen on borrow_check_query::is_some(); the
    TypedProgram for codegen still comes from check_query (no
    clone). codegen value type is BasicValueEnum<'ctx>;
    sentinel-runtime ships sentinel_alloc + sentinel_panic_oob
    per ADR 0015 D9; arrays + ?Struct heap payloads still leak
    (closes at C2.4 per ADR 0017 D8 / new sentinel_free symbol).
    ADRs 0001-0014 + 0016 ACCEPTED; ADR 0015 ACCEPTED-WITH-
    AMENDMENTS; **ADR 0017 PROPOSED** (Phase C2 kickoff with 14
    D-decisions: D1-D5 + D6 + D7 + D10-D13 exercised at C2.0.1
    + C2.0.2 + C2.1; D8 / D9 / D14 still pending across C2.2 →
    C2.5; ADR flips to ACCEPTED at C2.5 close-out).

    Borrow-checker state at C2.1 close: shared-only (`&T`
    only). Two `BorrowError` variants — `OutlivesSource`
    (use-after-scope) and `ReturnsLocalRef` (per ADR 0017 D7
    "second-class refs everywhere"). Borrow-source enum is
    `{ Local(VarId), Incoming, LocalAnonymous }`. Per-fn
    analysis; inter-procedural reasoning bounded to "a call
    returning a ref inherits the most-restrictive source among
    its ref args". Lexical formulation per ADR 0017 D6 — each
    borrow's lifetime is "from creation to enclosing scope
    end"; inner blocks push/pop scopes. C2.2 will extend the
    same crate with `&mut T` + shared-XOR-mutable rule (3
    new error variants: BorrowConflict /
    MutableBorrowOfShared / SharedBorrowOfMutable).

    Type universe at C2.0.2 / C2.1 (unchanged at C2.1): `{ I64,
    I32, Bool, Struct(StructId), Nullable(NullableInner),
    Array(ArrayElem), TypeParam(TypeParamId),
    GenericInstance(GenericInstanceId), Ref(RefId) }`.
    `refs: Vec<RefData>` interner on TypedProgram preserves
    `Type: Copy` across the 5th ADR running. NullableInner has
    `Ref(RefId)` for `?&T`; ArrayElem stays primitive-only
    (`[&T]` rejected with `TypeError::RefInArray` per ADR 0017
    D1; refs in struct fields rejected with `RefInStructField`
    per D7).

    Lexer state (unchanged from C2.0.1): keywords `let, fn,
    if, else, true, false, struct, null, mut`; punctuation
    `+ - * / = ( ) { } [ ] , ; : . ? & ->` `== != < <= > >=
    && || !`. `*` reused for deref; `&` reused for borrow-take
    + ref-type prefix.

    **Next: C2.2** — `&mut T` exclusive borrow + the shared-
    XOR-mutable rule per ADR 0017 D6. The largest sub-phase
    per D9's table (2-3 sessions estimated). Introduces borrow
    conflict detection: a `&mut T` borrow can't coexist with
    ANY other `&T` or `&mut T` of the same place; multiple
    `&T`s remain fine. New BorrowError variants:
    BorrowConflict, MutableBorrowOfShared, SharedBorrowOfMutable.
    Extends the C2.1 BorrowSource tracking with a "place
    tracking" representation (which binding does an lvalue
    ultimately denote, transitively through field-access /
    deref) + per-place conflict windows.

    Sub-phase split for the rest of C2 (per ADR 0017 D9 table,
    revisit at each sub-phase boundary):
      - ✅ C2.0.1 — lexer: `&` token + `mut` keyword
      - ✅ C2.0.2 — refs infrastructure
      - ✅ C2.1   — shared-only lexical borrow checker
      - C2.2   — &mut T + shared-XOR-mutable rule (largest;
                  this session if continuing)
      - C2.3   — move semantics + use-after-move
      - C2.4   — RAII / drop + sentinel_free runtime symbol;
                  **closes the C1.6+ heap-leak deferral**
      - C2.5   — polish + Polonius migration plan doc + ADR
                  0017 PROPOSED → ACCEPTED flip + STATE +
                  HANDOVER close-out

    Read in order:
      1. docs/HANDOVER.md §0 in full (~830 lines through §0.3)
      2. docs/decisions/0017-phase-c2-kickoff-and-region-plan.md
         (the canonical C2 design — especially D6 for the
         lexical-first borrow checker, D7 for second-class-refs,
         and the per-sub-phase split table; D6 exercise notes
         at C2.1 are in the ADR's status header)
      3. docs/STATE.md (last-updated banner — C2.1 landed;
         shared-only borrow checker. Previous C2.0.2 banner
         kept as pre-C2.1 context)
      4. crates/sentinel-borrow-check/src/lib.rs (~600 LOC
         including tests) — the analysis structure to extend
         at C2.2; see source_of_expr / source_of_lvalue /
         BorrowSource / FnCtx.
      5. docs/SENTINEL_DESIGN2.md §4 / §15 for the C2 long-term
         surface (named regions, ownership qualifiers,
         first-class refs via `'esc`) — the eventual target
         that ADR 0017's C2 minimum is the stepping stone for

    Sanity check at session start:
      cargo build --workspace
      cargo clippy --workspace --all-targets -- -D warnings
      cargo test --workspace                  # expect 889 passing
      cargo run --bin snc -- build tests/pass/c21_go_no_go.sentinel -o /tmp/c21
      /tmp/c21 && echo "exit=$?"              # expect "168" then "exit=0"

    Resume at C2.2 per ADR 0017. Extend
    sentinel-borrow-check with `&mut T` analysis + place-
    tracking + per-place conflict windows. New BorrowError
    variants land + new c22_* pass/fail fixtures. The
    BorrowCheckedProgram parallel-tree that ADR 0017 D6
    mentions hasn't been needed yet (C2.1 just returns
    Vec<BorrowError>); C2.2 may introduce it if the conflict-
    window analysis benefits from materialised lifetime
    metadata.

---

## 1. Scope of This Document

This is not a specification of Sentinel. The design documents are the
specification, and they are still partly under-specified by intent. This
document covers how to *build* the bootstrap compiler: environment,
tooling, architecture, milestones, testing, and the order in which to
attack the work.

The audience is a senior engineer or small team (one to three people)
with Rust experience and some compiler or PL background. If you have not
written a compiler before, plan to spend the first two weeks reading
"Crafting Interpreters" and the rustc dev guide before starting on
milestone zero.

---

## 2. Strategic Approach

Do not start by building the full Sentinel compiler. The design document
explicitly recommends a staged validation: prove the broker idea works
as a Rust library, prove the effects system works as a research
prototype, and only then commit to building the full language. This
handover document follows that staging.

The milestones below are organized as four phases. Phase A and Phase B
are the validation prototypes. Phase C is the bootstrap compiler proper.
Phase D is the path to self-hosting. Each phase has a clear go/no-go
decision point at the end. Do not skip the decision points. If Phase A
produces a broker that nobody wants to use, building Phase C is wasted
effort.

The expected calendar time, with a small focused team, is roughly:
Phase A six months, Phase B nine months (overlapping the second half of
Phase A), Phase C twelve to eighteen months, Phase D another nine to
twelve months. This is honest, not optimistic. Most language projects
underestimate by 2-3x; budget accordingly.

---

## 3. Environment Setup (macOS)

### 3.1 Toolchain

Install Rust via rustup. Use the stable channel for the compiler itself
and pin the version in `rust-toolchain.toml` so contributors get
reproducible builds.

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    rustup default stable
    rustup component add rustfmt clippy rust-analyzer
    rustup target add aarch64-apple-darwin x86_64-apple-darwin

Install LLVM via Homebrew. Pin to a specific major version because
`inkwell` and LLVM's C API are version-coupled.

    brew install llvm@18
    echo 'export PATH="/opt/homebrew/opt/llvm@18/bin:$PATH"' >> ~/.zshrc
    echo 'export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18' >> ~/.zshrc

Install supporting tools:

    brew install cmake ninja just ripgrep fd jq
    cargo install cargo-nextest cargo-insta cargo-deny mdbook

`just` is used as the command runner instead of `make`. `cargo-nextest`
is significantly faster than the default test runner for compiler test
suites. `cargo-insta` is used for snapshot testing of compiler output.

### 3.2 Repository Layout

Create a single workspace repository. Do not split into multiple repos
yet; the dependency churn early on will make multi-repo intolerable.

    sentinel/
    ├── Cargo.toml              # workspace manifest
    ├── rust-toolchain.toml
    ├── justfile
    ├── .github/workflows/
    ├── docs/
    │   ├── SENTINEL_DESIGN.md
    │   ├── SENTINEL_DESIGN2.md
    │   └── HANDOVER.md         # this file
    ├── crates/
    │   ├── sentinel-broker/        # Phase A deliverable
    │   ├── sentinel-effects-proto/ # Phase B deliverable
    │   ├── sentinel-syntax/        # lexer + parser + CST
    │   ├── sentinel-ast/           # AST types
    │   ├── sentinel-resolve/       # name resolution
    │   ├── sentinel-types/         # type/region/effect checking
    │   ├── sentinel-hir/           # typed HIR
    │   ├── sentinel-mir/           # SSA-form IR
    │   ├── sentinel-codegen/       # LLVM lowering
    │   ├── sentinel-driver/        # the `snc` binary
    │   ├── sentinel-runtime/       # the broker as runtime library
    │   └── sentinel-lsp/           # language server
    ├── tests/
    │   ├── ui/                     # compile-error tests
    │   ├── pass/                   # programs that should compile and run
    │   └── snapshots/              # insta snapshots
    └── examples/

### 3.3 Initial Workspace Manifest

The top-level `Cargo.toml` declares the workspace and pins dependency
versions centrally. Every member crate inherits from `[workspace.deps]`
rather than declaring its own versions. This avoids version drift, which
is the most common source of pain in multi-crate Rust projects.

Key dependencies to pin from day one: `logos` for lexing, `chumsky` or
hand-written recursive descent for parsing (recommend hand-written for
better error messages), `salsa` for the query engine, `inkwell` for
LLVM, `cranelift` for the debug backend, `bumpalo` and `typed-arena`
for AST allocation, `indexmap`, `rustc-hash`, `smallvec`, `tracing`,
`thiserror`, `miette` for diagnostics, `insta` for snapshot tests.

### 3.4 Build Commands

Define common commands in a `justfile`:

    default: build

    build:
        cargo build --workspace

    test:
        cargo nextest run --workspace

    fmt:
        cargo fmt --all

    lint:
        cargo clippy --workspace --all-targets -- -D warnings

    snc *args:
        cargo run --bin snc -- {{args}}

    check-all: fmt lint test

    bless:
        INSTA_UPDATE=always cargo nextest run --workspace

---

## 4. Phase A — The Broker Prototype

**Goal**: build the memory broker as a standalone Rust crate, ship it,
get real users, learn whether the API actually proves out.

**Duration**: three to six months.

**Go/no-go criterion**: at least three real Rust projects (yours or
external) have adopted the broker for non-trivial work and the API has
stabilized through their feedback. If after six months no one wants to
use it, the broker idea is wrong or the API is wrong, and Sentinel
should pause.

### 4.1 What to Build

The broker crate exposes a `Broker` struct that owns allocation policy
for a process. It provides:

  - Generational arenas with O(1) bulk free and safe dangling-handle
    detection. Handles are `(arena_id, slot_index, generation)` triples.
    Access through a handle checks the generation atomically.
  - Programmable allocation strategies as trait objects. The default
    strategies are bump, slab, and system-malloc, but users can plug in
    their own.
  - Memory budgets with structured-scope semantics. The Rust API uses a
    builder pattern since Rust does not have effect handlers.
  - Statistics queries (live bytes, peak, fragmentation, allocation
    counts by tag).
  - A recording mode that captures every allocation event into a ring
    buffer for deterministic replay.
  - Secret-memory policy with mlock, no-core-dump exclusion, and
    zero-on-free. On macOS this uses `mlock(2)`, `madvise(MADV_NOCORE)`
    where available, and explicit zero with a barrier on free.

What to *defer*: cross-process shared memory (hard, do it in Phase C
when you have the full language to express it). Memory-hard secret
storage (research, post-1.0). Argon2id integration (use the `argon2`
crate as a separate library, not part of the broker yet).

### 4.2 API Sketch

    use sentinel_broker::{Broker, Arena, Budget, Handle, Secret};

    let broker = Broker::new();
    let arena = broker.create_arena("request", 4 * 1024 * 1024);
    let handle: Handle<Request> = arena.alloc(Request::new());

    let req: &Request = handle.get()?; // returns Err if invalidated
    arena.drop(); // all handles into this arena now return Err

    let budget = Budget::new(8 * 1024 * 1024);
    budget.scope(|alloc| {
        let v: Vec<u8, _> = Vec::with_capacity_in(1000, alloc);
        // ...
    }).map_err(|over| /* graceful fallback */)?;

    let key: Secret<[u8; 32]> = broker.alloc_secret([0u8; 32]);
    // key is mlock'd, zeroed on drop, excluded from Debug output

### 4.3 Validation

Write three example programs that use the broker for real work:

  - A small HTTP server with per-request arenas.
  - A parser combinator library that uses bump allocation for AST nodes.
  - A key-value store with the budget API enforcing memory limits.

If writing these is awkward, the API is wrong. Iterate.

Publish the crate to crates.io as `sentinel-broker` once the API feels
stable. Watch what users do with it. The point is to discover whether
the broker concept survives contact with real code.

---

## 5. Phase B — The Effects Prototype

**Goal**: build a small interpreted language with algebraic effects and
the `secret` qualifier, just enough to learn whether the effects-as-
capabilities story works in practice.

**Duration**: six to nine months, can start at month three of Phase A.

**Go/no-go criterion**: you can write a small program that demonstrates
supply-chain capability enforcement, async-as-effect, and constant-time
operations on `secret` data, and the ergonomics feel reasonable.

### 5.1 What to Build

A tree-walking interpreter for a tiny language — call it Sentinel-Mini.
No classes, no regions, no broker integration. The point is to validate
the effect system in isolation.

Required features:

  - Hindley-Milner-style type inference with effect rows.
  - Effect declarations: `pure`, `io`, `network`, `throw`, `await`.
  - Effect handlers with `handle expr with { ... }` syntax.
  - The `secret T` qualifier with constant-time equality and a
    "no branching on secret" check.
  - A capability check: importing a module restricts its effects.

What to *defer*: anything not directly testing the effect system.
Performance is irrelevant; this is a research artifact.

### 5.2 Validation

Write three example programs:

  - A "supply chain attack" demo where importing a JSON parser fails
    because it declares the `network` effect.
  - An async demo where the same function runs synchronously in tests
    and asynchronously in production by swapping the effect handler.
  - A constant-time password verification demo that fails to compile
    if you try to branch on the comparison result.

Publish a short paper or technical report at the end of Phase B
documenting what worked and what did not. This is genuinely useful
output even if Sentinel never proceeds.

---

## 6. Phase C — The Bootstrap Compiler

**Goal**: build the production Sentinel compiler in Rust, targeting the
1.0 subset defined in SENTINEL_DESIGN2.md Section 15.

**Duration**: twelve to eighteen months.

**Go/no-go criterion**: the compiler can compile a non-trivial Sentinel
program (target: a TLS handshake implementation, or an HTTP server)
that exercises all 1.0 features.

### 6.1 Architecture

The compiler is a query-based pipeline built on Salsa. Every phase is
expressed as a memoized query over inputs; incremental recompilation
is foundational, not retrofitted.

The pipeline:

    source file
      -> [sentinel-syntax]    lexer + parser -> CST
      -> [sentinel-ast]       CST -> AST lowering
      -> [sentinel-resolve]   name resolution, module graph
      -> [sentinel-types]     type, region, nullability, secrecy,
                              effect inference and checking
      -> [sentinel-hir]       typed HIR with all qualifiers resolved
      -> [sentinel-mir]       SSA lowering, escape analysis,
                              bounds-check elision, constant-time
                              verification on secret data
      -> [sentinel-codegen]   LLVM IR via inkwell, or Cranelift for
                              fast debug builds

The driver crate (`snc`) wires the queries together and exposes the
command-line interface. The LSP crate (`sentinel-lsp`) reuses the
exact same query engine, which is the entire point of using Salsa.

### 6.2 Implementation Order Within Phase C

Build the pipeline end-to-end for the smallest possible language
subset first, then expand. This is the rustc approach and it works.

**C0 (month 1-3)**: lexer, parser, AST for a subset with only `let`,
arithmetic, `if`, and function calls. End-to-end compilation to LLVM
that produces a runnable binary. No type system yet; everything is
i64. The goal is to prove the pipeline plumbing works.

**C1 (month 3-6)**: bring up the type system. Add `struct`, basic
generics, and references. Implement non-nullable types and the `?T`
optional. Bounds-checked array access. At the end of C1 the compiler
should reject all the "obvious" memory-safety violations.

**C2 (month 6-9)**: regions and ownership. Named regions, second-class
references by default, move semantics, `&` and `&mut` borrows. This is
the hardest single phase; budget pessimistically. Use the Polonius
formulation of borrow checking; it generalizes more cleanly than the
NLL formulation when you add regions.

**C3 (month 9-12)**: effects. Integrate the lessons from Phase B.
Effect inference, effect handlers, async-as-effect, capability
enforcement at the module boundary. Add the `secret` qualifier with
constant-time operations and the speculation-barrier insertion in
codegen.

**C4 (month 12-15)**: classes, traits with named implementations,
delegation, structured concurrency, actors. Most of this is
"reasonable language design plumbing" rather than novel work, but the
volume is significant.

**C5 (month 15-18)**: broker integration, cross-process safety,
reproducible-build guarantees, stable ABI definition, LSP and tooling
polish.

### 6.3 Diagnostics

Diagnostic quality is not optional. The borrow checker, region
checker, and effect checker will produce confusing errors by default,
and Sentinel's whole pitch depends on these errors being
comprehensible. Use `miette` for rich diagnostics from day one.
Allocate at least 15% of compiler engineering time to error message
quality. Steal Elm's and Rust's diagnostic conventions shamelessly.

Every error should answer three questions: what is wrong, why is it
wrong, and what should I do about it. Test diagnostics with snapshot
tests so regressions are visible in PRs.

### 6.4 Testing Strategy

Three layers:

  - **Unit tests** in each crate for individual functions and types.
    Standard Rust practice.
  - **UI tests** in `tests/ui/`. Each is a Sentinel program plus an
    expected stderr. Modeled on rustc's UI test suite. These catch
    regressions in diagnostics and in what the compiler accepts or
    rejects.
  - **Execution tests** in `tests/pass/`. Each is a Sentinel program
    plus expected stdout. The test runner compiles and runs the
    program and compares output.

Use `cargo-insta` for snapshot management. Every PR runs the full
suite via `cargo nextest`. CI fails on any unblessed snapshot
difference.

### 6.5 Performance Targets

Compile time is part of the value proposition. Set targets early and
measure continuously:

  - Clean build of a 10K-line program: under 30 seconds.
  - Incremental build after a one-line change: under 1 second.
  - LSP "go to definition" latency: under 50ms p95.

These are aspirational but they shape architecture decisions. If you
hit a fork in the road, take the path that preserves these targets.

---

## 7. Phase D — Self-Hosting

**Goal**: rewrite the compiler in Sentinel, reach the four-stage
fixed point described in SENTINEL_DESIGN.md.

**Duration**: nine to twelve months after Phase C completes.

**Go/no-go criterion**: stage-three compiler compiles its own source
to a binary that, fed its own source, produces a byte-identical
binary.

### 7.1 Staging

Follow the four-stage plan from the design document exactly. Do not
attempt to self-host all at once; the half-and-half configurations
(Sentinel parser feeding Rust type checker, etc.) are what surface
language ergonomics problems.

Stage one is the easiest and most informative: port the lexer and
parser. If writing the parser in Sentinel is unpleasant, the
language is wrong, and you find out cheaply.

### 7.2 Keep the Rust Bootstrap Alive

Do not delete the Rust bootstrap when self-hosting succeeds. Maintain
it indefinitely as a reproducibility anchor and a defense against
trusting-trust attacks. Pin which Sentinel version the self-hosted
compiler is written in, separately from which Sentinel version it
implements. Every Sentinel release should be buildable from the Rust
bootstrap.

---

## 8. Open Questions to Resolve Early

These are listed in design document Section 18 but they need
*decisions* before Phase C, not just acknowledgment.

**Effects with traits**: can trait methods declare effects
polymorphically? If yes, design the row-polymorphism story now. If no,
document the workaround. This decision shapes the entire type system
and must be made before C3.

**Region inference vs explicit regions**: the design says "named,
visible regions" but practical ergonomics likely require some
inference. Decide where the line is before C2. Recommend: regions are
inferable within a function body but must be explicit at function
boundaries when more than one region is involved.

**Async runtime**: even with effects-as-async, you need a default
scheduler. Will Sentinel ship its own, or wrap an existing one (Tokio
via FFI)? Decide before C3. Recommend: ship a minimal scheduler in
the standard library, allow user-defined schedulers via effect
handlers.

**Stable ABI scope**: a stable ABI for the whole language is
extremely ambitious. Restrict to `extern "sentinel-stable"`
declarations explicitly, like Swift did with `@frozen`. Decide the
exact subset before C5.

**Generic dispatch default**: witness tables vs monomorphization.
The design says witness tables by default, but measure both on
realistic code before committing. Decide before C1.

Document each decision in `docs/decisions/NNNN-title.md` using the
Architecture Decision Record format. Future contributors will need
the reasoning, not just the outcome.

---

## 9. Team and Process

### 9.1 Minimum Viable Team

A realistic minimum is two senior engineers with compiler experience
plus one engineer doing tooling, build infrastructure, and developer
experience. A single person can do Phase A and start Phase B but
cannot realistically complete Phase C alone in any reasonable
timeframe.

If you only have one person, do Phase A, do a reduced Phase B, and
write a thorough postmortem. That alone is a significant contribution.

### 9.2 Process

Use a monorepo. Use trunk-based development with short-lived feature
branches. Require PRs to pass `just check-all` before merge. Require
ADRs for any decision that touches language semantics. Hold a weekly
design review focused on the open questions in Section 8.

Do not chase contributors aggressively in the first year. A small
focused team makes faster progress than a large unfocused one, and
language projects are particularly vulnerable to bikeshedding when
contributors arrive before the core design is stable.

### 9.3 Communication

Maintain a public design log as `mdbook` in `docs/`. Every significant
decision lands as a chapter. Publish quarterly progress reports. This
discipline forces clarity on what you actually built versus what you
planned, and it builds the credibility needed when you eventually
want adopters.

---

## 10. Day One Checklist

When you sit down to actually start:

  - Clone or create the `sentinel` repository with the layout in
    Section 3.2.
  - Install the toolchain from Section 3.1.
  - Copy SENTINEL_DESIGN.md, SENTINEL_DESIGN2.md, and HANDOVER.md
    into `docs/`.
  - Initialize the workspace with empty crates matching Section 3.2.
  - Set up CI to run `just check-all` on every PR.
  - Create `docs/decisions/0001-staged-validation.md` recording the
    decision to follow the Phase A through D plan.
  - Start Phase A milestone one: scaffold `sentinel-broker` with
    the `Broker::new()` constructor, the simplest possible arena, and
    a test that allocates and frees a value.

Ship something on day one. The hardest part of starting a multi-year
project is starting; the rest is iteration.

---

## 11. What to Do When Stuck

You will get stuck. Specific places it tends to happen:

  - **Borrow checker design** in C2. Read the Polonius papers, read
    the rustc dev guide chapter on NLL, look at how Hylo handles
    second-class references. Allocate four weeks of design time
    before writing code.
  - **Effect inference** in C3. Read the Koka and Effekt papers. The
    row polymorphism formulation is the standard one; implement it
    even though it is harder than the alternatives, because the
    alternatives do not compose.
  - **LLVM integration** anywhere. `inkwell` papers over most of the
    pain but not all. When in doubt, write the LLVM IR by hand first,
    confirm it does what you want, then figure out how to generate it
    from `inkwell`.
  - **Diagnostics that confuse users**. Find five people unfamiliar
    with Sentinel, show them the error, ask them to explain it. Their
    confusion is more informative than any internal review.

The general rule: when stuck for more than three days, write the
problem down as a design document, share it for review, and timebox
the resolution. Languages die from indecision more often than from
bad decisions.

---

## 12. Closing

Sentinel is an ambitious project, and the honest assessment in
SENTINEL_DESIGN2.md applies: most language projects at this level of
ambition do not reach widespread adoption. That is not a reason not to
build it. The ideas — programmable runtime, regions, effects-as-
capabilities, the `secret` qualifier — are worth exploring even if the
full language never ships at scale. Each phase produces value
independently. Each phase has a clear go/no-go decision. Each phase
teaches you something the next phase needs.

Build Phase A. See what happens. Decide from there.

Good luck.

*End of document.*
