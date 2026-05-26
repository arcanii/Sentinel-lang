# ADR 0012: Concrete C1 surface syntax — annotations, primitive types, comparisons, logicals

Status: ACCEPTED — every D-decision exercised at either C1.2
(D1-D4, half-D9, half-D10) or C1.3 (D5-D11). The C1.2 half
landed across commits af16655 / 90965a5 / ded07bc / c9a21ff;
the C1.3 half landed across 2801a81 (lexer) / cd1c0d4
(AST+parser+resolve+types+codegen) / ba5fd9d (truthy
retirement + 6 fixture rewrites + 7 new c13 pass-tests). All
twelve C1.3 tokens are in (`:` from C1.2.1; `true` `false` `==`
`!=` `<` `<=` `>` `>=` `&&` `||` `!` from C1.3.1). All seven
fixture rewrites are in (6 C0 if-using fixtures rewritten +
c05_go_no_go restructured to use `is_positive` + bool `pick`).
Date: 2026-05-26
Last touched: 2026-05-26 (C1.3 landed; D5-D11 status updated;
ADR flipped to ACCEPTED)
Related: 0010 (concrete C0 surface; D5 reserved `->` for this ADR's
annotation grammar, D9's C-style truthy retires here, D8's
comparison-operator gap closes here), 0011 (Phase C1 kickoff; D2
specifies explicit annotations at fn boundaries with inference
inside bodies, D3 lists the C1 primitive set, D6 schedules C1.2 +
C1.3 sub-phases, D7 specified that "ADR 0012 lands before C1.2",
D10 documents the C-style-truthy retirement)

## Context

ADR 0011 D7 deferred the concrete C1 surface syntax to a follow-up
ADR written after C1.0 + C1.1 shipped the front-end retrofit, on
the reasoning that annotation-grammar questions are easier to
argue against a real query-graph than a sketch. C1.0a-c landed the
Salsa machinery; C1.1.1-2 landed `sentinel-resolve` and rewired
codegen onto `ResolvedProgram`. The front-end is now query-shaped
and name-resolved.

C1.2 (per ADR 0011 D5 + D6) brings up `sentinel-types::check()`
real, which requires syntax for type annotations on parameters,
return types, and `let`-bindings. C1.3 (per ADR 0011 D6 + D10)
brings up multiple primitive types and retires ADR 0010 D9's
C-style truthy `if cond`, which requires `bool` literals,
comparison operators, and logical operators. Both sub-phases want
syntax decisions that don't churn — making them in one place,
before either lands, avoids the "wait, we already shipped the
annotation grammar and now C1.3 wants to extend it" coordination
problem.

This ADR pins the surface for C1.2 + C1.3 together. The 1.0 target
surface (SENTINEL_DESIGN2.md §3 + §15.1) reads as Rust-style with
`fn name(x: T) -> T` signatures, `let x: T = expr` annotations,
lowercase primitive types (`i32`, `i64`, `bool`, `f64`), and
postfix `?T` for nullability. C1 is a deliberately small subset:
no regions, no `secret` qualifier, no nullability (`?T` waits for
C1.5), no structs (C1.4), no generics (C1.7). Sentinel-Mini's
syntax (`==`, `<`, `>`) is a useful reference but Sentinel proper
uses `!=` (not `/=`) and `>=` / `<=` (not `≥` / `≤` etc.).

The C1.0b lexer's current token set is:

  - **Keywords**: `let`, `fn`, `if`, `else`
  - **Punctuation**: `+ - * / = ( ) { } , ; ->`
  - **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`
  - **Integer literals**: `[0-9]+`
  - **Skipped**: `[ \t\r\n]+`, `//[^\n]*`

C1.2 + C1.3 extend this with the additions listed in D9 below.

## Decision

Eleven D-numbered sub-decisions, split between C1.2 (D1-D4) and
C1.3 (D5-D8), with cross-cutting concerns in D9-D11.

### D1. Parameter and return-type annotations: `fn name(p: T) -> T`.

Confirms ADR 0010 D5's reservation of `->`. New token `:` (colon)
between parameter name and its type. Grammar at C1.2:

    fn_def        = 'fn' Ident '(' param_list ')' ret_ty? block
    param_list    = (param (',' param)*)? ','?
    param         = Ident ':' type
    ret_ty        = '->' type

Return-type annotation is **mandatory** at C1.2. The motivation is
ADR 0011 D2's "explicit annotations at fn boundaries"; making the
return type optional (defaulting to `()` or to the body's inferred
type) would invite inconsistency at function boundaries that C1's
monomorphic type checker is specifically trying to enforce. The
exception is `fn main` — see D11 for the migration rule.

Trailing comma in `param_list` is permitted (carries forward from
C0.5 / ADR 0010 D5).

### D2. Let-binding annotations: `let x: T = expr;`.

Annotation is optional per ADR 0011 D2. Grammar at C1.2:

    let_stmt      = 'let' Ident (':' type)? '=' expr ';'

If the annotation is present, the type checker enforces it against
the RHS. If absent, the type is inferred from the RHS. Same `:`
token as D1. No `let mut` yet; mutability is C2's concern with
regions and ownership.

### D3. Primitive type names: `i32`, `i64`, `bool` as recognized identifiers.

The primitive type names are **NOT** lexer keywords. They are
ordinary identifiers (matching the `[A-Za-z_][A-Za-z0-9_]*` rule)
that the parser/type-checker recognizes in type position. Same
approach as Rust.

Reasoning:

  - Reserving them as keywords would prevent users from writing
    `let i64 = 5;` (bind a value to a variable named `i64`), which
    is a small-but-real ergonomic surprise.
  - Type position and expression position are syntactically
    disjoint (type position only appears after `:` or `->`), so
    there's no ambiguity.
  - When C1.4 adds structs, `Foo` (a user-defined struct name) is
    also a Type-position identifier; the same machinery handles
    both cases.

C1.2 ships only `i64` as a recognized type name (matches the
i64-everything model). C1.3 adds `i32` and `bool` as recognized
names. Unknown type identifiers (e.g., `let x: Foo = ...` before
C1.4) surface as `TypeError::UnknownType { name, span }`.

The type-position grammar at C1.2 is the simplest possible:

    type          = Ident         // C1.2 recognizes "i64"; later phases extend

C1.4 will extend to `Ident` (struct names), C1.5 to `?type`,
C1.7 to `Ident '<' type_args '>'`. References (`&T`, `&mut T`),
regions (`@region T`), and `secret T` arrive at C2 / C3.

### D4. `i64` is the C1.2 universe; other primitives are C1.3.

C1.2 type-checks programs where every type annotation is `i64`.
This isolates the "annotation grammar + type-check infrastructure"
work from the "multiple types" work. Any annotation other than
`i64` (or an unknown identifier) surfaces as a type error.

C1.3 (per D5+D6+D7) adds `i32`, `bool`, comparison operators, and
logical operators in one go.

### D5. Bool literals: `true` and `false` as lexer keywords (C1.3).

Two new lexer keywords. Reserved everywhere; cannot be used as
identifiers. Their type is `bool`.

Reasoning:

  - They're literal values, not types. They appear in expression
    position, not type position. Unlike type names (D3), they
    can't be contextually recognized — they have to be reserved
    by the lexer.
  - Aligns with Rust, Sentinel-Mini, and every C-family language.
  - No existing C0 program uses `true` or `false` as an identifier
    (checked across `tests/pass/`), so the reservation is
    backwards-compatible with all fixtures.

### D6. Comparison operators: `== != < <= > >=` (C1.3).

Six new lexer tokens. All binary, left-associative (though their
result is `bool` so chaining is rare). Precedence: lower than
arithmetic (`+ -`) and higher than logical (`&& ||`). Both operands
must type to the same numeric type; result is `bool`.

    add_expr      = mul_expr (('+' | '-') mul_expr)*
    cmp_expr      = add_expr (('==' | '!=' | '<' | '<=' | '>' | '>=') add_expr)?

The comparison level is **non-associative**. `1 < 2 < 3` is a
parse error (or a type error — `1 < 2` is `bool`, `bool < 3` is a
type error; the parser-level rejection is the cleaner diagnostic).
Aligns with Rust; diverges from Python's chained comparisons. The
ergonomic loss is small; the cost of allowing chains is parser
complexity plus the risk of users thinking they mean what Python
means.

Sentinel-Mini's tokens are `==`, `<`, `>` only (no `!=`, no `<=`,
no `>=`). C1.3 lifts the surface to the complete six.

### D7. Logical operators: `&& || !` (C1.3).

Three new lexer tokens. `&&` and `||` are binary,
left-associative, short-circuit (the right operand is evaluated
only if needed). `!` is unary prefix. All operate on `bool` and
return `bool`.

Precedence, lowest to highest within the logical/comparison range:

    or_expr       = and_expr ('||' and_expr)*       left-assoc, short-circuit
    and_expr      = cmp_expr ('&&' cmp_expr)*       left-assoc, short-circuit
    cmp_expr      = add_expr (cmp_op add_expr)?     non-assoc (D6)
    add_expr      = mul_expr (('+' | '-') mul_expr)*
    mul_expr      = unary    (('*' | '/') unary)*
    unary         = ('-' | '!') unary | atom

Unary `!` sits at the same precedence as unary `-`. `!x == y`
parses as `(!x) == y`, not `!(x == y)` — matches Rust and C.

`!=` (D6) and `!` (D7) require longest-match lexing — `!=` lexes
as one token even though `!` is also a token. logos handles this
naturally.

### D8. `if` condition becomes `bool`-typed at C1.3.

Retires ADR 0010 D9's C-style truthy `if cond`. After C1.3:

  - `if cond { ... } else { ... }` requires `cond: bool`.
  - `if 5 { ... }` becomes a `TypeError::Mismatch { expected:
    bool, got: i64, ... }`.
  - The 7 C0 fixtures that use `if` with a non-bool condition
    (c04_if_true_branch, c04_if_false_branch, c04_if_with_var_cond,
    c04_else_if_chain, c04_if_with_print, c05_go_no_go, plus any
    we forget) get mechanical rewrites: `if x { ... }` becomes
    `if x != 0 { ... }`.

`else` remains mandatory through C1.3. The "no `else`" case
requires either Unit (C1.4 with structs) or `?T` (C1.5) to give
the if-expression a value when the condition is false; neither is
ready at C1.3.

### D9. Lexer additions across C1.2 + C1.3.

| Token      | Added in | Notes                              |
| ---------- | -------- | ---------------------------------- |
| `:`        | C1.2     | parameter and let-binding annotations |
| `true`     | C1.3     | reserved keyword                   |
| `false`    | C1.3     | reserved keyword                   |
| `==`       | C1.3     | comparison                         |
| `!=`       | C1.3     | comparison; lex before `!`         |
| `<`        | C1.3     | comparison; lex before `<=`        |
| `<=`       | C1.3     | comparison                         |
| `>`        | C1.3     | comparison; lex before `>=`        |
| `>=`       | C1.3     | comparison                         |
| `&&`       | C1.3     | logical and                        |
| `\|\|`     | C1.3     | logical or                         |
| `!`        | C1.3     | logical not (unary); lex after `!=` |

Twelve new tokens total. logos's longest-match guarantee handles
the lex-before-shorter cases (`!=` before `!`, `<=` before `<`,
`>=` before `>`) — list the longer pattern first in the enum.

Primitive type names (`i32`, `i64`, `bool` per D3) are NOT lexer
keywords — they remain `Ident` and get recognized in type position
by the parser / type checker.

### D10. Hard break at C1.2 + C1.3: fixture annotation rewrite.

ADR 0011 D8 already flagged this as "the second hard break" after
ADR 0010 D7's C0.5 `fn main() { ... }` rewrap. At C1.2:

  - All 22 pass-test fixtures gain return-type annotations on
    every `fn`: `fn main() { ... }` becomes
    `fn main() -> i64 { ... }`; `fn double(x) { x * 2 }` becomes
    `fn double(x: i64) -> i64 { x * 2 }`.
  - Parameters all gain `: i64` annotations.
  - `let`-bindings remain as-is (inference path per D2; explicit
    `let x: i64 = ...;` is also valid).
  - The lex_invalid_char UI fixture stays as-is (it tests the
    lexer; it has no fn definitions to annotate).
  - The parse_unbalanced_paren UI fixture (currently
    `fn main() { (1 + 2 }`) becomes
    `fn main() -> i64 { (1 + 2 }` to keep its program shape valid
    around the embedded parse error.

At C1.3:

  - The 7 if-using fixtures get their conditions rewritten from
    `if x { ... }` to `if x != 0 { ... }` (or equivalent).

Both passes are mechanical. ADR 0011 D8 already accepted the cost.

### D11. `fn main()` return type.

Per D1, `fn main` requires an `-> i64` annotation like every other
fn. Codegen continues to truncate to `i32` for the C ABI per ADR
0010 D11. The source-level type is `i64`; the LLVM-level type is
the i32 ABI shape. No source-level surface for the i32-truncation
— it's a codegen concern.

When `?T` lands at C1.5, `fn main() -> ?i64` might be considered
as the "graceful failure" signature; that's a C1.5+ revisit.

## Reasoning

The decisions cluster around four themes:

**Pin down what C1.2 + C1.3 need; nothing else.** D2's optional
let annotation, D6's restriction to six comparison ops, D7's
restriction to three logical ops, D8's continuing requirement of
mandatory `else`, D11's "`fn main` returns `i64`" all defer
features that don't earn their keep at C1.3's go-criterion (a
program with comparisons and conditional branching that compiles
and runs). The pattern matches ADR 0010's "defer everything else"
and ADR 0009 D1's "decide later with more information."

**Match Rust where it costs nothing.** D1 (`fn name(p: T) -> T`),
D2 (`let x: T = expr`), D3 (primitive names as identifiers, not
keywords), D5 (`true`/`false` as lexer keywords), D6 (six C-style
comparison operators), D7 (`&& || !` with short-circuit, standard
precedence), D8 (`if` condition is `bool`) all match Rust. The
1.0 target reads Rust-style; diverging at C1 just creates
migration cost when nothing is being gained.

**Surface-future-compatible where it costs nothing.** D3's
"primitives are identifiers, not keywords" means C1.4's `Foo`
(struct names) and C1.7's generics (`Vec<T>`) extend the
type-position grammar without lexer churn. D1's mandatory return
type means C1.2 doesn't have to grow inference for return types
later. D6's non-associative comparisons keep the door open for
chained comparisons (Python-style) if a future Sentinel version
wants them, by simply lifting the restriction.

**Take the small migration cost when it buys correctness.** D10's
22-fixture annotation rewrite + 7-fixture condition rewrite is
boring but mechanical. ADR 0011 D8 already accepted this. The
alternative (gradual annotation, inferred return types) makes
C1.2's type checker substantially more complex for no payoff once
C1.3 retires the C-style truthy anyway.

## Consequences

### Positive

- C1.2's parser extension is bounded: `fn_def` and `let_stmt`
  grow optional / required `: type` and `-> type` clauses; one new
  token (`:`). The hand-written recursive descent absorbs this as
  ~30 LOC.

- C1.3's lexer extension is ~10 tokens, all logos one-liners. The
  parser extension is the precedence ladder for comparisons +
  logicals — ~50 LOC. The type-checker's bool/i64 distinction
  flows from the existing type-equality machinery.

- C1's diagnostics gain a real `TypeError::Mismatch { expected,
  got, span }` once D3's type names + D8's bool-typed conditions
  are in. This is the first language-level type error in Sentinel,
  and the one that closes ADR 0010 D9's "deliberate temporary
  ugliness" loop.

- D3's "primitives as identifiers, not keywords" means structs
  (C1.4), nullable types (C1.5), and generics (C1.7) extend the
  type-position grammar without further lexer changes. Pattern
  established at C1.2 carries forward.

### Negative

- D10's two-pass fixture rewrite (C1.2 annotations, C1.3 if-
  condition fixes) creates the second hard break in pass-test
  history. ADR 0011 D8 already noted this; documenting it here
  for completeness.

- D1's mandatory return-type annotation is more verbose than
  Rust's optional `-> ()`. Sentinel will likely add `-> ()` /
  Unit at C1.4 (with structs); until then `fn main() -> i64`
  is the only shape, which is slightly noisy.

- D8's retirement of C-style truthy invalidates every C0 program
  using `if x` with non-bool `x`. Migration is mechanical (`if x
  != 0`) but real. Mitigated by the fact that all such programs
  are test fixtures.

- D6's non-associative comparison rule diverges from Python.
  Programmers coming from Python may write `0 < x < 10` and get
  a confusing diagnostic. The error wording should mention the
  rejection's reason explicitly.

### Neutral

- D11's `fn main() -> i64` decision punts the "what should `main`
  return at 1.0?" question to C1.5+ when `?T` and Unit are
  available. The C0 / C1.2 convention (i64 truncated to i32)
  stays as-is until then.

- The choice of `!` over `not` for unary negation is purely
  C-family convention. Sentinel-Mini doesn't have a unary not.
  No principled argument either way; `!` wins on
  familiarity-and-brevity.

## Alternatives considered

- **Optional return-type annotation, inferred from body.**
  Rejected per ADR 0011 D2's "explicit at fn boundaries"
  commitment. Inferred-from-body works in small examples and
  becomes a maintenance disaster in large ones (changing a
  function's body silently changes its signature). Rust learned
  this lesson; Sentinel inherits it.

- **Type names as lexer keywords (`reserved i64 { ... }`).**
  Rejected per D3 reasoning: the disjoint-context property
  means there's no ambiguity, so reservation buys nothing and
  costs `let i64 = 5;` as an ergonomic surprise. The Rust
  precedent confirms this is the right call.

- **Chained comparisons (`0 < x < 10`).** Rejected per D6
  reasoning: parser complexity + Python-like surprise risk.
  Future-compatible per D6's "lifting the restriction is
  monotonic" note.

- **`and` / `or` / `not` keywords instead of `&&` / `||` / `!`.**
  Rejected: Python-style logical keywords are more readable but
  diverge from Rust, C, the entire C-family, AND from
  Sentinel-Mini's existing `==`/`<`/`>` operator style. The
  consistency cost outweighs the readability win.

- **`==>` / `<==>` / etc. for logical implications.** Rejected:
  not in scope for C1. Imperative languages don't need
  implication operators at the language level; if Sentinel ever
  wants them (e.g., for refinement types or contract programming),
  they land in a separate ADR.

- **Defer `bool` to C2 with regions, type comparisons until
  then.** Rejected: would push the "compiler rejects obvious
  memory-safety violations" criterion (HANDOVER §6.2's C1 exit
  bar) past C1, breaking the phase narrative.

- **Add `_` (underscore) as a wildcard in let-bindings (`let _
  = ignored_expr;`) at C1.2.** Rejected: minor convenience, not
  needed for the type-system work. Adds at C1.4 or whenever it
  becomes a real ergonomic gap.

- **Tuple types `(T, T)` and tuple values `(x, y)` at C1.2.**
  Rejected: structs subsume tuples (C1.4) and tuples-without-
  structs is an awkward intermediate. If they appear, they appear
  alongside structs.

## Revisit

This ADR is **PROPOSED** until C1.2 and C1.3 land the syntax
decisions herein. Per-D revisit triggers:

- **D1** (mandatory return type): revisit at C1.4 when Unit /
  `()` arrives. Likely allows `fn name(p: T) { ... }` desugaring
  to `fn name(p: T) -> () { ... }`. May also revisit at C1.5
  when `?T` lands — `fn main() -> ?i64` for graceful failure
  shape.

- **D3** (primitives as identifiers): revisit at C1.4 (struct
  names join the type-position grammar) and C1.7 (generics add
  type-argument syntax). No expected lexer change at either
  point.

- **D6** (non-associative comparisons): revisit only if a real
  ergonomic complaint arises. Forward-compatible with chained
  comparisons (loosening the restriction is monotonic).

- **D7** (`!`, `&&`, `||` precedence): revisit at C4 when
  classes / methods land. Method-call syntax (`x.foo()`) might
  want tighter precedence than `!`, which currently sits at
  unary level; the existing decision is consistent with Rust
  and should hold.

- **D8** (`if` condition is `bool`): revisit at C1.5 when `?T`
  lands — `if let Some(x) = opt { ... }` is the Rust-style
  pattern, which is a parser extension not a precedence change.

- **D10** (fixture migration): once landed, no revisit. Pass-test
  fixtures stabilize after C1.3.

- **D11** (`fn main() -> i64`): revisit at C1.5 (Unit, `?T`) or
  C3 (effects — `fn main() uses io, ...`). The codegen-time
  i64-to-i32 truncation may need a more principled story when
  effects describe what `main` is allowed to do.

## Appendix: C1.2 + C1.3 target programs

For reference, the canonical C1.2 phase-go program (annotation
syntax + i64 type-check) might be:

    fn double(x: i64) -> i64 {
        x * 2
    }

    fn pick(cond: i64, a: i64, b: i64) -> i64 {
        if cond { a } else { b }
    }

    fn main() -> i64 {
        let x: i64 = 5;
        let y = pick(x, double(x), 0);
        print(y)
    }

This is the C0.5 go/no-go program with C1.2 annotations added.
Expected stdout: `10\n`. Same behavior as C0.5; the type
checker validates that every annotation matches its expression's
type, that the call argument counts and types align with the
callees' declared signatures, and that `main`'s body types to
`i64`.

The C1.3 phase-go program adds `bool`, comparisons, and a
proper boolean condition:

    fn double(x: i64) -> i64 {
        x * 2
    }

    fn is_positive(x: i64) -> bool {
        x > 0
    }

    fn pick(cond: bool, a: i64, b: i64) -> i64 {
        if cond { a } else { b }
    }

    fn main() -> i64 {
        let x: i64 = 5;
        let y = pick(is_positive(x), double(x), 0);
        print(y)
    }

Expected stdout: `10\n`. The C1.3 program type-checks the bool
flow end-to-end: `x > 0` returns `bool`, `is_positive(x)` returns
`bool`, `pick(bool, ...)` accepts a `bool` first argument,
`if cond` requires `cond: bool`.

`tests/pass/c12_go_no_go.sentinel` and
`tests/pass/c13_go_no_go.sentinel` (paired with their `.stdout`
fixtures) will be the concrete acceptance fixtures for C1.2 and
C1.3 respectively.
