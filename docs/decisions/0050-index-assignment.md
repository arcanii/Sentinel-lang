# ADR 0050: Mutable index assignment `a[i] = v`

Status: **PROPOSED** — lifts the ADR 0017 D12 deferral so an array / `Vec` element can be
written through an index. Phase 1 implements it in `snc` (the inkwell backend + the `snc
llvm` textual oracle), validated by an idiomatic, loop-based **ChaCha20 block** over a
`[secret i32]` state reproducing the RFC 8439 §2.3.2 test vector. Phase 2 mirrors it into the
self-hosted `scg` (`selfhost/types.sentinel`) and adds a corpus fixture so the differential
validates `scg == snc` byte-for-byte. Amendments (A1…) will record deviations from this plan.

Adds the assignment form `a[i] = v;` — writing the element at a public index `i` of a mutable
array `[T]` or vector `Vec<T>`. This closes the "array elements cannot be re-assigned" gap
that ADR 0017 D12 explicitly deferred (`TypeError::IndexAssignNotSupported`), surfaced by the
examples-as-tests track: a ChaCha20 block permutes a 16-word state *in place*
(`state[a] = state[a] + state[b]`, …), which is unexpressible when an array can only be
rebuilt by a whole new `let`.

## Decision

### Why now, and why an lvalue form (not a builtin)

The natural next example after the shipped ChaCha quarter-round (ADR 0049) is a full ChaCha20
block. A block applies the quarter-round to columns and diagonals of a 16-word state and feeds
the original state back — fundamentally an *in-place mutation of array state in a loop*. The
only idiomatic way to express that is `state[i] = …`. Every other in-place array algorithm
(sorts, buffers, sponge/permutation states) needs the same primitive, so this is a broadly
useful gap, not a one-off.

It is implemented by allowing `Index` as an assignment **lvalue**, not by adding a `set(a, i,
v)` builtin. A builtin would shift every user-fn `FnId` (builtins occupy a contiguous prefix
and FnIds print *by number* in the resolve / MIR / borrow / effects dumps), churning ~30 dump
assertions and the selfhost base constants — the same reason ADR 0048/0049 chose an operator /
expression node over builtins. The assignment statement, the `Index` expression, and the
lvalue machinery already exist; this ADR only *removes a rejection* and adds the element-store
lowering. The parser is already capable: `a[i] = v` already parses to `Assign { target:
Index{..}, value }` — all lvalue validation was deferred to the type checker.

### Semantics

`a[i] = v;` evaluates `v`, computes the address of element `i` of the collection `a` (the same
bounds-checked GEP the read path `a[i]` uses — a runtime `0 <= i < len` check that calls
`sentinel_panic_oob` on violation), and stores `v` there. It is a statement, evaluating to
nothing (like every other assignment). The base `a` must be a **mutable lvalue** (`let mut`, a
`mut` parameter, or a chain of field accesses / derefs ending in one) — checked exactly like
`a.f = v` by recursing on the base. The index `i` and the value `v` follow the existing
`Index`-read and assignment rules:

- **Index type.** `i` must be a public `i64`. A non-`i64` (including a `secret i64`) index is
  rejected by the existing `IndexNotInt` rule — the *same* gate the read path `a[i]` uses, so
  read and write are symmetric.
- **Value type.** `v` must unify with the element type. The assignment statement already
  type-checks the `Index` target first (yielding the element type), then type-checks `v` with
  that as the expected type and unifies — so storing a `secret i32` into a `[secret i32]`, an
  `i64` into a `[i64]`, etc. all work with no new code.
- **Value move/copy.** For a Copy element (a scalar or a `secret` scalar) the store is a plain
  overwrite. (Move elements are out of scope — see MVP below.)

### Constant-time

The constant-time discipline is **preserved with no new sink and no MIR change**:

- A **secret index** on the LHS is rejected. `a[secret] = v` fails type-checking via
  `IndexNotInt` (indices must be public `i64`), exactly as `let x = a[secret]` does today — so
  the write *address* can never depend on a secret, and the access pattern of an index-assign
  never leaks. This is the read rule applied unchanged to the write.
- A **secret value** stored is *not* a leak and is allowed: `a[i] = secret_word` (public `i`).
  Storing a secret into memory does not reveal it through timing; a subsequent read `a[j]`
  re-derives `secret` from the element type. The MIR lowering of an index-assign records the
  stored value as flowing into opaque memory (`MirOp::Opaque`, the existing field/deref-store
  behaviour) so taint is not lost; the index/base of the *target* are not a D5 sink because the
  type system already guarantees the index is public.

Consequently the `sentinel::mir::secret_leak` pass is unchanged: there is no new `SinkKind`,
and the secret-index rejection lives where the read's does (the type checker). A ChaCha20
block over a `[secret i32]` state therefore compiles only if every store uses a public index —
which a correct block does — and that successful build is itself the constant-time proof.

### ABI / additive

This is **purely additive** to the frozen `abi-v1` contract. No existing program emits an
index-assign, so no existing IR, symbol mangling, or calling convention changes; both the
merge and `--separate` paths and both bootstrap fixed points stay byte-identical. The new
lowering is the read path's element-address computation followed by a `store` instead of a
`load`. The corpus differential (`scg == snc llvm`) is unaffected until a fixture using the
construct is added (Phase 2), at which point both sides emit it identically.

### Stages touched

- **Parser / AST** — none. `a[i] = v` already parses to `Assign { target: Index{..}, value }`.
- **Types** (`sentinel-types`) — lift the `IndexAssignNotSupported` rejection in
  `check_mutable_lvalue`: the `Index` arm recurses on the base (mirroring the `FieldAccess`
  arm) to enforce base mutability, and gates the element type to Copy (a new
  `IndexAssignNonCopyElem` error for `Struct` / generic elements — the Move case). The value /
  index / element-type checks already fall out of the existing `Assign` statement logic.
- **Borrow check** (`sentinel-borrow-check`) — `walk_assign_target` gains an `Index` arm:
  walk the base as a write place (write-conflict on the base var, mirroring the field-assign
  recursion) and the index as a read. The RHS value follows the normal move/copy rules; a
  Copy scalar value is not consumed.
- **MIR / CT** (`sentinel-mir`) — none. An index-assign target lowers to `MirOp::Opaque(v)`
  (the existing non-Var store behaviour); the secret-index rejection is upstream in types.
- **Codegen — inkwell** (`sentinel-codegen`) — factor the read path's bounds-check + element
  GEP into a helper returning the element pointer; `lower_lvalue_ptr` gains an `Index` arm
  that calls it. The existing `Assign` lowering (eval value → `lower_lvalue_ptr` → store) then
  handles index-assign unchanged.
- **Codegen — `snc llvm` oracle** (`llvm_dump.rs`) — the textual mirror: `lower_lvalue_ptr`
  (or the assign handler) emits the same extractvalue/bounds-check/GEP sequence and stores.
- **Selfhost** (`selfhost/types.sentinel`, Phase 2) — mirror the assign handler's index-target
  path (compute the GEP element address, `cg_store` to it) so `scg` emits byte-identical IR;
  add `tests/pass/c55_index_assign` to drive all 8 selfhost stage differentials.

### MVP scope

- **Targets:** `[T]` arrays and `Vec<T>` (the read path already keys the data field on the
  type — field 1 for an array, field 2 for a `Vec`).
- **Elements:** Copy only — scalars (`i64`/`i32`/`u8`/`bool`) and `secret` scalars. This
  covers every constant-time / numeric use case (ChaCha is `secret i32`). A **Move** element
  (`Struct`, a generic `TypeParam` / `GenericInstance`) is rejected with `IndexAssignNonCopyElem`,
  deferring the drop-the-old-element semantics a Move overwrite would require. A later ADR can
  lift this once drop-on-overwrite is specified.
- **Base:** any mutable lvalue (`let mut` local, `mut` param, or a field/deref chain ending in
  one). `&mut [T]` element writes are *not* part of the MVP (no `&mut [T]` exists in the
  corpus); the ChaCha20 example mutates a local array directly.
- **Index:** public `i64`; bounds-checked at runtime (`sentinel_panic_oob`).

### Alternatives considered

- **A builtin `set(a, i, v)`** — rejected: FnId churn (above), and it reads worse than
  `a[i] = v`.
- **Rebuild-via-`let`** (the ADR 0017 D12 workaround) — rejected for in-place algorithms: a
  16-word ChaCha state rebuilt every store is O(n) per write and wholly unidiomatic.
- **16 unrolled scalars + a struct-returning quarter-round** (no language change) — a viable
  ChaCha20 block, but ~320 lines of mechanical write-back plumbing, and it leaves the gap open.
  Rejected in favour of fixing the gap (the gap-fix is the higher-value output per the track).
- **Including Move elements now** — deferred: requires specifying drop-on-overwrite, orthogonal
  to the constant-time / numeric use cases driving this.

## Consequences

- In-place array algorithms become expressible; the examples track can ship a real ChaCha20
  block (and future permutation-state crypto) idiomatically.
- The constant-time guarantee extends to writes *for free* (the read's index rule covers it),
  with no new sink to audit.
- A small, well-bounded surface (one rejection lifted + one codegen lowering per backend);
  no FnId shift, no new MIR op, no ABI change, both fixed points preserved.
- Deferred: Move-element index-assign (drop-on-overwrite), `&mut [T]` element writes, and
  `&mut a[i]` element borrows (the `check_mutable_borrow_target` site keeps erroring — out of
  this ADR's scope).
