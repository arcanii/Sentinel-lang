# ADR 0032: Phase D.1 — sum types (`enum`) + pattern matching (`match`)

Status: PROPOSED — the first Phase D sub-phase ADR under ADR 0031 (Phase D
kickoff) D4/D7. Sum types + `match` are the foundational self-hosting
prerequisite (an AST, a token stream, and `Result` are all sum types) and
a major general-purpose language feature in their own right. Flips to
ACCEPTED(-WITH-AMENDMENTS) as the sub-phase lands.

Date: 2026-05-30
Related:
  - **0031** (Phase D kickoff): D4 names sum types + `match` as the first
    prerequisite; D7 makes it D.1.
  - **0015** (arrays / nullable): `?Struct` lowers to `{ i1 valid, ptr
    payload }` (heap), which broke recursive struct cycles. Enums reuse
    this **heap-boxed payload** idea (D4) — *necessary* for recursive
    enums (an AST node references itself).
  - **0016** (generics / monomorphisation): generic enums (`Option<T>`)
    reuse the witness-table mono machinery — a **fast-follow**, not the
    first slice (D9).
  - **0017** (RAII drop): an enum owns its heap payload; scope-exit drop
    frees it + recurses (D6), reusing the `?Struct` drop path.
  - **0026 D5** (constant-time): a `match` on a `secret` tag is a
    secret-dependent branch — a D5 sink (D7). No `secret enum` at MVP, so
    this is a guard for the future.
  - **0011 / 0022** (interner-table-with-`Copy`-`Type` invariant; `::`
    path syntax): `Type::Enum(EnumId)` is the next interner variant;
    construction `Enum::Variant(args)` reuses the `Class::init` `::` form.

**(1/N + 2/N) update (2026-05-30) — DELIVERED.** (1/N) shipped the lexer
(`enum`/`match` tokens). (2/N) shipped the **AST + parser**: `EnumDecl` +
`VariantDecl` on `Program.enums`; `ExprKind::Match` + `MatchArm` +
`Pattern` (qualified `Enum::Variant(binds)` + `_`); `parse_enum_decl` +
`parse_match_expr` + pattern parsing (the s-expr `Display` covers them).
Additive — resolve rejects `enum`s (`EnumDeclNotYet`) + `match`
(`MatchNotYet`) until (3/N); the blast radius stayed in ast+syntax+resolve
(downstream crates match the resolved/typed trees, which gain no `Match`).
+10 tests across 1/N+2/N (1242), four-check green. Next: **(3/N) resolve +
types** — `Type::Enum` interner variant, variant construction + `match`
type-check + **exhaustiveness**.

**(3/N) update (2026-05-30) — DELIVERED.** The **type layer** lands;
`enum` + `match` now type-check end to end (codegen rejects until 4/N).
- **resolve:** `EnumId` + `ResolvedEnumDecl`/`ResolvedVariantDecl` on
  `ResolvedProgram.enums` (a Pass-0 enum table with struct/class/trait
  namespace-collision checks → `RedefinedEnum`/`DuplicateVariant`);
  `Name::Variant(args)` / `Name::Variant()` construction **disambiguated**
  from `ImplName::method` / `Class::init` (when the name is an enum →
  `ResolvedExprKind::EnumConstruct`); `match` → `ResolvedExprKind::Match`
  with `ResolvedPattern` + per-arm binding `VarId` scoping (snapshot/restore
  of `vars`, like handler-arm params; `_` slots get a VarId but aren't in
  scope; duplicate binding names → `DuplicatePatternBinding`). The
  `EnumDeclNotYet`/`MatchNotYet` rejections are dropped. `ImplCtx` grew an
  `enum_table` (no signature churn).
- **types:** `Type::Enum(EnumId)` (the eleventh interner-style variant) +
  `EnumData`/`VariantData` + `TypedProgram.enums` + `enum_data` accessor;
  enum names resolve in type position (`resolve_type_expr` precedence
  struct→class→enum→primitive). `EnumConstruct` type-checks (variant lookup,
  payload arity + per-arg coercion → `Type::Enum`); `match` type-checks
  (scrutinee is an enum; arms typed + unified; pattern bindings typed from
  payloads; **exhaustiveness** — every variant or a `_`). Five new
  `TypeError`s: `UnknownVariant`, `VariantPayloadArityMismatch`,
  `MatchScrutineeNotEnum`, `NonExhaustiveMatch`, `MatchArmTypeMismatch`.
  **Directly-recursive enums type-check** (the AST enabler — heap-boxed
  payloads per D4 need no nullable indirection, unlike recursive structs).
- **downstream (forced by the new Resolved/Typed variants):** codegen's
  `llvm_basic_type` lowers `Type::Enum` to the `{ i32 tag, ptr payload }`
  abi-v1 layout (so enum-typed **signatures lower**) + `mangle_type` renders
  by name; construction/`match` *expression* codegen rejects cleanly with
  `CodegenError::EnumCodegenNotYet` (the pre-pass walkers recurse; drop is a
  no-op gated by `needs_drop=false`). MIR → `Opaque` carrying operands
  (taint-safe; no `secret enum` so a match tag is never secret). effect-check
  + borrow-check pass-through walks (enum is Move; match arms move-merge like
  `if`). So `enum`/`match` **type-check but codegen rejects until (4/N)**.
+27 tests (1265), four-check green. Next: **(4/N) codegen** — the
`{tag,ptr}` construction + `switch` lowering + recursive payload drop +
abi-v1 enum-layout entry + `c5d1_enum` pass fixture; ADR flips to ACCEPTED.

## Context

Self-hosting (ADR 0031) is blocked first and foremost on **sum types**:
the bootstrap compiler's core data — `TokenKind`, `ExprKind`, `Type`,
`Result`/error enums — are all Rust `enum`s, and Sentinel 1.0 has no way
to express them (only product types: structs, `?T` 2-case nullables,
arrays, classes). This sub-phase adds general tagged unions + exhaustive
pattern matching — the single highest-unblock feature on the Phase D
roadmap, and a clean type-system + codegen addition in the C1–C4 mould.

## Decision

### D1. Goal.

Add `enum` declarations (tagged unions with optional per-variant tuple
payloads) and `match` expressions (exhaustive pattern matching), end to
end: lexer → parser → resolve → types → codegen → drop, with an
`abi-v1` layout entry and a phase-go fixture.

### D2. Surface syntax.

```sentinel
enum Shape {
    Unit,                 // unit variant (no payload)
    Circle(i64),          // tuple payload, arity 1
    Rect(i64, i64),       // tuple payload, arity 2
}

fn area(s: Shape) -> i64 {
    match s {
        Shape::Unit => 0,
        Shape::Circle(r) => r * r * 3,
        Shape::Rect(w, h) => w * h,
    }
}
```

  - **Declaration:** `enum Name { V1, V2(T), V3(T1, T2), … }` — unit and
    tuple-payload variants. Named-field variants are out of scope (D10).
  - **Construction:** `Name::Variant` (unit) / `Name::Variant(args)`
    (payload) — reuses the `::` path token (as `Class::init`).
  - **Match:** `match scrutinee { Pat => expr, … }`, arms comma-separated.
    Patterns: `Name::Variant` (unit), `Name::Variant(b1, b2)` (binds
    payload positionally), and `_` (wildcard; lexes as an `Ident`).
    `match` is an expression — all arms share a common result type (like
    `if`).
  - **Exhaustiveness:** every variant must be covered, or a `_` arm
    present; otherwise a type error (`NonExhaustiveMatch`).

### D3. Type representation.

A new interner variant **`Type::Enum(EnumId)`** (the next after
Struct/Class — the established Copy-`Type` + interner-table pattern), with
`EnumData { name, variants: Vec<VariantData> }` and
`VariantData { name, payloads: Vec<Type> }`, plus a `VariantId` (index)
for resolve. Mirrors how `StructDecl` / `ClassData` are interned.

### D4. Codegen representation (the `abi-v1` enum layout).

`enum` lowers to **`{ i32 tag, ptr payload }`**: a 4-byte discriminant
(variant index, source order) + an opaque pointer to a heap-allocated
struct of that variant's payload fields (`null` for unit variants).

**Why heap-boxed, not an inline union:** a recursive enum — which an AST
*requires* (`enum Expr { Bin(Expr, Expr) }`) — is infinitely sized inline,
the same problem `?Struct` solved by going through a pointer (ADR 0015
D11). Uniform heap-boxing handles recursion, reuses the `sentinel_alloc` +
`?Struct`-style drop machinery, and keeps the MVP simple. Cost: every enum
value heap-allocates; an **inline-small-non-recursive-enum optimisation**
(`{ tag, [maxpayload x i8] }`) is a recorded post-MVP follow-on. `abi-v1`
§2/§3 gain the enum-layout entry + a stability test.

### D5. `match` lowering.

Load the scrutinee's `tag`; emit an LLVM **`switch`** on it into one block
per arm. Each arm block loads the payload pointer, GEPs/loads the bound
fields into fresh locals (the pattern bindings), lowers the arm body, and
branches to a merge block where the arm results reconcile (the `if`-merge
machinery). A `_` arm is the switch default. Exhaustiveness is a
type-check guarantee (D2), so a non-`_` exhaustive switch needs no default
(emit `unreachable`, as the bounds-check path does).

### D6. Drop.

An enum value owns its heap payload. Scope-exit drop (the `DropPlan`
path): if `payload != null`, recurse-drop the active variant's payload
fields (by tag), then `sentinel_free` the payload — structurally the
`?Struct` drop arm, dispatched on the tag. Moved/returned enums stay on
the existing escape path (and become scope-arena-routable later, like
arrays — out of scope here).

### D7. Constant-time (D5 pass) interaction.

A `match` is control flow: its scrutinee `tag` feeds a `switch`. The MIR
lowering emits the tag read as a `Load` and the arm dispatch as
`Branch`-equivalents, so the D5 pass already treats a **`secret`-tagged
scrutinee as a leak** (a secret reaching a branch) — consistent with `if`.
There is no `secret enum` at MVP (enums are public), so this is a
correctness guard for the future, not an active path.

### D8. Pipeline / touch points.

  - **lexer (D.1 1/N):** `enum` + `match` keyword tokens (`=>` / `::` /
    `_`-as-`Ident` already exist). Additive.
  - **AST + parser (2/N):** `EnumDecl`, `ExprKind::Match { scrutinee,
    arms }`, `MatchArm { pattern, body }`, `Pattern { Variant(enum, var,
    bindings) | Wildcard }`; `parse_enum_decl`, `parse_match`.
  - **resolve (3/N):** `EnumId`/`VariantId` assignment; resolve
    `Enum::Variant` construction + match patterns to (enum, variant);
    pattern-binding scoping.
  - **types (3/N):** `Type::Enum` + `EnumData`; type-check construction
    (payload arity/types) + `match` (scrutinee is an enum, arms cover the
    variants — exhaustiveness, `NonExhaustiveMatch` / `UnknownVariant` /
    `MatchArmTypeMismatch`); arm bindings typed from the variant payloads.
  - **codegen (4/N):** the `{ tag, ptr }` layout (D4), construction
    (alloc payload + set tag), `match` switch-lowering (D5), recursive
    drop (D6); `abi-v1` enum-layout entry + stability test; a `c5d1_enum`
    pass fixture.
  - **MIR/D5:** `match` lowers to a tag `Load` + `Branch`-equivalent arms
    (D7); generic enough that the existing pass needs no special case
    beyond seeing the tag as a branch input.

### D9. Generic enums — a fast-follow, not the first slice.

`enum Option<T> { None, Some(T) }` reuses the witness-table mono machinery
(ADR 0016). To keep the first slice tractable, **MVP enums are
non-generic**; generic enums (and thus `Option`/`Result`) land as the
immediate follow-on (D.1b) once the representation + match + drop are
proven on concrete enums.

### D10. Out of scope (MVP).

Named-field variants (tuple payloads only); or-patterns (`A | B`); nested
/ deep patterns (top-level variant + positional bindings only); match
guards (`if` in an arm); literal patterns; `secret enum`; the
inline-small-enum optimisation (D4); exhaustiveness *reachability* (dead
arm) warnings.

### D11. Phase-go + fixture.

`tests/pass/c5d1_enum.sentinel`: a non-generic enum with unit + tuple
variants, constructed and `match`ed (e.g. the `Shape`/`area` example),
returning a computed exit code; plus a UI fixture for `NonExhaustiveMatch`.

### D12. Sub-phase split.

| Sub        | Title                                                          | Risk   |
|------------|----------------------------------------------------------------|--------|
| D.1 (1/N)  | lexer — `enum` + `match` keyword tokens. Additive.             | low    |
| D.1 (2/N)  | AST + parser — `enum` decl, `match` expr, patterns.            | medium |
| D.1 (3/N)  | resolve + types — `Type::Enum`, construction + match check +   | medium |
|            | exhaustiveness.                                                |        |
| D.1 (4/N)  | codegen — `{tag,ptr}` layout + switch lowering + drop +        | high   |
|            | `abi-v1` entry + `c5d1_enum`. ADR flip.                        |        |
| D.1b       | generic enums (`Option`/`Result`) via mono (ADR 0016 reuse).   | medium |

## Reasoning

**Why enums first (over strings/collections).** Of the Phase D
prerequisites, sum types unblock the most — every later piece (a string
*type*, a token *vec*, a `Result`) is easier to design once variants
exist, and the AST is the compiler's spine. It is also the cleanest pure
language feature (no OS/stdlib dependency), so it is the lowest-risk first
Phase D step.

**Why heap-boxed payloads.** Recursive enums are non-negotiable for an
AST, and inline unions can't represent them (infinite size) — exactly the
`?Struct` situation. Reusing the proven heap-payload + drop path is the
conservative MVP; the inline optimisation is a measured follow-on.

**Why non-generic first.** Generic enums multiply the design surface
(mono keys, witness tables) on top of a brand-new representation. Proving
`{tag,ptr}` + switch + drop on concrete enums first, then layering mono
(which already works for fns/structs), is the same incremental discipline
C1 used (concrete types before generics).

## Consequences

### Positive
- The foundational sum type lands — the AST/token/`Result` enabler for
  self-hosting — and Sentinel gains a major, generally-wanted feature.
- Reuses proven machinery (interner tables, `?Struct` heap-payload + drop,
  the `if`-merge, mono) — low novelty risk per piece.

### Negative
- Heap-per-enum-value until the inline optimisation lands.
- A real codegen + drop addition (4/N is the high-risk step); gated behind
  the type-check + the differential fixture corpus.

### Neutral
- No effect on the 1.0 Rust bootstrap's existing programs (additive).

## Revisit

PROPOSED until D.1 closes. Triggers:
- **D4**: if heap-per-value proves too costly for the self-hosted AST,
  bring the inline-small-enum optimisation forward.
- **D9**: confirm generic-enum timing once concrete enums land.
