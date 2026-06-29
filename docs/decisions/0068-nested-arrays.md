# ADR 0068: Nested arrays — lift the depth-1 array rule (`[[T]]`) via an array interner

Status: **PROPOSED** (2026-06-30). Direction confirmed by the maintainer
(chose "lift the array-depth rule" over a process-args builder API). Oracle-moving
→ follows the both-bootstrap-fixed-points rhythm (Rust `snc` + fixtures →
re-bless the per-stage differential → mirror into `selfhost/*.sentinel` → both
fixed points byte-identical → ACCEPTED).

Date: 2026-06-30

## Related

- **0015** (arrays `[T]`): D6 + the **C1.6 depth-1 amendment** introduced the
  rule this ADR lifts. The amendment bounded `?T` and `[T]` to a flat element
  subset (primitives + structs, never each other) because Rust's mutual
  recursion (`ArrayElem` has `Array`, `Array` has `ArrayElem`) would force a
  `Box`, breaking `Type: Copy`. It explicitly said "a future ADR adds them when
  generics or a more sophisticated type representation is in place." This is that
  ADR.
- **0016 / 0017 / 0019 / 0024 / 0066** (the interner-table precedent):
  `GenericInstance(GenericInstanceId)`, `Ref(RefId)`, `Secret(SecretId)`,
  `Task(TaskId)`, `Channel(ChanId)` all keep `Type: Copy` by storing a `u32`
  index into a side table instead of an inline `Box`. This ADR applies the same
  pattern to the array element.
- **0066** (threading + multi-processing): the immediate consumer. M2.1 process
  spawn wants `process_spawn(path: [u8], args: [[u8]])` (D7), which is
  unrepresentable under the depth-1 rule — the trigger for lifting it now.
- **0008** (secret / constant-time): nesting is a public *structural* property;
  the constant-time guarantee is unaffected (a `[secret u8]` element keeps its
  per-element taint via `ArrayElem::Secret`, orthogonal to nesting). See D4.

## Context

`[[u8]]` — an array of byte-arrays, the natural type for "a list of strings"
(e.g. process argv) — does not type-check today: resolving `[[T]]` raises
`TypeError::NestedArray`. The block is purely representational. `Type` is
`Copy + Hash` and that is load-bearing (the interners are value tables, not
`Arc`-wrapped — CLAUDE.md). `ArrayElem` (the inline element of `Type::Array`) has
a flat variant set — `I64/I32/Bool/U8/Struct/TypeParam/GenericInstance/Secret` —
with no `Array` variant, because an inline `ArrayElem::Array(ArrayElem)` is
infinitely sized and would need a `Box`, which is not `Copy`.

Every other "type that contains a type" in the language already solved this with
an **interner**: the contained type lives in a side table and the variant carries
a `u32` id. Arrays are the last container that stayed inline. Lifting the rule is
therefore the same well-worn move applied once more.

## Decisions

### D1. `ArrayElem` gains `Array(ArrayId)`, backed by a new `arrays` interner.

- Add **`ArrayElem::Array(ArrayId)`** (`ArrayId(pub u32)`, `Copy`). The outer
  `Type::Array(ArrayElem)` shape is **unchanged** — this is additive, so every
  existing `Type::Array(ae)` match site stays valid; only matches *on an
  `ArrayElem`* gain one arm.
- Add **`TypedProgram.arrays: Vec<ArrayElem>`** (the array-element interner; an
  `ArrayId` indexes it to recover the *inner* array's element). `[[u8]]` is
  `Type::Array(ArrayElem::Array(id))` where `arrays[id] = ArrayElem::U8`;
  `[[[u8]]]` nests via `arrays[id2] = ArrayElem::Array(id)`. Arbitrary depth
  falls out for free, like the other interners. `intern_array_elem(arrays, ae)`
  dedups by structural equality (mirrors `intern_secret`/`intern_channel`).
- `ArrayElem::to_type()` maps `Array(id)` → `Type::Array(arrays[id])` — but
  `to_type` has no table handle, so the promotion that needs the table goes
  through `TypedProgram` (a `array_elem_to_type(&self, ae)` helper) at the call
  sites that have the program; the bare `to_type()` keeps its current signature
  for the flat variants and is not called on `Array(_)` in those contexts (audited
  per the work-list).

### D2. Resolution — `[[T]]` is accepted; `NestedArray` is retired for arrays.

`resolve_type_expr`'s `TypeExprKind::Array(inner)` arm currently resolves the
inner and rejects `inner.is_array()` with `NestedArray`. New behavior: resolve
the inner to a `Type`; if it is itself a `Type::Array(inner_ae)`, intern
`inner_ae` into `arrays` and produce `Type::Array(ArrayElem::Array(id))`. The
existing `to_array_elem` / `to_array_elem_secret` demotes gain an `Array` case
(they currently return `None` for a `Type::Array` inner). `[?T]` (array of
nullable) stays deferred — this ADR lifts array-of-array only; nullable nesting
is a separate, lower-demand follow-up.

### D3. Codegen — recursive layout, indexing, and drop.

- **Layout (unchanged shape).** `[[u8]]` is still `{ i64 len, ptr data }`; the
  element stride is `sizeof(element)` where a nested-array element is itself a
  `{ i64, ptr }` (16 bytes), not a scalar. The element-size/stride computation
  (used for the literal's heap alloc + the index GEP) must consult the element
  type — for `ArrayElem::Array(_)` that is the array struct size, not a scalar
  width. This is the one genuinely new codegen logic.
- **Literal.** `[["a"],["bc"]]`-style: each element is lowered as its own array
  value (recursively) and stored into the outer buffer.
- **Index.** `a[i]` on `[[u8]]` yields a `[u8]` (the inner array struct), loaded
  from the GEP — the read path is the existing one with the element type widened
  to an aggregate.
- **Drop.** A `[[u8]]` owns its inner `[u8]`s, which own their byte buffers. Drop
  recurses: free each element's buffer, then the outer buffer. `needs_drop` for
  `Type::Array(ArrayElem::Array(_))` is true; the per-element drop walks the inner
  array. (Scalar-element arrays keep their single-`free` drop.)

### D4. Constant-time + secrets — unchanged.

Nesting is a public structural property (lengths + pointers are public, as for
any array). A `[secret u8]` *element* of a `[[secret u8]]` keeps its
`ArrayElem::Secret` taint, so indexing still yields `secret u8` and the
`secret_leak` MIR pass is unaffected. No new secret sink. `[[T]]` admits a secret
*scalar* leaf only (the depth-1 secret rule composes: the leaf may be
`secret SCALAR`, never `secret [T]`).

### D5. The self-host mirror is expected to be small.

`scg`'s array type is already represented as an interner "kind" with the element
as a **type handle** (`mk_array(c, elem_handle)`), so `mk_array(c, mk_array(c,
u8_handle))` already expresses `[[u8]]` structurally — the selfhost may need
little beyond confirming `type_of_typeexpr`'s `TArray` arm and the array codegen
(literal/index/drop + element stride) handle a nested element. The byte-identity
differential is the gate (D7 rhythm). The Rust→selfhost asymmetry is the same as
`?T` generalization (ADR 0066 M1.2b): the Rust side carries the representational
restriction (the inline `ArrayElem` enum), the selfhost is already handle-general.

### D6. Scope — array-of-array only this increment.

In scope: `[[T]]` for any depth, element leaf in the existing flat subset
(primitives / structs / `secret SCALAR` / generic). Out of scope (deferred,
named): `Vec<[T]>` (a `VecElem::Array` mirror — add when a consumer needs it),
`[?T]` (array of nullable), `?[T]` (nullable of array). The process-args consumer
(`[[u8]]`) is fully covered.

## Reasoning

The interner is not a new concept here — it is the *only* representation the
language has ever used for "a type containing a type" while preserving `Copy`,
and arrays are the last holdout. Choosing it makes nested arrays consistent with
generics/refs/secrets/tasks/channels rather than a special case. The alternative
(convert `Type::Array(ArrayElem)` → `Type::Array(ArrayId)` over a full-`Type`
table) is more general but rewrites every array match site in both compilers; the
additive `ArrayElem::Array(ArrayId)` keeps the blast radius to *matches on an
ArrayElem* plus the new table, and leaves the (far more numerous) `Type::Array`
matches untouched.

## Consequences

### Positive

- `[[T]]` (and deeper) becomes a first-class type; "a list of strings" is
  expressible — unblocking ADR 0066 M2.1 process argv and any future
  list-of-list data.
- Consistent with the existing interner pattern; `Type` stays `Copy + Hash`.
- Constant-time guarantee untouched (a structural, public change).

### Negative

- Codegen gains genuinely new logic (recursive element-size/stride + recursive
  drop) — the part most likely to harbor a bug; covered by a `tests/pass`
  fixture run through both back ends + both fixed points.
- One more interner table to thread (`arrays`), like `secrets`/`channels`.

### Neutral

- `Vec<[T]>` / `[?T]` / `?[T]` remain deferred (D6) — no consumer yet.

## Alternatives considered

- **A process-args builder API** (`process_new`/`process_arg`/`process_run`,
  each arg a flat `[u8]`) — avoids nested arrays entirely and was the
  recommended lower-cost path, but the maintainer chose the general language
  feature (reusable beyond processes). Recorded here as the rejected
  lower-scope option.
- **Full `Type::Array(ArrayId)` over a `Type` table** — most general, but a
  large rewrite of every array match site in both compilers for no near-term
  benefit over the additive `ArrayElem::Array`. Revisit only if a future feature
  needs array elements outside the `ArrayElem` subset.

## Revisit

- **D6 (scope):** add `Vec<[T]>` / `[?T]` / `?[T]` when a real `examples/`
  program needs them (the "build real programs → find the gap" discipline).
- **D1 (interner choice):** revisit the full-`Type` table only if `ArrayElem`'s
  flat-subset leaf becomes a limitation.
