# ADR 0052: `Vec<secret T>` — growable buffers of secret elements (`VecElem::Secret`)

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A6) — `Vec<secret T>` (growable buffers of secret
elements) is representable; `snc` types/resolves/codegens it and the self-hosted `scg`
mirrors it byte-for-byte over the corpus with **no selfhost change**; both bootstrap fixed
points hold and the full nextest is green. Amendments below record the (minimal) deviations
from the PROPOSED plan; they confirm the central front-end-only / byte-identical hypothesis,
plus the one substantive difference from ADR 0047 (the generic substitution round-trip).

Makes a *growable vector whose elements are individually secret* — e.g. `Vec<secret u8>`,
a variable-length secret byte buffer — a representable type. This is the language gap the
crypto track surfaced repeatedly (`sentinel_examples_and_corelibs`): real, idiomatic
constant-time code over a *variable* number of secret bytes (a streaming cipher, a MAC over
a message of unknown length, the full RFC 8439 §2.8.2 AEAD vector) needs to *build up* a
secret buffer, and today the only secret containers are fixed-size: `[secret T]` array
literals (ADR 0047) plus in-place index-assign (ADR 0050). `Vec<secret T>` is rejected
(`TypeError::VecElementNotSupported`), and `[x; N]` array-repeat is unsupported, so a
variable-length secret buffer cannot be expressed.

This is the direct sibling of ADR 0047 (`[secret T]` arrays), explicitly listed there as
deferred ("`Vec<secret T>` (`VecElem::Secret`) … the same one-variant move, left out until
a library needs them"). A library now needs it.

## Problem

`secret T` is the constant-time qualifier: the type system is the taint oracle, and the MIR
`secret_leak` pass rejects any `secret` value reaching a branch, a memory index, or a
divisor. Secrecy composes with scalars (`secret u8`) and, since ADR 0047, with the **array
element** (`[secret u8]`). It does **not** compose with the **`Vec` element**: `VecElem`
(the flat, `Copy` subset of `Type` that can sit inside `Vec<T>`, mirroring `ArrayElem`) has
no secret form, so `Vec<secret u8>` is rejected at type resolution
(`Type::to_vec_elem(Type::Secret(_)) -> None -> TypeError::VecElementNotSupported`).

The consequence: a secret buffer whose length is not known at the call site cannot be
*accumulated*. You can compare two fixed `[secret u8]` literals (`ct_memcmp`, ADR 0047) or
permute a fixed `[secret i32]` state in place (`chacha20_block`, ADR 0050), but you cannot
`push` secret bytes onto a growing buffer and reduce over it.

What we want:

```sentinel
// Build a variable-length secret byte buffer, then reduce it constant-time.
fn main() -> i64 {
    let mut buf: Vec<secret u8> = vec_new();   // Vec<secret u8>
    let mut i: i64 = 0;
    while i < n {
        push(&mut buf, secret_byte(i));        // push secret u8 (public-byte arg widens)
        i = i + 1;
    }
    // buf[j] : secret u8  (public index j, secret element) — seeds the CT check
    ...
}
```

`buf[j]` must come back `secret u8` (so a reduction stays in the secret domain and the CT
check fires), while the *index* `j` and the *length* `len(buf)`, the data pointer, and the
capacity stay public (a `Vec`'s address/length/capacity are not secret — only its
contents).

## Decision

Represent a secret `Vec` element as **`VecElem::Secret(SecretId)`** — exactly as ADR 0047
added `ArrayElem::Secret(SecretId)`. The `SecretId` names the element's `secret T` via the
existing `TypedProgram.secrets` interner; it is a `Copy` `u32` wrapper, so `VecElem` stays
`Copy` (and `Type: Copy` with it). `VecElem` already mirrors `ArrayElem`'s flat subset
member-for-member; this keeps them in lock-step.

A `Vec<T>` lowers as `{ i64 len, i64 cap, ptr data }` with **no element-type information in
the emitted code** (the data pointer is opaque; element stride is computed by stripping
`secret` to the underlying scalar LLVM type, `secret u8 -> i8`, identically to `u8`). So,
as for `[secret T]`, the change is **front-end-only** and the `abi-v1` `Vec` contract is
**untouched**. Concretely, all in `crates/sentinel-types/src/lib.rs`:

1. `enum VecElem` gains `Secret(SecretId)`.
2. `VecElem::to_type` gains `Secret(id) => Type::Secret(id)`. Load-bearing: the `Index` arm
   reads the element back through `to_type()`, so `v[j]` on a `Vec<secret u8>` promotes to
   `secret u8` with no other typer change, seeding the MIR taint pass automatically.
3. A **guarded demote** `Type::to_vec_elem_secret(&[SecretData])` (mirroring
   `to_array_elem_secret`): a `Type::Secret(id)` becomes `VecElem::Secret(id)` **only if**
   `secrets[id].inner` is itself a flat scalar (`secrets[id].inner.to_vec_elem().is_some()`),
   so `Vec<secret [T]>` / `Vec<secret ?T>` stay rejected (the depth-1 no-nested-collection
   rule). Used at the **one** `Vec<T>` annotation-resolution site. The bare `to_vec_elem`
   keeps rejecting `Secret`, so it stays symmetric with `to_array_elem`.
4. The **Index arm** demotes a `VecElem` to an `ArrayElem` for the typed node (codegen keys
   the data-pointer field on the *target* type, Vec field 2 vs array field 1). For a
   `Vec<secret u8>` this must produce `ArrayElem::Secret`, so the demote there switches from
   the bare `to_array_elem` (which would return `None` and panic the existing `.expect`) to
   the guarded `to_array_elem_secret(secrets)`. Index-assign `buf[j] = v` (ADR 0050) then
   works for free — `check_mutable_lvalue` already admits `ArrayElem::Secret` as a Copy
   element.

### The one substantive difference from ADR 0047: the generic substitution round-trip

`[secret T]` was *purely* additive because no generic function is instantiated over `[T]`.
`Vec`, by contrast, is built by **generic builtins** — `vec_new<T>() -> Vec<T>`,
`push<T>(&mut Vec<T>, T)`, `pop`, `len`, `vec_to_array`. So `let mut buf: Vec<secret u8> =
vec_new();` runs `vec_new`'s return type through generic substitution:
`substitute(Vec<T>, [T := secret u8])`. The substitution demote (`Type::substitute`,
`try_substitute`, `VecElem::substitute`) uses the **bare** `to_vec_elem`, which rejects
`secret` — so it would fall back to `Vec<T>` (the unsubstituted type parameter), and the
`let`-annotation match would fail.

This is the one place `Vec<secret T>` needs more than 0047's array move. The fix is a
second demote, **`Type::to_vec_elem_subst(self)`**, used at the three substitution sites:
it admits `Type::Secret(id) => VecElem::Secret(id)` directly. It needs **no `secrets`
table** — the `SecretId` is carried inside `Type::Secret(id)`; the table is only needed to
*validate* scalar-ness, which the resolution-site guard already does. So we avoid threading
`secrets` through `substitute` (the ~50-site ripple ADR 0047 A2 deliberately sidestepped).

Soundness of the `secrets`-free `to_vec_elem_subst`: a `Vec<secret NON-scalar>` is
unreachable. Every *source* spelling (`Vec<secret [u8]>`, a `let: Vec<secret [u8]>`
annotation, a `Vec<secret [u8]>` parameter) is rejected at resolution by the guarded
`to_vec_elem_secret`. The only way a non-scalar secret could reach substitution is a
contrived user generic returning a `Vec` of its type parameter
(`fn g<T>(x: T) -> Vec<T>`) instantiated with a `secret [u8]` argument — which today already
produces a malformed `Vec<TypeParam>` and appears in no corpus; it is documented as deferred
below (the depth-1 rule, same class as `[secret [T]]`).

`len` is unaffected (it is special-cased in `check_call`, reading the element structurally
off `Type::Vec(ve) => ve.to_type()` — a `Vec<secret u8>` gives `secret u8`, length stays
`i64`). The public-byte → secret-element ergonomic comes for free: `push(&mut buf, 0x61)`
pushes a `secret u8` because `push`'s element parameter substitutes to `secret u8`, an
ADR 0051 `is_secret_widen_target`, so the public byte is widened by `coerce_to_expected`.

Everything else is **unchanged**, confirmed by reading each stage:

- **Parser** already parses `Vec<secret u8>` (the type-argument sub-parser is the same
  recursive `parse_type`).
- **Codegen** (inkwell + the `llvm_dump.rs` oracle): `vec_new`/`push`/`pop`/index compute
  stride through the element's LLVM type with `secret` stripped, so `Vec<secret u8>` emits
  identically to `Vec<u8>`. No layout/IR change.
- **CT check** (`sentinel-mir`): taint is read off `Type::Secret(_)`. `buf[j]` is now
  `secret u8`, so any branch/index/divisor consuming it is flagged automatically; the `Vec`
  *base* is `Type::Vec(..)`, **not** `Type::Secret(..)`, so the public pointer/length/cap
  are correctly not flagged. A secret *index* is still rejected (`IndexNotInt`). No pass
  change.
- **Borrow check**: tracks moves/borrows, not element secrecy. No change.
- **Selfhost (`scg`)**: the self-hosted compiler uses a structural hash-consed type interner
  (no `VecElem` enum) — `Vec<secret u8>` is already `mk_vec(mk_secret(u8))`, accepted and
  lowered, and its inference reads the element structurally (`vec_elem_of`), so it computes
  the same `Vec<secret u8>` result type with no substitution-demote to trip on. It is
  silently already correct (exactly as it was for `[secret u8]` in ADR 0047 A4). **No
  selfhost change** — the corpus differential proves it.

## Byte-identity (the gate)

The change is purely additive: no secret-free program can construct a `VecElem::Secret`, so
the `Type`/`TypedProgram`/emitted-IR of all existing pass fixtures is identical, and both
bootstrap fixed points + the byte-for-byte selfhost differentials stay byte-identical. No
existing `.sentinel` fixture uses `Vec<secret T>` (it was unrepresentable).

To validate `snc`↔`scg` parity on the new construct, a **`tests/pass/c57_secret_vec`
fixture** exercising `Vec<secret u8>` (vec_new + push + index + a secret reduction) is added
— the load-bearing differential check that both backends emit it byte-identically (the
`examples/` corpus is not part of the selfhost differential). The full nextest is the gate.

## Scope / Deferred

MVP = `Vec<secret SCALAR>` (`Vec<secret u8>`, `Vec<secret i64>`, `Vec<secret bool>`):
`vec_new`, `push`, indexed read `buf[j] : secret u8`, indexed write `buf[j] = v` (ADR 0050),
`len`, `pop`. Deferred, each its own follow-up:

- **`vec_to_array` over a secret `Vec`** (`Vec<secret u8> -> [secret u8]`). The bridge's
  result type runs through the **array** substitution demote (`Type::substitute`'s `Array`
  arm), which ADR 0047 left on the bare `to_array_elem` (it rejects `secret`). Making that
  arm secret-aware is the symmetric move (a `to_array_elem_subst`), deferred to keep 0052
  Vec-focused; until then, a secret `Vec` is consumed by direct indexing / a `&Vec` borrow,
  not the array bridge.
- **Whole-`Vec` secret widen** `Vec<u8> -> Vec<secret u8>` (a `VecElem::Secret` arm in
  `is_secret_widen_target` / `coerce_to_expected`). An ADR 0051-style ergonomic; the
  per-element push widen already covers the common "push public bytes into a secret buffer"
  case, so the whole-`Vec` widen is left out.
- **`Vec<secret [T]>` / `Vec<secret ?T>`** — nested collections stay deferred (the depth-1
  rule); the resolution guard keeps them rejected, and the substitution soundness argument
  above keeps the contrived `g<T> -> Vec<T>` instantiation out of scope.
- **Secret index into a `Vec`** — a `secret i64` index is rejected today as `IndexNotInt`
  (the index type isn't exactly `i64`), exactly as for `[secret T]` arrays. The principled
  refinement (`strip_secret` the index so it reaches the `secret_leak` `MemoryIndex` sink)
  is the same one-line deferral ADR 0047 made. Either way the program is rejected (sound).

## Consequences

A programmer can build a variable-length constant-time secret buffer — the building block
the crypto track needs for streaming ciphers, message-length-independent MACs, and the full
RFC 8439 §2.8.2 AEAD vector. `std::security::ct` gains a constant-time equality over two
growable secret buffers (`ct_vec_eq` over `&Vec<secret u8>`), the variable-length sibling of
`ct_memcmp`, exercised by `examples/security/ct_vec_eq` — dogfooding modules + `--separate`
and the constant-time guarantee on real branch-free code over a built-up buffer.

## Amendments (as implemented)

The implementation tracked the plan; the entire compiler change is **one crate,
`sentinel-types`** (a new `VecElem` variant + its `to_type` arm + two demote helpers + four
one-line call-site swaps). No change to the parser, codegen (either backend), the CT check,
the borrow checker, or the self-hosted compiler.

- **A1 — representation, as proposed.** `VecElem::Secret(SecretId)` (Copy-preserving),
  `VecElem::to_type(Secret(id)) -> Type::Secret(id)`. Indexing `Vec<secret u8>` yields
  `secret u8` for free, seeding the existing MIR taint check. **Zero exhaustiveness breaks**
  across the whole workspace (`cargo build --workspace` clean) — every real path routes a
  `VecElem` through `.to_type()`, empirically confirming secrecy is a layout-free tag (the
  ADR 0047 A3 result, replayed for `Vec`).

- **A2 — the substitution round-trip: the one substantive difference from ADR 0047.** Because
  `Vec` is built by **generic builtins** (`vec_new<T>() -> Vec<T>`), `vec_new`'s result type
  is `substitute(Vec<T>, [T := secret u8])`. The substitution demote uses the **bare**
  `to_vec_elem` (which rejects `secret`), so it fell back to `Vec<T>` and the
  `let`-annotation match failed. Fixed with a second demote, **`Type::to_vec_elem_subst`**
  (`secrets`-free — the `SecretId` rides inside `Type::Secret`), used at the three
  substitution sites (`Type::substitute`, `try_substitute`, `VecElem::substitute`). This
  avoids threading `secrets` through `substitute` (the ~50-site ripple ADR 0047 A2
  sidestepped) while keeping the bare `to_vec_elem` symmetric with `to_array_elem` (both
  still reject `secret`). `[secret T]` never needed this — no generic builtin is instantiated
  over `[T]`.

- **A3 — the Index arm needed the secret-aware demote too.** A `v[i]` typed node stores the
  element as an `ArrayElem` (codegen keys the data-pointer field on the *target* type). For a
  `Vec<secret u8>` the demote must produce `ArrayElem::Secret`, so it switched from the bare
  `to_array_elem` (which returns `None` for `secret` → the existing `.expect` panics) to the
  guarded `to_array_elem_secret(secrets)`. Index-assign `buf[i] = v` (ADR 0050) then works
  with **no further change** — `check_mutable_lvalue` already admits `ArrayElem::Secret` as a
  Copy element.

- **A4 — no selfhost change; the corpus differential proves it.** `scg`'s structural type
  interner already represents `Vec<secret u8>` as `mk_vec(mk_secret(u8))`, accepts and lowers
  it, and infers `vec_new`'s element structurally (`vec_elem_of`) — so it computes the same
  `Vec<secret u8>` result type with no substitution-demote to trip on. The new
  `tests/pass/c57_secret_vec` fixture compiles through BOTH the `snc llvm` oracle and `scg`
  **byte-for-byte identically** across all 8 stage differentials (parse / resolve / types /
  mir / ctverifier / codegen), and both bootstrap fixed points hold.

- **A5 — the fixture routes around an unmirrored `secret u8` call-let-widen.** A first cut of
  the fixture built each secret byte with `let sb: secret u8 = i64_to_u8(i)`. That diverged:
  `snc` wraps the public `u8` call result in `(widen-secret … :secret u8)`, but `scg` does
  **not** apply the public→secret let-widen when the RHS is a **call** (it does mirror the
  widen for an int/char literal — c56 / c53 — and for a `u8` *variable* — c53's
  `let acc: secret u8 = zero`). This is a pre-existing `scg` gap in the ADR 0019/0051 let-widen
  (a call-RHS public→secret coercion), **orthogonal to `Vec<secret T>`** and never before
  exercised by a fixture. The fixture therefore binds the public byte to a `u8` variable and
  widens the *variable* (`let p: u8 = i64_to_u8(i); let sb: secret u8 = p;`), which `scg`
  mirrors — keeping the differential byte-identical without a `scg` change. (The
  `examples/security/ct_vec_eq` program, not part of the differential, keeps the nicer
  ergonomic: `push(&mut v, i64_to_u8(base + j))` widens the public byte at the push argument
  via the ADR 0051 *call-arg* widen.) Recorded as a follow-up: mirror the call-RHS
  public→secret let/coerce widen into `scg`.

- **A6 — the resolution guard is load-bearing because `secret [u8]` IS representable.** Unlike
  the hoped-for symmetry, a `secret` *can* wrap a non-scalar: `secret [u8]` interns as
  `Type::Secret(secret_id_for_[u8])` and type-checks (verified). So `to_vec_elem_secret`'s
  scalar guard is what rejects `Vec<secret [u8]>` (its `secrets[id].inner = [u8]` does not
  demote → `VecElementNotSupported`), holding the depth-1 rule. The `secrets`-free
  `to_vec_elem_subst` stays sound because every *source* spelling of `Vec<secret [u8]>` is
  blocked at resolution, so a `secret` reaching substitution wraps a scalar; the only escape
  (a contrived `fn g<T>(x: T) -> Vec<T>` instantiated with a `secret [u8]` argument) is
  pre-existingly malformed and deferred.

- **A7 — surface ergonomics confirmed for the corpus (not part of this ADR).** Cross-module
  `pub fn` over `Vec<secret u8>` crosses a `--separate` boundary fine (the example builds both
  ways). `len(Vec<secret u8>)` is `i64` (length public); a secret index is rejected
  `IndexNotInt`; a `Vec<secret u8>` element reaching a branch is rejected (the CT check fires)
  — all verified by negative probes and the example compiling at all.
