# `std/` — Sentinel core libraries

Real, idiomatic Sentinel library modules — the building blocks a programmer
gets instead of starting from scratch. They are written in Sentinel itself, and
they double as feature tests: every module is exercised by a program under
[`../examples/`](../examples/) that is compiled, run, and asserted on by
`cargo nextest` (see [the harness](../crates/sentinel-driver/tests/examples.rs)).

## Layout: functional categories

`std/` is subdivided by **functional category** (each a directory), and a module
is a `.sentinel` file inside one. The module's path is its file path relative to
this corpus root:

```
std/
  security/ct.sentinel   →  module  std::security::ct   (constant-time primitives)
  math/num.sentinel       →  module  std::math::num      (min/max/abs/clamp)
  ...
```

A program imports an item with `use std::<category>::<module>::<Item>;`. Module
discovery roots at the entry file's parent directory and maps
`use a::b::Item` → `<root>/a/b.sentinel` (no parent traversal), so the test
harness assembles a buildable project by dropping this whole `std/` tree next to
the example entry. See the harness doc-comment for the mechanics.

## Categories (current + planned)

| Category    | Module(s)            | Status                                   |
| ----------- | -------------------- | ---------------------------------------- |
| `security`  | `ct`, `siphash`, `chacha20`, `poly1305`, `aead`, `sha256`, `sha512`, `sha3`, `hmac`, `aes`, `aes_gcm`, `fe25519`, `x25519`, `ed25519` | ✅ ct scalars + `ct_memcmp` over `[secret u8]` + `ct_vec_eq` over `Vec<secret u8>` + `ct_rotl64`/`ct_rotl32`/`ct_rotr32`/`ct_rotr64`; `siphash24` keyed MAC; `chacha20_block` + `chacha20_xor` stream cipher; `poly1305` one-time MAC; `chacha20poly1305_encrypt` AEAD (full RFC 8439 §2.8.2 vector); `sha256` constant-time SHA-256 + `sha512` constant-time SHA-512 + `sha3` constant-time SHA3-256/SHA3-512 + SHAKE128/SHAKE256 XOFs + KMAC128/KMAC256 keyed MACs (the Keccak sponge / SP 800-185) over a `secret` message; `hmac_sha256` over a `secret` key; `aes128_encrypt_block` constant-time AES-128 (field-inversion S-box, no table); `aes128_gcm_encrypt` constant-time AES-128-GCM AEAD (GHASH GF(2^128)); `fe25519` the shared GF(2^255-19) field; `x25519` constant-time X25519 ECDH over a `secret` scalar (RFC 7748); `ed25519` constant-time Ed25519 SIGNING + verification over a `secret` seed (RFC 8032, composes fe25519 + sha512; verify decompresses a point via a field square root) |
| `math`      | `num`                | ✅ min/max/abs/clamp                       |
| `bits`      | `bits`               | ✅ `rotl64`/`rotr64`/`rotl32`/`rotr32`     |
| `bytes`     | `bytes`              | ✅ `eq`/`find`/`contains`/`count`/`starts_with`/`repeat` (over `&[u8]`) |
| `algorithms`| `seq`                | ✅ in-place `sort` (index-assign) + `binary_search` (over public `[i64]`) |

The list grows as examples force new building blocks.

## The point: find + fix language gaps

A real, idiomatic library hits the language's missing pieces, and finding them is
the most valuable output of this corpus.

- **Fixed — `[secret T]` arrays** (ADR 0047). `ArrayElem` gained a secret form, so
  a variable-length constant-time `memcmp` over a buffer of *secret* bytes
  (`security/ct::ct_memcmp` over `[secret u8]`) is now expressible. Surfaced +
  closed by this corpus (a front-end-only, byte-identical change).
- **Fixed — shift operators `<<` / `>>`** (ADR 0048). Logical right shift, with a
  constant-time rule (a shift by a *secret* amount is rejected like a secret
  divisor; a secret value shifted by a public amount is constant-time). Unblocks
  the `bits` rotate library and the SipHash-style ARX round
  (`examples/security/siphash_round`). (Phase 1 lands shifts in `snc`; the
  self-hosted mirror is Phase 2.)
- **Fixed — the integer cast `x as T`** (ADR 0049). Closes the "i32 values are
  unconstructible" gap (an integer literal is `i64`): `x as i32` truncates / `x as
  i64` sign-extends, preserving secrecy and constant-time (no CT sink). Unblocks a
  true 32-bit **ChaCha quarter-round** over `secret i32` words
  (`examples/security/chacha_qr`, RFC 8439 vector) and makes the `bits` 32-bit
  rotates runnable.
- **Fixed — mutable index assignment `a[i] = v`** (ADR 0050). Lifts the ADR 0017
  D12 deferral so an array / `Vec` element can be written in place through a public
  index (the read path's bounds-checked element GEP + a store). Constant-time is
  preserved with no new sink — a *secret* LHS index is rejected like a secret
  read-index, while a *secret value* stored at a public index is allowed. Unblocks
  a full, idiomatic **ChaCha20 block** that permutes a `[secret i32]` state in place
  (`examples/security/chacha20_block`, RFC 8439 §2.3.2 vector).
- **Fixed — `Vec<secret T>` growable secret buffers** (ADR 0052, `VecElem::Secret`).
  The sibling of `[secret T]` on the `Vec` path: a *variable-length* secret byte
  buffer (`Vec<secret u8>`) can now be built up with `vec_new` + `push` and indexed
  to yield `secret u8` (public index → the constant-time taint; the buffer's
  pointer/length/capacity stay public, a secret index is rejected). Front-end-only
  like `[secret T]`, plus a secret-aware generic-substitution round-trip
  (`Vec<T>[T:=secret u8]`) since the `Vec` builtins are generic. Unblocks
  message-length-independent constant-time code — `security/ct::ct_vec_eq` over two
  growable secret buffers (`examples/security/ct_vec_eq`).
- **Fixed — `vec_to_array` over a secret `Vec`** (ADR 0053, `to_array_elem_subst`).
  The symmetric completion of ADR 0052: `Vec<secret u8> → [secret u8]`, so a buffer
  built up with `push` feeds the existing `[secret u8]`-taking crypto. The array
  substitution demote is made secret-aware (the twin of 0052's `to_vec_elem_subst`),
  with no codegen or `scg` change. Makes the full RFC 8439 §2.8.2 114-byte AEAD
  vector idiomatic (build the secret plaintext by `push`, then `vec_to_array`), and
  underpins the `sha256` schedule/padding.

All seven surfaced gaps are now closed and fully self-hosted (snc + `scg`,
byte-identical, both bootstrap fixed points held). When a gap blocks a genuinely
idiomatic library, that block is the signal to add the feature (ADR-first if it
touches the frozen `abi-v1` contract). On the now-rich surface the corpus also
grew (no language change): the full §2.8.2 AEAD vector, a constant-time **SHA-256**
over a secret message (`sha256`), **HMAC-SHA256** over a secret key (`hmac`), a
constant-time **AES-128** block cipher (`aes`), and constant-time **X25519** ECDH
over a secret scalar (`x25519`, RFC 7748 — the first public-key primitive). AES and
X25519 are the sharpest demonstrations of the constant-time guarantee yet, because in
each the textbook implementation has a key-dependent side channel that simply does not
compile: AES's table-lookup S-box (`sbox[secret_byte]`) is a secret value indexing
memory, and X25519's Montgomery ladder would branch on the secret scalar bits
(`if bit { swap }`). Sentinel rejects both and forces the constant-time form — AES's
arithmetic, table-free S-box (GF(2^8) inversion `x^254` + the affine map), and X25519's
branch-free, mask-based conditional swap.
