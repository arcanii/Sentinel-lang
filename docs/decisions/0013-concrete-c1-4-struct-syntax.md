# ADR 0013: Concrete C1.4 surface syntax — struct declarations, field access, construction

Status: PROPOSED — no D-decision exercised yet; this ADR pins the
surface decisions before the C1.4 feat commits land per the
ADR-first-per-phase-boundary norm (HANDOVER §0.1). Will flip to
ACCEPTED when the C1.4 sub-phases all ship.
Date: 2026-05-26
Last touched: 2026-05-26 (initial PROPOSED draft)
Related: 0011 (Phase C1 kickoff; D3 commits to `struct` as part of
the C1 primitive set, D6 schedules C1.4 after C1.3), 0012 (concrete
C1 surface — established the D3 "primitives are identifiers, not
keywords" precedent that this ADR extends to user-defined struct
names; D6's non-associative comparison rule informs the comparable-
struct question), 0010 (concrete C0 surface — established the
recursive-descent parser shape that C1.4 extends with a top-level
struct production)

## Context

C1.3 landed the C1 primitive type surface: `i64`, `i32`, `bool`,
comparisons, logicals, the bool-typed `if` condition. The type
universe is now `{ I64, I32, Bool }`. ADR 0011 D3 listed
`struct Name { field: Type, … }` as part of the C1 type system;
C1.4 (per ADR 0011 D6) introduces it.

C1.3's heritage is largely complete: the parallel-tree pattern
(AST → ResolvedExpr → TypedExpr) absorbs new variants
mechanically; the salsa query graph passes types through
transparently; the codegen reads `expr.ty` to drive lowering.
What's new at C1.4 is **the first compound type**: every C1.0-1.3
value was a scalar (i1 / i32 / i64); a struct is multiple values
packed together, with field-by-name access. That changes the
codegen value type (no longer just `IntValue<'ctx>`), introduces
nominal type identity (two structs with identical fields are
distinct types), and adds a third top-level declaration form
(alongside `fn`).

The 1.0 target surface (SENTINEL_DESIGN2.md §3 + §15.1) reads
Rust-style for structs:

    struct Point { x: i64, y: i64 }

    let p = Point { x: 3, y: 4 };
    let d = p.x + p.y;

C1.4 ships this minimum: named-field structs, named-field
construction, field access via `.`. The features Rust has that
this ADR explicitly does not commit to at C1.4: tuple structs,
unit structs (`struct Marker;`), private fields, methods,
generics, derives. The ADR 0011/0012 pattern of "defer everything
not needed for the sub-phase go-criterion" continues.

The C1.3 lexer's current token set is:

  - **Keywords**: `let`, `fn`, `if`, `else`, `true`, `false`
  - **Punctuation**: `+ - * / = ( ) { } , ; : ->`
    `== != < <= > >= && || !`
  - **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`
  - **Integer literals**: `[0-9]+`
  - **Skipped**: `[ \t\r\n]+`, `//[^\n]*`

C1.4 extends this with the additions listed in D8 below.

## Decision

Twelve D-numbered sub-decisions covering syntax (D1-D6), lexer
additions (D8), recursion handling (D7), and what stays explicitly
out of scope (D9-D12).

### D1. Struct declaration syntax: `struct Name { field: Type, … }`.

Top-level (alongside `fn`). Rust-style grammar:

    struct_decl  = 'struct' Ident '{' field_list '}'
    field_list   = (field (',' field)*)? ','?
    field        = Ident ':' type

Trailing comma after the last field is permitted (consistent with
fn parameter lists per ADR 0010 D5). Empty structs are allowed:
`struct Empty {}` — sometimes useful as type-level markers. No
private fields at C1.4 (every field is public; the `pub` keyword
doesn't exist yet).

Struct declarations live at the top level only at C1.4. Local
struct declarations inside fn bodies are not in scope. The 1.0
language may or may not allow them; if it does, they land in a
later ADR — the surface decision is small and revisitable.

### D2. Field access syntax: `expr.field`.

Postfix `.` followed by an identifier. New `.` token (per D8).
Grammar at the atom level:

    atom         = ... | atom '.' Ident          // field access (C1.4)

Field access binds tighter than any unary or binary operator —
it's part of the atom. `-p.x` parses as `(-(p.x))`, `p.x + p.y`
as `((p.x) + (p.y))`, `f(p).x` as `((f(p)).x)`.

Field access on a non-struct type is a type error
(`TypeError::FieldAccessOnNonStruct { got, span }`); access of an
unknown field is a separate type error
(`TypeError::UnknownField { struct_name, field, span }`).

Chained field access (`a.b.c`) is naturally handled by parsing
postfix `.` repeatedly — the parser sees `a`, then `.b` → `a.b`,
then `.c` → `(a.b).c`.

### D3. Struct construction syntax: `Name { field: expr, … }`.

Rust-style named-field initialization:

    struct_lit   = Ident '{' field_init_list '}'
    field_init   = Ident ':' expr
    field_init_list = (field_init (',' field_init)*)? ','?

All fields must be present. Field order in the literal may differ
from the declaration order (Rust pattern); the type checker
validates that the set of named fields matches the declaration's
set exactly. Missing fields surface as
`TypeError::MissingField { struct_name, field, span }`; extra
fields as `TypeError::UnknownField { struct_name, field, span }`
(same variant as D2's unknown-field case — uniform diagnostic).

Field-init shorthand (`Foo { x, y }` ≡ `Foo { x: x, y: y }`) is
NOT in scope at C1.4. The full `field: expr` syntax always
required. The shorthand is a small ergonomic win that can be
added later without surface churn.

Functional update syntax (`Foo { x: 1, ..rest }`) is also NOT in
scope. Same revisit rule.

### D3a. Parser disambiguation: struct literal vs identifier-then-block.

The construction `Name { … }` looks like an identifier followed
by a block expression. The parser needs to disambiguate:

  - At the **statement level / general expression position**, a
    bare `Foo { ... }` could be either a struct literal OR an
    identifier reference followed by a block expression statement.
    Rust resolves this with context-sensitive parsing — struct
    literals are forbidden in certain positions (the condition of
    `if`, `while`, the iterable of `for`).
  - C1.4 follows the same precedent: struct literals are
    forbidden in the condition position of `if`. The grammar is
    `if_expr = 'if' expr_no_struct_lit block else_branch`. Inside
    a parenthesized expression or a let-binding RHS, struct
    literals are unambiguous (no following block).
  - The full set of struct-lit-forbidden positions is just `if`
    cond at C1.4 (no `while`, `for`, or `match` exists yet).

This is a small parser-internal mode flag (`allow_struct_lit:
bool`); the public surface is unchanged. Parens always escape:
`if (Foo { x: 1 }.x == 1) { ... }` parses unambiguously.

### D4. Struct types in type position: `Name` is a recognized identifier.

Per ADR 0012 D3, type-position identifiers are resolved by the
parser/type-checker, not the lexer. C1.4 extends `resolve_type_expr`
to look up identifiers against the struct table (after the
built-in `i64` / `i32` / `bool` cases). Order:

    "i64" / "i32" / "bool" → built-in
    Ident                  → look up in struct table → Type::Struct(StructId)
    (anything else)        → TypeError::UnknownType

The pattern is exactly the same machinery as ADR 0012 D3
established; only the lookup table grows. Future C1.5+ extensions
(`?T`, `Vec<T>`, generics) extend by additional pattern matches
on the parsed `TypeExpr`, not by changes to this lookup.

### D5. Nominal type equality.

Two structs with identical field names + field types but distinct
declaration names are distinct types. `Point` and `Vec2` with
`{ x: i64, y: i64 }` each don't implicitly convert. Same rule as
Rust, Go, Swift; opposite of TypeScript's structural typing.

The implementation: `Type::Struct(StructId)`; `Type` equality is
discriminant + StructId equality. The check() Mismatch variant
gains a sensible Display for struct types (the struct's source-
level name).

### D6. Struct equality via `==` / `!=`: deferred.

At C1.4, `==` / `!=` accept only operands of the same numeric or
bool type per ADR 0012 D6. Struct equality requires either:
  - Field-by-field structural compare (every field must be
    equatable); or
  - A user-defined equality method (C4 with classes/traits).

Both are reasonable; both add work that isn't on C1.4's path.
Defer to C1.5+ revisit; the type checker emits
`TypeError::Mismatch` (the existing variant) if a user tries
`some_struct == other_struct`. The diagnostic could be improved
(struct-specific help text) but the type-error itself fires
correctly.

### D7. Recursive structs: detect and reject at type-check time.

A struct that contains itself (directly or transitively) has
infinite size with no indirection. C1.4 has no references
(`&T` — C2), no nullability (`?T` — C1.5), no pointers — so a
recursive struct cannot be sized:

    struct Node { next: Node }   // -> infinite size

The type checker performs a cycle detection pass over the struct
declarations and surfaces a new error variant:

    TypeError::RecursiveStruct { name: String, cycle: Vec<String>, span: Span }

The cycle field traces the path (e.g., `["Tree", "Branch",
"Tree"]` for a mutually recursive cycle). C1.5's `?Node`
indirection breaks the cycle and lifts this restriction; at
C1.4 the type checker is the gate.

The cycle detection is a textbook tarjan/DFS over struct→field-
struct edges. Fast in practice; O(structs + fields).

### D8. Lexer additions at C1.4.

Two new tokens, listed in the order they're added to the `logos`
enum:

| Token    | Notes                                              |
| -------- | -------------------------------------------------- |
| `struct` | reserved keyword                                   |
| `.`      | field access; binds tightest as a postfix operator |

Two new tokens total. logos's longest-match is not relevant here
(there's no `..` / `...` / `.=` to disambiguate against — those
arrive when ranges / spread / etc. land, which is beyond C1).

Per ADR 0012 D3, primitive and user-defined type names remain
`Ident`-class lexer tokens. `struct` joins `let` / `fn` / `if` /
`else` / `true` / `false` as a true keyword (it sits at
declaration position, can't be confused with an expression).

### D9. Tuple structs, unit structs, derives: not at C1.4.

Tuple structs (`struct Pair(i64, i64)`) and unit structs
(`struct Marker;`) are conveniences that don't earn their keep
at C1.4. The same goes for derive macros (`#[derive(Eq, Debug)]`).
All revisitable in later ADRs without surface churn — they extend
the struct grammar without conflicting with the D1 shape.

### D10. Methods, generics, lifetimes: not at C1.4.

`impl Foo { fn method(self, …) { … } }` waits for C4 (classes
and traits). `struct Foo<T> { … }` waits for C1.7 (generics).
`struct Foo<'a> { … }` waits for C2 (regions). The struct
surface at C1.4 is "data only": declaration, construction,
field access.

### D11. `fn main() -> i64` invariant stays.

Same as ADR 0012 D11. `main` returns `i64`; codegen truncates
to `i32` for the C ABI. Structs can be returned from non-main
functions but not from `main`.

### D12. Phase-go program for C1.4.

The canonical C1.4 acceptance program lives at
`tests/pass/c14_go_no_go.sentinel`:

    struct Point { x: i64, y: i64 }

    fn manhattan(p: Point) -> i64 {
        p.x + p.y
    }

    fn main() -> i64 {
        let p = Point { x: 3, y: 4 };
        print(manhattan(p))
    }

Expected: stdout `7\n`, exit 0. Exercises declaration + construction
+ field access + struct-as-fn-parameter + struct-as-let-binding
+ type-checker's struct-type rule + codegen's struct lowering.

## Reasoning

The decisions cluster around four themes:

**Pin down what C1.4 needs; nothing else.** D6 (no struct
equality), D9 (no tuple / unit / derive), D10 (no methods /
generics / lifetimes), D3's shorthand + spread exclusions all
defer features that don't earn their keep at C1.4's
go-criterion. Same pattern as ADR 0010 / 0012 — small surface,
revisitable.

**Match Rust where it costs nothing.** D1 (`struct Name { field:
Type, … }`), D2 (postfix `.field`), D3 (`Name { field: expr, … }`),
D3a (struct-lit-forbidden-in-if-cond), D5 (nominal equality) all
match Rust. The 1.0 target reads Rust-style; diverging at C1.4
just creates migration cost.

**Surface-future-compatible where it costs nothing.** D9's
deferred tuple/unit/derives extend D1 without breaking the
existing grammar. D10's deferred methods plug into a separate
`impl` form that doesn't touch struct decls. D6's deferred `==`
on structs is a relaxation, not a syntax change.

**Take a small migration cost when it buys correctness.** D7's
recursive-struct detection is new error-path work at C1.4 that
ADR 0011 didn't pre-commit to, but it's strictly necessary —
without it, codegen would either generate infinitely-sized
LLVM struct types (compile error or infinite loop) or quietly
miscompile. Detecting it at the type-checker gives a clean
diagnostic and aligns with the "obvious-memory-safety-violations
rejected at end of C1" exit criterion in HANDOVER §6.2.

## Consequences

### Positive

- C1.4's parser extension is bounded: one new top-level form
  (struct_decl) + one new atom form (field access) + one new
  expression form (struct literal). The hand-written recursive
  descent absorbs this as ~50-80 LOC.

- C1.4's codegen change is more substantial than C1.3 but
  bounded: introduce LLVM struct types in pass 1 alongside fn
  declarations; lower struct literals via build_insert_value
  (or alloca-and-store) chains; lower field access via
  build_struct_gep + build_load. The codegen value type widens
  from `IntValue<'ctx>` to `BasicValueEnum<'ctx>` — that's a
  non-trivial refactor but mechanical, and once landed it's
  ready for arrays (C1.6) and other compound types.

- C1.4's diagnostics gain `TypeError::FieldAccessOnNonStruct`,
  `TypeError::UnknownField`, `TypeError::MissingField`,
  `TypeError::RecursiveStruct`, `ResolveError::RedefinedStruct`.
  Each maps to a clear miette `#[diagnostic(code(...))]` entry.

- D7's cycle detection sets the precedent for C1.5's
  reference-cycle detection (when `&T` lands, ownership cycles
  via references will need similar analysis) and C1.7's
  generic-instantiation cycles.

### Negative

- D7's "reject recursive structs" creates a real expressivity
  gap until C1.5's `?T` lands. Users who want trees or linked
  lists at C1.4 cannot have them; they'll wait for C1.5. The
  mitigation is small (C1.5 is the next sub-phase) and the
  diagnostic is honest (it tells them to use `?T` when it
  arrives). Mentioned here for completeness.

- D3a's parser mode flag (struct-lit-forbidden-in-if-cond) is
  the first piece of context-sensitive parsing in the codebase.
  Mostly bounded — it's a Boolean threaded through `parse_expr`
  and reset at every `(` / `{`. Rust has the same complexity.

- The codegen value-type widening (`IntValue<'ctx>` →
  `BasicValueEnum<'ctx>`) touches every codegen function.
  Mechanical but pervasive — expect the C1.4 codegen commit to
  be the largest in C1's history.

### Neutral

- D11's `fn main() -> i64` decision is unchanged from C1.3.
  Structs can't be returned from `main` because the C ABI
  expects `i32` and there's no struct-to-i32 conversion. Trying
  to return a struct from `main` is rejected at type-check by
  the existing return-type-mismatch rule (`main` is declared
  `-> i64`, struct return value doesn't match).

## Alternatives considered

- **Structural typing (TypeScript-style).** Two structs with the
  same field names + types are interchangeable. Rejected per D5
  reasoning: Sentinel 1.0's target is Rust-style nominal typing;
  structural typing complicates name resolution + type errors +
  the eventual generics / traits story. Future-incompatible with
  the rest of the language design.

- **Positional struct construction (`Foo(1, 2)`).** Rejected
  per D3 reasoning: named-field construction is Rust's default
  for non-tuple structs, and named-field is more readable for
  structs with 3+ fields. Positional construction is fine for
  tuple structs (D9) which arrive in a later ADR.

- **Field-init shorthand (`Foo { x, y }`).** Rejected at C1.4
  per D3; small ergonomic win, doesn't earn its keep yet, easy
  to add without surface churn.

- **Allow recursive structs and let codegen emit infinite-size
  LLVM types (or pointer-of-self).** Rejected per D7
  reasoning: the language doesn't have indirection primitives
  at C1.4, so recursive structs literally can't be represented.
  Codegen-time detection is uglier than type-checker detection
  (the LLVM error message is opaque). Future C1.5 lifts the
  restriction via `?T`.

- **Private fields (`pub` / `priv` keywords) at C1.4.**
  Rejected: visibility is a module-system concern; C1.4 has no
  module system. Defer to C2 when modules + visibility land
  together.

- **Struct equality via `==` (field-by-field structural compare)
  at C1.4.** Rejected per D6 reasoning: the implementation isn't
  hard, but the question of "which types are equatable" (does
  every field need `==`? what about future `Bool` vs `Float`
  semantics?) is bigger than C1.4 should pre-commit to. Defer
  to C1.5+ revisit.

- **Defer struct codegen, type-check structs but reject at
  codegen.** Rejected: lets users write programs that type-check
  but don't compile, which contradicts the "if it types it
  runs" invariant established at C1.2. Real codegen support is
  the C1.4 deliverable.

- **`{ x = 3, y = 4 }`-style construction (no leading struct
  name).** Rejected: ambiguous with block expressions, doesn't
  match Rust, and infers struct type from context which C1's
  monomorphic checker isn't ready for.

## Revisit

This ADR is **PROPOSED** until C1.4 lands the syntax decisions
herein. Per-D revisit triggers:

- **D1** (struct decl shape): revisit at C1.5 if `?T`
  interaction surfaces unexpected grammar issues. Likely no
  change.

- **D2** (field access): revisit at C4 (classes/methods) — once
  methods exist, `x.method()` shares the same postfix-`.`
  syntax. The parser disambiguation (`Ident '(' ...` after `.`
  is a method call, vs `Ident` alone is a field access) is
  a small extension.

- **D3** (struct construction): revisit at C1.4-end if the
  shorthand `Foo { x, y }` proves a real ergonomic gap. Cheap
  to add.

- **D3a** (parser disambiguation): revisit when `while` / `for`
  / `match` arrive (C2+); each adds new struct-lit-forbidden
  positions following the same Rust pattern.

- **D5** (nominal equality): revisit only if the rest of the
  language pivots to structural typing, which is unlikely given
  ADR 0001's Rust-style direction.

- **D6** (no struct `==` at C1.4): revisit at C1.5+ when the
  set of equatable types is stable. Likely lifts to "all-
  equatable-fields → structural compare".

- **D7** (recursive struct rejection): revisit at C1.5 when
  `?T` lands and provides the indirection that breaks the cycle.
  The TypeError variant either stays (for users who write a
  recursive struct without `?`) or relaxes to "no cycle without
  some indirection".

- **D8** (`.` + `struct` tokens): revisit at C2+ when ranges /
  spread (`..`, `...`) might want to share lexer space. The
  longest-match rule should make co-existence painless.

- **D11** (`fn main() -> i64`): revisit at C1.5+ same as ADR
  0012's D11.

## Appendix: C1.4 phase-go program annotated

The full C1.4 phase-go program with the syntax pieces labeled:

    struct Point { x: i64, y: i64 }
    //     ^^^^^                            D1: top-level struct decl
    //           ^                          D1: braced field list
    //             ^                        D1: trailing fields OK

    fn manhattan(p: Point) -> i64 {
    //              ^^^^^                   D4: struct name in type
    //                                          position; resolves via
    //                                          struct table to
    //                                          Type::Struct(StructId)
        p.x + p.y
    //   ^^^                                D2: postfix field access
    //                                          binds as part of atom
    }

    fn main() -> i64 {
        let p = Point { x: 3, y: 4 };
    //          ^^^^^                       D3: struct literal at expr
    //                                          position (let RHS, so
    //                                          D3a doesn't fire)
    //                ^                     D3: braced field-init list
    //                  ^^^^                D3: `field: expr` form
        print(manhattan(p))
    //                  ^                   pass-by-value struct
    }

Expected behaviour:
  - check(): the `p` binding's inferred type is `Type::Struct(StructId
    for Point)`. `manhattan(p)`'s call-arg type matches the signature.
    `p.x + p.y` types as I64 + I64 → I64; `manhattan` returns I64.
    `print(I64)` accepts the arg. `main` returns I64.
  - codegen(): emits an LLVM struct type for Point (`{ i64, i64 }`).
    The Point literal stores 3 and 4 via build_insert_value into a
    fresh struct value. `manhattan` reads p via build_struct_gep at
    offsets 0 and 1, adds, returns. main truncates to i32.
  - runtime: stdout `7\n`, exit 0.

C1.4's go/no-go fixture is paired with the existing c05_go_no_go
(which keeps testing the C1.3 surface) and c12_go_no_go (not yet
shipped — see ADR 0011 D6 backlog) to give the suite a per-sub-
phase acceptance harness.

`tests/pass/c14_go_no_go.sentinel` (paired with its `.stdout`
assertion in the pass.rs harness) will be the concrete acceptance
fixture.
