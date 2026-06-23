# ADR 0047: `[secret T]` — arrays of secret elements (`ArrayElem::Secret`)

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A6) — `[secret T]` (arrays of secret elements) is
representable; `snc` types/resolves/codegens it and the self-hosted `scg` mirrors it
byte-for-byte over the corpus with **no selfhost change**; both bootstrap fixed points hold
and the full nextest is green. Amendments below record the (minimal) deviations from the
PROPOSED plan; they confirm the central front-end-only / byte-identical hypothesis.

Makes an *array whose elements are individually secret* — e.g. `[secret u8]`, a secret
byte buffer — a representable type. This is the language gap surfaced by the
examples-as-tests + core-library track (`sentinel_examples_and_corelibs`): a real,
idiomatic **constant-time `memcmp` over secret bytes** (the flagship `std::security::ct`
primitive) needs to index a buffer of secret bytes with a *public* loop counter and have
each element come back `secret`. It was flagged "not yet representable" since C3.1
(ADR 0019 D5) and called out in the README's deferred list and `to_array_elem`'s own
comment.

## Problem

`secret T` is the constant-time qualifier: the type system is the taint oracle, and the
MIR `secret_leak` pass rejects any `secret` value reaching a branch, a memory index, or a
divisor. Today secrecy composes with **scalars** (`secret i64`, `secret u8`) but not with
the **array element**: `ArrayElem` (the flat, `Copy` subset of `Type` that can sit inside
`[T]`) has no secret form, so `[secret u8]` is rejected at type resolution
(`Type::to_array_elem(Type::Secret(_)) -> None -> TypeError::NestedArray`).

The consequence for real crypto code: a constant-time tag/MAC compare over a *variable
number* of secret bytes cannot be written. Fixed-width compares over a handful of `secret`
*scalars* work (see `std::security::ct::ct_diff` + `examples/security/secure_compare`), but
the moment the secret data lives in a buffer you must index it, and indexing a `[u8]`
yields a *public* `u8` — losing the secret tag, so the compiler can no longer prove the
reduction is branch-free.

What we want:

```sentinel
// a, b : [secret u8]; n public. 0 iff equal, constant-time (public index, no early-out).
fn ct_memcmp(a: [secret u8], b: [secret u8], n: i64) -> secret u8 {
    let mut acc: secret u8 = a[0] ^ b[0];   // a[i], b[i] : secret u8
    let mut i: i64 = 1;
    while i < n {
        acc = acc | (a[i] ^ b[i]);           // public index i, secret elements
        i = i + 1;
    }
    acc                                      // caller declassifies + compares == 0
}
```

`a[i]` must come back `secret u8` (so the XOR/OR reduction stays in the secret domain and
the CT check fires), while the *index* `i` and the *length* `n`/`len(a)` stay public (the
buffer's address and length are not secret — only its contents).

## Decision

Represent a secret array element as **`ArrayElem::Secret(SecretId)`** — the `SecretId`
already names the full `secret T` via the existing `TypedProgram.secrets` interner, exactly
as `Type::Secret(SecretId)` does for scalars, and as `ArrayElem::Struct(StructId)` stores
an id rather than a payload.

Why `SecretId` and not `Box<...>`: `ArrayElem` is `Copy`, and `Type: Copy` depends on it
(the invariant every type-representation ADR since C1.5 has preserved by storing interner
ids, never recursive boxes). `SecretId` is a `Copy` `u32` wrapper, so the variant keeps
`ArrayElem: Copy`. This is precisely why `[secret T]` is feasible where `[?T]` / `[[T]]`
were deferred (those need mutual recursion / a box).

The change is **front-end-only and additive**. Concretely (all in
`crates/sentinel-types/src/lib.rs`):

1. `enum ArrayElem` gains `Secret(SecretId)`.
2. `ArrayElem::to_type` gains `Secret(id) => Type::Secret(id)`. This is the load-bearing
   line: `Index` returns `elem_ty.to_type()`, so `s[i]` on a `[secret u8]` promotes the
   element back to `Type::Secret(sid)` = `secret u8` with **no other typer change**, which
   in turn seeds the MIR taint pass automatically (the type *is* the taint).
3. A **guarded demote** (a small helper, not a change to the bare `to_array_elem`):
   `Type::Secret(id)` becomes `ArrayElem::Secret(id)` **only if** `secrets[id].inner` is
   itself a flat scalar element (`secrets[id].inner.to_array_elem().is_some()`). Applied at
   the sites that resolve an array's element type (the `[T]` annotation arm and the two
   array-literal demotes). The bare `to_array_elem` keeps rejecting `Secret` (so the
   generic-substitution round-trip is unaffected, and any other caller stays conservative).
4. Exhaustiveness: any remaining `match` on `ArrayElem` gains a `Secret` arm (build-driven;
   `cargo build` flags each). Real ones route through `.to_type()`.

Everything else is **unchanged**, confirmed by reading each stage:

- **Parser** already parses `[secret u8]` into `Array(Secret(u8))` (the element sub-parser
  is the same recursive `parse_type`; `[secret secret u8]` still errors `DoubleSecret`).
- **Index typing** is free (point 2). The index-must-be-`i64` check is unchanged, so the
  flagship's *public* index works; a *secret* index (`secret i64`) is still rejected (today
  as `IndexNotInt`) — see "Deferred".
- **Codegen** (both the inkwell backend and the `llvm_dump.rs` oracle): a `[secret T]` is
  the same `{ i64 len, ptr data }` as `[T]`; element stride is computed through
  `elem_ty.to_type()` then a secret-stripping LLVM-type funnel, so `secret u8 -> i8`
  identically to `u8`. **No layout/IR change** — the `abi-v1` array contract is untouched.
- **CT check** (`sentinel-mir`): taint is read off `Type::Secret(_)` only. `s[i]`'s result
  type is now `secret u8`, so any branch/index/divisor consuming it is flagged
  automatically; the array *base* is `Type::Array(ArrayElem::Secret(..))`, **not**
  `Type::Secret(_)`, so the public base pointer is correctly *not* flagged as an address
  leak. A secret *index* is still flagged `MemoryIndex`. No pass change.
- **Selfhost (`scg`)**: the self-hosted compiler uses a structural hash-consed type
  interner (no `ArrayElem` enum), so `[secret u8]` is already `mk_array(mk_secret(u8))` —
  it already *accepts and lowers* secret arrays (it was silently more permissive than the
  Rust oracle, harmlessly, because no fixture exercised it). **No selfhost change.**

## Byte-identity (the gate)

The change is purely additive: no secret-free program can construct an `ArrayElem::Secret`,
so the `Type`/`TypedProgram`/emitted-IR of all 123 pass fixtures is identical, and both
bootstrap fixed points + the byte-for-byte selfhost differentials stay byte-identical. No
existing `.sentinel` fixture uses `[secret T]` (it was unrepresentable). The full nextest
is the gate.

To actually *validate* snc↔scg parity on the new construct (the `examples/` corpus is not
part of the selfhost differential), a **`tests/pass/` fixture exercising `[secret u8]`** is
added — the load-bearing differential check that both backends emit it byte-identically.

## Scope / Deferred

MVP = `[secret SCALAR]` (`[secret u8]`, `[secret i64]`, `[secret bool]`). Deferred, each
needing its own follow-up:

- **Secret index via the CT check.** A `secret i64` index is rejected today as
  `IndexNotInt` (the index type isn't exactly `i64`). The principled behavior — `strip_secret`
  the index for the `i64` check so it reaches the `secret_leak` `MemoryIndex` sink (the
  documented oracle for index leaks) — is a one-line typer refinement, deferred to keep this
  change minimal and byte-identity-trivial. Either way the program is rejected (sound).
- **`Vec<secret T>`** (`VecElem::Secret`) and **`?(secret T)`** (`NullableInner::Secret`) —
  the same one-variant move, left out until a library needs them.
- **`[secret [T]]` / `[secret ?T]`** — nested collections stay deferred (the depth-1 rule);
  the guard in point 3 keeps them rejected.
- **Array-level secret widen** `[u8] -> [secret u8]` (e.g. a secret key from a string
  literal). Not required for the flagship (a `[secret u8]` is built from a literal of secret
  bytes); a natural future ergonomic.

## Consequences

`std::security::ct` gains `ct_memcmp` over `[secret u8]`, and the flagship example
`examples/security/ct_memcmp` builds a variable-length constant-time secure compare — the
track's headline, dogfooding modules + `--separate` and the constant-time guarantee on
real branch-free code.

## Amendments (as implemented)

The implementation tracked the plan almost exactly; the entire compiler change is **one
crate, `sentinel-types`** (a new `ArrayElem` variant + its `to_type` arm + a guarded demote
helper + three one-line call-site swaps). No change to the parser, the typer's Index/array
machinery beyond the demote, codegen (either backend), the CT check, or the self-hosted
compiler.

- **A1 — representation, as proposed.** `ArrayElem::Secret(SecretId)` (Copy-preserving),
  `ArrayElem::to_type(Secret(id)) -> Type::Secret(id)`. Indexing `[secret u8]` yields
  `secret u8` for free, seeding the existing MIR taint check.
- **A2 — the guard is a new method, not a signature change.** Rather than threading
  `secrets` into the bare `Type::to_array_elem` (which would ripple into the
  generic-substitution round-trip, where secrets don't compose today), the secret-element
  acceptance lives in a new `Type::to_array_elem_secret(&[SecretData])`. It admits a
  `Type::Secret(id)` element only if `secrets[id].inner.to_array_elem().is_some()` (the
  secret wraps a flat scalar), and is used at exactly the three array-element resolution
  sites (the `[T]` annotation arm + the empty/non-empty array-literal demotes). The bare
  `to_array_elem` is unchanged, so substitution stays conservative.
- **A3 — zero exhaustiveness breaks.** Adding the variant broke **no** `match` anywhere in
  the workspace: every real code path routes an `ArrayElem` through `.to_type()`, and the
  only `matches!` on raw variants (the codegen arena-routing optimization gate) degrades
  gracefully (a `[secret u8]` literal falls back to libc malloc rather than the bump arena —
  sound, a noted perf follow-up). This empirically confirms secrecy is a layout-free tag.
- **A4 — no selfhost change; the corpus differential proves it.** The self-hosted `scg`
  uses a structural type interner that already represented `[secret u8]`
  (`mk_array(mk_secret(u8))`), so `sentinel_codegen_matches_oracle_on_corpus` compiles the
  new `tests/pass/c53_secret_array_memcmp` fixture through BOTH the `snc llvm` oracle and
  `scg` and finds them **byte-for-byte identical** — and the typer corpus differential
  matches too (secret intern order is consistent across the two). Both bootstrap fixed
  points hold.
- **A5 — the two rejections hold (verified by negative probes).** A secret index
  (`a[secret i64]`) is rejected as `IndexNotInt` (the deferred CT-check-based refinement is
  unchanged — still sound), and `[secret [u8]]` is rejected as `NestedArray` (the A2 guard
  works). The flagship and corpus fixture use a **public** loop index, so neither path is
  touched.
- **A6 — surface ergonomics (not part of this ADR, noted for the track).** Building secret
  values for an array hits two existing rough edges: mixed secret/public arithmetic is
  rejected (a constant must be bound `secret` before combining — e.g.
  `let ones: secret i64 = 0 - 1;`), and a plain integer literal does not coerce to `u8`
  (use `i64_to_u8(0)`). Both are candidates for future ergonomic ADRs (e.g. a public→secret
  operand widen, an array-level `[u8] -> [secret u8]` widen).
