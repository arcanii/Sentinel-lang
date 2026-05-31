# ADR 0034: Phase D.3 — growable collections (`Vec<T>`; `String` = `Vec<u8>`)

Status: **ACCEPTED-WITH-AMENDMENTS** — the growable-`Vec<T>` MVP is complete
(D.3 (1/N) `Type::Vec` + `vec_new`/`push`/`len`; D.3 (2/N) `v[i]` + `pop` + the
`Vec<u8>`→`[u8]` bridge + `String` = `Vec<u8>`), compiling + running end to end,
leak-free. The third Phase D sub-phase ADR under ADR 0031 (Phase D kickoff) D4
item 3. After sum types (D.1 / ADR 0032) and strings + a byte type (D.2 / ADR
0033), **growable collections** landed: a lexer accumulates an identifier/number
byte-by-byte and a parser accumulates a list of tokens / AST nodes, neither of
which the fixed-size `[T]` array can express (no `push`, no growth). Deviations
from this ADR's letter are recorded under Amendments; the out-of-scope items
(D8 — a `Map`, droppable-element `Vec` drop, `Vec`-in-generic-fns, etc.) remain
future work.

Date: 2026-05-31
Related:
  - **0031** (Phase D kickoff): D4 item 3 names "growable collections — `Vec<T>`
    + a `Map` keyed on the above, broker-backed; generics already exist to
    parameterise them." D5 sequences the self-host port, whose lexer/parser
    consume this.
  - **0033** (strings + byte type): a fixed string **IS a `[u8]`**. The parallel
    lever here — **a growable string IS a `Vec<u8>`** — extends that: `String`
    is not a new nominal type, it is exactly a growable byte vector.
  - **0015** (arrays / nullable): `[T]` lowers to `{ i64 len, ptr data }`. A
    `Vec<T>` is the same idea plus a **capacity** field — `{ i64 len, i64 cap,
    ptr data }` — and **mutation** (`push`). `Type::Vec(VecElem)` mirrors
    `Type::Array(ArrayElem)` exactly (D3), reusing the array index / bounds-check
    / drop / move machinery.
  - **0016** (generics / monomorphisation): the **flat element-subset** model
    (`ArrayElem`) — not full fn/struct generics — is how `[T]` parameterises its
    element; `Vec<T>` reuses it (`VecElem`). So `Vec<u8>` / `Vec<i64>` /
    `Vec<Point>` are distinct concrete types with element-generic builtins, **no
    new monomorphisation machinery**.
  - **0017** (references / mutability / borrow check): `push(&mut v, x)` is the
    first heap **mutation** primitive — it reuses `&mut T` + the shared-XOR-
    mutable borrow rule (the notable new interaction; D6). A `Vec` is **Move**
    (owns a heap buffer), like `[T]`.
  - **0022** (classes): generic *classes* are deferred (D1), so `Vec<T>` is a
    **builtin generic type** (like `[T]`), **not** a user `class Vec<T>` with
    methods. The operations are builtins (`push` / `vec_new` / `len` / `v[i]`).
  - **0028** (broker scope arenas): the roadmap says "broker-backed", but a `Vec`
    **grows by `realloc`**, which the broker's bump arena cannot do (bump =
    allocate-only, bulk-free). So the MVP uses libc `malloc`/`realloc`/`free`
    (as `[T]` already uses libc `malloc`); broker-backing is deferred (D10).

## Context

Of the remaining Phase D prerequisites (ADR 0031 D4), **growable collections**
are next. The 1.0 + D.1/D.2 language has only **fixed** `[T]` arrays (`len`,
indexing, no `push`/growth) and **fixed** `[u8]` strings. A compiler cannot run
without growth: the lexer builds a token's text one byte at a time; the parser
builds a token stream and AST-node lists whose sizes are unknown up front.

This sub-phase adds a **growable, owned, mutable `Vec<T>`** — a heap vector with
a length, a capacity, and amortised-O(1) `push` — and defines **`String` =
`Vec<u8>`** (a growable byte buffer; the nominal `String` that ADR 0033 D8
deferred, refined to "a growable string is its bytes" to match the 0033 lever).

The unifying decision is **`Vec<T>` is `[T]` plus capacity and mutation** —
modelled the same way (`Type::Vec(VecElem)` parallel to `Type::Array(ArrayElem)`,
the same flat element subset, element-generic builtins), reusing the array
index / bounds-check / move / drop machinery wholesale. The genuinely new pieces
are the **capacity field + growth (`realloc`)**, the **`push` mutation
primitive** (`&mut Vec`), and the `Vec`-construction/operation codegen + runtime.

## Decision

### D1. Goal.

Add a growable `Vec<T>` (and `String` = `Vec<u8>`), end to end (types → codegen
→ runtime → drop), with **`vec_new`** (empty), **`push(&mut v, x)`** (append +
grow), **`len(v)`** (reuse), and **`v[i]`** (bounds-checked element read) —
enough to write a lexer that accumulates identifier/number bytes into a
`Vec<u8>` and compares the result against keyword `[u8]`s, and (with the
droppable-element follow-on) a parser that accumulates `Vec<Token>`.

### D2. Surface syntax.

```sentinel
fn read_word(src: [u8], start: i64, end: i64) -> Vec<u8> {
    let mut w: Vec<u8> = vec_new();      // empty growable byte buffer
    let mut i: i64 = start;
    // (loops arrive at D.6; recursion / a helper substitutes for now)
    push(&mut w, src[i]);                 // append a byte; grows on demand
    w                                     // moved out (owned)
}

fn main() -> i64 {
    let mut v: Vec<i64> = vec_new();
    push(&mut v, 10);
    push(&mut v, 20);
    push(&mut v, 12);
    let n: i64 = len(v);                  // 3 (reuse the array `len`)
    v[0] + v[2] + n                        // 10 + 12 + 3 = exit 25
}
```

  - **`Vec<T>`** is a builtin generic type name, usable anywhere a type is
    (params, returns, `let` annotations). It parses with **no lexer or parser
    change** — `Vec<u8>` is already a `TypeExprKind::Generic { name: "Vec", args:
    [u8] }` (the C1.7 generic-type syntax); the types layer recognises the name
    `Vec` and produces `Type::Vec(VecElem)`.
  - **`vec_new()`** — an empty `Vec<T>` (len 0, cap 0, null/dangling data). Its
    element type `T` is inferred from the binding's annotation (bidirectional,
    mirroring how `null`'s `?T` is inferred — ADR 0014 D2).
  - **`push(&mut v, x)`** — appends `x` (typed `T`) to `v` (typed `&mut
    Vec<T>`), growing the buffer if `len == cap`. Returns `i64` 0 (a statement-
    shaped builtin like `print`).
  - **`len(v)`** (→ `i64`) reuses the existing builtin, extended to accept a
    `Vec<T>` argument as well as `[T]` (both are "a collection with a length").
  - **`v[i]`** (→ `T`) reuses the `Index` postfix + the C1.6 bounds-check
    (`0 <= i < len`), reading element `i`.
  - **`String` = `Vec<u8>`** — the growable byte buffer. No separate nominal
    type at the MVP; a `String` is exactly a `Vec<u8>` (the 0033 lever).

### D3. Representation — **`Vec<T>` is `[T]` plus capacity + mutation**.

`Type::Vec(VecElem)` is a new `Type` variant, parallel to `Type::Array(
ArrayElem)`. `VecElem` is the **same flat element subset** as `ArrayElem` (the
concrete cases: `I64` / `I32` / `Bool` / `U8` / `Struct(id)`; generic-context
elements `TypeParam` / `GenericInstance` are deferred — `Vec` inside a generic
fn body — alongside the array ones). `Vec<u8>` is `Type::Vec(VecElem::U8)`, etc.

LLVM layout (`abi-v1`): **`{ i64 len, i64 cap, ptr data }`** — 24 bytes, align 8.
This is `[T]`'s `{ i64 len, ptr data }` with a capacity field inserted; `data`
points at a heap buffer of `cap * sizeof(T)` bytes, the first `len` of which are
live. The element `Type` is tracked at the `sentinel-types` level (like `[T]`),
not in the LLVM type (LLVM sees only `{ i64, i64, ptr }`).

`Vec<T>` reuses, unchanged: indexing + bounds-check (D2), move / use-after-move
(it is **Move** — owns the buffer), and the returned-`Vec`-escapes path. It does
**not** reuse arena routing (a `Vec` reallocs; ADR 0028's bump arena cannot —
D10). The new pieces are capacity, growth, and `push`-mutation.

### D4. The `Vec<T>` type — a builtin generic (not a class).

`Vec<T>` is a **builtin** parameterised type, exactly as `[T]` is — **not** a
user `class Vec<T>` (generic classes are deferred, ADR 0022 D1). The element is
the flat `VecElem` subset (D3), and the operations are **builtins** generic over
the element (they recover `T` from the typed argument, the way array indexing /
`len` already recover the element from `[T]`). So **no new monomorphisation** is
needed — `Type::Vec` is the next exhaustive-`Type`-match cascade after
`Type::U8` (mechanical, wide; the ADR 0032/0033 "land the arms together, build,
then semantics" discipline).

### D5. Operations + growth (MVP).

  - **`vec_new() -> Vec<T>`** — `{ len = 0, cap = 0, data = null }`. The element
    type is inferred from context (D2). No allocation until the first `push`.
  - **`push(&mut v, x: T)`** — if `len == cap`, grow: `new_cap = max(1, cap * 2)`,
    `realloc` the buffer to `new_cap * sizeof(T)`, write `data[len] = x`, `len +=
    1`. Amortised O(1). A new runtime symbol `sentinel_vec_grow(ptr data, i64
    cap, i64 new_cap, i64 elem_size) -> ptr` (or inline `realloc`) backs it.
  - **`len(v) -> i64`** — reuse the C1.6 builtin, extended to `Vec` (reads the
    `len` field; `Vec` and `[T]` both lower the length to field 0).
  - **`v[i] -> T`** — reuse the C1.6 bounds-checked `Index` (`0 <= i < len`).
  - **`pop(&mut v) -> T`** *(D.3 (2/N))* — remove + return the last element
    (panic on empty, like an OOB index — a `?T`-returning `pop` is deferred).
  - **`Vec<u8> -> [u8]` bridge** *(D.3 (2/N))* — a `vec_to_array(v) -> [u8]`
    (copy the live `len` bytes into a fresh `[u8]`) so a built `String` can be
    `str_eq`'d against a keyword `[u8]`. (Alternatively, extend `str_eq` to
    accept `Vec<u8>`; the bridge keeps the `str_eq` surface unchanged.)
  - **Capacity hints / `with_capacity` / `insert` / `remove` / slicing /
    iterators — DEFERRED** (D10).

### D6. Mutation + the borrow check.

`push(&mut v, x)` is the **first heap-mutation primitive**. It takes `v` by
`&mut Vec<T>` and reuses the C2.2 shared-XOR-mutable rule unchanged: while a
`push` holds `&mut v`, no other borrow of `v` is live. `v` must be declared
`let mut`. The borrow checker already enforces all of this for `&mut` to structs
/ `*p = e` assignment; `push` is just another `&mut`-taking call. No new
borrow-check surface — only confirm a builtin taking `&mut Vec` participates in
the existing `&mut` liveness tracking (the runtime-builtin arg rule, ADR 0033
A3, must treat a `&mut Vec` arg as a mutable borrow, not a by-value move).

### D7. Codegen + runtime.

  - **`vec_new`** — build `{ 0, 0, null }` (no allocation).
  - **`push`** — load `{len, cap, data}` from the `&mut Vec` pointer; if `len ==
    cap`, call the growth runtime (`realloc` to `max(1, cap*2) * sizeof(T)`),
    store the new `cap` + `data`; `data[len] = x`; `len += 1`; store back.
  - **`v[i]`** — the existing bounds-checked GEP+load (the `Vec` data ptr is
    field 2, not field 1 — the one layout delta from `[T]`).
  - **`len`** — extract field 0 (same as `[T]`).
  - **Drop** — free the `data` buffer (libc `free`; `null`-safe for an unpushed
    `Vec`). **Element drop:** for a `Vec` of **primitive** elements (`u8` / `i64`
    / `i32` / `bool` — including `Vec<u8>` = `String`) this is leak-free (no
    element owns heap). A `Vec` of **droppable** elements (`Vec<Struct>` where the
    struct owns heap, `Vec<[u8]>`) needs per-element recursive drop — **deferred**
    (D10; the same shape as the enum payload-drop follow-on, ADR 0032 A1).
  - **Runtime:** one new symbol — `sentinel_vec_grow` (or a direct `realloc`
    extern) — joining the `abi-v1` contract.

### D8. Out of scope (MVP).

A `Map` / `HashMap` (a separate sub-phase / ADR — needs hashing + the `Vec`
foundation first); **droppable-element `Vec` drop** (`Vec<Struct>` / `Vec<[u8]>`
recursive element free — D7, a follow-on); `Vec` inside generic fn bodies
(`VecElem::TypeParam` / `GenericInstance`); `with_capacity` / capacity hints;
`insert` / `remove` / `clear` / `swap`; slicing / sub-views; iterators / `for`;
a `?T`-returning `pop`; `[secret T]` / `secret Vec`; broker-backing the buffer
(the bump arena cannot `realloc` — D10 reasoning); UTF-8-validated `String`
distinct from `Vec<u8>` (bytes only, per the 0033 lever).

### D9. Pipeline / sub-phase split.

| Sub        | Title                                                          | Risk   |
|------------|----------------------------------------------------------------|--------|
| D.3 (1/N)  | `Type::Vec(VecElem)` + the cascade; `vec_new` / `push` / `len` | high   |
|            | typed + codegen + the growth runtime + `&mut Vec` borrow +     |        |
|            | drop (primitive-element). End to end — a growable `Vec<u8>` /  |        |
|            | `Vec<i64>` builds + measures. (No lexer/parser change.)        |        |
| D.3 (2/N)  | `v[i]` element read (reuse `Index`); `pop`; the `Vec<u8>` →    | medium |
|            | `[u8]` bridge (`str_eq` a built string against a keyword).     |        |
| D.3 (3/N)  | close — `c5d3_collections` phase-go (leak-free via             | low    |
|            | `leaks --atExit`) + `abi-v1` `Vec` entry + ADR flip.           |        |

The `Type::Vec` cascade (every exhaustive `Type` match across types / codegen /
borrow-check / mir) is the same coordinated-arms discipline as `Type::U8`
(ADR 0033 (3/N)) / `Type::Enum` (ADR 0032 (3/N)); a `Vec` is Move + heap-owning,
so it groups with `Array` in most arms (Move, needs-drop, a struct-shaped value).
Builtins shift the `FnId` base again (the ADR 0033 (3/N) lesson: a handful of
hardcoded-`FnId` test sites move).

### D10. Phase-go + fixture.

`tests/pass/c5d3_collections.sentinel`: build a `Vec<u8>` by `push`ing the bytes
of an identifier, read it back with `v[i]`, bridge it to a `[u8]`, and `str_eq`
it against a keyword — plus a `Vec<i64>` push/index/len corpus — returning a
computed exit code; **verified leak-free via `leaks --atExit`** (the `Vec`
buffer is freed at scope-exit drop). Plus a `u8`/`i64` `Vec` unit corpus + a UI
fixture (e.g. `push` on a non-`mut` `Vec` → a borrow/`Immutable` error, or a
`Vec<u8>` vs `Vec<i64>` element `Mismatch`).

## Reasoning

**Why `Vec<T>` is `[T]` + capacity (not a class, not new generics).** The
fixed `[T]` array already solves element typing (the flat `ArrayElem` subset),
indexing + bounds checks, move semantics, and heap drop — all built + tested.
A growable vector is that plus a capacity field and `push`; modelling it as
`Type::Vec(VecElem)` reuses every one of those pieces and adds only growth +
mutation. A user `class Vec<T>` would need generic classes (deferred), and a
fn/struct-generic `Vec<T>` would drag in monomorphisation the flat subset
already avoids for `[T]`. The lowest-risk path is the one that mirrors the
proven array machinery.

**Why `String` = `Vec<u8>`.** ADR 0033 established that a fixed string is its
bytes (`[u8]`); a growable string is its growable bytes (`Vec<u8>`). A lexer
accumulating an identifier wants exactly a growable byte buffer, not a Unicode
abstraction. Keeping `String` = `Vec<u8>` (rather than a distinct nominal type)
inherits the whole `Vec` machinery for free and matches the 0033 lever; a
UTF-8-validated nominal `String` is a later refinement gated on need.

**Why libc `realloc`, not the broker.** The roadmap's "broker-backed" is
aspirational: ADR 0028's bump arena is allocate-only + bulk-free, so it cannot
`realloc` a growing buffer (the load-bearing `Vec` operation). `[T]` already
uses libc `malloc`; `Vec` uses libc `malloc`/`realloc`/`free` for the same
reason. A broker growth-arena is a measured later optimisation.

## Consequences

### Positive
- Growable, mutable, owned collections land — the lexer's token buffer and (with
  the droppable-element follow-on) the parser's token/node lists become
  expressible — with maximal reuse of the proven `[T]` machinery (low novelty
  risk) and **no lexer/parser change**.
- `push` introduces the first heap-mutation primitive over the existing `&mut` +
  borrow check, exercising shared-XOR-mutable on a real growable structure.

### Negative
- A real codegen + runtime addition (the `{len,cap,ptr}` repr, `push`-with-
  growth, a `realloc` runtime symbol), gated behind the type layer + a
  differential phase-go.
- Another `Type`-variant cascade (the `Vec` arms across every exhaustive match)
  — mechanical but wide (the ADR 0032/0033 lesson).
- Droppable-element `Vec` drop is deferred (a `Vec<Struct>`/`Vec<[u8]>` leaks its
  elements until the follow-on — leak-safe for the primitive-element `Vec<u8>` /
  `Vec<i64>` the lexer needs; the enum-A1-shaped completeness follow-on).

### Neutral
- No effect on existing programs (additive; `repro.rs` byte-identical for pre-D.3
  fixtures — an unused `Vec` runtime declaration emits nothing).

## Amendments

Recorded as the sub-phases land; the ADR stays PROPOSED until D.3 (3/N) closes,
then flips to ACCEPTED-WITH-AMENDMENTS.

### D.3 (1/N) — `Type::Vec` + `vec_new` / `push` / `len` (landed)

End to end: a growable `Vec<u8>` / `Vec<i64>` builds + measures, leak-free under
`leaks --atExit` (`tests/pass/c5d3_collections.sentinel`, exit 67). Deviations
from this ADR's letter:

- **A1 — `String` deferred to (2/N).** D5 recommends `String` = `Vec<u8>` as a
  thin alias. Confirmed but deferred: in (1/N) a string literal still types to
  `[u8]` and the `[u8]`↔`Vec<u8>` bridge is (2/N), so recognising the `String`
  name now would make `let s: String = "hi"` a `Mismatch`. The alias lands in
  (2/N) alongside the bridge so `String` is ergonomic the moment it appears.
  (1/N) is plain `Vec<u8>` / `Vec<i64>`.
- **A2 — return-type pushdown extended to `Vec`.** `vec_new()` infers its
  element from the expected type. The body-tail expected-type seeding (which fed
  `null` / generic struct literals) only fired for `Nullable` / `GenericInstance`
  return types; `Type::Vec` was added (three sites: free fn / method / trait
  method) so `fn f() -> Vec<i64> { vec_new() }` infers, matching the `let`-
  annotation path.
- **A3 — `len` overload is a contained special-case.** D5's "reuse / extend
  `len`" is an early `id == LEN_FN_ID` branch in `check_call` accepting `[T]` or
  `Vec<T>` (the uniform generic path unifies one param shape only). The `[T]`
  behaviour — including the `Mismatch` on a non-collection arg — is preserved
  exactly. `vec_new` / `push` need NO special-case: they flow through the uniform
  path (an explicit `(Vec, Vec)` arm in `unify_one` binds the element).
- **A4 — `sentinel_realloc`, not `sentinel_vec_grow`.** D7 offered a Vec-specific
  grow helper OR a realloc extern; the plainer `sentinel_realloc(ptr, new_size)
  -> ptr` was chosen (codegen computes `max(1, cap*2) * sizeof(T)` inline).
  `realloc(null, n) == malloc(n)` also serves the first push, so there is no
  separate first-allocation path.
- **A5 — `VecElementNotSupported`.** A new `TypeError` rejects a non-flat `Vec`
  element (`Vec<[T]>` / `Vec<Vec<T>>` / `Vec<?T>` / `Vec<&T>`) — the `Vec`
  analogue of `NestedArray`, enforcing the D8 flat-subset deferral.
- **Arena routing — no change needed.** A `Vec` binding's initialiser is a
  `vec_new()` `Call`, not an `ArrayLit`, so `compute_arena_routed`'s
  `is_primitive_array_lit` gate already excludes every `Vec` — D3's "a `Vec`
  reallocs, the bump arena cannot" holds for free, with no edit to the routing
  predicate (and its UAF-safety invariant is untouched).
- **`&mut Vec` borrow.** D6 confirmed: the runtime-builtin-arg rule (ADR 0033
  A3) was extended so a `&`/`&mut` reference argument to a builtin registers a
  borrow (mutable for `&mut`) via the normal `Ref`/`RefMut` path, not a
  non-consuming by-value read. `push` thus participates in shared-XOR-mutable;
  a non-`mut` `Vec` is rejected (`BorrowMutOfImmutable`).

Deferred to (2/N): `v[i]` element read (the `Index` node carries `ArrayElem` and
`lower_index` hard-codes the field-1 array data pointer — real typed-tree +
codegen work, not a trivial reuse), `pop`, the `Vec<u8>`→`[u8]` bridge, and the
`String` alias (A1). Deferred to (3/N): the `c5d3` phase-go close + this flip.

### D.3 (2/N) — `v[i]` + `pop` + the bridge + `String` (landed, MVP complete)

This sub-phase folded in the thin (3/N) "close" (the comprehensive phase-go + the
ADR flip), since `v[i]` + `pop` + the bridge + `String` exhaust the D.3 MVP
surface (everything else is D8-deferred) and the `abi-v1` `Vec` entry already
landed in (1/N). `tests/pass/c5d3_collections.sentinel` is now the comprehensive
phase-go (a `Vec<i64>` push/index/pop/len + escape, and a `String` "let" built /
indexed / bridged / `str_eq`'d), exit 55, **0 leaks** under `leaks --atExit`.

- **B1 — `v[i]` reuses the `Index` node (no new typed variant).** D5 lists `v[i]`
  as reuse; the lighter realisation chosen: since `VecElem` and `ArrayElem` are
  the identical flat subset, a `Vec` index demotes its element to an `ArrayElem`
  for the existing `TypedExprKind::Index`, and `lower_index` picks the data
  pointer field from the (secret-stripped) target type — **field 2** for a `Vec`
  vs **field 1** for an array. No `VecIndex` node, so no typed-tree cascade
  (mir / hir / borrow-check / substitute all unchanged). `len` (field 0) and the
  C1.6 bounds-check + OOB trap are reused verbatim.
- **B2 — `pop` / `vec_to_array` are new builtins via the uniform path.** Both
  generic over `T`; `pop<T>(&mut Vec<T>) -> T` (its `&mut Vec<T>` an interned
  mutable Ref like `push`'s) and `vec_to_array<T>(Vec<T>) -> [T]` flow the same
  uniform generic-call inference as `vec_new`/`push` (the `(Vec,Vec)` /
  `(Ref,Ref)` `unify_one` arms). `pop` decrements `len` (the buffer is retained,
  not shrunk) and traps on empty via `sentinel_panic_oob` (idx −1, len 0).
  `vec_to_array` is **non-consuming** (borrows the Vec per the ADR 0033 A3 rule)
  and `memcpy`s the live `len * sizeof(T)` bytes into a fresh `sentinel_alloc`'d
  `[T]`, so the Vec and the array own independent buffers (both freed at scope
  exit). The bridge keeps `str_eq`'s `[u8]`/`[u8]` surface unchanged (vs.
  overloading `str_eq` on `Vec<u8>`).
- **B3 — `String` = `Vec<u8>` (Amendment A1 resolved).** The bare type name
  `String` resolves to `Type::Vec(VecElem::U8)` in `resolve_type_expr`'s Ident
  arm. A string *literal* is still a `[u8]` (so `let s: String = "hi"` is a
  Mismatch); building a `String` is `vec_new` + `push` (a `[u8]` -> `Vec<u8>`
  direction is future work). The `vec_to_array` bridge closes the loop the other
  way (a built `String` -> `[u8]` for keyword comparison).
- **FnId base.** `pop` / `vec_to_array` are builtins FnId 9 / 10, so user fns now
  start at FnId 11 (main 9 -> 11); the hardcoded-FnId test sites shifted again.

## Revisit

PROPOSED until D.3 closes. Triggers:
- **D3**: if a self-host stage needs `Vec` inside a generic fn / `Vec<Vec<T>>`
  nesting, lift the flat-element-subset restriction (the `[T]` D6 deferral).
- **D7**: when the parser needs `Vec<Token>` (droppable elements), land the
  recursive element-drop follow-on (coordinate with the enum payload-drop model,
  ADR 0032 A1).
- **D8**: bring a `Map`/`HashMap` forward (its own ADR) once symbol tables are
  the bottleneck; settle hashing + the broker-growth question then.
