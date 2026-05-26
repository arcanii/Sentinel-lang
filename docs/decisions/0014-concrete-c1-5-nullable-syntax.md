# ADR 0014: Concrete C1.5 surface syntax — nullable types `?T`, `null` literal, null-check semantics

Status: ACCEPTED-WITH-AMENDMENTS — D1, D2, D3, D5, D6, D7, D8, D9,
D11 all fully exercised at C1.5.1 + C1.5.2-6 (commits dff8642 +
1d0adae). Two amendments uncovered during implementation:

  - **D4 representation amendment**: the ADR proposed
    `Type::Nullable(Box<Type>)` but the implementation went with a
    flat `NullableInner` subset enum instead (see Reasoning
    addendum below). The subset enum keeps `Type` `Copy` and makes
    `?(?T)` structurally unrepresentable. Strictly an
    implementation choice; the surface semantics (no nested
    nullables per D6) match the ADR.
  - **D10 deferral**: the recursive-struct cycle-check
    relaxation is documented but NOT exercised at C1.5. The
    `?T = { i1, T }` flat-inline LLVM representation makes
    recursive structs (e.g. `struct Node { next: ?Node }`) have
    infinite LLVM size. Proper indirection requires heap
    allocation (malloc/free), which is C1.6+ territory. The
    cycle detector at C1.5 walks nullable struct edges
    conservatively (still rejects recursive nullables). D10's
    spirit is preserved as the future target; D10's text said
    "relaxes" but the runtime behavior says "deferred."

Date: 2026-05-26
Last touched: 2026-05-26 (C1.5 landed; status flipped to
ACCEPTED-WITH-AMENDMENTS with the D4 + D10 details documented)
Related: 0011 (Phase C1 kickoff; D3 lists `?T` as part of the C1
type system, D6 schedules C1.5 after C1.4), 0013 (concrete C1.4
surface — D7's recursive-struct rejection lifts here when nullable
indirection becomes available), 0012 (concrete C1 surface — D3
"primitives are identifiers, not keywords" precedent informs the
`null` keyword decision: `null` is a literal, not a type name, so
it joins `true` / `false` / `struct` as a true reserved keyword)

## Context

C1.4 landed the first compound type (structs) but explicitly
rejected recursive structs (ADR 0013 D7) because Sentinel at C1.4
has no indirection primitive — `struct Node { next: Node }` has
infinite size. C1.5 (per ADR 0011 D6) introduces nullable types
`?T`, which provide the smallest piece of indirection in the
language: a `?T` value is either `null` or holds a `T`, and its
runtime representation includes a discriminator (`valid: bool`)
alongside the payload. With `?T` available, `struct Node { value:
i64, next: ?Node }` becomes representable: every `Node` has a
`next` that's either `null` (the list ends) or a `Node` (the list
continues), and the structure has a finite max-depth because each
level either terminates or recurses through a fresh allocation.

The 1.0 target surface (SENTINEL_DESIGN2.md §4.3) reads:

    fn find(list: List, key: i64) -> ?i64 {
        if list.has(key) { list.get(key) } else { null }
    }

C1.5 ships this minimum: postfix `?T` type syntax, `null` literal,
implicit `T → ?T` widening, equality against `null`, two builtins
(`unwrap_or` and `is_some`), and the recursive-struct cycle-check
relaxation. What this ADR explicitly does NOT commit to at C1.5:
pattern matching (`if let Some(x) = opt { … }`), Rust-style `?`
propagation operator, optional chaining (`x?.field`), force-unwrap
(`x!`), the `??` default operator. All of those land in later
ADRs without conflicting with the C1.5 grammar — they're parser
extensions, not retroactive changes.

The C1.4 lexer's current token set is:

  - **Keywords**: `let`, `fn`, `if`, `else`, `true`, `false`,
    `struct`
  - **Punctuation**: `+ - * / = ( ) { } , ; : . ->`
    `== != < <= > >= && || !`
  - **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`
  - **Integer literals**: `[0-9]+`
  - **Skipped**: `[ \t\r\n]+`, `//[^\n]*`

C1.5 extends this with the additions listed in D8 below.

## Decision

Eleven D-numbered sub-decisions covering syntax (D1-D3, D6), the
type-level rules (D4-D5, D7), lexer additions (D8), builtins (D9),
the cycle-detector relaxation (D10), and what stays explicitly
out of scope (D11).

### D1. Nullable type syntax: postfix `?T`.

A `?` token immediately preceding a type expression denotes its
nullable variant. Grammar extension at the type-expr level:

    type         = '?'? base_type
    base_type    = Ident          // i64 / i32 / bool / struct name

Nesting is forbidden — `??T` is a parse error (per D6 below).
Sentinel doesn't have a nested-Optional use case at C1.5; future
phases may revisit. Whitespace is allowed between `?` and the base
type (`? i64` parses the same as `?i64`), matching the rest of the
grammar's whitespace tolerance.

Postfix-on-the-type wins over prefix-on-the-name (Rust-style
`Option<T>`) because:

  - It's shorter and reads as "T-or-null" rather than "an
    Option-of-T," which matches the value-level semantics
    (`?i64` is "either an i64 or null").
  - It avoids importing a generic-types surface decision into
    C1.5 — generics arrive at C1.7. `Option<T>` would force a
    generics-y feel at the surface level before generics work.
  - It composes naturally with future C2+ extensions: `?&T`
    (nullable reference) reads correctly; `?[T]` (nullable
    array) does too.

### D2. The `null` literal.

A new lexer keyword. Its type at parse time is unspecified; the
type checker uses bidirectional checking against the surrounding
context to infer it:

    let x: ?i64 = null;       // null has type ?i64 (annotation)
    let y = find_or(null, 0); // null has type matching the param
    return null;              // null has type matching the return

A bare `let x = null;` (no annotation, no inference target)
surfaces as `TypeError::AmbiguousNull { span }` — Sentinel's
monomorphic type checker has no way to pick `T`. This is a new
error variant added at C1.5.

The `null` value's runtime representation matches the
discriminator-half of a `?T` value: a 0 in the `valid` field; the
payload is undefined.

### D3. Implicit `T → ?T` widening at expression position.

When the type checker's expected type at an expression position is
`?T`, an expression that produces `T` is implicitly widened to
`?T`:

    fn maybe(x: bool) -> ?i64 {
        if x { 42 } else { null }    // 42 widens i64 → ?i64
    }

The widening is automatic and does not require an explicit
`some(x)` constructor. This matches the broader language's
boundary-typing approach (ADR 0011 D2) — at fn boundaries, the
declared return type drives the inference; the body's expressions
flow naturally into the declared shape.

Coercions outside expression position (e.g., `let x: ?i64 = 5`)
follow the same rule because the let annotation provides the
expected type.

The widening is one-way: `T → ?T` is implicit, `?T → T` is NOT
(requires an explicit unwrap per D9). Without this asymmetry,
"forgot to null-check" bugs would slip past the type checker.

### D4. Type-level representation: `Type::Nullable(NullableInner)` (amended).

**Amendment from PROPOSED → ACCEPTED-WITH-AMENDMENTS**: the
original D4 proposed `Type::Nullable(Box<Type>)`. The C1.5
implementation chose a flat subset enum instead:

    enum Type {
        I64,
        I32,
        Bool,
        Struct(StructId),
        Nullable(NullableInner),
    }

    enum NullableInner {
        I64,
        I32,
        Bool,
        Struct(StructId),
    }

Both Type and NullableInner are `Copy` + `Clone` + `Hash` + `Eq`.

Why the amendment:

  - **Keeps `Type` `Copy`.** The Box<Type> approach forces Type
    to give up Copy because Rust enums with Box fields can't
    derive Copy. ~20-30 use sites would need `.clone()` /
    `&Type` updates in sentinel-types + sentinel-codegen.
    Tedious and noisy.
  - **`?(?T)` becomes structurally unrepresentable.** NullableInner
    doesn't have a Nullable variant, so there's no way to
    construct a nested-nullable Type value. The D6 no-nesting
    rule is enforced by the type system itself, not just at
    `resolve_type_expr`.
  - **Cost: a duplicate constructor list.** Every future Type
    variant needs a corresponding NullableInner entry. Each
    addition is one extra line; small.

The Box<Type> approach stays available if future generics work
surfaces a real `??T` use case. For C1.5 the flat subset is the
ergonomic win.

The non-nesting rule from D6 is still enforced redundantly at
the parser (`??T` is a `ParseError::NestedNullable`) and at
`resolve_type_expr` (defense in depth — if a future code path
ever constructs nested TypeExprs internally, the resolver
catches it).

### D5. Bidirectional checking for `null` literals.

The check pass operates in two modes:

  - **Synthesis** (default): the expression's type is computed
    bottom-up from its parts. `42` synthesizes `I64`; `x + y`
    synthesizes `l.ty` after checking equality with `r.ty`.
  - **Checking** (when expected type is known): the expression
    is checked against an expected type. A `null` literal in
    checking mode types as the expected type (provided it's
    `?T`). A `T`-typed expression in checking-`?T` mode widens.

C1.5's bidirectional checking is limited:

  - Let annotations push the annotation down to the RHS.
  - Fn return-type pushes down to the body.
  - Fn call-arg types push down to each argument.

These are sufficient for C1.5's productive use cases. Full
bidirectional inference (e.g., `if cond { null } else { null }`
without an outer annotation) is out of scope.

### D6. No nested nullables (`??T`).

`?T` where `T` itself is `?U` (i.e., `??U`) is rejected at parse
time. The grammar's `type = '?'? base_type` permits only one
leading `?`; a second `?` would land as `type = '?' '?' base_type`
which doesn't parse.

Practically, nested nullables don't earn their keep at C1.5 — the
semantics of "an Optional that might be null" duplicate the inner
nullable. Future ADRs may revisit (e.g., for explicit
distinguishability when null has different meanings at different
levels), but the safe default is to reject.

The error surfaces as a `ParseError::NestedNullable { span }`
variant, not a `TypeError`, because the rejection is structural
at the grammar level.

### D7. Equality against `null`.

C1.5 extends the comparison rule to accept `null` as an operand
when the other operand is `?T`:

    x: ?i64 == null   // legal — produces bool
    null == x: ?i64   // legal — symmetric
    x: ?i64 != null   // legal
    x: i64 == null    // type error — null requires ?T

Only `==` and `!=` are extended; `<` / `<=` / `>` / `>=` on
nullables remain type errors (D11 list of out-of-scope items).

The codegen for `x == null` is a comparison of the discriminator
field against `false`. The codegen for `x != null` is a comparison
against `true`.

### D8. Lexer additions at C1.5.

Two new tokens, listed in the order they're added to the `logos`
enum:

| Token  | Notes                                            |
| ------ | ------------------------------------------------ |
| `null` | reserved keyword; bare literal in expressions    |
| `?`    | type-position prefix marker (D1) — no overload   |

Two new tokens total. The `?` token is intentionally NOT
overloaded as an expression-position operator at C1.5 — Rust's
`?` propagation, Swift's optional chaining, and similar uses are
all out of scope (D11). This leaves the token available for those
future uses without ambiguity at C1.5.

Per ADR 0012 D3, type names remain `Ident`-class tokens. `null`
joins `let` / `fn` / `if` / `else` / `true` / `false` / `struct`
as a true keyword.

### D9. Builtins: `unwrap_or` and `is_some`.

Two new runtime builtins, registered the same way `print` is per
ADR 0011 D11 / C1.1.1's resolve pass:

  - `unwrap_or(x: ?T, default: T) -> T` — returns the wrapped
    value if `x` is non-null, otherwise `default`.
  - `is_some(x: ?T) -> bool` — returns `true` if `x` is non-null,
    `false` if `x` is null.

These are special-cased in the resolve / types layers because
they're generic over `T` and Sentinel doesn't have generics yet
(C1.7). The implementation registers them with a sentinel `T` that
the type checker unifies against the argument's `?T` type at each
call site. This is a pragmatic workaround for the
"want-generics-but-don't-have-them" problem at C1.5 — the
machinery is bounded (just two functions) and disappears at C1.7
when real generics arrive.

Together, `unwrap_or` + `is_some` give users productive access to
nullable values:

  - "I have a default" → `unwrap_or(x, 0)`
  - "Check then use" → `if is_some(x) { unwrap_or(x, 0) /* known
    non-null */ } else { … }` — though flow-typing the unwrap is
    out of scope at C1.5, so `unwrap_or` is still required inside
    the if-then.

`unwrap` (panic-if-null) is intentionally NOT added — the safer
`unwrap_or` plus the `is_some` check covers the productive cases
without introducing panic-on-null behavior. A future ADR may add
it (or a force-unwrap operator `x!`) once Sentinel has a clear
panic semantics for runtime errors.

### D10. Recursive-struct cycle-check relaxation (DEFERRED to C1.6+).

**Amendment from PROPOSED → ACCEPTED-WITH-AMENDMENTS**: D10's
spirit is preserved but the runtime implementation is deferred.

The ADR proposed relaxing the cycle detector: cycles via nullable
edges (`?Node`) would be accepted because the indirection breaks
the cycle at runtime. C1.5 implementation discovered the codegen
representation `?T = { i1, T }` (flat-inline) means a struct
field of type `?Node` literally contains a Node value, not a
pointer-to-Node. So `struct Node { next: ?Node }` has infinite
LLVM size and triggers stack overflow in LLVM's struct-size
computation.

Proper indirection requires heap allocation (malloc/free), which
is C1.6+ territory. C1.5's cycle detector therefore walks
nullable struct edges as if they were direct edges — the
conservative stance.

What this means at C1.5:

    struct Node { value: i64, next: ?Node }     // STILL ERROR
    struct Tree { l: ?Tree, r: ?Tree }          // STILL ERROR
    struct Bad { x: Bad }                       // STILL ERROR (unchanged from C1.4)
    struct Pair { first: ?i64, second: i64 }    // OK — non-recursive

Non-recursive nullable struct fields work fine. The recursive
case waits for C1.6+ when malloc / smart-pointer indirection
arrives — at that point the cycle detector relaxes and these
declarations type-check.

The deferral is captured in the `detect_struct_cycle` docstring
in sentinel-types and in STATE.md decision 72.

### D11. Out of scope at C1.5.

The following nullable-related features are explicitly deferred
to later ADRs. They're listed here so the C1.5 scope-cut is
unambiguous:

  - **Pattern matching** (`if let Some(x) = opt { … }`,
    `match opt { Some(x) => …, None => … }`) — requires the full
    pattern grammar; defer to C1.6 or C2.
  - **Force-unwrap operator** (`x!`) — would conflict with the
    prefix `!` for logical not without careful position-based
    disambiguation; defer to a later ADR after pattern matching
    lands.
  - **Optional chaining** (`x?.field`) — requires the `?` token
    in expression position; the C1.5 scope keeps `?` type-only.
  - **Null-coalescing default operator** (`x ?? default`) — a new
    `??` lexer token. The `unwrap_or` builtin covers the same
    semantics for C1.5; the operator form is ergonomic sugar.
  - **`?` propagation operator** (Rust-style `let x = maybe()?;`
    early-returns null from the enclosing fn) — useful but
    requires the enclosing fn's return type to also be `?T`,
    which is a flow-analysis pass C1.5 doesn't have yet.
  - **Flow typing** inside a null-checked branch (so that
    `if is_some(x) { x } else { … }` could let `x` be `T` instead
    of `?T` in the then-branch). Useful but adds non-trivial
    machinery; defer to C1.6+.
  - **Nested nullables** (`??T`) — D6 rejects.
  - **Nullable struct fields beyond same-struct recursion** —
    fields like `value: ?i64`, `next: ?Other` work via the
    standard `?T` rules; the D10 relaxation specifically covers
    SAME-struct recursion. Nullable fields pointing to DIFFERENT
    structs are valid at C1.5 without any cycle-check change.

## Reasoning

The decisions cluster around four themes:

**Pin down the minimum-viable surface; nothing else.** D5's
limited bidirectional checking, D7's restriction to `==` / `!=`,
D9's two builtins (no `unwrap`, no `expect`, no operator-form),
D10's surgical cycle-detector relaxation, and D11's defer list
all keep C1.5 small. The C1.5 win is "the type universe gains
nullability and the recursive-struct restriction lifts" — anything
beyond that is a future ADR.

**Match Rust where it doesn't conflict with the existing C1
shape.** D1 (`?T` postfix), D2 (`null` keyword), D7 (`== null`
syntax) all have Rust-equivalent semantics even though Rust
prefers `Option<T>` / `None`. The terse postfix `?` reads as a
"yes-or-null" marker that the rest of the language can grow
around. Sentinel's surface stays lighter than Rust's at the
nullable level while preserving the semantics.

**Future-compatible representation choices.** D4's
`Box<Type>` representation, D8's deliberate non-overload of `?` in
expression position, D9's pragma-marked generic builtins, D10's
nullable-edge tracking in the cycle detector all leave room for
later additions without retroactive changes. ADR 0011's broader
"infrastructure-first ordering" principle applies here at the
type-level.

**Take the small upfront cost.** D4's `Type` loses `Copy` (Box
fields don't allow it) — that's a one-time mechanical refactor
costing ~20-30 `.clone()` additions across the codebase. The
alternative (a flat NonNullable enum subset) would force
maintaining two parallel enums in lockstep for every future Type
variant. Take the refactor cost now.

## Consequences

### Positive

- C1.5's parser extension is bounded: one new optional-prefix
  rule in `parse_type`, one new atom (`null` keyword in
  `parse_atom`). ~10-20 LOC.

- C1.5's resolve extension is minimal: `null` literal needs no
  name resolution; the `?T` syntax is type-position, which
  resolve already passes through unchanged.

- C1.5's type-check extension is the substantive layer: the
  bidirectional-checking infrastructure (D5), the implicit
  widening (D3), the new TypeError variants (D2's
  AmbiguousNull, plus reuse of Mismatch for type errors), the
  cycle-detector relaxation (D10). ~80-120 LOC.

- C1.5's codegen extension: `?T` lowers to LLVM struct
  `{ i1, T }` where the i1 is the valid bit and T is the
  payload. NullLit lowers to `{ false, undef }`. Equality vs
  `null` lowers to a compare on the valid bit. `unwrap_or` and
  `is_some` builtins lower as inline LLVM via the resolve-
  registered FnId. ~60-80 LOC.

- C1.4's recursive-struct restriction lifts. Linked lists,
  trees, and other self-referential data structures become
  representable in Sentinel programs.

- The `?` token reservation at the type-position level keeps
  the door open for the rich ergonomic surface in later ADRs
  (`?.`, `?`, `!`, `??`) without ambiguity.

### Negative

- D4's `Type` loses `Copy`. ~20-30 mechanical `.clone()` /
  `&Type` changes across the codebase. Tedious but routine.

- D9's "generic builtins" (`unwrap_or`, `is_some`) are a
  pragmatic workaround for the absence of real generics. The
  resolve / types layers grow special-cases for these two
  functions; the special cases evaporate at C1.7. The C1.5 cost
  is small but the design debt is visible.

- D5's limited bidirectional checking means `let x = null;`
  (bare, no annotation) is an error. Users who write that pattern
  will see `TypeError::AmbiguousNull` and need to add an
  annotation. Documented in the error's `help` text.

- D11's deferred features create a real ergonomic gap. `?.` /
  `?` propagation / pattern matching all materially improve
  nullable ergonomics; C1.5 ships without them. Users who write
  `if is_some(x) { unwrap_or(x, 0) } else { 0 }` will find it
  verbose. Mitigation: the gap is explicit in this ADR and the
  follow-up ADRs are sub-phase-sized, not "needs a rewrite."

### Neutral

- D6's no-nested-nullables rule may surprise users coming from
  languages that allow `Option<Option<T>>`. Documented in the
  ParseError variant's `help` text. Future ADRs may revisit if
  generics surface a real use case.

- D8's reservation of `?` for type-position-only at C1.5 means
  the `?` token has just one role. Multi-role tokens
  (e.g., `<` for less-than AND for generics) often confuse the
  parser; staying single-role at C1.5 is the simpler path.

## Alternatives considered

- **`Option<T>` Rust-style instead of `?T`.** Rejected per D1
  reasoning: requires generics surface at C1.5, doesn't match
  Sentinel's 1.0 target syntax (SENTINEL_DESIGN2.md §4.3 uses
  `?T`), and is heavier on the eyes for the same semantics.

- **`null` as a polymorphic `?<T>` value with explicit type
  argument.** E.g., `null<i64>`. Rejected: requires generics
  surface at C1.5 — same reasoning as `Option<T>`. Bidirectional
  checking (D5) gives the same expressivity without the surface
  cost.

- **`some(x)` constructor for explicit widening, no implicit T
  → ?T coercion.** Rejected per D3 reasoning: explicit `some(x)`
  at every widening point is noisy; the implicit-widening rule
  matches Rust's "T into Option<T>" pattern in fn return position
  and is easy to read. The asymmetry (implicit T → ?T, explicit
  unwrap for ?T → T) preserves safety.

- **`unwrap` (panic-if-null) builtin at C1.5.** Rejected per D9
  reasoning: Sentinel doesn't have well-defined panic semantics
  yet; introducing a panic-prone builtin now would force a
  half-baked panic story. `unwrap_or` covers the safe cases.

- **Pattern matching (`if let Some(x) = opt { … }`) at C1.5.**
  Rejected per D11 reasoning: requires the full pattern grammar
  (binding patterns, refutable patterns, exhaustiveness checking
  if `match`). That's a separate language feature with its own
  ADR-sized design space. Deferring keeps C1.5 small.

- **Force-unwrap operator `x!` at C1.5.** Rejected: conflicts
  with prefix `!` for logical not without careful
  postfix-disambiguation in parse_postfix (similar to C1.4's
  `.field` extension but adding another postfix complicates the
  loop). Defer to a later ADR.

- **Flow typing inside `if is_some(x) { … }` so that `x` is `T`
  in the then-branch.** Rejected at C1.5 because it requires
  introducing a per-branch type environment; the existing
  HashMap<VarId, Type> doesn't have a per-block scope notion.
  Defer to C1.6+ when local scoping is needed for other reasons.

## Revisit

This ADR is **PROPOSED** until C1.5 lands the syntax decisions
herein. Per-D revisit triggers:

- **D1** (postfix `?T`): no expected revisit. Future generic
  types `Vec<?T>` compose naturally.

- **D3** (implicit T → ?T widening): revisit at C4 if effects
  surface a "can't widen without effect cost" case. Unlikely
  but possible.

- **D4** (Type::Nullable(Box<Type>)): revisit if a flat-enum
  arena-based representation becomes profitable for performance
  (C1.7 generics may push this). The existing Box representation
  is correct, just potentially slower than a flat one.

- **D6** (no nested nullables): revisit at C1.7 (generics) if
  legitimate `Option<Option<T>>` use cases arise. Likely fine
  as-is.

- **D7** (`== null` only): revisit if a real need for
  `< null` or similar ordering emerges. Unlikely.

- **D9** (special-case builtins): retires at C1.7 when generics
  let `unwrap_or` / `is_some` become regular generic fns.

- **D10** (cycle-check nullable relaxation): revisit if C2's
  references introduce a third indirection type — the detector
  may need a unified "indirection kind" tracker rather than just
  a "nullable-crossed" bit.

- **D11** (out-of-scope list): each item gets its own future
  ADR. The list is the to-do queue for ergonomic nullable
  features through Sentinel 1.0.

## Appendix: C1.5 phase-go programs

For reference, two canonical C1.5 acceptance programs.

### Phase-go 1: `?T` value flow

    fn find_or(x: ?i64, default: i64) -> i64 {
        unwrap_or(x, default)
    }

    fn main() -> i64 {
        let some: ?i64 = 42;
        let none: ?i64 = null;
        print(find_or(some, 0) + find_or(none, 100))
    }

Expected: stdout `142\n`, exit 0. Exercises:

  - postfix `?T` type syntax (D1) — `?i64` in two annotations
  - `null` literal (D2) — assigned to `none`
  - implicit `T → ?T` widening (D3) — `42` widens to `?i64`
  - `unwrap_or` builtin (D9)
  - normal i64 arithmetic + print

### Phase-go 2: Recursive struct via nullable

    struct Node { value: i64, next: ?Node }

    fn make_pair(a: i64, b: i64) -> Node {
        Node { value: a, next: Node { value: b, next: null } }
    }

    fn main() -> i64 {
        let pair = make_pair(3, 4);
        let head_val = pair.value;
        let tail_val = unwrap_or(is_some_of_node(pair.next), 0);
        print(head_val + tail_val)
    }

Expected: stdout `7\n`, exit 0. Exercises:

  - recursive struct via `?Node` (D10's cycle-check relaxation)
  - struct literal with nullable field (D3 widening for `null`)
  - field access on a struct that has nullable fields
  - the cycle detector accepts the Node→?Node cycle

The second program is intentionally a bit awkward — accessing
`pair.next.value` directly is what users want, but at C1.5 we
don't have flow typing or pattern matching. The `unwrap_or` form
plus `is_some` works but reads heavy. This is the documented
ergonomic gap that future ADRs close.

`tests/pass/c15_go_no_go_value.sentinel` and
`tests/pass/c15_go_no_go_recursive.sentinel` (paired with their
test functions in pass.rs) will be the concrete acceptance
fixtures for C1.5.
