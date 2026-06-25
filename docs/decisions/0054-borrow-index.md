# ADR 0054: Borrowing an array / Vec element — `&mut a[i]` and `&a[i]`

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A5) — the feature is in `snc` (Phase 1) and required
**no change** to the self-hosted `scg` (Phase 2); the corpus fixture `tests/pass/c59_borrow_index`
validates `scg == snc` byte-for-byte across all 8 selfhost stage differentials, both bootstrap
fixed points hold, and the full nextest is green. Amendments below record the deviations from the
PROPOSED plan.

Adds the borrow forms `&mut a[i]` (yielding `&mut T`) and `&a[i]` (yielding `&T`) — taking a
reference to the element at a public index `i` of an array `[T]` or vector `Vec<T>`. This closes
the element-borrow gap that ADR 0050 (mutable index assignment) explicitly deferred: an array
element could be *written* (`a[i] = v`) but not *borrowed* and passed by reference to a function.
The constant-time SHA-256 work surfaced it (it threaded its state through a `&mut [secret i32]`
whole-array borrow + index-assign because an element `&mut a[i]` was rejected with
`IndexAssignNotSupported`).

## Decision

### Why now, and why an lvalue relaxation (not a builtin)

`&mut a[i]` is the natural completion of the mutable-array story. ADR 0050 made `a[i]` a write
**target**; this ADR makes it a borrow **place** — the symmetric capability. The idiomatic use is
passing an element to a helper that mutates it in place, `f(&mut a[i])`, which `a[i] = expr` cannot
express (the helper needs the element *by mutable reference*, not a fresh value).

It is implemented by allowing `Index` as a borrow target — the same lvalue machinery that already
handles `&mut x` and `&mut s.f`. No builtin (which would shift FnIds in the dumps, ADR 0048/0049/
0050's reasoning), no new AST node (`&mut a[i]` already parses as `Unary(RefMut, Index{..})`), no
new codegen (the element-address GEP factored by ADR 0050 is reused).

### Semantics

`&mut a[i]` computes the address of element `i` of the collection `a` — the **same bounds-checked
GEP the read path `a[i]` uses** (a runtime `0 <= i < len` check that calls `sentinel_panic_oob` on
violation) — and yields it as a `&mut T` (for `&a[i]`, a `&T`), where `T` is the element type. The
base `a` must be a **mutable lvalue** for `&mut` (`let mut`, a `mut` parameter, or a field/deref
chain ending in one) — checked exactly like `&mut s.f` by recursing on the base. `&a[i]` (a shared
borrow) does not require `a` to be mutable. The index `i` follows the existing rule: it must be a
public `i64` (`IndexNotInt`), so a **secret index is rejected** for borrows exactly as for reads and
writes — the constant-time story comes for free, with no new MIR sink. Borrowing an element whose
*value* is secret (`&mut a[i]` where `a: [secret i64]`) is fine — the access pattern is public, only
the referent is secret.

### Borrow granularity (A4)

`&mut a[i]` borrows the **whole array** `a` (binding-precise), not the individual element — the
same conservative, pre-Polonius choice the borrow checker already makes for `&mut s.f` (which
borrows the whole struct). A runtime index cannot distinguish loans at analysis time without NLL/
Polonius. Consequence: two simultaneous element borrows of the same array (`swap(&mut a[i], &mut
a[j])`) are **rejected** (a `BorrowConflict` on `a`), exactly as `&mut s.f` + `&mut s.g` is. Each
*sequential* element borrow is transient (it ends when the borrowing call returns), so the common
case — `f(&mut a[i])` in a loop — works. Element-granular borrows are deferred to the Polonius
migration (ADR 0018 D6), tracked in `borrow-check-limitations.md`.

## Implementation

### Phase 1 — `snc` (one arm) (A3)

The entire Phase-1 change is the `Index` arm of `check_mutable_borrow_target` in
`crates/sentinel-types/src/lib.rs`: it previously returned `TypeError::IndexAssignNotSupported`
unconditionally; it now **recurses on the base** (`check_mutable_borrow_target(inner_target, …)`),
identical in shape to the adjacent `FieldAccess` arm. Everything else was already in place:

- **AST / parser**: unchanged. `&mut a[i]` parses to `Unary(RefMut, Index{..})`.
- **Type-check dispatch**: unchanged. The `Ref`/`RefMut` arm of `check_expr` already type-checks
  the inner `Index` (which yields the element type and rejects a secret index), checks it is an
  lvalue (`Index` already qualifies), calls `check_mutable_borrow_target` only for `&mut`, and
  interns the result as `&[mut] <elem-type>`. So `&a[i]` (shared) **already worked** before this
  ADR — only the `&mut` path was gated (A5).
- **Borrow-check**: unchanged. `source_of_lvalue` / `walk_expr_lvalue` already recurse through an
  `Index` projection to the base binding, so `&mut a[i]` registers a whole-array borrow on `a` with
  no new code (A4).
- **Codegen (both backends)**: unchanged. `&mut a[i]` lowers through `lower_lvalue_ptr`'s `Index`
  arm — the element-address GEP `lower_index_elem_ptr` that ADR 0050 factored — reused verbatim
  (A2). A borrow of a place is "the address is the value", so the `RefMut`/`Ref` wrapper adds
  nothing once the inner `Index` produced an address.
- **MIR / constant-time**: unchanged. No new sink — the secret-index rejection happens in the type
  stage during the inner `Index` check.

### Phase 2 — `scg` (no change) (A1)

The self-hosted compiler needed **no change**. `scg` has no validation phase (the Rust `snc`
oracle validates; `scg` mirrors *code generation*), and its codegen already routes a borrowed
`Index` place through `cg_emit_index_addr` — the element-address helper ADR 0050 added — via the
existing `cg_suppress` mechanism (the `Unary` `&`/`&mut` arm suppresses the inner load; the `Index`
arm emits the address GEP and signals a place). So `&mut a[i]` emits byte-identically to the
oracle with the code already present. `tests/pass/c59_borrow_index` (array + Vec + shared `&a[i]` +
a secret-valued element) drives all 8 selfhost differentials; they pass byte-for-byte and both
fixed points hold.

### No Copy gate for borrows (A5)

ADR 0050's index-*assign* gates the element type to Copy scalars (a Move element would need
drop-on-overwrite). A **borrow** does not overwrite or drop, so the `Index` arm here mirrors
`FieldAccess` (a plain recurse) with **no element-type gate** — `&mut a[i]` is permitted for any
element type the array can hold, exactly as `&mut s.f` is for any field type. The MVP fixture +
example exercise scalar and `secret` scalar elements; non-Copy element borrows are sound (a
reference is not a move) and fall out for free.

## Validation

`tests/pass/c59_borrow_index` (exit 42): `&mut a[i]` on an array through a loop, `&a[i]` shared
borrow, `&mut v[i]` on a `Vec`, and `&mut` on a `secret i64` element — all reduced and asserted.
`examples/math/inplace` adds `std::math::num::clamp_assign(x: &mut i64, …)` and clamps array + Vec
elements in place via `clamp_assign(&mut a[i], …)` across a module boundary (dogfooding modules +
`--separate` with the new borrow), built both ways. The secret-index rejection is reconfirmed
(`&mut a[secret_i]` → `IndexNotInt`).

## Consequences

- The mutable-array story is complete: an element can be read (`a[i]`), written (`a[i] = v`, ADR
  0050), and now borrowed (`&mut a[i]` / `&a[i]`).
- Whole-array borrow granularity over-rejects simultaneous distinct-element borrows (the swap
  idiom); acceptable pre-Polonius and documented.
- `IndexAssignNotSupported` is no longer constructed (its diagnostic arm is retained defensively);
  the variant may be removed in a later cleanup.
