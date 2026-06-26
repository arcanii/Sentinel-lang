# HANDOVER.md — Sentinel Bootstrap Compiler Implementation

This document is the practical handover for starting work on the Sentinel
bootstrap compiler in Rust. It assumes you have read SENTINEL_DESIGN.md
and SENTINEL_DESIGN2.md and have decided to proceed with the staged
validation approach described in Section 16.3 of the design document.

Read this top to bottom once before writing any code. Then use it as a
reference as you work through the milestones.

---

## 0. Current Implementation Status

> **The bootstrap fixed point is reached — the Sentinel compiler compiles
> itself — and the per-unit separate-compilation back end is functionally
> complete.** This section used to carry a full milestone-by-milestone running
> log; that chronology is archived in [`HISTORY.md`](HISTORY.md). For the
> authoritative current state read [`STATE.md`](STATE.md); this section is the
> concise handoff + resume pointer. Sections 1–12 below are the durable
> implementation plan + working norms.

**Done**

- **Phases A–C** complete; **Sentinel 1.0** closed (2026-05-30) with
  machine-verified constant-time `secret` (`sentinel::mir::secret_leak`).
- **Phase D self-hosts** (ADR 0031–0045). The language build-out — sum types +
  `match`, strings + `u8`, growable `Vec<T>`, file I/O, `while`/`break`/
  `continue`, modules/`use` — plus the full compiler port to
  `selfhost/*.sentinel`, validated byte-for-byte against the Rust `snc` oracle.
  Full-corpus codegen parity is reached (all 123 pass fixtures); the bootstrap
  fixed point holds via both the merge-to-source and self-hosted-merge paths.
  The Rust `snc` remains the bootstrap seed + differential oracle.
- **Per-unit separate compilation** (ADR 0037 (a)) is functionally complete:
  `snc build --separate` → per-unit objects + module-qualified `abi-v1`
  linking, with every `pub` item kind crossing a boundary (incl. cross-**unit**
  `perform`/`handle`), `linkonce_odr` generic dedup (primitives + cross-module
  structs + enums), and item-granular incremental caching (`.o.fp` sidecars).
  Opt-in until full parity with the merge path (`snc merge` / `snc build`, both
  still green). Full detail in the `sentinel_separate_compilation` auto-memory
  + ADR 0037.
- **ADR 0046** (the partial-move-through-field double-free, the one historical
  borrow-check *under*-rejection) closed in both `snc` and `scg`.
- **External-review plan** (`docs/REVIEW_ACTION_PLAN.md`, untracked): the P0
  band, P1, P2 (bar P2.4), P3.1, and P3.2 (this STATE/HANDOVER split) are done.
- **Constant-time check audited** against the post-Phase-D language: SOUND — a
  secret reaching an `if`/`while` condition, a secret `&&`/`||`, a secret
  array/Vec index, a secret `match` scrutinee, or a secret divisor is all
  rejected. Added the `while` conformance test (`c52_secret_in_while`) and made
  the `SecretBranch` diagnostic name the actual construct.

**▶ Resume at — the ACTIVE TRACK: examples-as-tests + core libraries (UNDERWAY —
NINE language gaps closed; a comprehensive constant-time crypto suite shipped; a
complete loopback SSH-2 transport SESSION — KEX + KDF + the chacha20-poly1305@openssh.com
record cipher — assembled from it, and the ADR 0056 sockets are now FULLY LIVE (runtime
primitives + compiler builtins + the `scg` mirror), with `examples/net/tcp_echo` running a
real loopback TCP echo on a concurrent `spawn`ed server task AND `examples/net/ssh_over_tcp`
running the whole SSH-2 handshake over a real socket (a distributed KEX between separate
client/server tasks); HEAD `aac69a8`, 1588 tests,
four-check green).** Real,
idiomatic Sentinel programs that double as feature tests + the first **core
libraries**. **Dogfoods modules + `--separate`**, **stress-tests the constant-time
guarantee on real code**, and surfaces concrete language gaps — finding + fixing those
is the most valuable output. The crypto suite is now a full stack, all constant-time +
on canonical vectors: **hashes** SHA-256 / SHA-512 / SHA-3 (SHA3-256/512); **XOFs**
SHAKE128/256; **MACs** HMAC-SHA256 + KMAC128/256; **SP 800-185 derived functions**
cSHAKE128/256 + TupleHash128/256 + ParallelHash128/256; **AEAD** ChaCha20-Poly1305 +
AES-128-GCM; **block cipher** AES-128 (table-free field-inversion S-box); **key
exchange** X25519; **signatures** Ed25519 (SIGN + VERIFY, the latter with point
decompression via a field sqrt + the `S < L` malleability check); and **KDF**
HKDF-SHA256 (RFC 5869, extract-then-expand composing HMAC); and **X448** ECDH + **Ed448**
signatures (RFC 7748 / RFC 8032) over the new shared `fe448` field (GF(2^448-2^224-1),
28 radix-2^16 limbs — radix 2^28 would overflow i64). The shared `fe25519`/`fe448`
fields underpin X25519/Ed25519 and X448/Ed448. The ninth language gap — **the numeric
gap**, ADR 0055's `u128` type (the 128-bit / radix-2^51 path) — is now closed too,
demonstrated by `fe25519_64` + `x25519_64` (X25519 at radix 2^51, the field multiply in
`secret u128`, cross-checked against the radix-2^16 X25519). All four owner-approved
options for that run (HKDF, SP 800-185, X448/Ed448, the numeric gap) are SHIPPED.

**Flagship direction — a secure network service.** The owner asked whether an `sshd`
or a webserver is the better showcase; `sshd` was chosen (it is crypto end-to-end, so
the constant-time/`secret` guarantee covers ~all of it, and the shipped suite already
IS an SSH cipher suite). Both are blocked on the same missing piece — **sockets** — so
the service is being built **loopback-first**. Two pieces landed: **ADR 0056** (a TCP
sockets runtime surface — seven file-I/O-style builtins, blocking + one-OS-thread-task-
per-connection mapping onto `scope`/`spawn`, error-returning, the socket as a public
declassify boundary; now IMPLEMENTED end to end) and **`std::net::ssh`** — the SSH-2
transport key exchange (`curve25519-sha256` + `ssh-ed25519`, RFC 4253 / RFC 8731): the
SSH wire codec, the exchange hash, the host-key signature, and the §7.2 KDF, all in the
secret domain, run loopback (`examples/net/ssh_handshake`, validated against a Python
model cross-checked vs paramiko + pyca). FINDING: SSH's `mpint` of the secret shared
secret has a value-dependent length (a ~1-bit side channel); Sentinel's type system
forces it into an explicit `declassify` (the audit point) — a leak other SSH stacks
make silently. The **record cipher** has now landed too — `std::net::ssh_cipher`, the
`chacha20-poly1305@openssh.com` binary-packet AEAD (seal / open / tag-verify) that
protects every packet after NEWKEYS, composing the shipped ChaCha20 + Poly1305
(`examples/net/ssh_channel`: a sealed packet matches a pyca-anchored model, the tag
verifies, the payload round-trips, a tampered record is rejected). The three pieces are
stitched into ONE end-to-end loopback session in `examples/net/ssh_session` (KEX →
host-key auth → key derivation → an encrypted application packet; `ssh_kdf` gained a
second SHA-256 block for the 64-byte key). So the loopback SSH transport is complete
end to end. Toward running it over a REAL connection, ADR 0056's sockets are now LIVE
end to end: the runtime layer (`sentinel-runtime`: the seven libc TCP primitives, with a
Rust loopback-echo test, `b90a889`), the compiler builtins (`tcp_listen`/`local_port`/
`accept`/`connect`/`read`/`write`/`close` at FnId 14..=20, mirroring the file-I/O
builtins — resolve/types/codegen + the `scg` builtin-table bump, `0845b8a`), and a real
Sentinel program over them — `examples/net/tcp_echo` (`669665a`): bind 127.0.0.1:0,
`spawn` a concurrent server task, connect, send, read the echo back, exit 42. The
FnId-shift crux is closed (builtins 0..=20 ⇒ user fns start at #21; byte-exact, both
bootstrap fixed points hold). AND the SSH transport now runs OVER a real socket —
`examples/net/ssh_over_tcp` (`aac69a8`): the client and server are separate concurrent
tasks doing a DISTRIBUTED curve25519-sha256 KEX (each derives the shared secret K
independently), host-key auth, the §7.2 KDF, and an encrypted record across the wire; the
wire-public values are `declassify`d to send and widened back to `[secret u8]` on receipt
(K never crosses the socket), so the loopback `sshd` transport is now real-socket end to
end. The cleanly-open next
steps are in **Next**
below,
with array-repeat `[x; N]` and the `scg` widen-mirror deferred as low value.

- **Decisions locked with the owner:** top-level `std/` + `examples/`, each
  subdivided by **functional category** (security, math, …), examples mirroring
  std. The harness (`crates/sentinel-driver/tests/examples.rs`) assembles a temp
  project (copies the `std/` tree next to the flattened entry — module discovery
  roots at the entry's dir), builds each example BOTH `--separate` and merged, and
  asserts both back ends agree with the expected exit (a free differential). The
  flagship sequencing targets `[secret T]` arrays; the lib set is `ct` / `math` /
  `bits` (post-shifts) / `bytes`, built incrementally.
- **Done (commits on `main`):** `25e95a8` foundation (scalar `ct` lib +
  `secure_compare` + harness + corpus READMEs), `d1dace8` `math::num`, `a4f4b45`
  docs, and **four fully self-hosted language features** (each ADR-first, byte-
  identical across `snc` + `scg`, both bootstrap fixed points held):
  - **`1a84081` — `[secret T]` arrays (ADR 0047 A1–A6) + `ct_memcmp` over
    `[secret u8]`.** Arrays of secret ELEMENTS (`ArrayElem::Secret(SecretId)`) — a
    front-end-only change (one crate); the selfhost corpus differential proves `scg`
    == `snc llvm` on `tests/pass/c53_secret_array_memcmp`.
  - **`c3e3d7c` (Phase 1, snc) + `47abfd5` (Phase 2, selfhost mirror) — shift
    operators `<<` / `>>` (ADR 0048 A1–A5).** Logical right shift; a shift by a
    *secret* amount is rejected (a secret value by a public amount is
    constant-time). Reconstructed in the parser from two span-adjacent `<`/`>`
    (no lexer token, so nested generics still close). Shipped `std/bits` (rotates)
    + `std/security/ct::ct_rotl64` + a **SipHash-style ARX round over secret words**
    (`examples/security/siphash_round`) — the recognizable branch-free primitive.
    Validated by `tests/pass/c53_shift` across all 8 selfhost differentials.
  - **`99eadb5` (Phase 1, snc) + `0b69a7b` (Phase 2, selfhost mirror) — integer
    cast `x as T` (ADR 0049 A1–A4).** Closes the i32-construction gap (an int
    literal is `i64`); trunc/sext/zext, preserves secrecy, no CT sink (a new
    `ExprKind::Cast`, the `as` token reused; chosen over conversion builtins, which
    would shift FnIds in dumps). Shipped `std/security/ct::ct_rotl32` + a true
    32-bit **ChaCha quarter-round over `secret i32` words**
    (`examples/security/chacha_qr`) reproducing the RFC 8439 §2.1.1 vector.
    Validated by `tests/pass/c54_cast`. The `u8` conversion builtins remain.
  - **`977d6b5` (Phase 1, snc) + `873775f` (Phase 2, selfhost mirror) — mutable
    index assignment `a[i] = v` (ADR 0050 A1–A5).** Lifts the ADR 0017 D12
    deferral so an array / `Vec` element is written through a public index (the
    read path's bounds-checked element GEP + a store; the GEP factored — inkwell
    `lower_index_elem_ptr`, scg `cg_emit_index_addr` — and shared by read+write).
    Constant-time with NO new sink: a secret LHS index is rejected by the existing
    `IndexNotInt` rule (same as reads); a secret value stored is fine; MIR
    unchanged (`Opaque(value)`, like a field/deref store). MVP = Copy elements
    (scalars + `secret` scalars); a Move element → `IndexAssignNonCopyElem`.
    Parser unchanged (already parsed `Assign{Index,..}`). Shipped a full **ChaCha20
    block** over a `[secret i32]` state permuted in place
    (`examples/security/chacha20_block`, RFC 8439 §2.3.2). Validated by
    `tests/pass/c55_index_assign` across all 8 selfhost differentials.
- **Crypto band on the shipped 0047–0050 surface (no language change):**
  `eabafdb` **SipHash-2-4 keyed MAC** (`std/security/siphash`, canonical
  `0xa129ca…` vector + tamper detection), `26a58e6` **ChaCha20 stream cipher**
  (`std/security/chacha20::chacha20_xor`, RFC 8439 §2.4.2; the block refactored
  into the shared lib), `48b1248` **Poly1305 one-time MAC** (`std/security/
  poly1305`, radix-2^26 `secret i64` limbs, constant-time freeze, RFC 8439 §2.5.2
  — verified across 10 message lengths). These MACs each hit the "bind every
  public constant secret first" friction → motivated ADR 0051.
- **`a05659e` (Phase 1, snc) + `c68e670` (Phase 2, selfhost mirror) — implicit
  public→secret widening (ADR 0051 A1–A4).** A public `T` widens to `secret T` in
  operand position (`secret_x + 5`), as a call arg, a return, and `[u8] → [secret
  u8]` — via the existing `WidenToSecret` node (a codegen no-op). Monotone +
  sound: the CT sinks (shift amount / divisor / branch / index) are untouched and
  still reject (Div + shifts are EXCLUDED from the widen). The **operand widen**
  (binop+cmp) is fully self-hosted (`tests/pass/c56_operand_widen`); the call-arg/
  array/return widens are snc-side (no selfhost source or corpus fixture uses
  them, so the differentials + fixed points stay byte-identical without them).
- **`8b09774` — `std/algorithms/seq`** (in-place insertion `sort` via index-assign
  + `binary_search` over public `[i64]`) + `examples/algorithms/sort_search` — the
  first `algorithms` category (the public-data counterpart to `security`).
- **`b625601` — ADR 0051 payoff cleanup:** the crypto libs (`ct` / `siphash` /
  `poly1305`) dropped their widen-only `let`s (`ct_not` = `x ^ (0 - 1)`, etc.; the
  SipHash per-word + Poly1305 per-limb `let _: secret …` widens) — same behavior,
  re-verified by the example tests.
- **`5f02d36` — ChaCha20-Poly1305 AEAD** (`std/security/aead::chacha20poly1305_encrypt`
  + `examples/security/chacha20poly1305`, RFC 8439 §2.8): composes the shipped
  ChaCha20 + Poly1305 (OTK gen counter-0 → encrypt counter-1 → MAC
  AAD‖pad‖CT‖pad‖le64×2). Required **`poly1305` to take a SECRET key**
  (`&[secret u8]`, read via the secrecy-preserving `(secret u8) as i64` cast —
  `u8_to_i64` is public-only); the standalone poly1305 example now builds its key
  via the `[u8] → [secret u8]` widen. Reproduces both the ciphertext + tag for the
  §2.8.2 key/nonce. Only declassify boundaries: the public ciphertext + tag.
- **`Vec<secret T>` growable secret buffers (ADR 0052, `VecElem::Secret`) — fully
  self-hosted, the sixth gap closed.** The sibling of `[secret T]` (ADR 0047) on the
  `Vec` path: a *variable-length* secret byte buffer (`Vec<secret u8>`) built with
  `vec_new` + `push` and indexed to yield `secret u8` (public index → the CT taint;
  pointer/length/capacity public; a secret index rejected `IndexNotInt`; a branch on
  a secret element rejected). One crate (`sentinel-types`): `VecElem::Secret` +
  `to_type` arm + a guarded `to_vec_elem_secret` (resolution) + the Index-arm demote
  (`to_array_elem_secret`, else the `.expect` panics) — codegen/MIR/borrow unchanged
  (layout-free secret tag). The ONE substantive difference from `[secret T]`: the
  `Vec` builtins are GENERIC, so `vec_new<T>()` over `T:=secret u8` needs the
  substitution round-trip to yield `Vec<secret u8>` — a second `secrets`-free demote
  `to_vec_elem_subst` (no `secrets`-threading ripple). **No selfhost change** (`scg`'s
  structural interner already represents `mk_vec(mk_secret(u8))`); `tests/pass/
  c57_secret_vec` proves `scg`==`snc` byte-for-byte across all 8 differentials, both
  fixed points hold. Shipped `security/ct::ct_vec_eq` (the variable-length sibling of
  `ct_memcmp`, over two growable secret buffers) + `examples/security/ct_vec_eq`
  (builds the buffers by `push` in a loop). FINDINGS: `secret [u8]` (secret-of-array)
  IS representable, so the resolution guard is load-bearing to reject
  `Vec<secret [u8]>`; cross-module `pub fn` over `Vec<secret u8>` crosses `--separate`
  fine; `scg` does NOT mirror a `secret u8` let-widen from a CALL result (it mirrors
  literal/var let-widens + the operand widen) — orthogonal ADR 0019/0051 gap, the
  fixture routes around it (bind the public byte to a `u8` var, widen the var).
- **`vec_to_array` over a secret `Vec` (ADR 0053) + the full §2.8.2 vector + SHA-256 +
  HMAC.** Continuing "do these in recommended order":
  - **ADR 0053 — `vec_to_array(Vec<secret u8>) -> [secret u8]`** (the seventh gap, the
    symmetric completion ADR 0052 deferred). `vec_to_array` is a generic builtin, so
    its result ran through the ARRAY substitution demote (bare `to_array_elem`, rejects
    secret → fell back to `[T]`); fixed with `to_array_elem_subst` (the array twin of
    `to_vec_elem_subst`) at the three array substitution sites. No codegen/CT/`scg`
    change; `tests/pass/c58_secret_vec_to_array` byte-identical across all 8
    differentials. (`cf9b0ab`.)
  - **Full RFC 8439 §2.8.2 114-byte AEAD vector** (`examples/security/chacha20poly1305_full`)
    — the payoff: the 114-byte secret plaintext is built by `push` into a `Vec<secret u8>`
    and `vec_to_array`'d into the `[secret u8]` the AEAD consumes; ciphertext + tag match
    byte-for-byte (verified vs a Python reference), both `--separate` + merge.
  - **`std/security/sha256` — a constant-time SHA-256 over a `secret` message** (`a573822`).
    Branch-free `secret i32` compression; the 64-word schedule is a `Vec<secret i32>`
    (ADR 0052), the padded message a `Vec<secret u8> → [secret u8]` (ADR 0053), the
    running state mutated in place through a `&mut [secret i32]` borrow (ADR 0050, so it
    is never moved across the block loop — the move-checker rejects threading an
    aggregate by value through a loop; this is THE finding). Ch via the no-NOT identity
    `g ^ (e & (f ^ g))`; added `ct_rotr32`. Verified vs NIST "abc"/""/100×'a' (multi-block).
    Drafted by a focused sub-agent iterating against a Python reference, then reviewed +
    re-verified.
  - **`std/security/hmac` — HMAC-SHA256 over a `secret` key** (`d50c7b0`), composing
    sha256. The key + both padded blocks are secret → fully constant-time; the over-long
    key is hashed first via an `if`-expression branching on the PUBLIC key length.
    Verified vs RFC 4231 TC1/TC2/TC6 (TC6's 131-byte key exercises the hash-first path).
- **`std/security/aes` — a constant-time AES-128 block cipher** (`5fd89c7`), the
  sharpest constant-time demonstration so far + a no-language-change library increment.
  The textbook table-lookup S-box (`sbox[secret_byte]`) is a secret value indexing
  memory → REJECTED by the CT check, so the library computes the S-box arithmetically:
  S(x) = Affine(x^-1) with the GF(2^8) inverse as `x^254` (a fixed squaring/multiply
  chain) + the affine map (byte-rotations + `^ 0x63`) — table-free, branch-free, and it
  reproduces the standard AES S-box for all 256 inputs. Every transform (ShiftRows,
  MixColumns via xtime/gf_mul3, AddRoundKey, the RotWord/SubWord/Rcon key schedule) is
  branch-free with PUBLIC indices/shift amounts. KEY IDIOM: a byte is a byte-valued
  `secret i64` in [0,255] (AES is pure 8-bit GF math, so the i64 masks `& 255`/`& 1`/
  `>> 7`/`0 - x` need NO width casts — unlike SHA-256's `secret i32`), entering via
  `(secret u8) as i64` and leaving via `(secret i64) as u8`; the 16-byte state +
  176-byte key schedule are mutated in place through `&mut [secret i64]` /
  `&Vec<secret i64>` borrows (ADR 0050). Only the robust OPERAND widen is used (`^ 99`,
  `^ rcon[j]`) — `xtime`/`gf_mul3` avoid passing public constants as secret call-args.
  Verified vs FIPS-197 §C.1 + AES-128(0,0), both `--separate` + merge; an independent
  differential review confirmed it over 5000 random key/plaintext pairs. Scope = the
  raw ECB block primitive (no mode). NO ADR, NO `scg` change, NO fixture (library growth,
  snc-only example like the §2.8.2 full vector).
- **`std/security/x25519` — constant-time X25519 / ECDH** (`b671436`), the FIRST
  public-key primitive + (with AES) the sharpest constant-time demonstration. A naive
  Montgomery ladder branches on the secret scalar bits (`if bit { swap }`), leaking the
  private key by timing → REJECTED by the CT check, so the ladder's conditional swap is
  the branch-free mask-based `sel25519`; the build is the proof the scalar mult is
  constant-time in the secret scalar. NO language change (library growth) — the probe
  proved it expressible: field GF(2^255-19) = 16 limbs radix 2^16 in `secret i64` (the
  TweetNaCl rep; the schoolbook multiply's accumulator peaks ~2^43 < 2^63, so NO 128-bit
  arithmetic — deliberately NOT radix-2^51, which WOULD hit the 128-bit-multiply wall =
  the real numeric gap, dodged). KEY IDIOMS: (1) carries need an ARITHMETIC right shift
  on signed limbs, but `>>` is logical (ADR 0048) → a branch-free `arith_shr(x,n) =
  (x>>n) | ((0-((x>>63)&1)) << (64-n))` reconstructs it (no new operator); (2) field
  elements are mutated in place via `&mut [secret i64]` borrows and NEVER aliased —
  TweetNaCl's output-aliases-input ops become in-place `fadd_assign`/`fsub_assign` or a
  scratch `tmp` + `fe_copy`; (3) NEW borrow idioms (probe-proven): forwarding a `&mut`
  borrow into a nested fn (`fmul`→`car25519`) and forwarding a `&` borrow twice
  (`fsq`→`fmul`) both work. Verified vs RFC 7748 §5.2 + the §6.1 DH round-trip (both
  parties' shared secrets agree + match the published value), both `--separate` + merge;
  an independent review modeled the Sentinel source vs a big-integer ladder ground truth
  over 50 random pairs (no pass-by-luck), confirming the no-alias ladder is byte-identical
  + the Fermat exponent is exactly 2^255-21. NO ADR/`scg`/fixture (snc-only example).
  Scope = raw X25519 (one scalar mult; ECDH = two calls).
- **Four-increment session (owner-approved "do all four", HEAD `b671436`→`3f35b67`),
  four-check green each, NEVER pushed:**
  - **`std/security/sha512` — constant-time SHA-512** (`d6f7ef9`), the 64-bit-word twin
    of SHA-256. NATIVE `secret i64` words (no width casts — cleaner than SHA-256's
    `secret i32`), 80 rounds, 128-byte blocks, the SHA-512 rotations; + `ct_rotr64`.
    Verified vs NIST "abc"/""/200×'a'. Library growth, no gap.
  - **`std/security/aes_gcm` — constant-time AES-128-GCM AEAD** (`ac57eeb`), composing
    the AES block + GHASH. The GHASH GF(2^128) carry-less multiply (a 128-bit value =
    two `secret i64` limbs [hi, lo]; branch-free bit-by-bit shift-and-XOR + reduce, the
    textbook version branches on bits of the SECRET auth key H = AES_K(0)) is the new
    field primitive — probe-proven, no 128-bit integer type needed. ⚠ FINDING: the
    Python ref had to be validated vs OpenSSL (the McGrew "TC3" tag I first hand-typed
    was a transcription error); reproduces the McGrew/OpenSSL TC4 vector (60B pt + 20B
    AAD, partial blocks) + a no-AAD differential. Library growth, no gap.
  - **`&mut a[i]` / `&a[i]` element borrows (ADR 0054)** (`0ec884f`) — the eighth
    language gap + the only COMPILER change. Phase 1 = ONE arm (the `Index` arm of
    `check_mutable_borrow_target` recurses on the base, like `FieldAccess`); the
    element-address GEP (ADR 0050) + the borrow-check Index recursion + the Ref typing
    were ALL already in place, so codegen + borrow-check needed NO change. Whole-array
    borrow granularity (pre-Polonius); secret index still `IndexNotInt`. Phase 2 = NO
    `scg` change (codegen reuses ADR 0050's `cg_emit_index_addr`); `tests/pass/
    c59_borrow_index` validates `scg`==`snc` across all 8 differentials. +
    `std::math::num::clamp_assign(&mut i64,…)` + `examples/math/inplace`.
  - **`std/security/ed25519` — constant-time Ed25519 SIGNING** (`3f35b67`), the capstone.
    Factored the shared **`std/security/fe25519`** field module (GF(2^255-19), the proven
    X25519 radix-2^16 ops made `pub` + the Edwards constants d2/Bx/By); ed25519 composes
    it + `sha512`. A faithful TweetNaCl `crypto_sign` port: extended-coord `point_add`,
    the cswap double-and-add ladder over the SECRET scalar (branch-free), `point_pack`,
    `modL` (signed shifts via `arith_shr`). A point = 4 separate `[secret i64]` (aggregates
    can't move through a loop). ⚠ NEW IDIOM: forwarding a `&mut` param as a `&` arg needs
    an explicit reborrow `&(*x)`. Drafted by a focused sub-agent vs a `cryptography`-verified
    reference, then an independent review modeled the Sentinel source vs `cryptography` over
    120 random seeds (600/600 pk+sig) + 27015 modL cases + 55 point-add/ladder cases — all
    byte-identical. Reproduces 3 RFC 8032 vectors (incl. empty msg). Library growth, no gap.
  - **Ed25519 VERIFY** (`9e497a0`) — the natural completion (owner: "proceed on
    recommendation"). `ed25519_verify` decompresses `-A` from the public key (recover x
    from y via a field square root `(num/den)^((p+3)/8)` = the new `fe25519::fe_pow2523`
    `z^((p-5)/8)` chain + `fe_sqrtm1`/`fe_d`/`unpack25519`; the sqrt branch-correction +
    parity sign-fix are branch-free mask selects), computes `[S]B - [h]A`, accepts iff it
    re-encodes to `R`. A probe reproduced TweetNaCl `unpackneg(pk1)` byte-for-byte first.
    ⚠ FINDING (independent review, 150 valid + 310 forgery + 1000 decode-parity cases vs
    `cryptography`): the faithful TweetNaCl `crypto_sign_open` LACKS the RFC 8032 `S < L`
    check → `(R, S+L)` is a second accepted signature (malleability, a false-accept).
    FIXED with a constant-time MSB-first lexicographic `S < L` compare AND-ed into the
    accept; the example asserts the malleated sig is rejected. Verify is public (the
    boolean is the sole declassify) but stays branch-free (reuses the CT machinery).
  - **`std/security/sha3` — constant-time SHA-3 / Keccak** (`8908461`) — SHA3-256 +
    SHA3-512, the Keccak SPONGE (a different construction from SHA-2's Merkle-Damgård).
    The Keccak-f[1600] permutation (24 rounds θ/ρ/π/χ/ι) is branch-free bitwise ops over
    a 25-lane `[secret i64]` state mutated in place: XOR/AND, rotations by PUBLIC ρ
    offsets (`ct_rotl64`), and χ's complement via the no-NOT identity `~B = B ^ (0-1)`.
    Round constants + ρ offsets are PUBLIC tables, the `x mod 5` index wraps are public
    arithmetic, so only the lane VALUES are secret — the build is the CT proof. The
    sponge absorbs at the rate (136 B for SHA3-256, 72 B for SHA3-512) with the SHA-3
    pad `0x06..0x80`. Verified vs a hashlib-checked reference over abc / "" / multi-block
    / the 135-byte padding edge (`0x06` and `0x80` share a byte) — both instances. NO
    compiler/scg change (library growth). **Extended with the SHAKE128/256 XOFs**
    (`8d0b4f8`): `keccak_sponge` gained a domain byte (0x1F vs 0x06) + an arbitrary-
    length multi-block/partial squeeze (emit ≤ rate bytes, permute, repeat), so output
    is any length — `sha3_256/512` keep their fixed output, `shake128/256(msg, out_bytes)`
    are the XOFs. Verified vs hashlib over lengths 16..400 incl. >rate (two-permutation)
    outputs + the XOF prefix-extension property. **Then KMAC128/256** (`b6e031b`, SP
    800-185): the Keccak keyed MACs, wrapping a SECRET key around cSHAKE with the
    `left_encode`/`right_encode`/`encode_string`/`bytepad` length encodings (the new
    pieces) + the 0x04 cSHAKE domain byte. CT: the encodings prefix the input with
    PUBLIC byte-counts, so only the key+message bytes are secret. The Python ref
    reproduces 5 NIST SP 800-185 samples (empty + tagged customization, both variants,
    4- + 200-byte data); the Sentinel port reproduces 3 byte-for-byte.
- **Also done:** `d1dace8` `math::num` + `3e98443` **`std/bytes`** (`eq`/`find`/
  `contains`/`count`/`starts_with`/`repeat` over `&[u8]` borrows) + `examples/bytes/
  scan` — the agreed `ct`/`bytes`/`bits`/`math` starter set is complete. (Finding:
  byte utilities must take `&[u8]`, not `[u8]` by value, or the first call consumes
  the array; `&[u8]` params + `(*a)[i]` indexing work today.)
- **Next (open, owner's call — none yet approved):**
  - **More crypto** — the §2.8.2 vector, SHA-256/512, SHA-3 (Keccak sponge), HMAC,
    HKDF, the full SP 800-185 derived-function family (cSHAKE / KMAC / TupleHash /
    ParallelHash), AES + AES-GCM, X25519, Ed25519 (SIGN + VERIFY), and X448 + Ed448
    (SIGN + VERIFY, over the new fe448 field) are all shipped — a full asymmetric +
    symmetric + hash + XOF + keyed-MAC + KDF suite. The **numeric gap is now CLOSED**:
    ADR 0055's `u128` type (LLVM `i128`) shipped, with `fe25519_64` + `x25519_64` (X25519
    at radix 2^51) as the demonstrator — the field multiply runs in `secret u128`,
    cross-checked byte-for-byte against the radix-2^16 X25519. (Of the run's four items
    only the numeric gap needed a language change; HKDF / SP 800-185 / X448 / Ed448 were
    pure library work. Ed448's port surfaced a real bug the Python model had hidden — its
    Barrett reduction leaned on a final big-int `% L` after only 3 conditional
    subtractions; the constant-time port does 8 always-executed masked subtractions.)
    Cleanly open next: **toward a real `sshd`** — the loopback transport is complete END
    TO END (KEX + KDF in `std::net::ssh` + the `chacha20-poly1305@openssh.com` record
    cipher in `std::net::ssh_cipher`, stitched in `examples/net/ssh_session`), and the ADR
    0056 sockets are now LIVE end to end: runtime layer (`b90a889`), compiler builtins at
    FnId 14..=20 (`0845b8a`, the FnId-shift crux closed byte-exact — builtins 0..=20 ⇒
    user fns start at #21, both bootstrap fixed points hold), and a real Sentinel program
    over them — `examples/net/tcp_echo` (`669665a`: bind 127.0.0.1:0, `spawn` a concurrent
    server task, connect/send/echo/read, exit 42; harness-tested on both back ends). AND
    THE SSH SESSION NOW RUNS OVER A REAL SOCKET — `examples/net/ssh_over_tcp` (`aac69a8`):
    the client and server are SEPARATE concurrent tasks (server `spawn`ed on an OS thread
    bound to a real ephemeral listener; client connecting from the main task) doing a
    DISTRIBUTED curve25519-sha256 KEX — each peer holds only its own ephemeral secret and
    derives the shared secret K independently (real DH, not one fn computing both views) —
    then ssh-ed25519 host-key auth, the §7.2 KDF, and an encrypted record that round-trips
    across the wire. ⚠ THE TRUST BOUNDARY (the point): the whole handshake is `[secret u8]`
    (build = the CT proof), but a socket carries PUBLIC bytes; the wire-public values
    (ephemeral pubkeys, host key, signature, ciphertext record) are `declassify`d per byte
    to send and WIDENED back to secret (public->secret, ADR 0049) on receipt — K and the
    session key never cross the socket. A `read_exact` helper coalesces a TCP write split
    across reads. `examples/net/ssh_session` stays as the vector-anchored in-memory
    reference. So the loopback `sshd` transport is now REAL-SOCKET end to end. Cleanly open
    next toward a fuller sshd: a thin `std/net/tcp` wrapper over the raw builtins; a
    bidirectional / multi-packet channel (seqnr-incrementing record stream both ways);
    optionally the `ssh-userauth` / `connection` layers. Also
    open: a `secp256k1` / `P-256` field at radix 2^52 (more
    `u128` mileage + a recognizable new curve, possibly ECDSA); the **`scg` mirror of
    `u128`** (+ a `tests/pass/cNN` fixture) to fully self-host it (ADR 0055 deferred this
    — snc-side only today); more SP 800-185 XOF variants.
  - **Two deferred items from the list, now LOW value — recommend skipping unless
    wanted:**
    - **array-repeat `[x; N]`** — SHA-256/HMAC built cleanly WITHOUT it (`Vec<secret T>`
      + `push` covers fixed- and variable-length buffers), so it is no longer driven by
      any real program. A genuine minor convenience (a `[0; N]` initializer), but a full
      pipeline feature (parser + types + codegen + selfhost mirror) for small marginal
      value over `Vec`. Deferred, not blocked.
    - **Self-hosting completion** — mirror into `scg` the ADR 0051 call-arg/array/return
      widens + the call-RHS `secret` let-widen (ADR 0052 A5). Pure completionism: NO
      selfhost-corpus program uses these constructs, so mirroring them has zero
      functional impact (and carries byte-identity risk) until a fixture needs them.
      Deferred.
    - **`&mut a[i]` element borrows — DONE** (ADR 0054, `0ec884f`). The remaining
      borrow-checker limit is element-granular loans (whole-array granularity
      over-rejects `swap(&mut a[i], &mut a[j])`), deferred to the Polonius migration.
  - **More `std` categories** — networking / threading / process need runtime/
    syscall surface that does not exist yet (a real gap to scope first).
  - **Lower value (and partly blocked):** **Linux CI (P2.4)** needs a Linux/CI
    environment (cannot be validated on this macOS host — needs the owner's CI); the
    separate-comp tail (class/generic-instance dedup) is explicitly low-value; the
    deeper post-codegen constant-time verifier is research-hard.
  Keep both bootstrap fixed points + the selfhost differentials byte-identical (the
  full nextest is the gate).

**Other open tracks** (owner's call, none on the critical path):

- **Harden constant-time `secret` (the deep version)** — the check is sound for
  the current language (see Done) but runs pre-LLVM and doesn't force
  constant-time *emission*; a post-codegen / post-optimization verifier is the
  highest-ceiling but research-hard item. The real-program work above should
  inform it (you'll learn what the optimizer does to branch-free secret code).
- **Linux CI (P2.4)** — glibc surfaces heap bugs macOS masks; worth landing
  before `abi-v1` ossifies. Rest of P3 (LSP, diagnostics); P4 (perf, deferred
  ergonomics like `[secret T]` arrays — which the flagship work above will
  likely force).
- **Separate-compilation tail** — class / generic-instance type-arg dedup and
  trait/class-method dedup. Low value (classes can't be imported; trait impls
  are unit-local) — recommend not grinding it.
- **Borrow checker** is pre-Polonius — sound but over-rejects; the documented
  over-rejections are in [`borrow-check-limitations.md`](borrow-check-limitations.md),
  deferred to the Polonius migration.

**Working norms** (the four-check per change, ADR-first for frozen-ABI changes,
never push, additive — keep the merge path + both fixed points green) live in
`## Conventions` of [`STATE.md`](STATE.md) and in §9 / §11 below.

## 1. Scope of This Document

This is not a specification of Sentinel. The design documents are the
specification, and they are still partly under-specified by intent. This
document covers how to *build* the bootstrap compiler: environment,
tooling, architecture, milestones, testing, and the order in which to
attack the work.

The audience is a senior engineer or small team (one to three people)
with Rust experience and some compiler or PL background. If you have not
written a compiler before, plan to spend the first two weeks reading
"Crafting Interpreters" and the rustc dev guide before starting on
milestone zero.

---

## 2. Strategic Approach

Do not start by building the full Sentinel compiler. The design document
explicitly recommends a staged validation: prove the broker idea works
as a Rust library, prove the effects system works as a research
prototype, and only then commit to building the full language. This
handover document follows that staging.

The milestones below are organized as four phases. Phase A and Phase B
are the validation prototypes. Phase C is the bootstrap compiler proper.
Phase D is the path to self-hosting. Each phase has a clear go/no-go
decision point at the end. Do not skip the decision points. If Phase A
produces a broker that nobody wants to use, building Phase C is wasted
effort.

The expected calendar time, with a small focused team, is roughly:
Phase A six months, Phase B nine months (overlapping the second half of
Phase A), Phase C twelve to eighteen months, Phase D another nine to
twelve months. This is honest, not optimistic. Most language projects
underestimate by 2-3x; budget accordingly.

---

## 3. Environment Setup (macOS)

### 3.1 Toolchain

Install Rust via rustup. Use the stable channel for the compiler itself
and pin the version in `rust-toolchain.toml` so contributors get
reproducible builds.

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    rustup default stable
    rustup component add rustfmt clippy rust-analyzer
    rustup target add aarch64-apple-darwin x86_64-apple-darwin

Install LLVM via Homebrew. Pin to a specific major version because
`inkwell` and LLVM's C API are version-coupled.

    brew install llvm@18
    echo 'export PATH="/opt/homebrew/opt/llvm@18/bin:$PATH"' >> ~/.zshrc
    echo 'export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18' >> ~/.zshrc

Install supporting tools:

    brew install cmake ninja just ripgrep fd jq
    cargo install cargo-nextest cargo-insta cargo-deny mdbook

`just` is used as the command runner instead of `make`. `cargo-nextest`
is significantly faster than the default test runner for compiler test
suites. `cargo-insta` is used for snapshot testing of compiler output.

### 3.2 Repository Layout

Create a single workspace repository. Do not split into multiple repos
yet; the dependency churn early on will make multi-repo intolerable.

    sentinel/
    ├── Cargo.toml              # workspace manifest
    ├── rust-toolchain.toml
    ├── justfile
    ├── .github/workflows/
    ├── docs/
    │   ├── SENTINEL_DESIGN.md
    │   ├── SENTINEL_DESIGN2.md
    │   └── HANDOVER.md         # this file
    ├── crates/
    │   ├── sentinel-broker/        # Phase A deliverable
    │   ├── sentinel-effects-proto/ # Phase B deliverable
    │   ├── sentinel-syntax/        # lexer + parser + CST
    │   ├── sentinel-ast/           # AST types
    │   ├── sentinel-resolve/       # name resolution
    │   ├── sentinel-types/         # type/region/effect checking
    │   ├── sentinel-hir/           # typed HIR
    │   ├── sentinel-mir/           # SSA-form IR
    │   ├── sentinel-codegen/       # LLVM lowering
    │   ├── sentinel-driver/        # the `snc` binary
    │   ├── sentinel-runtime/       # the broker as runtime library
    │   └── sentinel-lsp/           # language server
    ├── tests/
    │   ├── ui/                     # compile-error tests
    │   ├── pass/                   # programs that should compile and run
    │   └── snapshots/              # insta snapshots
    └── examples/

### 3.3 Initial Workspace Manifest

The top-level `Cargo.toml` declares the workspace and pins dependency
versions centrally. Every member crate inherits from `[workspace.deps]`
rather than declaring its own versions. This avoids version drift, which
is the most common source of pain in multi-crate Rust projects.

Key dependencies to pin from day one: `logos` for lexing, `chumsky` or
hand-written recursive descent for parsing (recommend hand-written for
better error messages), `salsa` for the query engine, `inkwell` for
LLVM, `cranelift` for the debug backend, `bumpalo` and `typed-arena`
for AST allocation, `indexmap`, `rustc-hash`, `smallvec`, `tracing`,
`thiserror`, `miette` for diagnostics, `insta` for snapshot tests.

### 3.4 Build Commands

Define common commands in a `justfile`:

    default: build

    build:
        cargo build --workspace

    test:
        cargo nextest run --workspace

    fmt:
        cargo fmt --all

    lint:
        cargo clippy --workspace --all-targets -- -D warnings

    snc *args:
        cargo run --bin snc -- {{args}}

    check-all: fmt lint test

    bless:
        INSTA_UPDATE=always cargo nextest run --workspace

---

## 4. Phase A — The Broker Prototype

**Goal**: build the memory broker as a standalone Rust crate, ship it,
get real users, learn whether the API actually proves out.

**Duration**: three to six months.

**Go/no-go criterion**: at least three real Rust projects (yours or
external) have adopted the broker for non-trivial work and the API has
stabilized through their feedback. If after six months no one wants to
use it, the broker idea is wrong or the API is wrong, and Sentinel
should pause.

### 4.1 What to Build

The broker crate exposes a `Broker` struct that owns allocation policy
for a process. It provides:

  - Generational arenas with O(1) bulk free and safe dangling-handle
    detection. Handles are `(arena_id, slot_index, generation)` triples.
    Access through a handle checks the generation atomically.
  - Programmable allocation strategies as trait objects. The default
    strategies are bump, slab, and system-malloc, but users can plug in
    their own.
  - Memory budgets with structured-scope semantics. The Rust API uses a
    builder pattern since Rust does not have effect handlers.
  - Statistics queries (live bytes, peak, fragmentation, allocation
    counts by tag).
  - A recording mode that captures every allocation event into a ring
    buffer for deterministic replay.
  - Secret-memory policy with mlock, no-core-dump exclusion, and
    zero-on-free. On macOS this uses `mlock(2)`, `madvise(MADV_NOCORE)`
    where available, and explicit zero with a barrier on free.

What to *defer*: cross-process shared memory (hard, do it in Phase C
when you have the full language to express it). Memory-hard secret
storage (research, post-1.0). Argon2id integration (use the `argon2`
crate as a separate library, not part of the broker yet).

### 4.2 API Sketch

    use sentinel_broker::{Broker, Arena, Budget, Handle, Secret};

    let broker = Broker::new();
    let arena = broker.create_arena("request", 4 * 1024 * 1024);
    let handle: Handle<Request> = arena.alloc(Request::new());

    let req: &Request = handle.get()?; // returns Err if invalidated
    arena.drop(); // all handles into this arena now return Err

    let budget = Budget::new(8 * 1024 * 1024);
    budget.scope(|alloc| {
        let v: Vec<u8, _> = Vec::with_capacity_in(1000, alloc);
        // ...
    }).map_err(|over| /* graceful fallback */)?;

    let key: Secret<[u8; 32]> = broker.alloc_secret([0u8; 32]);
    // key is mlock'd, zeroed on drop, excluded from Debug output

### 4.3 Validation

Write three example programs that use the broker for real work:

  - A small HTTP server with per-request arenas.
  - A parser combinator library that uses bump allocation for AST nodes.
  - A key-value store with the budget API enforcing memory limits.

If writing these is awkward, the API is wrong. Iterate.

Publish the crate to crates.io as `sentinel-broker` once the API feels
stable. Watch what users do with it. The point is to discover whether
the broker concept survives contact with real code.

---

## 5. Phase B — The Effects Prototype

**Goal**: build a small interpreted language with algebraic effects and
the `secret` qualifier, just enough to learn whether the effects-as-
capabilities story works in practice.

**Duration**: six to nine months, can start at month three of Phase A.

**Go/no-go criterion**: you can write a small program that demonstrates
supply-chain capability enforcement, async-as-effect, and constant-time
operations on `secret` data, and the ergonomics feel reasonable.

### 5.1 What to Build

A tree-walking interpreter for a tiny language — call it Sentinel-Mini.
No classes, no regions, no broker integration. The point is to validate
the effect system in isolation.

Required features:

  - Hindley-Milner-style type inference with effect rows.
  - Effect declarations: `pure`, `io`, `network`, `throw`, `await`.
  - Effect handlers with `handle expr with { ... }` syntax.
  - The `secret T` qualifier with constant-time equality and a
    "no branching on secret" check.
  - A capability check: importing a module restricts its effects.

What to *defer*: anything not directly testing the effect system.
Performance is irrelevant; this is a research artifact.

### 5.2 Validation

Write three example programs:

  - A "supply chain attack" demo where importing a JSON parser fails
    because it declares the `network` effect.
  - An async demo where the same function runs synchronously in tests
    and asynchronously in production by swapping the effect handler.
  - A constant-time password verification demo that fails to compile
    if you try to branch on the comparison result.

Publish a short paper or technical report at the end of Phase B
documenting what worked and what did not. This is genuinely useful
output even if Sentinel never proceeds.

---

## 6. Phase C — The Bootstrap Compiler

**Goal**: build the production Sentinel compiler in Rust, targeting the
1.0 subset defined in SENTINEL_DESIGN2.md Section 15.

**Duration**: twelve to eighteen months.

**Go/no-go criterion**: the compiler can compile a non-trivial Sentinel
program (target: a TLS handshake implementation, or an HTTP server)
that exercises all 1.0 features.

### 6.1 Architecture

The compiler is a query-based pipeline built on Salsa. Every phase is
expressed as a memoized query over inputs; incremental recompilation
is foundational, not retrofitted.

The pipeline:

    source file
      -> [sentinel-syntax]    lexer + parser -> CST
      -> [sentinel-ast]       CST -> AST lowering
      -> [sentinel-resolve]   name resolution, module graph
      -> [sentinel-types]     type, region, nullability, secrecy,
                              effect inference and checking
      -> [sentinel-hir]       typed HIR with all qualifiers resolved
      -> [sentinel-mir]       SSA lowering, escape analysis,
                              bounds-check elision, constant-time
                              verification on secret data
      -> [sentinel-codegen]   LLVM IR via inkwell, or Cranelift for
                              fast debug builds

The driver crate (`snc`) wires the queries together and exposes the
command-line interface. The LSP crate (`sentinel-lsp`) reuses the
exact same query engine, which is the entire point of using Salsa.

### 6.2 Implementation Order Within Phase C

Build the pipeline end-to-end for the smallest possible language
subset first, then expand. This is the rustc approach and it works.

**C0 (month 1-3)**: lexer, parser, AST for a subset with only `let`,
arithmetic, `if`, and function calls. End-to-end compilation to LLVM
that produces a runnable binary. No type system yet; everything is
i64. The goal is to prove the pipeline plumbing works.

**C1 (month 3-6)**: bring up the type system. Add `struct`, basic
generics, and references. Implement non-nullable types and the `?T`
optional. Bounds-checked array access. At the end of C1 the compiler
should reject all the "obvious" memory-safety violations.

**C2 (month 6-9)**: regions and ownership. Named regions, second-class
references by default, move semantics, `&` and `&mut` borrows. This is
the hardest single phase; budget pessimistically. Use the Polonius
formulation of borrow checking; it generalizes more cleanly than the
NLL formulation when you add regions.

**C3 (month 9-12)**: effects. Integrate the lessons from Phase B.
Effect inference, effect handlers, async-as-effect, capability
enforcement at the module boundary. Add the `secret` qualifier with
constant-time operations and the speculation-barrier insertion in
codegen.

**C4 (month 12-15)**: classes, traits with named implementations,
delegation, structured concurrency, actors. Most of this is
"reasonable language design plumbing" rather than novel work, but the
volume is significant.

**C5 (month 15-18)**: broker integration, cross-process safety,
reproducible-build guarantees, stable ABI definition, LSP and tooling
polish.

### 6.3 Diagnostics

Diagnostic quality is not optional. The borrow checker, region
checker, and effect checker will produce confusing errors by default,
and Sentinel's whole pitch depends on these errors being
comprehensible. Use `miette` for rich diagnostics from day one.
Allocate at least 15% of compiler engineering time to error message
quality. Steal Elm's and Rust's diagnostic conventions shamelessly.

Every error should answer three questions: what is wrong, why is it
wrong, and what should I do about it. Test diagnostics with snapshot
tests so regressions are visible in PRs.

### 6.4 Testing Strategy

Three layers:

  - **Unit tests** in each crate for individual functions and types.
    Standard Rust practice.
  - **UI tests** in `tests/ui/`. Each is a Sentinel program plus an
    expected stderr. Modeled on rustc's UI test suite. These catch
    regressions in diagnostics and in what the compiler accepts or
    rejects.
  - **Execution tests** in `tests/pass/`. Each is a Sentinel program
    plus expected stdout. The test runner compiles and runs the
    program and compares output.

Use `cargo-insta` for snapshot management. Every PR runs the full
suite via `cargo nextest`. CI fails on any unblessed snapshot
difference.

### 6.5 Performance Targets

Compile time is part of the value proposition. Set targets early and
measure continuously:

  - Clean build of a 10K-line program: under 30 seconds.
  - Incremental build after a one-line change: under 1 second.
  - LSP "go to definition" latency: under 50ms p95.

These are aspirational but they shape architecture decisions. If you
hit a fork in the road, take the path that preserves these targets.

---

## 7. Phase D — Self-Hosting

**Goal**: rewrite the compiler in Sentinel, reach the four-stage
fixed point described in SENTINEL_DESIGN.md.

**Duration**: nine to twelve months after Phase C completes.

**Go/no-go criterion**: stage-three compiler compiles its own source
to a binary that, fed its own source, produces a byte-identical
binary.

### 7.1 Staging

Follow the four-stage plan from the design document exactly. Do not
attempt to self-host all at once; the half-and-half configurations
(Sentinel parser feeding Rust type checker, etc.) are what surface
language ergonomics problems.

Stage one is the easiest and most informative: port the lexer and
parser. If writing the parser in Sentinel is unpleasant, the
language is wrong, and you find out cheaply.

### 7.2 Keep the Rust Bootstrap Alive

Do not delete the Rust bootstrap when self-hosting succeeds. Maintain
it indefinitely as a reproducibility anchor and a defense against
trusting-trust attacks. Pin which Sentinel version the self-hosted
compiler is written in, separately from which Sentinel version it
implements. Every Sentinel release should be buildable from the Rust
bootstrap.

---

## 8. Open Questions to Resolve Early

These are listed in design document Section 18 but they need
*decisions* before Phase C, not just acknowledgment.

**Effects with traits**: can trait methods declare effects
polymorphically? If yes, design the row-polymorphism story now. If no,
document the workaround. This decision shapes the entire type system
and must be made before C3.

**Region inference vs explicit regions**: the design says "named,
visible regions" but practical ergonomics likely require some
inference. Decide where the line is before C2. Recommend: regions are
inferable within a function body but must be explicit at function
boundaries when more than one region is involved.

**Async runtime**: even with effects-as-async, you need a default
scheduler. Will Sentinel ship its own, or wrap an existing one (Tokio
via FFI)? Decide before C3. Recommend: ship a minimal scheduler in
the standard library, allow user-defined schedulers via effect
handlers.

**Stable ABI scope**: a stable ABI for the whole language is
extremely ambitious. Restrict to `extern "sentinel-stable"`
declarations explicitly, like Swift did with `@frozen`. Decide the
exact subset before C5.

**Generic dispatch default**: witness tables vs monomorphization.
The design says witness tables by default, but measure both on
realistic code before committing. Decide before C1.

Document each decision in `docs/decisions/NNNN-title.md` using the
Architecture Decision Record format. Future contributors will need
the reasoning, not just the outcome.

---

## 9. Team and Process

### 9.1 Minimum Viable Team

A realistic minimum is two senior engineers with compiler experience
plus one engineer doing tooling, build infrastructure, and developer
experience. A single person can do Phase A and start Phase B but
cannot realistically complete Phase C alone in any reasonable
timeframe.

If you only have one person, do Phase A, do a reduced Phase B, and
write a thorough postmortem. That alone is a significant contribution.

### 9.2 Process

Use a monorepo. Use trunk-based development with short-lived feature
branches. Require PRs to pass `just check-all` before merge. Require
ADRs for any decision that touches language semantics. Hold a weekly
design review focused on the open questions in Section 8.

Do not chase contributors aggressively in the first year. A small
focused team makes faster progress than a large unfocused one, and
language projects are particularly vulnerable to bikeshedding when
contributors arrive before the core design is stable.

### 9.3 Communication

Maintain a public design log as `mdbook` in `docs/`. Every significant
decision lands as a chapter. Publish quarterly progress reports. This
discipline forces clarity on what you actually built versus what you
planned, and it builds the credibility needed when you eventually
want adopters.

---

## 10. Day One Checklist

When you sit down to actually start:

  - Clone or create the `sentinel` repository with the layout in
    Section 3.2.
  - Install the toolchain from Section 3.1.
  - Copy SENTINEL_DESIGN.md, SENTINEL_DESIGN2.md, and HANDOVER.md
    into `docs/`.
  - Initialize the workspace with empty crates matching Section 3.2.
  - Set up CI to run `just check-all` on every PR.
  - Create `docs/decisions/0001-staged-validation.md` recording the
    decision to follow the Phase A through D plan.
  - Start Phase A milestone one: scaffold `sentinel-broker` with
    the `Broker::new()` constructor, the simplest possible arena, and
    a test that allocates and frees a value.

Ship something on day one. The hardest part of starting a multi-year
project is starting; the rest is iteration.

---

## 11. What to Do When Stuck

You will get stuck. Specific places it tends to happen:

  - **Borrow checker design** in C2. Read the Polonius papers, read
    the rustc dev guide chapter on NLL, look at how Hylo handles
    second-class references. Allocate four weeks of design time
    before writing code.
  - **Effect inference** in C3. Read the Koka and Effekt papers. The
    row polymorphism formulation is the standard one; implement it
    even though it is harder than the alternatives, because the
    alternatives do not compose.
  - **LLVM integration** anywhere. `inkwell` papers over most of the
    pain but not all. When in doubt, write the LLVM IR by hand first,
    confirm it does what you want, then figure out how to generate it
    from `inkwell`.
  - **Diagnostics that confuse users**. Find five people unfamiliar
    with Sentinel, show them the error, ask them to explain it. Their
    confusion is more informative than any internal review.

The general rule: when stuck for more than three days, write the
problem down as a design document, share it for review, and timebox
the resolution. Languages die from indecision more often than from
bad decisions.

---

## 12. Closing

Sentinel is an ambitious project, and the honest assessment in
SENTINEL_DESIGN2.md applies: most language projects at this level of
ambition do not reach widespread adoption. That is not a reason not to
build it. The ideas — programmable runtime, regions, effects-as-
capabilities, the `secret` qualifier — are worth exploring even if the
full language never ships at scale. Each phase produces value
independently. Each phase has a clear go/no-go decision. Each phase
teaches you something the next phase needs.

Build Phase A. See what happens. Decide from there.

Good luck.

*End of document.*
