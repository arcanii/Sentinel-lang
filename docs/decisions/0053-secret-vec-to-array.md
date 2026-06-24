# ADR 0053: `vec_to_array` over a secret `Vec` (`to_array_elem_subst`)

Status: **ACCEPTED-WITH-AMENDMENTS** (A1–A4) — `vec_to_array` over a `Vec<secret T>` produces
`[secret T]`; `snc` types/lowers it and the self-hosted `scg` mirrors it byte-for-byte over
the corpus with **no selfhost change**; both bootstrap fixed points hold and the full nextest
is green.

Makes `vec_to_array(v)` over a `Vec<secret T>` produce a `[secret T]` — the
`Vec<secret u8> → [secret u8]` bridge. This is the symmetric completion of ADR 0052,
which legalized `Vec<secret T>` but explicitly deferred this one item ("`vec_to_array` over a
secret `Vec` … needs the ARRAY substitution demote made secret-aware too").

## Problem

ADR 0052 added `Vec<secret u8>` (build a variable-length secret buffer with `vec_new` +
`push`). But the natural next step — hand that buffer to existing `[secret u8]`-taking code
(`chacha20_xor`, `poly1305`, `ct_memcmp`) — needs the `Vec → [T]` bridge `vec_to_array`. Today
`vec_to_array(Vec<secret u8>)` is rejected:

```
× type mismatch: expected [secret u8], found [<T>]
  let a: [secret u8] = vec_to_array(v);
```

`vec_to_array<T>(v: Vec<T>) -> [T]` is a generic builtin, so its result type is
`substitute([T], [T := secret u8])`. The **array** substitution demote (`Type::substitute`'s
`Array` arm, `try_substitute`, `ArrayElem::substitute`) uses the **bare** `to_array_elem`,
which rejects `secret` — so the result falls back to `[T]` (the unsubstituted type parameter),
and the `[secret u8]` annotation match fails. This is the exact bug ADR 0052 fixed on the
`Vec` path with `to_vec_elem_subst`; the `Array` path was left for this ADR.

## Decision

Add **`Type::to_array_elem_subst`** — the array twin of ADR 0052's `to_vec_elem_subst`. Like
the bare `to_array_elem` but also admits a substituted `secret SCALAR`
(`Type::Secret(id) => ArrayElem::Secret(id)`), used at the three array substitution sites
(`Type::substitute`'s `Array` arm, `try_substitute`'s `Array` arm, `ArrayElem::substitute`).
It needs **no `secrets` table** — the `SecretId` rides inside `Type::Secret(id)` (the table is
only needed to *validate* scalar-ness, which the array-element **resolution** guard
`to_array_elem_secret` already does, ADR 0047). The bare `to_array_elem` stays conservative
(unchanged); the `[u8] → [secret u8]` widen's expected-decomposition (`coerce_to_expected`,
ADR 0051) keeps using the bare demote.

Soundness, exactly as ADR 0052 A6: a `[secret NON-scalar]` is unreachable on this path. Every
source spelling (`[secret [u8]]` etc.) is rejected at resolution by `to_array_elem_secret`'s
scalar guard, and `vec_to_array` over a `Vec<secret [u8]>` can't arise (`Vec<secret [u8]>` is
itself rejected at resolution). A `secret` reaching array substitution therefore wraps a
scalar. (The only escape — a contrived `fn g<T>(x: T) -> [T]` instantiated with a `secret [u8]`
argument — is pre-existingly malformed and stays deferred, the depth-1 rule.)

**No codegen change.** `lower_vec_to_array` computes the element stride through
`llvm_basic_type(elem_ty)`, which strips `secret` to the underlying scalar (`secret u8 -> i8`),
so a secret `Vec → [T]` copy emits identically to a public one. **No CT-check / borrow change.**

## Byte-identity (the gate)

Purely additive: the array substitution demote behaves identically for every existing program
(it only differs when a TypeParam binds to a `secret` during array substitution — which no
corpus program does, since `vec_to_array`-over-secret was unrepresentable). Both bootstrap
fixed points + the selfhost differentials stay byte-identical. A `tests/pass/c58_secret_vec_to_array`
fixture exercises `Vec<secret u8> → [secret u8]` to prove `scg` == `snc` byte-for-byte (the
self-hosted `scg`'s structural interner already lowers it — no selfhost change expected, the
fixture is the gate).

## Consequences

A secret buffer built with `Vec<secret u8>` (ADR 0052) can be converted to `[secret u8]` and
fed to the existing constant-time crypto. The headline use: the full RFC 8439 §2.8.2 AEAD
vector, whose 114-byte secret plaintext is now built by `push` into a `Vec<secret u8>` and
`vec_to_array`'d into the `[secret u8]` `chacha20_xor` consumes — instead of a 114-element
literal (`examples/security/chacha20poly1305_full`).

## Amendments (as implemented)

- **A1 — implemented as proposed.** `Type::to_array_elem_subst` (the array twin of
  `to_vec_elem_subst`) admits `Type::Secret(id) => ArrayElem::Secret(id)`, wired at the three
  array substitution sites (`ArrayElem::substitute`, `Type::substitute`'s `Array` arm,
  `try_substitute`'s `Array` arm). The bare `to_array_elem` and the `coerce_to_expected`
  `[u8] → [secret u8]` widen decomposition are unchanged. `vec_to_array(Vec<secret u8>)` now
  types as `[secret u8]` (was `expected [secret u8], found [<T>]`). One crate
  (`sentinel-types`); `vec_to_array` is NOT special-cased in `check_call` (unlike `len`) — it
  flows the uniform generic path, so its return type `[T]` substitutes correctly once the
  demote is secret-aware.

- **A2 — no codegen / CT / borrow / selfhost change.** `lower_vec_to_array` computes element
  stride through `llvm_basic_type` (strips `secret` to the scalar), so a secret `Vec → [T]`
  copy emits identically. `scg` reads the `Vec` element structurally (`vec_elem_of`) and lowers
  `Vec<secret u8> → [secret u8]` already; `tests/pass/c58_secret_vec_to_array` is byte-for-byte
  identical across all 8 stage differentials, both fixed points hold.

- **A3 — byte-identity confirmed.** Making the array substitution demote secret-aware only
  changes behavior when a TypeParam binds to a `secret` in array position — which no existing
  corpus program does (`vec_to_array`-over-secret was unrepresentable). The full selfhost
  corpus differential is byte-identical, so the broadly-used array substitution path is
  untouched for every prior program.

- **A4 — the §2.8.2 payoff.** `examples/security/chacha20poly1305_full` reproduces the COMPLETE
  RFC 8439 §2.8.2 AEAD vector — a 114-byte plaintext + 12-byte AAD — with the secret plaintext
  built by `push` into a `Vec<secret u8>` and bridged to the `[secret u8]` `chacha20_xor`
  consumes via `vec_to_array`. Both the 114-byte ciphertext and the 16-byte tag match the
  published vector byte-for-byte (verified against a Python reference), in both the merge and
  `--separate` builds. Before ADR 0052 + 0053 this needed a 114-element literal of pre-bound
  secret bytes; now it is a loop.
