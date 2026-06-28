# STATE.md — Sentinel Implementation Status

This document tracks what is actually built. When it disagrees with
HANDOVER.md, STATE.md is the source of truth. New contributors (or
new chat sessions) should be able to read this file and understand
the current state of the workspace without re-reading every commit.

## Current State (2026-06-28)

> **Phase C closed at Sentinel 1.0 (2026-05-30); Phase D self-hosts; the
> per-unit separate-compilation back end is functionally complete.**
> The milestone-by-milestone chronology that used to live here has been
> archived to [`HISTORY.md`](HISTORY.md) (P3.2 cleanup). Sections A/B/C below
> are the durable per-crate reference; the [README](../README.md) is the
> overview.

**Latest (2026-06-28) — explicit early `return`, effect-free path (ADR 0065 stages 1–2).** Sentinel
now has a C-style **`return expr`** that exits a function early, instead of only the tail
expression. It is a **divergent expression** (`ExprKind::Return`, Rust-style `return: !`): valid as
a branch tail (`if guard { return x } else { … }`), so a function can bail out. Typing checks the
inner against the enclosing fn's return type (stashed on the per-fn `VarTypeEnv`) and uses a
structural divergence predicate (`expr_diverges`/`block_diverges`) so a `return` branch unifies with
the other branch and a fully-returning body skips the tail-vs-return-type match. Codegen reuses the
ADR 0036 `break`/`continue` machinery with the floor set to the **function**: evaluate the inner,
drop every live scope frame (value-aware so the returned binding survives — no use-after-free), then
`ret` via a shared `build_fn_return` (the `main` i64→i32 / effecting-kont ABI), parking a dead block
for the now-unreachable remainder. **Constant-time UNCHANGED** — `return` is unconditional control
flow, not a branch on a value, so it is no new `secret_leak` sink (a secret `if`-condition is still
rejected at the `if`); returning a `secret` value is fine. Implemented **snc-side** with the
demonstrator in `examples/` (`examples/lang/early_return.sentinel`, snc-only like u128/f64 — OUT of
the scg differential, so both fixed points stay byte-identical without the stage-4 mirror) +
`tests/ui/c65_return_type_mismatch`. Windows four-check green; analysis unit tests + the selfhost
**dump** differential unchanged (no regression). **Pending:** stage 3 (`return` crossing a `handle`
— the D6 kont-frame unwind) + stage 4 (selfhost mirror + both fixed points). Known v1 stubs:
`match`-arm divergence and the `snc llvm` text-IR dump for `return`. See ADR 0065 + HANDOVER §0.

**Latest (2026-06-28) — library layout reorg + the `Sentinel::` core base (ADR 0064).** The
first-party libraries now live under a top-level **`sentinel_library/`** tree so the repo can
host many of them. A new **`Sentinel::` core base** (`sentinel_library/Sentinel/`) is the
identity-defining foundation — the constant-time `secret` vocabulary the batteries build on:
**`Sentinel::ct`** (the branch-free constant-time primitives, moved from `std::security::ct`)
and **`Sentinel::secrets`** (the `widen`/`reveal` public⇄secret boundary helpers, DRY'd from
three byte-identical copy-paste sites — `examples/export/crypto_lib` +
`tools/trust/{sign,keygen}_core`). `std::` stays the batteries, relocated to
`sentinel_library/std/` (namespace unchanged) and `use`-ing `Sentinel::ct` in its crypto
modules. NOT oracle-moving (a filesystem move + module-path edits + a harness search-root
change; module resolution is generic path-mapping, so no compiler change; no `selfhost/` file
moves) → no re-bless / `selfhost` mirror, both bootstrap fixed points byte-identical. (`secret`
is a keyword, so the boundary module is `Sentinel::secrets`, plural.) Done in two four-check-green
phases (relocate, then carve out the core). Windows-verified; see ADR 0064 + HANDOVER §0.

**Latest (2026-06-28) — a module-search path (ADR 0037 point 12).** `snc` now
resolves a `use`d module's file by trying the entry file's own directory first
(the unchanged primary root) and then **fallback search dirs**: the repeatable
`--lib-path <dir>` flag (on `build` / `build --lib`/`--shared` / `build --separate`)
and the `SNC_LIB_PATH` env (path-separated, honored by every subcommand), tried in
that priority order — so an entry OUTSIDE the library tree can `use std::…` (e.g.
`demos/win32/messagebox_compact.sentinel` builds in place against the repo `std/`).
First hit wins (a local module still shadows a library one); when none is configured,
discovery is byte-identical to before. NOT oracle-moving (it changes only where a
module's source is read, not the lowered program) → no re-bless / `selfhost` mirror.
Windows-verified; see HANDOVER §0.

**Latest (2026-06-28) — code signing & supply-chain trust, v1 (ADR 0061
ACCEPTED-WITH-AMENDMENTS).** A new **`sentinel-trust`** crate + `snc` subcommands make
*"the code you build is the code you reviewed, signed by an authorized party"* a build
property (BACKLOG2 §2 / AI_TOOLING §7.1). **Verify** — in-process Ed25519 + SHA-512 in
Rust, the **byte-identical twin** of `std::security::ed25519` (KAT-validated on RFC 8032),
so the build-time verifier sits inside `snc`'s trust boundary. **Sign** stays Sentinel
(the dogfooded `tools/trust/{sign,keygen}_core.sentinel`); **`snc keygen` / `snc sign` /
`snc verify`** orchestrate. The signature covers the artifact's **raw bytes — comments
included** (only the carrier is excluded; "verify the exact bytes you compile"), via a
Rust-only canonical payload, so there is no second format impl. **`snc build
--require-signatures off|warn|strict`** + `--trust <manifest>` (the consumer
`sentinel-trust.toml`) gates the build (D7); **capability bounding** (D6) refuses a
trusted-but-over-reaching module (v1 enforces `ffi`). NOT oracle-moving (a driver+resolve
gate; the in-file sig block is a `//` comment) → no re-bless / `selfhost` mirror.
Windows-verified; see ADR 0061 + HANDOVER §0.

**Latest (2026-06-28) — file-level conditional compilation (ADR 0062).** A program or
`std` module can now ship platform-specific codepaths/libraries. Module `a::b` resolves
per the active target, most-specific first, to `b_<os>.sentinel` → `b_unix.sentinel`
(linux/macos) → `b.sentinel`; the `_<os>` suffix is a *selector* stripped to recover
module `a::b`, so importers always write `use a::b::item`. Target = the host OS by default
(codegen is host-only, ADR 0060); **`snc build --target windows|linux|macos`** selects
another platform's codepaths. Resolution stays base-major over the ADR 0037 search path,
suffix-minor within a base (a local module still shadows a library one). NOT oracle-moving
(only the resolver's candidate list grows; no `selfhost` file uses a suffix → byte-identical
resolution; no lex/parse/IR change) → no re-bless / `selfhost` mirror. Item-level `#[cfg]`
(in-file, oracle-moving) is the deferred follow-up. Windows-verified; see ADR 0062.

- **Phase A — `sentinel-broker`** ✅ complete. Production-shape memory
  subsystem (generational arenas, bump + slab strategies, scoped budgets,
  secret-memory policy: `mlock` + zero-on-free). Also backs compiled programs
  as per-scope bump arenas (C5.4). See **Section A**.
- **Phase B — `sentinel-effects-proto`** ✅ complete. Sentinel-Mini research
  interpreter: Hindley-Milner inference, row-polymorphic effect tracking, deep
  handlers, `secret T` constant-time check. Validated the design. See
  **Section B**.
- **Phase C — the bootstrap compiler** ✅ complete, **closed at Sentinel 1.0**
  (ADR 0025 + 0030 ACCEPTED). Every `sentinel-*` crate (syntax/ast/resolve/
  types/borrow-check/effect-check/hir/mir/codegen/runtime/driver) lowers the
  full language to native code via LLVM 18. Headline: **machine-verified
  constant-time `secret`** — a MIR pass (`sentinel::mir::secret_leak`) rejects
  any secret reaching a branch, a memory index, or a divisor (the type system
  is the taint oracle; the check runs pre-LLVM, so it constrains the program,
  not the optimized machine code — see the README for the precise boundaries).
  See **Section C**.
- **Phase D — self-hosting** ✅ the **bootstrap fixed point is reached**: the
  Sentinel-built compiler (`scg`) compiles its own multi-module source to LLVM
  IR byte-identical to the Rust `snc` oracle, and the resulting binary
  reproduces itself. Two movements:
  - **Movement 1 — language build-out** (ADR 0031–0037): D.1 sum types +
    `match`, D.2 strings + `u8`, D.3 growable `Vec<T>`, D.4 file I/O, D.5
    `while`/`break`/`continue`, D.6 modules / multi-file (`use`).
  - **Movement 2 — the port** (ADR 0038–0045 ACCEPTED-WITH-AMENDMENTS): every
    stage of `snc` rewritten in `selfhost/*.sentinel`, validated byte-for-byte
    against the Rust oracle; **full-corpus codegen parity reached** (all 123
    pass fixtures). The Rust `snc` stays the bootstrap seed + differential
    oracle.
- **Per-unit separate compilation** (ADR 0037 (a)) 🟢 **functionally
  complete.** `snc build --separate` compiles each module to its own object,
  linked at link time via module-qualified `abi-v1` symbols. Every `pub` item
  kind crosses a module boundary — fns, structs, enums, generics, traits,
  effects, including cross-**unit** `perform`/`handle`. Generic instances
  dedup across importers via origin-qualified `linkonce_odr` symbols
  (primitives + cross-module structs + enums). Rebuilds are **incremental** at
  ITEM granularity (an `.o.fp` content-fingerprint sidecar; an unchanged unit
  reuses its cached `.o`). Opt-in until full parity with the merge path
  (`snc merge` / `snc build`, both still green). Remaining tail (lower value):
  class / generic-instance type-arg dedup, trait/class-method dedup. Full
  detail in the `sentinel_separate_compilation` auto-memory + ADR 0037.
- **`sentinel-lsp`** — stub (post-1.0, ADR 0025 D10).
- **Core libraries + examples-as-tests** 🟢 **underway** (the active track). A
  top-level `std/` (functional categories) + `examples/` corpus of real,
  idiomatic Sentinel programs that double as feature tests — each built BOTH via
  `--separate` and the merge path and asserted on (`crates/sentinel-driver/tests/
  examples.rs`). Shipped libraries: `std/security` (`ct` constant-time primitives
  incl. `ct_memcmp` + `ct_vec_eq` + `ct_rotl64`/`ct_rotl32`/`ct_rotr32`; `siphash`
  SipHash-2-4 keyed MAC; `chacha20` block + stream cipher; `poly1305` one-time MAC
  over a secret key; `aead` ChaCha20-Poly1305 AEAD composing them; `sha256` /
  `sha512` constant-time SHA-256 / SHA-512 + `sha3` SHA3-256/512 + SHAKE128/256 XOFs +
  KMAC128/256 keyed MACs + cSHAKE128/256, TupleHash128/256, ParallelHash128/256 (the
  Keccak sponge / SP 800-185 derived functions) over a `secret` message; `hmac`
  HMAC-SHA256 over a `secret` key; `hkdf` HKDF-SHA256 extract-then-expand key
  derivation (RFC 5869, composing `hmac`); `aes` a constant-time AES-128 block cipher with a
  table-free, field-inversion S-box; `aes_gcm` constant-time AES-128-GCM AEAD (GHASH
  GF(2^128)); `fe25519` the shared GF(2^255-19) field; `x25519` constant-time X25519
  ECDH over a `secret` scalar (RFC 7748 — the first public-key primitive); `ed25519`
  constant-time Ed25519 SIGNING + verification over a `secret` seed (RFC 8032,
  composing fe25519 + sha512; verify decompresses a point via a field square
  root); `fe448` the shared GF(2^448-2^224-1) field (28 radix-2^16 limbs — radix
  2^28 would overflow i64, so the small radix keeps the multiply within i64) +
  `x448` constant-time X448 ECDH over a `secret` scalar (RFC 7748) + `ed448`
  constant-time Ed448 SIGNING + verification over a `secret` seed (RFC 8032, composes
  fe448 + SHAKE256; the edwards448 a=1 law + a branch-free Barrett reduction mod the
  group order); `fe25519_64` the GF(2^255-19) field at **radix 2^51** (5 limbs, the
  idiomatic ref10/donna form — the field multiply runs in `secret u128`, ADR 0055's
  new 128-bit integer type) + `x25519_64` X25519 over it (cross-checked byte-for-byte
  against the radix-2^16 `x25519`)), `std/math/num`,
  `std/math/float` (the PUBLIC `f64` helpers — `abs`/`min`/`max`/`sq`/`hypot`/`lerp`/
  `trunc`/`discriminant` + the **libm transcendentals** `sine`/`cosine`/`tangent`/`exp_e`/
  `ln`/`powf`/`round_down`/`round_up`/`angle_of` bound through the ADR 0057 FFI — the float
  counterpart to `std/math/num`, exercised by `examples/math/quadratic` +
  `examples/math/transcendental`, ADR 0058),
  `std/sys/posix` (the first `std/sys/*` FFI wrapper — `pid`/`parent_pid`/`uid`/`gid` over
  the libc identity calls via `extern "C"`, ADR 0057; `examples/sys/process_ids`),
  `std/bits/bits` (rotates),
  `std/bytes/bytes` (`[u8]`
  utilities over `&[u8]` borrows), `std/text/str` (the string library: case folding,
  trim, substring search, slice, concat/repeat/replace, lexicographic compare, decimal
  `parse_int`/`int_to_str` + the float `parse_f64`/`f64_to_str` (ADR 0058), index-based
  `split_count`/`split_nth`, pad — exercised by
  `examples/text/str_demo`), `std/algorithms/seq` (in-place insertion
  `sort` + `binary_search` over public `[i64]`), `std/collections/map` (a string-keyed
  hash map `[u8]`→`i64`: parallel-array + index-chaining storage, public FNV-1a hash,
  power-of-two resize/rehash; `examples/collections/map_demo`), `std/data/json` (a JSON
  parser + serializer over a cons-list recursive `Json` enum — integer `Num(i64)` AND
  non-integer `Float(f64)` numbers (ADR 0058: the parser detects a `.`/exponent, the
  serializer emits via `f64_to_str`), strings with escapes, booleans, null, nested arrays +
  objects; `examples/data/json_demo`),
  and `std/net` (`tcp` — a thin
  ergonomic layer over the raw socket builtins: `read_exact` + the secret<->public
  byte-boundary helpers both socket examples share; `ssh` — the SSH-2
  transport key exchange, `curve25519-sha256` + `ssh-ed25519`, RFC 4253 / RFC 8731; +
  `ssh_cipher` — the `chacha20-poly1305@openssh.com` binary-packet record cipher that
  protects every packet after NEWKEYS; stitched into a complete end-to-end loopback
  session in `examples/net/ssh_session` (KEX → host-key auth → key derivation → an
  encrypted application packet): together the constant-time crypto core of an `sshd`,
  run loopback. Real kernel **sockets are now live** (ADR 0056): the
  `tcp_listen`/`local_port`/`accept`/`connect`/`read`/`write`/`close` builtins (libc,
  loopback-only), exercised by `examples/net/tcp_echo` — a real localhost TCP echo with
  a concurrent `spawn`ed server task — and `examples/net/ssh_over_tcp`, which runs the
  whole SSH-2 handshake **over a real socket**: separate concurrent client/server tasks
  do a distributed curve25519-sha256 KEX (each derives K independently; the wire-public
  values are `declassify`d to send and widened back to `[secret u8]` on receipt, K never
  crossing the wire), host-key auth, key derivation, and an encrypted record round-trip.
  `examples/net/ssh_channel_stream` runs the **data phase**: a bidirectional, multi-packet
  encrypted channel — both directional keys (`C`/`D`), a request/response ping-pong of N
  `chacha20-poly1305@openssh.com` records each way under per-direction sequence numbers
  (a fresh nonce per packet), each record tag-verified before it is decrypted. The KEX is
  now factored into a reusable API (`std/net/ssh_handshake`:
  `ssh_client_handshake`/`ssh_server_handshake` run a handshake half over a socket and
  return an `SshSessionKeys` struct incl. the session id), and `examples/net/ssh_full_session`
  runs a **complete session** — handshake + data phase — over one connection via it. On top,
  `std/net/ssh_userauth` + `examples/net/ssh_pubkey_auth` add RFC 4252 §7 **publickey
  authentication**: the client signs a request bound to the session id (the first exchange
  hash H) so a signature can't be replayed across sessions, and the server verifies the
  Ed25519 signature and enforces authorized_keys before granting access. And the connection
  layer (`std/net/ssh_connection` + `examples/net/ssh_channel_window`) adds RFC 4254 channel
  messages — a "session" channel with **windowed flow control** (a 32-byte payload crosses in
  two 16-byte `CHANNEL_DATA` chunks gated by a server-replenished window). So the full SSH-2
  sequence a real `sshd` runs — transport KEX → host-key auth → key derivation → encrypted
  channel → publickey user auth → windowed session channel — now runs over real sockets,
  and `examples/net/ssh_exec` runs a `CHANNEL_REQUEST "exec"` command request on top
  (`ssh user@host some-command`: open a channel, run a command, read back its output, exit
  status, and close; the framed record I/O `ssh_send_record`/`ssh_recv_record` lives in
  `std/net/ssh_connection`).
  The
  crypto examples reproduce the
  canonical test vectors (SipHash `0xa129ca6149be45e5`, RFC 8439 §2.3.2 / §2.4.2 /
  §2.5.2 / the full 114-byte §2.8.2 AEAD vector; NIST SHA-256 "abc"/""/multi-block;
  RFC 4231 HMAC-SHA256 TC1/TC2/TC6; FIPS-197 §C.1 + AES-128(0,0); RFC 7748 §5.2 +
  §6.1 X25519/DH; NIST SHA-512 "abc"/""/multi-block; the McGrew/OpenSSL AES-GCM "TC4"
  vector; RFC 8032 Ed25519 sign+verify vectors; NIST SHA3-256/512). It has surfaced +
  closed **eight** language
  gaps so far, each ADR-first; seven are fully self-hosted (snc + `scg`,
  byte-identical, both fixed points held — ADR 0047 / 0048 / 0049 / 0050 / 0052 /
  0053 / 0054), and ADR 0051's operand widen is too (its call-arg / array / return
  widens are snc-side ergonomics):
  - **`[secret T]` arrays** — arrays of secret elements (ADR 0047,
    `ArrayElem::Secret`) — enabling a variable-length constant-time `memcmp` over
    secret bytes (`examples/security/ct_memcmp`).
  - **Shift operators `<<` / `>>`** (ADR 0048; logical right shift; a shift by a
    *secret* amount is rejected, a secret value by a public amount is
    constant-time) — enabling `std/bits` + a SipHash-style ARX round over secret
    words (`examples/security/siphash_round`).
  - **Integer cast `x as T`** (ADR 0049) — closes the "i32 unconstructible" gap
    (an int literal is `i64`); trunc/sext/zext, preserves secrecy, no CT sink —
    enabling a true 32-bit **ChaCha quarter-round** over `secret i32` words
    reproducing the RFC 8439 vector (`examples/security/chacha_qr`).
  - **Mutable index assignment `a[i] = v`** (ADR 0050) — lifts the ADR 0017 D12
    deferral so an array / `Vec` element can be written through a public index
    (the read path's bounds-checked element GEP + a store). Constant-time
    preserved with no new sink (a secret LHS index is rejected by the existing
    `IndexNotInt` rule, exactly as for reads; a secret value stored is fine).
    MVP scope is Copy elements (scalars + `secret` scalars). Enables an
    idiomatic, loop-based full **ChaCha20 block** over a `[secret i32]` state
    permuted in place, reproducing the RFC 8439 §2.3.2 vector
    (`examples/security/chacha20_block`).
  - **Implicit public→secret widening** (ADR 0051) — a public `T` widens to
    `secret T` in operand position (`secret_x + 5`), as a call argument, a return,
    and `[u8] → [secret u8]` — removing the "bind every constant secret first"
    ceremony the crypto MACs hit. Monotone + a codegen no-op; the constant-time
    sinks are untouched (shift amount / divisor / branch / index still reject).
    The operand widen is self-hosted (`tests/pass/c56_operand_widen`); the
    call-arg/array/return widens are snc-side.
  - **`Vec<secret T>` growable secret buffers** (ADR 0052, `VecElem::Secret`) — the
    sibling of `[secret T]` on the `Vec` path: a *variable-length* secret byte
    buffer (`Vec<secret u8>`) is built with `vec_new` + `push` and indexed to yield
    `secret u8` (public index → the constant-time taint; pointer/length/capacity
    public; a secret index rejected). Front-end-only like `[secret T]`, plus a
    secret-aware generic-substitution round-trip (`vec_new<T>()` over `T:=secret u8`)
    since the `Vec` builtins are generic. Unblocks message-length-independent
    constant-time code — `ct::ct_vec_eq` over two growable secret buffers
    (`examples/security/ct_vec_eq`); validated `scg`==`snc` byte-for-byte
    (`tests/pass/c57_secret_vec`).
  - **`vec_to_array` over a secret `Vec`** (ADR 0053, `to_array_elem_subst`) — the
    symmetric completion of ADR 0052: `Vec<secret u8> → [secret u8]`, so a growable
    secret buffer feeds the existing `[secret u8]`-taking crypto. The array
    substitution demote is made secret-aware (the twin of 0052's `to_vec_elem_subst`);
    no codegen/`scg` change (`tests/pass/c58_secret_vec_to_array` byte-identical). It
    is the payoff that makes the full 114-byte §2.8.2 vector idiomatic (build the
    secret plaintext by `push`, then `vec_to_array`).
  - **`&mut a[i]` / `&a[i]` element borrows** (ADR 0054) — borrowing an array / Vec
    element as a reference, completing the mutable-array story ADR 0050 began
    (read `a[i]`, write `a[i] = v`, now borrow `&mut a[i]`). A one-arm type-checker
    relaxation (`Index` joins `FieldAccess` as a borrow target), the element-address
    GEP reused from ADR 0050; whole-array borrow granularity (pre-Polonius); a secret
    index still rejected (`IndexNotInt`). Fully self-hosted with NO `scg` change
    (`tests/pass/c59_borrow_index` byte-identical); `examples/math/inplace` clamps
    array / Vec elements in place via `clamp_assign(&mut a[i], …)`.
  The crypto MACs (SipHash / ChaCha20 stream / Poly1305) drove ADR 0051, the
  variable-length secret-buffer friction drove ADR 0052, and feeding a built-up
  secret buffer to the `[secret u8]` crypto drove ADR 0053 —
  "build real programs → find the gap → fix it". On the now-rich surface, the track
  also shipped (no language change except ADR 0054) the full RFC 8439 §2.8.2 114-byte
  AEAD vector, a constant-time **SHA-256** + **SHA-512** over a secret message,
  **HMAC-SHA256** over a secret key, a constant-time **AES-128** block cipher (`aes`),
  **AES-128-GCM** AEAD (`aes_gcm`, the GHASH GF(2^128) carry-less multiply as the new
  field primitive), constant-time **X25519** ECDH (`x25519`, the first public-key
  primitive), and constant-time **Ed25519** SIGNING + verification (`ed25519`, RFC 8032, composing
  the shared `fe25519` field + `sha512`; verify decompresses a point via a field
  square root and enforces the `S < L` canonicality check), and **SHA-3** /
  SHA3-256/512 + the **SHAKE128/256 XOFs** + the **KMAC128/256 keyed MACs** (`sha3`,
  the Keccak sponge — a different construction shape from SHA-2's Merkle-Damgård; the
  XOFs add arbitrary-length output via a multi-block squeeze, and KMAC (SP 800-185)
  wraps a secret key around cSHAKE). AES, X25519, and Ed25519 are its sharpest constant-time
  demonstrations: in each, the textbook implementation has a key-dependent side
  channel that does not compile — AES's table-lookup S-box (`sbox[secret_byte]`) is a
  secret value indexing memory, and the X25519 / Ed25519 scalar-multiplication ladders
  would branch on the secret scalar bits — so Sentinel forces the constant-time form
  (a table-free field-inversion S-box, a branch-free mask-based conditional swap). See
  the `sentinel_examples_and_corelibs` auto-memory.
  Beyond those self-hosted gaps, the track added **two new scalar TYPES, `snc`-side only**
  (a `Type` variant causes no `FnId` shift, so — like the demonstrators living in
  `examples/`, not `tests/pass` — the `scg` mirror is deferred and every selfhost
  differential + both bootstrap fixed points stay byte-identical): **`u128`** (ADR 0055,
  the 128-bit integer for radix-2^51 field arithmetic, `secret`-able) and **`f64`** (ADR
  0058, an IEEE-754 double for "math functions" beyond integers). `f64` is **PUBLIC-ONLY**:
  float ops are not constant-time on real hardware, so `secret f64` is a type error and
  floats are a disjoint public domain — the constant-time guarantee is unweakened (contrast
  `secret u128`, valid because integer ops can be constant-time). `f64` ships float
  literals (`3.14`/`1e9`, stored as IEEE bits), `+ - * /` + unary `-` + ordered `fcmp`,
  the int↔float `as` casts (`sitofp`/`fptosi`/…), and `sqrt` (the `llvm.sqrt.f64`
  intrinsic, surfaced as a `UnaryOp` so it needs no `FnId`); `std/math/float` +
  `examples/math/quadratic` (the quadratic formula + 2-D geometry) are the demonstrator.
  The **`extern "C"` FFI import (ADR 0057, Phase 1 value ABI)** is now implemented too: a
  user-declarable `extern "C" { fn …; }` block over the public `i64`/`f64` ABI, resolved
  by SYMBOL NAME (a user-range `FnId`, like a cross-module import — no builtin `FnId`
  shift) and declared `External` under the bare C symbol so `cc` links it against libc; a
  `secret` reaching an `extern` arg is rejected (the FFI fence). It makes OS / native
  bindings *library* work: `std/sys/posix` (process identity) + the `std/math/float` libm
  transcendentals are the wrappers. **Phase 1b (ADR 0057 A6) now adds the `ptr` opaque type
  + `ptr_of`/`ptr_of_mut`** — a Sentinel buffer's data pointer crosses to a pointer-taking
  libc call (no runtime change, no `FnId` shift: `ptr` is a `Type` variant and `ptr_of` is a
  `UnaryOp` intrinsic, like `f64`/`sqrt`). `std/sys/ffi` wraps `getentropy` (OS randomness
  into a `[u8]` via `ptr_of_mut`) + `strlen` (over a NUL-terminated `cstr` via `ptr_of`); the
  fence holds: `ptr_of` of a `&[secret u8]` is rejected. **A7 completes the import buffer ABI
  with the C-string READ-back**: an `is_null(ptr) -> bool` intrinsic + `cstr_read`/`env_get`
  (null-safe `strlen` + `memcpy`, both libc externs — no runtime change), so `getenv` is
  callable from Sentinel; `examples/sys/ffi_buffers` proves all of it (`strlen("hello")==5`,
  two `getentropy` draws differ, `env_get("PATH")` non-empty + a bogus var empty). Still
  deferred: distinguishing unset from empty (`?[u8]`), `i32`/`u32` widths, by-pointer
  structs, extra-library `-l`, `dlopen`, Win32. And the **C-ABI export (ADR 0059, Phase 1a value
  ABI)** — the inverse, Sentinel-as-a-library — is now implemented too: an `export "C" fn`
  annotation (un-mangled C symbol, secret-fenced) + a `snc build --lib` static-archive
  mode (no `main`; the object + the runtime staticlib bundled into one `.a`) +
  `--emit-header`. THE HEADLINE works end to end: a C program (`examples/export/driver.c`)
  links the snc-built `.a` + header and calls a Sentinel `export "C"` constant-time
  conditional select that widens its public ints to `secret`, runs the machine-checked
  branch-free select, and `declassify`s — a foreign caller getting a **verified
  constant-time primitive over a plain C ABI** (`tests/export.rs` asserts exit 42). So
  **all three big-list rocks (0057 FFI import · 0058 floats · 0059 C-ABI export) are now
  implemented.** And the export side reaches real byte-buffer crypto (Phase 1b): a `&[u8]`
  export param is presented to C as a `(const uint8_t*, int64_t)` pair via a generated
  wrapper, so `examples/export/ct_select::ct_byte_eq` — a verified constant-time byte
  comparison (the MAC/tag-verification primitive) — is callable from C over real buffers.
  **The owned-`[u8]` RETURN ABI is now implemented too (ADR 0059 A7)** — an `export "C" fn
  … -> [u8]` hands C a heap buffer via two generated trailing out-params
  `(uint8_t** out_data, int64_t* out_len)` (the out-param convention, chosen over a
  by-value struct return for ABI robustness), freed with the exported `sentinel_free_bytes`.
  THE HEADLINE THESIS now works end to end: `examples/export/digest_lib::sha256_oneshot` is
  a **verified-constant-time SHA-256 callable from C** (it widens public bytes to `secret`,
  runs the machine-checked compression, declassifies the digest), and a C driver
  (`examples/export/digest_driver.c`) checks it against the NIST "abc" vector and frees the
  buffer — verified constant-time crypto as a drop-in C library, the whole point of the
  export side; `repeat_byte` shows a variable-length return. **And `--lib` is now
  MULTI-MODULE (ADR 0059 A8)** — it discovers the `use` graph and merges it (the executable
  build's Path-A machinery), so a library can span modules: `examples/export/crypto_lib`
  `use`s the REAL `std::security::sha256` + `std::security::hmac` (no inlining) and exports
  `sha256_oneshot` + `hmac_sha256_oneshot` to C (checked against NIST SHA-256 / RFC 4231
  HMAC vectors) — the verified-constant-time crypto suite as a drop-in C library, at scale.
  **And `--shared` now emits a `.dylib` (ADR 0059 A9)** for `dlopen` / `ctypes` — the same
  PIC object, linked `cc -dynamiclib` instead of archived; `examples/export/dlopen_driver.c`
  `dlopen`s a `--shared`-built `digest_lib.dylib`, `dlsym`s `sha256_oneshot` +
  `sentinel_free_bytes`, and runs the verified-constant-time SHA-256 through the shared
  library (the substrate the Python/Rust binding generators build on). Still deferred: the
  caller-provides-buffer convention (fixed-size, no-alloc outputs), the Linux `cc -shared`
  path, and the per-language binding generators.
  The float follow-ups also landed: the libm transcendentals (above), `f64`⇄string conversion
  (`std/text/str::parse_f64`/`f64_to_str`), and `std/data/json` now parsing/serializing
  non-integer numbers as `Float(f64)`.

**1636 tests across the workspace**, four-check green (build · `cargo nextest`
· `cargo test --doc` · `clippy -D warnings`). macOS / Apple Silicon / LLVM 18.

## Section A — sentinel-broker

Production-shape crate. The broker provides generational arenas,
two allocation strategies, scoped budgets, secret memory, recording,
and diagnostics. Intended for adoption beyond the Sentinel compiler
itself per HANDOVER §4.

### A.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| A0    | Dev dependencies (thiserror, tracing, proptest)    | Done   | 9c7474d |
| A1    | Foundation types (ArenaId, Generation, ...)        | Done   | 9c7474d |
| A2    | Bump arena + generational handles + destroy_arena  | Done   | 9c7474d |
| A3    | Pluggable AllocStrategy (Bump + Slab), builder     | Done   | f606d19 |
| A3.5  | Per-slot generations + slab recycling              | Done   | 37ab02b |
| A4    | Scoped allocation budgets                          | Done   | 493ee7b |
| A5    | Stats, list_arenas, where_is                       | Done   | 15d751c |
| A6    | Recording mode (event log, ring buffer)            | Done   | 2e8fb8b |
| A7    | Secret-memory policy (mlock + zero-on-free)        | Done   | f3170bf |
| A8    | Validation examples / integration demos            | Done   | 683981d |
| A9    | Fallible builders + BrokerError carries OS detail  | Done   | 755e710 |

Test coverage as of A9: 69 tests (62 lib + 5 integration + 2
proptest). The count dropped from 70 → 69 between A8 and A9 because A9
incidentally removed `strategy::slab::tests::slab_free_returns_not_implemented`,
an obsolete test that survived A3.5 (slab recycling) and asserted the
*opposite* of the correct slab behavior — slab DOES support free as of A3.5.
The correctly-named `bump_free_returns_not_implemented` (which matches
invariant #3) is retained.

Doctests: 1 passing + 6 ignored. Clippy clean under `-D warnings`
across crate and examples.

### A.2 Crate Layout

    crates/sentinel-broker/
      Cargo.toml
      benches/arena_bench.rs
      src/
        lib.rs            crate root + re-exports
        arena.rs          Arena (strategy + recorder + counters)
        broker.rs         Broker, ArenaHandle, within_budget
        budget.rs         Budget, BudgetScope, BudgetArenaBuilder
        builder.rs        ArenaBuilder (bump/slab + try_bump/try_slab as of A9)
        error.rs          BrokerError enum (no longer Copy as of A9)
        handle.rs         Handle<T>, HandleRef<'a, T>
        ids.rs            ArenaId, BudgetId, SlotIndex, Generation,
                          SlotGeneration, monotonic counters
        recording.rs      Recorder, Event (A6)
        secret.rs         SecretPolicy, SecretStrategy (A7)
        stats.rs          BrokerStats, ArenaSummary, HandleLocation (A5)
        strategy/
          mod.rs          AllocStrategy trait + AllocOk/SlotPtr/StrategyKind
          bump.rs         BumpStrategy
          slab.rs         SlabStrategy (freelist + per-slot generations)
      examples/
        token_bucket.rs       high-frequency slab allocation
        request_pipeline.rs   bump-per-request under budgets, with recorder
        credential_store.rs   secret slab with STRICT/LENIENT, raw zero-on-free verification
      tests/
        integration.rs    end-to-end API tests
        proptest.rs       property-based isolation/invalidation tests

### A.3 Public API Surface

All re-exports from `sentinel_broker`:

- Core: `ArenaHandle`, `Broker`, `Arena`, `Handle`, `ArenaBuilder`
- IDs: `ArenaId`, `BudgetId`, `Generation`, `SlotGeneration`, `SlotIndex`
- Strategies: `AllocStrategy`, `StrategyKind`
- Budgets (A4): `Budget`, `BudgetScope`, `BudgetArenaBuilder`
- Stats (A5): `ArenaSummary`, `BrokerStats`, `HandleLocation`
- Recording (A6): `Event`, `Recorder`
- Secret (A7): `SecretPolicy`, `SecretStrategy`
- Errors: `BrokerError`

#### A.3.1 Broker

- `Broker::new()`
- `Broker::with_recorder(Arc<Recorder>)` (A6)
- `broker.create_arena(name, capacity) -> ArenaHandle`
- `broker.arena(name).capacity(n).bump() -> ArenaHandle` (panics on
  misuse; see also `try_bump`)
- `broker.arena(name).slab(slot_size, slot_align, slot_count) -> ArenaHandle`
- `broker.arena(name).secret(policy).bump()/.slab(...)` (A7)
- `broker.arena(name).try_bump() -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.arena(name).try_slab(...) -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.destroy_arena(id)` invalidates all handles
- `broker.live_arena_count()`
- `broker.stats() -> BrokerStats` (A5)
- `broker.list_arenas() -> Vec<ArenaSummary>` (A5, sorted by id)
- `broker.where_is(&handle) -> Option<HandleLocation>` (A5)
- `broker.recorder() -> Option<&Arc<Recorder>>` (A6)
- `broker.within_budget(cap, |scope| { ... })?` (A4, nestable)

#### A.3.2 Arena / ArenaHandle

- `arena.alloc(value) -> Result<Handle<T>, BrokerError>`
- `arena.free(&handle)` (slab only; bump returns `NotImplemented`)
- `handle.get() -> Result<&T, BrokerError>`
- `handle.is_live()`, `arena_id()`, `slot()`, `slot_generation()`
- `arena.__raw_slot_bytes_for_diagnostics(slot)` (A8, doc-hidden,
  forensic tooling only)

#### A.3.3 BrokerError variants

`UseAfterFree`, `UseAfterFreeSlot`, `OutOfMemory`, `InvalidSlot`,
`UnknownArena`, `BrokerPoisoned`, `BudgetExceeded`, `NotImplemented`,
`SecretMemory { reason: String, os_errno: Option<i32> }` (A9 shape),
`BuilderMisuse { reason: &'static str }` (A9).

As of A9, `BrokerError` no longer derives `Copy`. `SecretMemory`
carries the underlying OS error number where available (previously
the OS errno was only logged via `tracing::warn!`).

### A.4 Design Invariants

These are properties the test suite enforces; future changes must
preserve them.

1. Arena destruction invalidates every handle. `destroy_arena(id)`
   removes the broker's `Arc<Arena>` from its map and calls
   `Arena::invalidate()`, which advances generation atomically.
   `Handle::get()` then returns `BrokerError::UseAfterFree`.

2. Per-slot generations defeat ABA. Each slab slot has its own
   generation counter. Reusing a slot increments it, so a handle to
   the prior occupant returns `BrokerError::UseAfterFreeSlot`.

3. Bump strategy never recycles. `BumpStrategy::free` returns
   `NotImplemented`; only `SlabStrategy` supports free. Bump's whole
   point is O(1) bulk free via arena destruction.

4. Budgets pre-charge reserved capacity. `arena("a").capacity(N).bump()`
   inside `within_budget(cap, ...)` charges N to the budget chain
   BEFORE the arena exists. Reservation, not usage, is what counts.
   Nested budgets charge both inner and every ancestor.

5. Budget refunds are atomic. If `try_charge` walks the chain and
   exceeds any cap, all prior charges in that walk are refunded
   before returning `BudgetExceeded`.

6. Recording never affects behaviour. If no recorder is attached,
   the hot path is an `Option::None` branch. If recording fails
   (mutex poisoned), the event is dropped silently — recording is
   observation, not enforcement.

7. All counters use `Ordering::Relaxed`. Snapshots from `stats()`
   may show momentary inconsistency across fields under concurrent
   load. This is expected and acceptable.

8. `Broker::with_recorder` is construction-time only. No runtime
   swap, no `AtomicPtr`. The recorder is set once and read on every
   event-emitting path via `&Option<Arc<Recorder>>`.

9. (A9) Panicking and fallible builders coexist. `.bump()` and
   `.slab()` panic on misuse and are kept for tests and demos that
   want construction-time failure surfacing as a panic. `.try_bump()`
   and `.try_slab()` are exact structural twins returning `Result`.
   New code prefers the fallible variants.

### A.5 Known Limitations / Tech Debt

- `Arena::with_strategy` is `#[allow(dead_code)]` — kept as a
  convenience wrapper but only the recorder-aware variant
  `with_strategy_and_recorder` is used.
- `Recorder` uses `Mutex<Vec<Event>>`; under very high concurrent
  allocation it serializes through one mutex. Acceptable for now.
- Bounded-ring `Recorder::record` uses `Vec::remove(0)` (O(n)) on
  overflow. A `VecDeque` would be better for larger caps.
- No benchmark gate in CI. `benches/arena_bench.rs` exists but is
  not exercised on every PR.
- Doctests are ignored to avoid pulling test-only types into the
  public examples. Should be tagged `no_run` and fleshed out before
  publishing.
- (BACKLOG §0.1 remaining) Bump `slot_size_hint` is `None`; the
  `SlotInfo.size` field exists but is dead-code. Either return it
  from a bump-side override or document diagnostics as slab-only.
- (BACKLOG §0.1 remaining) `Event` variants are not `#[non_exhaustive]`.
  Consumers serializing events would see field additions as
  breaking changes.

---

## Section B — sentinel-effects-proto (Sentinel-Mini)

Research-grade tree-walking interpreter. Built to validate
Sentinel's effect-system design before committing to the Phase C
production compiler per HANDOVER §5. The crate is explicitly
expected to be thrown away or rewritten once its lessons are
absorbed.

### B.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| B0    | Scaffold: lex + parse + eval, no types or effects  | Done   | d090ca1 |
| B1    | HM type inference, letrec, span-tracked errors     | Done   | e6b06cd |
| B2.0  | Row scaffold: Ty::Fun carries empty Row            | Done   | 2cd81a7 |
| B2.1  | Remy-style row unification                         | Done   | 323ab33 |
| B2.2a | Effect-surface lexer/AST/parser                    | Done   | a3fd3cc |
| B2.2b | Wire Perform through pipeline as type error        | Done   | 62405f2 |
| B2.3a | Effect-inference judgment refactor (no semantics)  | Done   | fd8eef6 |
| B2.3b1 | Row mechanics (Lambda mints ρ, App via arrow_with); default-close residual rows | Done   | f2a17d9 |
| B2.3b2-a | Perform inference + UnknownEffect; eff_env from prog.effects | Done   | 4c69ed7 |
| B2.3b2-b | Row generalization in Scheme; instantiate freshens row vars; drop EffectNotYetSupported | Done   | 47cc5a1 |
| B2    | Effect rows and effect declarations                | Done   |        |
| B3.0  | Handler surface (lexer + parser + AST + placeholders) | Done   | 821b16a |
| B3.1a | row_split + HandlerLabelNotInRow                   | Done   | febf379 |
| B3.1b | Handler typing rule + DuplicateHandlerArm          | Done   | e7958e1 |
| B3.2a | Handler runtime scaffolding (Step/Continuation/Frame)| Done   | bdda217 |
| B3.2b | Handler runtime (Perform reifies, Handle dispatches) | Done   | a9cefb1 |
| B3.2c | Positive runtime coverage (4 integration tests)      | Done   | 8e3de20 |
| B3.2  | Handler runtime (operation reification + dispatch)   | Done   |         |
| B4.0a | Surface AST + SecretsNotYetSupported placeholder     | Done   | 1693b8c |
| B4.0b | Lexer Token::Secret + Token::Declassify              | Done   | 63cd57b |
| B4.0c | Parser secret prefix + declassify atom + DoubleSecret| Done   | 0b6b2ce |
| B4.0  | Secret/declassify surface (B4 phase 0 of 3)          | Done   |         |
| B4.1a | 4 TypeError variants + D2 unify + Declassify typing  | Done   | e760d57 |
| B4.1b | D3 If/Div + D4 comparisons; drop placeholder         | Done   | 52acc0a |
| B4.1  | Secret typing (unify, infer, four CT rejections)     | Done   |         |
| B4.2  | Three Phase B validation demos + README/STATE refresh| Done   | 9541969 |
| B4    | Secret T qualifier and constant-time check           | Done   |         |
| B?    | Broker-as-value-heap integration (bonus)           | Planned |        |

Test coverage as of B1: 95 tests (8 lexer + 11 parser + 11 eval +
4 span + 7 types + 41 infer + 7 diag + 6 integration). Clippy
clean under `-D warnings`. No doctests yet.

Test coverage as of B2.0: 98 tests (B1 carry-over 95 + 3 new in
`types.rs`: `b20_empty_row_renders_as_empty_string`,
`b20_arrow_with_empty_row_is_unchanged_from_b1`,
`b20_rowvar_display_uses_r_prefix`). Clippy clean under
`-D warnings`; `clippy::result_large_err` allowed crate-wide in
`lib.rs` because widening `Ty::Fun` with `Row` pushed
`TypeError::Mismatch` over the 128-byte threshold. See B.5
design decision 12.

Test coverage as of B2.1: 111 tests (B2.0 carry-over 98 + 13 new
in `infer.rs`: ~10 `b21_unify_row_*` cases covering empty rows,
var binding, matching labels, mismatched payloads, label
rewriting, occurs-check, disjoint open tails, and closed-row
failures; ~3 row display tests). Clippy clean under
`-D warnings`. See B.5 design decision 13.

Test coverage as of B2.2: 134 tests (B2.1 carry-over 111 + 23
new: 6 lexer tokens, 11 parser (effect decls, do-perform,
uppercase-label rule, paren ty-expr, missing-semicolon,
arrow-required signature, right-associative arrows, do
inside arithmetic), 3 infer (Perform rejected with
EffectNotYetSupported, span targets label, pure-body
infers), 1 eval (direct-eval defence in depth), 2
integration (pure body evaluates, do is rejected with
rendered caret). Clippy clean under `-D warnings`. See
B.5 design decisions 14 and 15.

Test coverage as of B2.3a: 134 tests (B2.2 carry-over 134 + 0
new). B2.3a is a pure behavior-identical refactor per ADR 0005
D9: the inference judgment changes from `(Subst, Ty)` to
`(Subst, Ty, Row)`, every arm returns `Row::Empty`, every
recursive `infer` call site is threaded with a new `EffectEnv`
parameter that is unused at this phase, `Scheme` gains a
`row_vars: Vec<RowVar>` field (empty by default), and `infer_top`
gains a strict residual-row check that is unreachable in B2.3a.
All 134 existing test assertions pass unchanged; only the four
`Scheme { .. }` struct-literal call sites in tests were updated
to include the new (empty) `row_vars` field. Clippy clean under
`-D warnings`. See B.5 design decision 16 for the ADR 0005 D9
divergence (`TypeError::UnhandledEffects` and the strict
`infer_top` check were pulled forward from the D9 B2.3b list).

B1 landed across five commits: spans + Spanned AST + `let rec`
(abfb3d9), types scaffold (b3589ea), inference driver wired into
`run` (24a3db8), proper HM let-rec typing (72c0996), and
hand-rolled caret diagnostics (e6b06cd).

### B.2 Crate Layout

    crates/sentinel-effects-proto/
      Cargo.toml
      src/
        lib.rs            re-exports + `run()` convenience + `MiniError`
                          (now incl. `MiniError::render(src) -> String`)
        lexer.rs          logos tokeniser, `Token` (incl. `Rec`), `LexError`
        ast.rs            `Expr = Spanned<ExprKind>`, `BinOp`, `LetRec` variant
        parser.rs         hand-written recursive descent, precedence climbing,
                          `ParseError`, span-threading
        eval.rs           tree-walking interpreter, persistent `Env`,
                          `Value`, `EvalError`, `let rec` via `OnceLock`
        span.rs           `Span { start: u32, end: u32 }`, `Spanned<T>` (B1.1)
        types.rs          `Ty`, `TyVar`, `Scheme`, free-var sets (B1.4)
        infer.rs          HM Algorithm W: `Subst`, `unify`, `instantiate`,
                          `generalize`, `TypeEnv`, `infer`, `infer_top`,
                          `TypeError` (B1.4-B1.6)
        diag.rs           `LineCol`, `locate`, `render` -- hand-rolled
                          rustc-style caret diagnostics (B1.7)
      tests/
        integration.rs    end-to-end pipeline tests

### B.3 Language Surface (B0)

Pure expression calculus with HM type inference. Everything is an
expression. No statements, no effects, no `secret` yet.
Recursion is supported via `let rec` (B1.3); types are inferred
with let-polymorphism and let-rec generalization (B1.5/B1.6).

Grammar (informal):

    expr      := let | letrec | if | lambda | compare
    let       := "let" IDENT "=" expr "in" expr
    letrec    := "let" "rec" IDENT "=" lambda "in" expr
    if        := "if" expr "then" expr "else" expr
    lambda    := "fn" "(" IDENT ")" "=>" expr
    compare   := add ( ("==" | "<" | ">") add )?
    add       := mul ( ("+" | "-") mul )*
    mul       := app ( ("*" | "/") app )*
    app       := atom ( "(" expr ")" )*
    atom      := INT | BOOL | IDENT | "(" expr ")"

Comments are `// to end of line`.

Single-parameter lambdas only. Multi-parameter functions are written
curried (`fn(x) => fn(y) => ...`).

### B.4 Public API Surface

All re-exports from `sentinel_effects_proto`:

- AST: `Expr = Spanned<ExprKind>`, `ExprKind`, `BinOp`, `expr` constructor helper
- Spans: `Span`, `Spanned<T>`
- Lexer: `Token`, `LexError`,
  `lex(source) -> Result<Vec<(Token, Span)>, LexError>`
- Parser: `ParseError`,
  `parse(&[(Token, Span)]) -> Result<Expr, ParseError>`
- Eval: `Value`, `EvalError`, `Env`, `Step`, `Continuation`,
  `eval(&Expr, &Env) -> Result<Step, EvalError>` (B3.2a return type;
  `crate::run` bridges `Step::Value` → `Value` and surfaces
  `Step::Op` as `EvalError::UnhandledOpAtTopLevel`)
- Types: `Ty`, `TyVar`, `Scheme`
- Inference: `TypeError`, `TypeEnv`, `EffectEnv`, `Subst`,
  `TyVarSupply`, `unify`, `instantiate`, `generalize`, `infer`,
  `infer_top`, `infer_program`
- Top-level: `MiniError`,
  `run(source) -> Result<Value, MiniError>` (lex+parse+infer+eval),
  `MiniError::render(&self, source) -> String` for caret diagnostics

The `diag` module is `pub mod diag` but its items are reached
through `MiniError::render`; they are not re-exported at the
crate root in B1.

### B.5 Design Decisions (B0)

1. Hand-written recursive descent over `chumsky` / `lalrpop`. Per
   HANDOVER §3.3 (production compiler) and our B0 reasoning: the
   grammar will churn as effects and qualifiers are added; hand-written
   parsers absorb grammar changes more cheaply than combinator chains.
2. Plain `Box`-allocated AST. No `bumpalo`. The language is small
   enough that heap traffic is irrelevant for a research artifact.
3. Persistent `Arc`-cons-list environment for closures. Standard
   Crafting-Interpreters shape. May be revisited if broker-as-value-heap
   integration lands.
4. Errors are not span-tracked at B0. Spans land with B1 alongside
   the type checker so error highlighting can be meaningful from
   the start.
5. `BrokerError`-style two-flavour API (panicking + fallible) is
   NOT adopted here. Effects-proto is throwaway research code;
   panicking-only is acceptable and simpler. If a panic-free API
   becomes useful for embedding, it lands then.
6. (B1.1/B1.2) AST nodes carry spans via a `Spanned<T>` wrapper,
   not an inline `span` field on each variant. Confirmed cheap;
   parser pattern is `Spanned::new(kind, start_span.merge(end_span))`.
7. (B1.3) `rec` is a reserved keyword (`Token::Rec`), not a
   contextual one. `let rec` is the only place it can appear in B1.
8. (B1.4) Substitutions are eager (`apply` on `bind`), not
   union-find. Idempotency is maintained by `compose`. Fine at
   B1 scale; revisit only if profiling demands.
9. (B1.5/B1.6) Inference is Algorithm W in the textbook shape.
   `let rec` uses the standard HM treatment: monomorphic recursive
   occurrence inside the RHS, generalized scheme in the body.
   Polymorphic recursion is therefore unavailable without
   annotations -- this is intentional and matches ML/Haskell
   without explicit type signatures.
10. (ADR 0002) Function arrows are bare `Fun(Ty, Ty)`. Effect rows
    are deferred to B2 to keep B1 focused.
11. (B1.7) Diagnostics are hand-rolled (`diag.rs`, ~110 LoC, no
    `miette` dependency). Phase C will likely adopt miette; the
    prototype validates the shape (line/col header, source-line
    excerpt, caret underline) cheaply. `Display` for `MiniError`
    stays terse; pretty rendering is opt-in via `.render(src)`.
12. (B2.0) `Ty::Fun` now carries a `Row` per ADR 0002 / ADR 0004.
    `Row` is a distinct enum (`Empty | Var(RowVar) | Cons { .. }`),
    `RowVar` is a distinct kind from `TyVar`, `Subst` carries
    parallel `map` / `row_map` fields. B2.0 ships behaviour-
    preserving: every arrow gets `Row::Empty`, `unify_row` is a
    stub handling only the empty-vs-empty case (B2.1 fills in
    Remy-style row unification). Clippy `result_large_err` allowed
    crate-wide because `Row` pushed `TypeError` past the lint's
    128-byte threshold; STATE.md decision 5 already documented
    that effects-proto does not optimise error shape.
13. (B2.1) Row unification follows Remy 1989 / Leijen's
    extensible records: `unify_row` handles empty-vs-empty,
    var binding (with `row_occurs` check), matching `Cons`
    heads by recursing on arg/ret/tail, and label rewriting
    via `rewrite_row` when heads differ. Two new
    `TypeError` variants `RowMismatch` and `RowOccursCheck`
    carry the offending row and span. `unify` now threads
    `&mut TyVarSupply` to mint fresh row tails during
    rewriting; all call sites (App, If, BinOp, LetRec) were
    updated. Unit tests use `RowVar` IDs >= 100 to avoid
    collisions with the fresh-supply counter (which starts
    at 0). Inference still mints only `Row::Empty` at
    lambda introduction; user-visible effect behaviour
    arrives in B2.3.
14. (B2.2a) Surface for effect declarations and operations.
    Five new tokens (`Colon`, `Semicolon`, `Arrow`, `Effect`,
    `Do`) make `effect` and `do` reserved keywords; neither
    was used as an identifier in B1. Grammar:
    `effect Label : TyExpr ;` where TyExpr is required to be
    an arrow at the top level, and `do Label(arg)` at the
    atom level. Effect labels must start with an uppercase
    ASCII letter (parser-enforced via
    `ParseError::EffectLabelNotUpper`). A new surface-level
    `TyExpr` enum lives in `ast.rs` deliberately distinct
    from the inference-time `Ty` so the parser does not
    depend on the type system. Fix-A grammar (single
    `TyExpr` then split-on-arrow) was chosen over
    `ArgTy '->' RetTy` because the latter is ambiguous when
    `ArgTy` itself contains `->`. ADR 0004 will be amended
    in B2.5 to reflect the actual grammar production.
15. (B2.2b) `Perform` is parseable but rejected by inference
    with `TypeError::EffectNotYetSupported { label, span }`
    where span targets the label identifier (not the `do`
    keyword) so diagnostics caret the meaningful token.
    `EvalError::EffectNotYetSupported(String)` exists for
    defence in depth (callers bypassing inference) and is
    span-less to match the B1 `EvalError` precedent
    (decision 5 lineage; full span enrichment is a backlog
    item). `run()` now pipelines through `parse_program` +
    `infer_program`; effect declarations are parsed but
    inert (typing environment is unchanged). Real effect
    rows wire in B2.3.

16. (B2.3a, ADR 0005 D9) Effect-inference judgment refactored
    behavior-identical: `infer` returns `(Subst, Ty, Row)` and
    takes a new `eff_env: &EffectEnv` parameter; `Scheme` gains
    a `row_vars: Vec<RowVar>` field (empty default, source-
    compatible with `Scheme::mono`); every arm returns
    `Row::Empty`; `Perform` keeps its B2.2b `EffectNotYetSupported`
    behavior. Divergence from ADR 0005 D9 B2.3a list, deliberate:
    `TypeError::UnhandledEffects { row, span }` and the strict
    residual-row check in `infer_top` were pulled forward from
    the D9 B2.3b list. Both are unreachable in B2.3a (every arm
    returns `Row::Empty`, so `s.apply_row(&Row::Empty)` is always
    `Row::Empty`) and land here to isolate B2.3b's diff to
    semantics only -- this strengthens the bisection-point property
    ADR 0005 D9 Consequences sec.1 calls for. `UnknownEffect` is
    *not* pulled forward; it remains strictly B2.3b because it
    would be genuinely dead (never constructed) in B2.3a. Commit
    fd8eef6.

17. (B2.3b1, ADR 0005 D2 + ADR 0006 amended) Row mechanics landed
    behavior-extending: `Lambda` mints fresh `ρ` and embeds it in
    the arrow via `Ty::arrow_with`; the lambda's own row
    contribution stays `Row::Empty`. `App` mints `ρ_call`, unifies
    callee against `arrow_with(t_arg, ρ_call, result)`, and unions
    `r_callee`, `r_arg`, and resolved `ρ_call` into its
    contribution. `Let`, `LetRec`, `If`, `BinOp` union
    subexpression contributions. `Perform` still rejects with
    `EffectNotYetSupported` (B2.3b2 wires it). Default-close (ADR
    0006 D1) extended: `Row::Var` in *contributions* is treated as
    `Row::Empty` (in `row_union` and at `infer_top`'s residual
    check), because `App` legitimately produces free `ρ_call`s
    whenever the callee is a `Ty::Var` (recursive calls,
    higher-order parameters). Net +7 tests (one earlier B2.3b1
    test with a wrong premise was deleted). Commit f2a17d9.

18. (B2.3b2, ADR 0005 D9 closed) Perform inference + row
    generalization landed as a two-commit split. **b2.3b2-a**
    (4c69ed7) wires `do Label(arg)` through the effect environment:
    `infer_program` populates `EffectEnv` from `prog.effects`; the
    `Perform` arm looks up the label, infers and unifies the arg
    against the declared arg type, and contributes a closed
    single-label `Cons` row union'd with the arg's row. Unknown
    labels surface as a new `TypeError::UnknownEffect { label,
    span }` targeting the label token. **b2.3b2-b** (47cc5a1)
    extends `generalize` to quantify free row variables in the
    type (minus those free in the env) into `Scheme.row_vars`;
    `instantiate` freshens them via `fresh_row_var`. Row
    *contributions* are intentionally excluded from generalization
    — they describe the RHS's latent effect, not its binding
    scheme; conflating them would make every let-binding look
    effectful from the outside, breaking the default-close
    presentation contract (ADR 0006 D3). The placeholder
    `TypeError::EffectNotYetSupported` is removed entirely now
    that `Perform` is fully typed; `EvalError::EffectNotYetSupported`
    stays until B3 handlers land. Split rationale: semantics
    first, generalization second, each independently bisectable
    and each under ~300 LOC. Net +14 tests (10 b23b2_* unit + 5
    b23b2b_* unit + 2 integration − 2 obsolete b22b_perform_*
    rewritten − 1 integration test repurposed).


19. (B3.0, ADR 0007 D1/D2) Handler surface landed: `handle e with
    { L(x, k) => body, ..., return v => ret_body }` with arms
    comma-separated, trailing comma permitted, empty `{}` rejected,
    arm labels required to be uppercase (mirroring `do Label`).
    `handle` parses at `parse_expr` precedence (peer of `if`, `let`,
    `fn`). `return` becomes a globally reserved keyword — no
    existing test or code used `return` as an identifier, but the
    reservation is real and future surface work should account for
    it. AST: `ExprKind::Handle { body: Box<Expr>, arms:
    Vec<HandlerArm>, ret_arm: Option<ReturnArm> }`, with `HandlerArm`
    carrying `label`, `label_span`, `arg`, `kont`, `body`, `span` and
    `ReturnArm` carrying `var`, `body`, `span`. The return arm is
    optional; absence semantically defaults to `return v => v`, with
    the default introduced by the typer (D1 in the ADR), not the
    parser, so synthesized AST nodes with fake spans never enter the
    diagnostic path. New `ParseError` variants
    `HandlerArmLabelNotUpper` and `EmptyHandler`. Placeholder
    inference and eval errors (`TypeError::HandlersNotYetSupported`
    and `EvalError::HandlersNotYetSupported`) keep the build whole
    until B3.1 (typing) and B3.2 (runtime) replace them. Commit
    821b16a.

20. (B3.1, ADR 0007 D3/D4) Handler typing rule landed in two commits.
    **B3.1a** (febf379) adds `row_split` as the dual of the existing
    `rewrite_row`: given a row and a label, returns the discovered
    `(arg_ty, ret_ty)` signature and the residual row, with four
    cases per ADR 0007 D4 (head match, deeper match, row-var case
    minting fresh `α`/`β`/`tail` and binding the var, empty-row
    erroring with the new `HandlerLabelNotInRow`). Critically the
    row-var case mints a fresh *type* var for both arg and ret —
    this is what makes handlers compose with row-polymorphic
    callers, because the handler arm's typing of `x` and `k` later
    pins those fresh vars to concrete types. `rewrite_row` was not
    reused: it expects a known signature to unify against;
    `row_split` reads the signature out. Distinct errors, distinct
    intent, easier to diagnose. **B3.1b** (e7958e1) wires the
    typing rule per D3: infer body, reject duplicate arm labels with
    `DuplicateHandlerArm`, peel each arm's label out of the body's
    row threading substitution, capture the post-peel residual as
    `r_outer_initial`, mint `t_result`, type each arm body under
    `env + {x: arg_ty, k: ret_ty -> t_result ! r_outer_initial}` and
    unify with `t_result`, union each arm body's own row
    contribution into `r_accumulated`, handle the return arm
    (or default to `t_body = t_result` when absent). The
    continuation arrow uses `r_outer_initial` rather than the
    final `r_accumulated` — the standard Eff/Koka calculus, sound
    because `k`'s declared row is what `k` requires of its
    invocation context, not what the arm body around the `k`
    invocation may additionally perform. `TypeError::HandlersNotYetSupported`
    removed (variant + `lib.rs` span arm). Three planned items
    landed as no-ops: (a) the D7 canary collapses to
    `b31b_handle_identity_discharges_effect` because
    `infer_program`'s default-close policy (ADR 0006 D6) hides
    observable row polymorphism at the public type level — the
    canary's load-bearing property (generalize/instantiate compose
    with handler typing) is satisfied transitively by all `b31b_*`
    tests passing without modifying that machinery; (b) the D8
    positive/negative pair is unreachable through `infer_program`
    (permissive) and uncallable through `infer_top` (no effect
    env), so strictness coverage comes indirectly through
    `b31b_handle_two_arms_discharges_both` and
    `pipeline_handle_typechecks_then_runtime_is_placeholder`; (c)
    extending `TypeError::UnhandledEffects` was reviewed and
    rejected — `Row`'s `Display` already renders residuals as
    `{Label1, Label2 | tail}`, human-readable as-is. ADR 0007
    status flipped to ACCEPTED for B3.0 + B3.1; D9 amended with
    completion-marker status. Net +20 lib tests (4 b31_row_split_*
    + 7 b31b_* + 9 b30_ from B3.0) + 1 integration test rewritten
    (`pipeline_handle_typechecks_then_runtime_is_placeholder`).
    Eval-side placeholders remain pending B3.2.
Test coverage as of B3.2: 183 tests (169 lib + 14 integration). Lib
count unchanged from B3.1 because B3.2a was scaffolding (no behavior
change) and B3.2b rewrote three placeholder-asserting tests in place
rather than adding new ones (one direct-eval Perform test:
`b22b_eval_perform_directly_returns_effect_error` →
`b32b_eval_perform_directly_reifies_step_op`; one direct-eval Handle
test: `b30_eval_handle_directly_returns_handlers_not_yet_supported` →
`b32b_eval_handle_no_arms_no_ret_is_identity_on_value`; one
integration test: `pipeline_handle_typechecks_then_runtime_is_placeholder`
→ `pipeline_handle_resumes_with_arg_through_run`, the first Sentinel
program with effects to actually compute a value through `run()`).
B3.2c added four integration tests covering two-effect dispatch,
non-trivial arm body work, arm-body reading outer Let bindings via
the env bundled into `Value::Resumption`, and explicit return-arm
post-resume. Clippy clean under `-D warnings`; no doctests. See B.5
design decision 21.

21. (B3.2, ADR 0007 D5) Handler runtime landed in three commits.
    **B3.2a** (bdda217) is scaffolding: `eval`'s return type changed
    from `Result<Value, EvalError>` to `Result<Step, EvalError>` where
    `Step = Value(Value) | Op { label, arg, kont }`; every existing
    arm threaded `Step::Value` propagation (Int/Bool/Var/Lambda wrap;
    Let/LetRec/If/App/BinOp match on the recursive eval result,
    propagating Value on the happy path and pushing a `Frame` onto
    kont + re-raising Op otherwise). `Continuation` is a Vec<Frame>
    enum, not a boxed `FnOnce` — chosen for Debug/Clone ergonomics
    and for keeping the resume-point states explicit and greppable.
    Eight Frame variants enumerated by counting resume-points in the
    existing eval arms (LetBody, LetRecBody, IfBranch, AppArg,
    AppCall, BinOpRight, BinOpApply, PerformReify). `Continuation::resume`
    declared as `todo!("B3.2b")` so the API signature was fixed.
    Perform and Handle arms kept their B2.2b/B3.0 placeholder errors
    unchanged; tests stayed at 169 + 10 + 0. Two new EvalError
    variants added: `UnhandledOpAtTopLevel { label }` for
    defence-in-depth at `crate::run` (mirroring B2.2b's posture —
    the type system's default-close at `infer_program` should
    prevent open rows at the top level, but the runtime checks
    anyway), and `ContinuationAlreadyResumed` reserved for B3.2b's
    one-shot enforcement. `Step` re-exported from the crate root.

    **B3.2b** (a9cefb1) is the substantive commit. Five sub-patches
    in the working tree, single commit at the end: (1)
    `Value::Resumption { kont, arms, ret_arm, env }` variant added
    + `apply` refactored to dispatch on it (initially without the
    deep re-wrap, filled in next); (2) `Frame::HandleFwd { arms,
    ret_arm, env }` variant added — the 9th Frame, not visible in
    B3.2a because Handle didn't propagate before — and `handle_step`
    helper added as the shared dispatch hub used both by the Handle
    arm at evaluation start and by `apply` at resume re-wrap; (3)
    `Perform` arm now reifies — eval the arg, on Value produce
    `Step::Op { label, arg: v, kont: Continuation::empty() }`, on Op
    push `Frame::PerformReify` and re-raise; (4) `Handle` arm now
    dispatches — eager-Arc the arms and ret_arm once per Handle
    evaluation (so subsequent Resumption/HandleFwd clones are cheap
    refcount bumps), eval the body, route the resulting Step
    through `handle_step`; (5) cleanup —
    `EvalError::EffectNotYetSupported` and `EvalError::HandlersNotYetSupported`
    deleted now that their producing arms gained real
    implementations, `#[allow(dead_code)]` stripped from Frame.
    Three placeholder tests were rewritten in place to assert real
    behavior. The bundled-context shape of `Value::Resumption`
    (kont + arms + ret_arm + env) was chosen over a bare
    `Value::Continuation` because the deep re-wrap data must travel
    with the resumption value — `apply`'s dispatch stays in one
    place (sibling to `Value::Closure`) without an App-site lookup
    into handler state, and the alternative (synthesising a
    Lambda AST node that calls a builtin) requires a builtin-
    function mechanism the prototype does not have.

    **B3.2c** (8e3de20) adds four pipeline tests through `run()`
    along axes the B3.2b integration test did not cover:
    `pipeline_two_effect_handler_discharges_both` exercises deep
    re-wrap (apply's `handle_step` after `kont.resume`) and the
    splice mechanism (step_frame's `BinOpRight` branch producing a
    nested Op with `BinOpApply` prepended onto its inner kont);
    `pipeline_arm_body_computes_with_op_arg_before_resume` covers
    the rhs-raises BinOp path (sibling to the previous test's
    lhs-raises path) and confirms arm bodies can do non-trivial
    work with the operation's argument before invoking k;
    `pipeline_arm_body_uses_outer_let_binding` confirms the arm
    body sees outer Let bindings via the env bundled into
    `Value::Resumption` (the closest the current language reaches
    toward a state handler without CPS state-threading);
    `pipeline_return_arm_runs_on_resumed_value` exercises
    `handle_step`'s Some-ret_arm branch (the B3.2b integration test
    had no ret_arm, hitting only the None/identity branch).

    Two structural decisions worth recording inline. (a)
    `Continuation` derives `Clone` because `Value` is `Clone` and
    `Value::Resumption` holds `Continuation` by value. The
    `Cell<bool>` resumed-flag copies its bool on clone, so cloning
    a *resumed* `Continuation` produces a clone that also refuses
    to resume — the one-shot guarantee therefore holds
    per-Continuation-instance, not per-logical-resumption. Nothing
    in eval clones a Resumption today; user-level multi-shot via
    `Value::Clone` is not statically prevented. Stricter alternatives
    (move-only `Resumption` variant, or `Rc<Cell<bool>>` for a
    shared flag) are recorded inline at the `Continuation`
    definition for Sentinel proper. (b) A real CPS-state state
    handler in the Eff/Koka sense — Get/Put paired effects threading
    mutable state through resumption — was scoped *out* of B3.2c
    in favor of the simpler `pipeline_arm_body_uses_outer_let_binding`
    test. The prototype's unary lambdas preclude `k(state, value)`,
    and CPS-state via `state -> result` returns produces a test
    program ~6 lines long with a trace that obscures what is being
    tested. The load-bearing mechanic (outer Let visible inside arm
    via bundled env) is covered. Deferred to whatever phase
    introduces multi-arg functions or records. Filed in ADR 0007's
    "Considered and rejected" section.

    Net 0 lib tests, +4 integration tests, −1 lib test renamed,
    −2 lib test rewrites in place, −1 integration test rewrite in
    place. Placeholder-error variants gone:
    `EvalError::EffectNotYetSupported`,
    `EvalError::HandlersNotYetSupported`. New variants:
    `EvalError::UnhandledOpAtTopLevel`,
    `EvalError::ContinuationAlreadyResumed`. New public types: `Step`,
    `Continuation` (with `is_empty()` for tests).

Test coverage as of B4.0: 209 tests (192 lib + 17 integration). Net
+23 lib (+5 b40_ in types.rs from B4.0a, +3 b40a_ placeholder tests
in infer.rs, +6 b40b_ lexer tests, +9 b40c_ parser tests) and +3
integration (declassify rejected at infer, effect-decl-with-secret
rejected at infer, double-secret rejected at parse). Clippy under
`-D warnings` is broken by 5 pre-existing lints (Arc-not-Send/Sync,
extend-vs-append, into_iter-in-IntoIterator-context, unnecessary
map_or) that pre-date B4 and were inherited from B3.2; not a B4
regression. cargo test remains the project's green-gate. See B.5
design decision 22.

22. (B4.0, ADR 0008 D9) Secret/declassify surface landed in three
    commits. **B4.0a** (1693b8c) is the surface AST + placeholder.
    `Ty::Secret(Box<Ty>)` chosen over (a) qualifier-field-on-every-Ty
    and (c) parallel qualifier lattice per ADR 0008 D1: shape (b)
    falls out of HM unification with zero new machinery, while the
    no-α-leak unification restriction (D2) substitutes for full
    qualifier polymorphism. Idempotent smart constructor
    `Ty::secret` collapses `Secret(Secret(_))` so substitution and
    unification call sites don't worry about flattening; the parser
    separately rejects literal `secret secret T` (B4.0c) so the
    surface complains early but the inference layer is robust
    regardless. Display per D6 with arrow-parens (`secret int` vs
    `secret (a -> b)`). `Subst::apply` recurses via `Ty::secret`;
    `unify` adds a structural `(Secret, Secret)` arm. The B4.0 Var
    arm in `unify` will happily bind a TyVar to a `Ty::Secret(_)` —
    D2's no-α-leak rule is B4.1 — but this is unobservable in B4.0
    because every entry point that introduces `Ty::Secret` into
    inference is gated: `infer`'s `Declassify` arm returns
    `SecretsNotYetSupported`, and `infer_program` walks each effect
    decl's signature with a new `tyexpr_find_secret_span` helper,
    rejecting any decl that mentions `secret`. `eval` gets a
    `Declassify` arm that delegates to `inner.eval` (the
    declassification is type-level only; `Value` is qualifier-blind
    by B0 design, so no resume-point work is needed) — unreachable
    in B4.0 via the full pipeline because inference rejects first,
    but exists so eval is total over `ExprKind`. 3 placeholder tests
    in infer.rs build AST directly because the lexer/parser do not
    yet recognise the keywords; 5 `b40_*` tests in types.rs (added
    in the prior session, landed in this commit) pin display,
    idempotency, free-var recursion, close_rows recursion.

    **B4.0b** (63cd57b) adds `Token::Secret` and `Token::Declassify`
    to the lexer. Both globally reserved — cannot be used as
    identifiers, matching the policy applied to `rec` (B1.3) and the
    handler keywords (B3.0). Logos token attributes only; the
    existing Ident regex is tried after keyword matches. 6 lexer
    tests (standalone for each keyword, reserved-status in let-bind
    position for each, `secret Bytes` and `declassify(x)` full
    streams).

    **B4.0c** (0b6b2ce) adds the parser surface. `secret T` as a
    prefix on type atoms in `parse_ty_atom`, binding tighter than
    `->` per ADR 0008 D6: `secret Int -> Bool` parses as
    `(secret Int) -> Bool`, not `secret (Int -> Bool)`. This
    precedence falls out of recursing on `parse_ty_atom` (not
    `parse_ty_expr`); users wanting the arrow inside the secret
    write `secret (Int -> Bool)`. `declassify(e)` as
    atom-precedence expression in `parse_atom`, mandatory parens
    paralleling `do Label(arg)` and preserving the audit-point
    property called out in D5. `ParseError::DoubleSecret` rejects
    literal `secret secret T` (caught at the immediately-recursive
    call site) and `secret (secret T)` (caught after the paren
    collapse returns `TyExpr::Secret(_, span_with_parens)`). 9
    parser tests + 3 integration tests through `run()`. The
    integration trio confirms: full-pipeline `declassify(1)`
    rejected at inference with `SecretsNotYetSupported`;
    full-pipeline `effect ReadKey : Int -> secret Int ; 0` likewise;
    full-pipeline `effect F : Int -> secret secret Int ; 0` rejected
    at parse with `DoubleSecret` (proves the early parser rejection
    short-circuits ahead of the placeholder).

    Three structural decisions worth recording inline. (a) `secret`
    binds tighter than `->`. Considered the inverse (arrow tighter,
    so `secret Int -> Bool` parses as `secret (Int -> Bool)`),
    rejected because the former matches Rust's `&mut T -> U` reading
    that ADR 0008 D6 cites as precedent, and because `secret`
    binding loosely would force users writing single-arrow effect
    signatures `Int -> secret Bytes` to add redundant parens around
    the `secret Bytes` ret type. (b) `tyexpr_find_secret_span` walks
    the surface `TyExpr` rather than the lowered `Ty` because the
    surface tree carries human-source spans and is the right layer
    to point a diagnostic at. The walker is recursive structural —
    no row-handling needed because effect-decl signatures are pure
    `Int`/`Bool`/`Arrow`/`Secret` at this scope (no inline
    polymorphism in the surface yet). (c) Both `secret` in effect
    decls and `declassify` in expressions block at inference, not
    earlier. The parser produces well-formed AST for both surfaces;
    rejection happens at the inference layer specifically so that
    `infer_program` is the one boundary that decides "we're not
    ready to type secrets" — when B4.1 lands, the rejection sites
    delete and the typing rules replace them, no parser or lexer
    churn.

    Net +23 lib tests, +3 integration tests, no rewrites in place
    (the surface is wholly new). New `TypeError` variant:
    `SecretsNotYetSupported`. New `ParseError` variant:
    `DoubleSecret`. New `Token` variants: `Secret`, `Declassify`.
    New `Ty` variant: `Secret(Box<Ty>)` + `Ty::secret` smart
    constructor. New `ExprKind` variant: `Declassify { inner, span }`.
    New `TyExpr` variant: `Secret(Box<TyExpr>, Span)`. ADR 0008
    status flipped from PROPOSED to ACCEPTED with note that D1/D5/D6
    are confirmed by B4.0 and D2/D3/D4/D8 land in B4.1.

Test coverage as of B4.1: 222 tests (203 lib + 19 integration).
Net +13 lib / +2 integration from B4.0's 192 + 17. B4.1a landed
+5 lib (Declassify-on-non-secret-is-SecretFlow rename of the
B4.0a placeholder test, plus +5 fresh tests covering D2 direct,
Declassify positive via synthetic env, SecretFlow via catch-all,
Secret-Secret recursion sanity, Secret-Int-vs-Secret-Bool mismatch
on inners), 0 net integration (rename in place of the declassify
placeholder). B4.1b landed +8 lib (effect-decl-with-secret tests
rewritten from rejection to positive [+2 in place], +6 fresh
covering SecretBranch, SecretDivisor, D4 on three comparison shapes,
D4-Lt-Bool-rejects-on-inner) and +2 integration (password-verify
chain rejects with SecretBranch -- the HANDOVER §5.2 deliverable;
secret-in-arithmetic-is-SecretFlow). The effect-decl-rejection
integration test was rewritten as a positive type-check (net 0).
The placeholder variant `TypeError::SecretsNotYetSupported` is
gone. Clippy under `-D warnings` still has the 5 pre-existing
lints inherited from B3.2; chip filed to clean up separately.
See B.5 design decision 23.

23. (B4.1, ADR 0008 D2-D7) Secret typing landed in two commits.
    **B4.1a** (e760d57) is the foundation. Four new `TypeError`
    variants: `SecretFlow { from, to, span }` (the public/secret
    unification failure, raised by the catch-all arm of `unify`
    when either side is `Ty::Secret(_)` and the other is
    non-secret-non-Var), `SecretEscapesPolymorphism { var, span }`
    (D2 no-α-leak, raised by the Var arm of `unify` when a bare
    TyVar would bind to a secret type), `SecretBranch { span }`
    (declared; fires in B4.1b), `SecretDivisor { span }` (declared;
    fires in B4.1b). The `unify` Var arm gains a `matches!(t,
    Ty::Secret(_))` short-circuit; the catch-all Mismatch arm
    splits into SecretFlow (one side Secret) and Mismatch (neither
    side Secret). The Declassify infer arm replaces its B4.0a
    `SecretsNotYetSupported` placeholder with the real D5 rule:
    mint a fresh α, unify `t_inner` against `Ty::Secret(α)`, return
    `s.apply(α)` as the result type. Three failure cases handled
    naturally by the unify machinery: inner is concrete non-secret
    → SecretFlow via catch-all; inner is bare TyVar →
    SecretEscapesPolymorphism via Var arm; inner is Secret(t) →
    Secret-Secret arm binds α := t, result is t.

    **B4.1b** (52acc0a) is the CT-specific rejections + cleanup.
    `infer`'s If arm rejects `cond : Ty::Secret(_)` with
    SecretBranch before the Bool unify so the diagnostic is
    dedicated rather than the generic SecretFlow. `infer`'s BinOp
    arm rejects Div with a secret divisor with SecretDivisor
    (Sentinel-Mini has no Mod; ADR D3's `Div | Mod` is
    forward-looking). The Eq/Lt/Gt arms gain the D4 comparison
    rule: when either operand types as Secret(_), unwrap that side
    to its inner type, unify the other operand against the inner
    (the (Secret, Secret) arm of unify handles the both-secret case
    naturally via the same code path because both inners become
    non-Secret here), and produce Secret(Bool) as the result type.
    For Lt/Gt the inner type must additionally be Int per the
    existing binop_signature; that unify fires after the cross-side
    unify and produces a SecretFlow if the inner isn't Int (or
    Mismatch if neither side was secret).

    The `infer_program` `tyexpr_find_secret_span` walker and the
    associated SecretsNotYetSupported variant are deleted in B4.1b.
    ADR 0008 D7 (effect signatures may mention `secret`) is
    confirmed end-to-end: `effect ReadKey : Int -> secret Int ;`
    now type-checks, and `do ReadKey(0)` flows a Secret(Int) value
    into inference where D2/D3/D4 keep it safe.

    Two structural decisions worth recording inline. (a) The Var
    arm's D2 check is `matches!(t, Ty::Secret(_))` rather than a
    deeper walk that would refuse to bind any TyVar that contains a
    Secret inside a structural type (like `Fun(_, _, Secret(_))`).
    The shallow check is sufficient because Secret-inside-Fun is
    not a "bare TyVar binds to secret" violation -- the inner
    binding is fine if the function itself is concrete. Forward
    compatibility note: if a future ADR promotes the restriction
    to full qualifier polymorphism (shape (c)), the shallow check
    becomes a quantifier-restricted bind rule; the existing call
    sites already test it at the Var arm so the migration is
    contained.

    (b) The D4 comparison rule for Lt/Gt unifies the (potentially
    secret-unwrapped) lhs inner type against Int as a SECOND
    unification step, after the cross-side unify. This makes
    `Lt(Secret(Bool), Secret(Bool))` reject with Mismatch (the
    unwrapped Bool vs Int) rather than SecretFlow (which would be
    odd because both sides are secret-and-equal). The trade-off:
    diagnostic for Lt-on-secret-non-Int is "expected Int, found
    Bool" rather than something CT-specific. Acceptable because
    such a program is rare and the inner-type mismatch is what the
    user actually needs to fix.

    A positive end-to-end test for declassify-on-Secret cannot be
    written because ADR 0008 D5 intentionally omits a `classify`
    primitive (the Secret-introduction dual). The Secret-introducing
    path is restricted to typing of `do L(arg)` for effect-decls
    naming secret; resuming a continuation requires producing a
    Secret value, and the surface has no form that does so. Positive
    D5 coverage lives in lib's
    `b41a_declassify_on_secret_unwraps_the_inner_type` via a
    synthetic env. The integration tests file carries an inline note
    explaining the gap.

    D8 (generalization participation) was implicit by B4.0a's
    structural recursion of `collect_free_vars` and
    `collect_free_row_vars` into `Ty::Secret(_)`. No dedicated test
    added because no surface program exposes the generalization
    boundary with a polymorphic secret-typed value -- D2 prevents
    that by construction. Confirmed by code review of types.rs.

    Net +13 lib / +2 integration. New TypeError variants land:
    SecretFlow, SecretEscapesPolymorphism, SecretBranch,
    SecretDivisor. Variant removed: SecretsNotYetSupported.

Test coverage as of B4.2: 226 tests (203 lib + 23 integration).
B4.2 lands one commit covering the three HANDOVER §5.2 Phase B
validation demos plus README/STATE refresh. Net +4 integration
(supply-chain handler-mismatch, async-as-effect doubling-handler,
async-as-effect identity-handler, polished password-verify with the
CT-chain rationale block); 0 lib changes. The polished
password-verify is added next to the terse
`pipeline_password_verify_naive_rejects_with_secret_branch` rather
than replacing it -- the terse form is useful as a
regression-pin; the polished form is what HANDOVER §5.2 actually
calls for as the Phase B deliverable. Clippy clean under
`-D warnings`; no doctests. See B.5 design decision 24.

24. (B4.2, ADR 0008 D9 + HANDOVER §5.2) Phase B's three validation
    demos all live as integration tests under the `pipeline_b42_`
    prefix in `crates/sentinel-effects-proto/tests/integration.rs`.
    Implementation choices:

    (a) Tests, not example files. The crate has no `examples/`
    directory and remains library-only. Examples would have
    required `crate::run` and `Value` exposed as `cargo run`
    targets; the test runner already exposes both via the
    pipeline path. The trade-off accepted: examples are
    discoverable via `cargo run --example`, tests are
    discoverable via `cargo test pipeline_b42_`. The latter is
    sufficient given the prototype's "throwaway research artifact"
    framing in HANDOVER §5.

    (b) Supply-chain demo asserts `HandlerLabelNotInRow{label:
    "Storage"}`. The error diagnostic is from the handler's
    perspective ("I said I'd handle Storage but the body never
    raised it") rather than the user's ("the body raised Network
    without my permission"). Both readings are correct; the row
    machinery refuses the program either way. ADR 0007's row
    discipline doesn't carry user-intent metadata, so the
    diagnostic frame is mechanical. A comment in the test
    explains the supply-chain framing.

    (c) Async-as-effect demo is a pair of tests with the same
    `let app = fn(n) => do Tick(n) + 1 in ...` prefix; only the
    handler arm differs (`Tick(x, k) => k(x * 2)` vs
    `Tick(x, k) => k(x)`). The byte-identical-source assertion is
    pinned via test pairing, not via shared-string-constant
    machinery (which would require helper indirection and
    obscure the demo). The two tests assert different `Value::Int`
    results, which is the load-bearing observation.

    (d) Polished password-verify demo carries the full CT-chain
    rationale as a comment block above the test: the D7 effect
    signature, the D4 comparison rule producing `Secret<Bool>`,
    the D3 `SecretBranch` rejection of the surrounding `if`. The
    naive comment in the original B4.1b test is intentionally
    short; the polished version is the "publishable" form of the
    demo.

    (e) No featured `classify : T -> secret T` primitive added.
    The B4.2 task offered it as a design call (it would enable a
    positive end-to-end declassify test); rejected for B4.2 because
    Secret-introduction is exactly what ADR 0008 D5 deliberately
    omits to preserve the audit-point property. Adding `classify`
    would require its own ADR amendment justifying the exception
    and a strong-comment audit marker. Deferred indefinitely.

    Net +4 integration tests, 0 lib changes (no new variants, no
    surface changes; the demos are exercise programs over the
    existing surface). README updated to mark B1-B4 done, refresh
    the test count, and add a "What works today" paragraph for
    effects-proto with the `cargo test pipeline_b42_` invocation
    that runs the three demos. ADR 0008 D9 amendment updated to
    flip B4.2 from roadmap to landed. Phase B is now complete;
    Phase C (bootstrap compiler) per HANDOVER §6 is the next major
    phase.


### B.6 Known Limitations (intentional at B1)

- No effects. The whole reason this crate exists. (B2 onward — done.)
- `secret` qualifier and constant-time check: complete (B4.0 surface,
  B4.1 typing, B4.2 validation demos). D2 (no-α-leak), D3 (the four
  CT rejections), D4 (comparisons produce `Secret<Bool>`), D5
  (declassify typing) all wired. All three HANDOVER §5.2 Phase B
  validation deliverables exist as integration tests under the
  `pipeline_b42_` prefix. A positive end-to-end `declassify` test is
  intentionally absent because ADR 0008 D5 omits a `classify`
  primitive (audit-point property); positive D5 coverage lives in
  lib's `b41a_declassify_on_secret_unwraps_the_inner_type` via a
  synthetic env.
- No REPL, no driver binary. Library-only.
- `let rec` RHS must be a syntactic lambda. Parser enforces with
  `ParseError::LetRecNotLambda`. Relaxing this in B3 (when handlers
  arrive) is an open question; see ADR 0003.
- Polymorphic recursion is rejected, as in ML/Haskell without an
  explicit type signature. Test
  `b16_letrec_recursive_occurrence_is_monomorphic_inside_body`
  locks this.
- Equality (`==`) is polymorphic at the type level (`forall a. a -> a -> Bool`).
  The evaluator still rejects equality on closures; B2/B3 may
  refine via type-class-style constraints.
- `EvalError` variants carry no spans. Eval errors are rare
  post-type-check (div-by-zero, non-function application on a
  closure-typed value, the letrec uninitialised internal error)
  but they render without carets. B2 backlog item.
- Multi-line spans in `diag::render` clip to the first line. Sentinel-Mini
  programs are usually one-liners; Phase C diagnostics will handle
  multi-line ranges properly.
- Closures `clone()` the body `Expr` into an `Arc<Expr>`. Acceptable
  for a research artifact; revisit if body-clone becomes a hot path
  or if the broker becomes the value heap.
- `Value` does not derive `Eq` (closures aren't comparable); the
  error types also drop `Eq` to keep them embeddable in each other.
  They remain `Debug + Clone + PartialEq`, which is what tests use.

---

## Section C — bootstrap compiler (HANDOVER §6)

Phase C is the production Sentinel compiler in Rust per ADR 0009. C0
is the end-to-end pipeline for the smallest language subset (let,
arithmetic, if, function calls) compiling to LLVM with no type
system — everything i64 — to prove the pipeline shape works. Six
sub-phases C0.0-C0.5.

The Phase C compiler crates are populated in pipeline order across
C0-C5 per ADR 0009 D7. As of C0.0, sentinel-syntax has a lexer; the
remaining nine compiler crates (sentinel-ast, sentinel-resolve,
sentinel-types, sentinel-hir, sentinel-mir, sentinel-codegen,
sentinel-driver, sentinel-runtime, sentinel-lsp) remain 20-line
scaffold stubs.

### C.1 Phase Tracker

| Phase | Title                                                          | Status  | Commit  |
|-------|----------------------------------------------------------------|---------|---------|
| C0.0  | Tokens + lexer + tests/ui/ harness + 1 lex-error UI test       | Done    | 8f37381 |
| C0.1  | Hand-written parser + AST + `snc parse` subcommand             | Done    | 7e32e8c |
| C0.2  | LLVM codegen + `snc build` + first runnable binary + tests/pass/ | Done  | 0b07931 |
| C0.3  | let bindings + variable references (i64 everywhere)            | Done    | 80d2b6b |
| C0.4  | if/else + block expressions + `print` calls + first stdout     | Done    | baf68fc |
| C0.5  | fn definitions + main entry; **C0 go/no-go passes**            | Done    | 6ce8336 |
| C1.0a | sentinel-base crate: Salsa db trait + SourceFile input + Diagnostic accumulator | Done | 09dc8c3 |
| C1.0b | Wrap lex/parse as `#[salsa::tracked]` queries; driver uses SentinelDatabase | Done | 557cc60 |
| C1.0c | Codegen-salsa decision: defer until typed HIR (C1.2+); ADR 0011 D1 amended | Done | 8b58644 |
| C1.1.1 | Scaffold sentinel-resolve: VarId/FnId, parallel-tree resolved AST, resolve() + salsa query | Done | 438dd16 |
| C1.1.2 | Codegen consumes ResolvedProgram; driver chains resolve_query | Done | 9374edf |
| ADR 12 | Concrete C1 surface syntax (annotation grammar + bool/cmp/logical ops) | Done | 6ab3661 |
| C1.2.1 | Lexer `:` token                                                | Done | af16655 |
| C1.2.2 | AST/parser annotation grammar + 22 fixture rewrite             | Done | 90965a5 |
| C1.2.3 | sentinel-types scaffold (Type, TypedProgram, check, check_query) | Done | ded07bc |
| C1.2.4 | Codegen consumes TypedProgram; driver chains check_query       | Done | c9a21ff |
| C1.3.1 | Lexer: 11 C1.3 tokens (`true` `false` `==` `!=` `<` `<=` `>` `>=` `&&` `\|\|` `!`) | Done | 2801a81 |
| C1.3.2-4 | AST + parser + resolve + types + codegen for bool/cmp/logic/`!`; type universe widens to `{I64, I32, Bool}` | Done | cd1c0d4 |
| C1.3.5 | Retire ADR 0010 D9 C-style truthy; rewrite 6 if-fixtures; add 7 c13 pass-tests | Done | ba5fd9d |
| ADR 13 | Concrete C1.4 surface syntax (struct decl + literal + field access) | Done | e93635b |
| C1.4.1 | Lexer: `struct` keyword + `.` token                            | Done | f34b401 |
| C1.4.2-6 | AST + parser + resolve + types + codegen for structs (bundled) | Done | aa8f252 |
| ADR 14 | Concrete C1.5 surface syntax (`?T` nullables + null + unwrap_or/is_some) | Done | 3cb1238 |
| C1.5.1 | Lexer: `null` keyword + `?` token                              | Done | dff8642 |
| C1.5.2-6 | AST + parser + resolve + types + codegen for `?T` (bundled)   | Done | 1d0adae |
| ADR 15 | Concrete C1.6 surface syntax (arrays + heap + len + D11 unlock) | Done | 8924d38 |
| C1.6.1 | Lexer: `[` and `]` tokens                                      | Done | 3cfd49f |
| C1.6.2-6 | Runtime + AST + parser + resolve + types + codegen for arrays (bundled) | Done | 8c5bbbe |
| C1.7+ | Generics                                                        | Planned |         |

ADR 0010 (concrete C0 surface syntax) lands between C0.0 and C0.1
per ADR 0009 D8.

Test coverage as of C0.0: 13 sentinel-syntax tests (11 lexer + 1
smoke + 1 UI integration). The UI test runs
`tests/ui/lex_invalid_char.sentinel` (one-line `let x = @`) through
the lexer and snapshots the miette-formatted diagnostic at
`crates/sentinel-syntax/tests/snapshots/ui__ui_lex_invalid_char.snap`.

Test coverage as of C0.1: sentinel-syntax gains 23 parser unit
tests (six covering the precedence ladder and left-associativity
on both add and mul; three covering unary minus including the
unary-binds-tighter-than-mul rule; three covering span tracking
across full / parenthesized / unary expressions; seven covering
parse errors — unmatched open paren, unexpected close paren, EOF
after operator, EOF after unary, lex-error passthrough, int-lit
overflow, trailing garbage; plus an int_lit_zero edge case and a
nested-parens test). One new UI integration test
(`ui_parse_unbalanced_paren`, snapshotted at
`ui__ui_parse_unbalanced_paren.snap`) snapshots the
`unmatched_paren` diagnostic for the fixture `(1 + 2`. sentinel-ast
gains 6 Display tests (int literal, binary, nested precedence,
unary, plus the BinOp::symbol and UnaryOp::symbol helpers) on top
of its existing smoke. Workspace delta: +30 active tests.

Test coverage as of C0.2: sentinel-codegen lifts out of scaffold-
stub status with 1 new target-init probe (`target_init_does_not_panic`)
on top of its existing smoke. sentinel-driver gains 5 pass tests at
`crates/sentinel-driver/tests/pass.rs` (`pass_c02_arithmetic`,
`pass_c02_precedence`, `pass_c02_parens`, `pass_c02_unary`,
`pass_c02_division`) covering the four operators, precedence,
parens, and unary minus via the full pipeline. The runner uses
`CARGO_BIN_EXE_snc` to locate the snc binary that cargo builds
before integration tests run; per-fixture executables land in
`target/sentinel-pass/` (gitignored). Workspace delta: +6 active
tests (353 total).

Test coverage as of C0.3: sentinel-ast gains 5 Display tests
covering Var, StmtKind::Let, StmtKind::Expr, Program with empty
stmts (which prints just the tail — preserving C0.1/C0.2 output
verbatim), and Program with stmts (one stmt per line + tail).
sentinel-syntax lib gains 14 parser tests: 2 Var-in-expression
tests, 6 program-level happy paths (empty stmts, single let,
multiple lets, expr-statement followed by tail, let-uses-let, span
tracking on `let` covering keyword through `;`), and 6 program-
level error cases (empty input, only-let-no-tail, missing `;`,
missing `=`, missing identifier, bare `let`). sentinel-codegen
gains 4 lib tests: empty-stmts program, let program, undefined
variable rejection, redeclared variable rejection. sentinel-driver
gains 4 pass tests (`pass_c03_simple_let`,
`pass_c03_multiple_lets`, `pass_c03_let_uses_let`,
`pass_c03_expr_statement` — the last verifies that an expression-
statement is computed but its result is discarded). Workspace
delta: +27 active tests (380 total).

A UI snapshot for the `undefined_variable` codegen diagnostic was
deliberately deferred — the lib unit tests cover the error variants
and a dedicated UI runner for codegen errors lands when C0.4+
introduces more codegen-stage diagnostics worth coordinating
(unbound call target, arity mismatch, etc.).

Test coverage as of C0.4: sentinel-ast gains 7 new Display tests
covering block/if/call/expr_block and the call-zero/one/two-arg
variants. sentinel-syntax lib gains 20 new parser tests: 3 for
blocks (simple, with stmt, in arithmetic position as atom), 4 for
if/else (simple, else-if chain, with var condition, in parens
inside arithmetic), 6 for calls (zero/one/multi args, trailing
comma, in arithmetic, with complex arg), 1 for var-vs-call
disambiguation, 1 for a program with a print-statement, and 5
error cases (if missing else, if missing then-block, empty block,
call unclosed args, block unclosed after tail). sentinel-codegen
gains 6 new tests (if expression, block expression, call to
print, undefined function, arity mismatch too-many, arity
mismatch too-few). sentinel-runtime gains its first real
function (`sentinel_print`) with a smoke + return-zero test.
sentinel-driver gains 8 new pass-test fixtures (print_simple,
print_then_tail, if_true_branch, if_false_branch, if_with_var_cond,
else_if_chain, block_expression, if_with_print) — five assert
on exit codes, three assert on stdout content + exit. Workspace
delta: +43 active tests (423 total).

Test coverage as of C0.5: sentinel-ast gains 4 new FnDef-related
Display tests (display_program_one_main, display_program_two_fns,
display_fn_def_zero_params, display_fn_def_multi_params) and
retires 2 stmt+tail Program tests, net +2. sentinel-syntax lib
gains 14 new parser tests covering fn-def parsing (single fn,
fn with one/multi/trailing-comma params, multi-fn programs,
fn-def Display round-trip, span tracking) plus 6 fn-def error
cases (top-level not-fn, top-level bare expr, missing name, missing
parens, missing body, bad param name); a `parse_block_str` public
function is added so the existing C0.3-0.4 parse_program_* tests
keep working with brace-wrapped inputs. sentinel-codegen gains 1
net test (compile_main_with_int_lit, compile_main_with_let_program,
compile_rejects_missing_main, compile_rejects_redefined_function,
compile_rejects_user_redefining_print, compile_multi_fn_with_
forward_ref, compile_call_to_user_fn_arity_check — 7 new tests but
with restructuring of the C0.4 tests around the new main-required
shape, the net is +1). sentinel-driver gains 5 new pass-test
fixtures: c05_simple_fn (double + main), c05_multi_arg_fn (add),
c05_forward_ref (main calls fn defined after it), c05_call_chain
(quad = double(double(...))), and the C0 acceptance program
**c05_go_no_go** (the ADR 0010 appendix double + pick + main with
print). All 17 pre-C0.5 fixtures are mechanically rewrapped in
`fn main() { ... }` for the hard-break top-level shape change.
The UI fixture parse_unbalanced_paren rewrites to
`fn main() { (1 + 2 }` so it remains valid C0.5 program shape with
the embedded error. Workspace delta: +22 active tests (445 total).

Test coverage as of C1.0b: sentinel-syntax gains a new `query`
module hosting two `#[salsa::tracked]` queries and seven unit tests
covering positive lex / positive parse / diagnostic-accumulation on
lex error / diagnostic-accumulation on parse error / lex error
propagated through parse with lex stage preserved / cache stability
across reruns for both queries. sentinel-driver's bin gains a
concrete `SentinelDatabase` struct (Storage<Self> + salsa::Database
impl with no-op salsa_event + SentinelDb impl); `run_parse` and
`run_build` are refactored to instantiate the DB, set a SourceFile,
call `parse_query`, and collect diagnostics via the accumulator.
sentinel-ast types `Spanned<T>`, `Block`, `Param`, `FnDef`,
`Program`, `StmtKind`, `ExprKind` gain `#[derive(Hash)]`
(non-behavioral; required for salsa-friendliness of any future
tracked-struct fields, even though C1.0b avoids tracked structs by
going through the accumulator). Workspace delta: +7 active tests
(453 total: 90 syntax lib + 2 ui + 22 ast + 13 codegen + 22 pass +
2 runtime + 3 base + 5 stub + 69 broker + 226 effects-proto). All
22 C0 pass-test fixtures still run end-to-end through the new
query-based driver path; the C0 go/no-go program at
`tests/pass/c05_go_no_go.sentinel` still produces stdout `10\n`
exit 0. See C.3 design decisions 33-36.

### C.2 Crate Layout (C0.5)

    crates/sentinel-base/                 (C1.0a)
      Cargo.toml          deps: salsa, thiserror, tracing
      src/
        lib.rs            SourceFile (#[salsa::input] with `path`
                          and `text` fields); SentinelDb trait
                          (#[salsa::db], inherits salsa::Database);
                          Diagnostic accumulator (#[salsa::
                          accumulator]) with stage / severity /
                          code / message / span fields. Test-only
                          TestDb verifies the salsa machinery
                          (3 tests). Downstream pipeline crates
                          plug into SentinelDb at C1.0b (lex/parse)
                          and C1.0c (codegen).

    crates/sentinel-ast/
      Cargo.toml          deps: tracing, thiserror
      src/
        lib.rs            Span (= Range<usize>), Spanned<T>, BinOp
                          (Add|Sub|Mul|Div), UnaryOp (Neg), ExprKind
                          (IntLit | Var | Unary | Binary | Block | If
                          | Call), Expr = Spanned<ExprKind>; StmtKind
                          (Let { name, name_span, value } | Expr),
                          Stmt = Spanned<StmtKind>; Block { stmts,
                          tail, span }; Param { name, span }; FnDef
                          { name, name_span, params, body: Block,
                          span }; Program { fns: Vec<FnDef>, span }
                          (C0.5 top-level — was stmts+tail at
                          C0.3-0.4); Display impls for all (Program
                          prints fn-defs newline-separated, FnDef
                          prints `(fn name (params) body)`).
                          **C1.0b**: Spanned<T>, Block, Param, FnDef,
                          Program, StmtKind, ExprKind all derive
                          Hash; BinOp / UnaryOp already did. Required
                          for salsa-friendliness of any future
                          tracked-struct field, even though C1.0b
                          itself routes errors through the accumulator
                          and avoids tracked-struct fields.

    crates/sentinel-syntax/                (C1.0b adds query module)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path, C1.0b), logos, miette, salsa
                          (C1.0b), thiserror, tracing
                          dev-deps: insta
      src/
        lib.rs            module declarations + public re-exports
                          (lex, LexError, TokenKind from lexer;
                          parse, parse_expr, parse_block_str,
                          ParseError, Parser from parser;
                          lex_query, parse_query from query — C1.0b;
                          Program, Span, Spanned, Stmt, StmtKind
                          re-exported from sentinel-ast)
        lexer.rs          logos-based TokenKind (4 keywords + 11
                          punctuation kinds + Ident + IntLit; skip
                          patterns for whitespace and `//` line
                          comments); imports Spanned from
                          sentinel-ast; LexError (miette::Diagnostic);
                          pure lex() fn returning (tokens, errors)
        parser.rs         hand-written recursive descent. Three
                          pure entry points: parse(src) ->
                          Result<Program> at C0.5+ parses one or
                          more fn-defs (parse_program loops over
                          parse_fn_def, which eats `fn Ident
                          ( params? ) block`); parse_expr(src) ->
                          Result<Expr> retains the single-expression
                          contract for existing C0.1 tests + REPL;
                          parse_block_str(src) -> Result<Block>
                          parses a brace-wrapped block in isolation
                          (used by tests + future REPL/completion).
                          Internal: parse_block (for `{ stmt* tail
                          }` from `if` branches, atoms, and fn
                          bodies), parse_let_stmt (`let Ident =
                          expr ;`), parse_if (with else-if chain
                          via synthetic Block wrapping), parse_atom
                          dispatches IntLit / Ident-with-`(`-is-call
                          / bare-Ident-is-Var / `{`-is-Block /
                          `(`-is-paren. ParseError variants
                          unchanged since C0.3: Lex (transparent),
                          UnexpectedToken, UnexpectedEof,
                          UnmatchedParen, IntLitOverflow
        query.rs          (C1.0b) two `#[salsa::tracked]` queries:
                          lex_query(db, file) -> Vec<Spanned<TokenKind>>
                          (return_ref) and parse_query(db, file) ->
                          Option<Program> (return_ref). Each
                          accumulates `sentinel_base::Diagnostic`s
                          for errors via the salsa::Accumulator
                          trait. Private helpers
                          lex_error_to_diagnostic and
                          parse_error_to_diagnostic perform the
                          (stage, code, message, span) conversion;
                          ParseError::Lex forwards to the lex
                          converter so a lex-error-through-parse
                          still carries the lex stage/code. The
                          queries are independent — parse_query calls
                          `parse(src)` directly rather than depending
                          on lex_query, matching parse's existing
                          fail-fast-on-lex-error semantics. Test-only
                          TestDb mirrors the one in sentinel-base
                          (7 tests).
      tests/
        ui.rs             integration runner; shared themed-none
                          handler at 80 cols; ui_lex_invalid_char,
                          ui_parse_unbalanced_paren
        snapshots/
          ui__ui_lex_invalid_char.snap
          ui__ui_parse_unbalanced_paren.snap

    crates/sentinel-resolve/                (C1.1: populated)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path), sentinel-syntax (path), miette,
                          salsa, thiserror, tracing
      src/
        lib.rs            Stable identifiers: VarId(u32), FnId(u32),
                          plus the PRINT_FN_ID const (= FnId(0)).
                          FnSignature { id, name, name_span: Option<Span>,
                          arity, is_main, is_runtime }. Parallel-tree
                          resolved AST: ResolvedProgram { fns:
                          Vec<ResolvedFnDef>, fn_signatures: Vec<FnSignature>,
                          span }; ResolvedFnDef, ResolvedParam,
                          ResolvedBlock, ResolvedStmt(Kind),
                          ResolvedExpr(Kind) all mirror their AST
                          counterparts with Var/Call replaced by
                          IDs; binding sites retain their source
                          name for diagnostics + IR debug names.
                          ResolveError with 6 variants:
                          UndefinedVariable, RedeclaredVariable,
                          UndefinedFunction, ArityMismatch,
                          RedefinedFunction, MissingMain (moved from
                          CodegenError at C1.1.2). resolve(program:
                          &Program) -> Result<ResolvedProgram,
                          ResolveError> is the pure-function entry
                          point: pass 1 builds the fn table
                          (`print` as FnId(0), user fns following),
                          pass 2 resolves each fn body with a
                          per-fn vars HashMap. RHS of `let x = expr`
                          resolves BEFORE binding x, so `let x = x`
                          with no outer x is UndefinedVariable.
                          resolve_query(db, file) -> &Option<ResolvedProgram>
                          is the `#[salsa::tracked]` wrapper that
                          chains on sentinel_syntax::parse_query;
                          errors flow through the Diagnostic
                          accumulator with stage="resolve" and parse
                          errors propagate transitively. 21 tests
                          (positive paths + each error variant + 4
                          salsa query tests including parse-error
                          propagation and cache validation).

    crates/sentinel-types/                (C1.2.3: populated)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path), sentinel-resolve (path),
                          sentinel-syntax (path), miette, salsa,
                          thiserror, tracing
      src/
        lib.rs            Type universe: `Type::I64` only at C1.2 per
                          ADR 0012 D4; C1.3 adds I32 + Bool. Display
                          renders lowercase identifier (`i64`).
                          Parallel-tree typed AST mirrors
                          ResolvedProgram with `ty: Type` fields on
                          expressions: TypedProgram { fns, fn_signatures,
                          span }; TypedFnSignature { id, name,
                          name_span, param_types: Vec<Type>, return_type,
                          is_main, is_runtime }; TypedFnDef, TypedParam
                          (with `ty: Type`), TypedBlock (with
                          `ty: Type` == tail.ty), TypedStmt(Kind),
                          TypedExpr { kind, span, ty }, TypedExprKind
                          (variants identical to ResolvedExprKind).
                          TypeError with 4 variants: UnknownType
                          (annotation says something other than known
                          type name), Mismatch (cross-expression type
                          clash), ReturnTypeMismatch (fn body type ≠
                          declared return), CallArgMismatch (call arg
                          type ≠ callee param type). At C1.2 only
                          UnknownType is reachable since everything
                          types to I64; the others activate at C1.3.
                          check(program: &ResolvedProgram) ->
                          Result<TypedProgram, TypeError> pure-function
                          entry point: pass 1 builds typed_signatures
                          by resolving every fn's TypeExpr annotations
                          (print pre-registered); pass 2 walks each fn
                          body with a per-fn VarTypeEnv: HashMap<VarId,
                          Type> seeded from params. check_query(db,
                          file) -> &Option<TypedProgram> is the
                          #[salsa::tracked] wrapper that chains on
                          sentinel_resolve::resolve_query; errors flow
                          through the Diagnostic accumulator with
                          stage="types"; upstream stage diagnostics
                          propagate transitively. 15 unit tests:
                          positive paths (minimal main, annotated/
                          unannotated let, params+use, if/else
                          branches, print call, full go/no-go), 3
                          UnknownType paths (param, return, let
                          annotation), 4 salsa query tests including
                          resolve-error propagation and cache
                          validation.

    crates/sentinel-codegen/                (C1.2.4 rewrite — consumes TypedProgram)
      Cargo.toml          deps: sentinel-ast (path), sentinel-resolve
                          (path), sentinel-types (path, C1.2.4),
                          inkwell (llvm18-0 feature, workspace-pinned),
                          miette, thiserror, tracing
                          dev-deps: sentinel-syntax (for src-string
                          test driving via parse + resolve + check)
                          lints.rust: unsafe_code = "allow" (inkwell
                          uses unsafe internally for FFI)
      src/
        lib.rs            compile_to_object(program: &TypedProgram,
                          output_path) builds an LLVM module
                          containing all fns declared by the program.
                          **Two-pass**: pass 1 declares every fn by
                          iterating program.fn_signatures (the runtime
                          `print` maps to LLVM symbol `sentinel_print`
                          via signature.is_runtime; otherwise the
                          source name); pass 2 emits each user fn
                          body. `main` returns i32 (C ABI shape,
                          truncated from i64 body value) — gated on
                          signature.is_main via the TypedFnSignature;
                          other fns return i64. CodegenCtx<'ctx, 'a>
                          threads &context + builder + i64_type +
                          HashMap<FnId, FunctionValue> fns table +
                          current_fn + HashMap<VarId, PointerValue>
                          vars map. C1.2.4 changes: input shape
                          (Typed* instead of Resolved*) and how
                          is_main is read (via program.signature(id)
                          instead of fn_def.signature(program)).
                          Otherwise the LLVM lowering is unchanged
                          from C1.1.2 — at C1.2 every type is I64 so
                          the Type field on TypedExpr is not yet
                          driving instruction selection; that's
                          C1.3's bool/i32 work. find_var_name walk
                          ported to TypedBlock/TypedExpr. CodegenError
                          unchanged (5 LLVM-lowering-only variants
                          since C1.1.2).

    crates/sentinel-runtime/
      Cargo.toml          deps: tracing, thiserror
                          [lib] crate-type = ["lib", "staticlib"]
                          (the staticlib output is libsentinel_
                          runtime.a, linked into Sentinel programs
                          by the driver via the system cc; the rlib
                          remains for Rust consumers)
      src/
        lib.rs            sentinel_print(i64) -> i64 via
                          `#[no_mangle] extern "C"` — writes the
                          i64 to stdout as ASCII decimal + newline
                          and returns 0 (the call expression's
                          value per ADR 0010 D11)

    crates/sentinel-driver/                (C1.2.4 chains check_query)
      Cargo.toml          deps: sentinel-base (path, C1.0b),
                          sentinel-codegen (path), sentinel-resolve
                          (path, C1.1.2), sentinel-syntax (path),
                          sentinel-types (path, C1.2.4),
                          miette (with "fancy" feature), salsa
                          (C1.0b), thiserror, tracing
      src/
        main.rs           snc binary; subcommands `parse` and `build`
                          (C0.1+ shape; both lifted from Expr to
                          Program at C0.3). **C1.0b**: defines
                          `SentinelDatabase` (storage: salsa::Storage
                          <Self>) with `#[salsa::db]` impls for
                          `salsa::Database` (no-op salsa_event) and
                          `SentinelDb`. `run_parse` calls
                          sentinel_syntax::parse_query and pretty-
                          prints the AST. **C1.2.4**: `run_build`
                          chains sentinel_types::check_query (which
                          depends transitively on resolve_query and
                          parse_query in the salsa graph), collects
                          diagnostics via
                          check_query::accumulated::<Diagnostic>
                          — picks up parse + resolve + types-stage
                          diagnostics in one collection — and feeds
                          the resulting &TypedProgram to
                          sentinel_codegen::compile_to_object.
                          Diagnostics render through miette::MietteDiagnostic
                          (constructed at runtime from
                          stage/code/message/span — drops per-variant
                          help text and label text; rough but
                          functional, refinement deferred). `build`
                          then invokes the system `cc` on the
                          emitted `.o` plus `libsentinel_runtime.a`
                          (found via current_exe().parent()) to
                          produce the executable. Output defaults
                          to <file_stem>; exit codes 0 / 1 / 2.
      tests/
        pass.rs           pass-test runner; reads workspace-root
                          tests/pass/*.sentinel; uses CARGO_BIN_EXE_snc
                          to locate the binary cargo built for the
                          integration tests; builds each fixture into
                          target/sentinel-pass/ and asserts on the
                          executable's exit code

    tests/                                (workspace root, ADR 0009 D5)
      ui/
        lex_invalid_char.sentinel         `let x = @` fixture
        parse_unbalanced_paren.sentinel   `(1 + 2` fixture
      pass/                               (all wrapped in `fn main() { ... }` at C0.5)
        c02_arithmetic.sentinel           `6 + 7` -> exit 13
        c02_precedence.sentinel           `1 + 2 * 3` -> exit 7
        c02_parens.sentinel               `(5 + 3) * 2 - 1` -> exit 15
        c02_unary.sentinel                `-(-5)` -> exit 5
        c02_division.sentinel             `12 / 3` -> exit 4
        c03_simple_let.sentinel           `let x = 5; x` -> exit 5
        c03_multiple_lets.sentinel        `let x = 3; let y = 4; x + y` -> exit 7
        c03_let_uses_let.sentinel         `let a = 2; let b = a * 3; b + 1` -> exit 7
        c03_expr_statement.sentinel       `let x = 1; x + 99; 5` -> exit 5
        c04_print_simple.sentinel         `print(42)` -> stdout "42\n", exit 0
        c04_print_then_tail.sentinel      let + 2x print + tail -> stdout "7\n14\n", exit 21
        c04_if_true_branch.sentinel       `if 1 { 42 } else { 99 }` -> exit 42
        c04_if_false_branch.sentinel      `if 0 { 42 } else { 99 }` -> exit 99
        c04_if_with_var_cond.sentinel     let x=5; if x { x*2 } else { 0 } -> exit 10
        c04_else_if_chain.sentinel        let x=0; if/else-if/else -> exit 3
        c04_block_expression.sentinel     `let r = { let y = 4; y + 1 }; r * 2` -> exit 10
        c04_if_with_print.sentinel        if/print -> stdout "100\n", exit 0
        c05_simple_fn.sentinel            double + main -> exit 14
        c05_multi_arg_fn.sentinel         add(5, 6) -> exit 11
        c05_forward_ref.sentinel          main calls triple defined later -> exit 12
        c05_call_chain.sentinel           quad(3) = double(double(3)) -> exit 12
        c05_go_no_go.sentinel             ADR 0010 appendix: double + pick + main
                                          with print -> stdout "10\n", exit 0

    .cargo/
      config.toml         workspace-local cargo config (C0.2): [env]
                          sets LLVM_SYS_180_PREFIX (non-forcing —
                          developers with the env in zshrc are
                          unaffected); target.'cfg(target_os =
                          "macos")' rustflags add /opt/homebrew/lib
                          and /usr/local/lib to the link search path
                          so the linker can find brew-installed
                          zstd/libxml2 that LLVM 18 references

Three scaffold-stub compiler crates remain at 20-line
`crate_name() + smoke` stubs per ADR 0009 D7. Updated population
schedule (sentinel-resolve populated at C1.1, sentinel-types at
C1.2):

  - sentinel-hir:      C1.3+ (typed HIR may replace TypedProgram
                       once enough type-system features land to
                       motivate a separate HIR layer; or HIR may
                       remain folded into sentinel-types)
  - sentinel-mir:      C2 (SSA-form IR for region/borrow checking)
  - sentinel-lsp:      C5

### C.3 Design Decisions (C0)

ADR 0009 (D1-D8) is authoritative; in-source highlights:

1. Lexer uses `logos`. ADR 0009 D4 prescribes hand-written recursive
   descent for the parser only; lexers benefit from the regex-DFA
   payoff with no ergonomic cost.
2. `lex(src: &str) -> (Vec<Spanned<TokenKind>>, Vec<LexError>)` is
   a pure function per ADR 0009 D1a's "C0 pipeline stages are pure
   functions" discipline. No shared mutable state. No `&mut Cx`
   threading. Diagnostics accumulate via the return value.
3. No CST/AST split (ADR 0009 D4). The lexer's output is a flat
   `Vec<Spanned<TokenKind>>`; C0.1's parser will produce a direct
   AST enum.
4. Keywords (`let`, `fn`, `if`, `else`) lex via dedicated `#[token]`
   rules. logos's longest-match guarantees `letter` lexes as
   `Ident`, not `Let` + `ter`. See `lex_keyword_prefix_is_ident`.
5. Valid tokens still flow through when `LexError`s occur (the
   lexer collects errors rather than fail-fast). C0.1's parser
   decides whether to stop on lex errors or continue.
6. UI snapshot uses `GraphicalTheme::none()` + `width(80)` for
   host-independence. Terminal-width detection and ANSI colors
   would make snapshots host-dependent.
7. Workspace-root `tests/ui/` holds data files (HANDOVER §3.2); the
   integration runner lives in `crates/sentinel-syntax/tests/ui.rs`.
   insta snapshots stay at insta's default location (next to the
   runner). Centralizing snapshots under workspace-root
   `tests/snapshots/` is deferred until there's more than one
   runner crate to coordinate.
8. Parser is the pure function `parse(src: &str) -> Result<Expr,
   ParseError>` per ADR 0009 D1a. Lex errors block parsing and
   surface through a transparent `ParseError::Lex(LexError)` variant
   — the front end has a single error type for the driver to handle.
   Parse errors are fail-fast (one error per call); error recovery
   is deferred until parser ergonomics demand it.
9. `Span` and `Spanned<T>` live in `sentinel-ast` rather than
   `sentinel-syntax` because the AST is conceptually below syntax
   in the pipeline. The lexer's `Spanned<TokenKind>` and the parser's
   `Spanned<ExprKind>` are the same generic type. `sentinel-syntax`
   re-exports `Span` and `Spanned` for crates that consume tokens
   and AST nodes together.
10. Parens are syntactic only — they are not represented as a
    distinct AST node. `(1 + 2)` and `1 + 2` produce the same AST
    shape; the outer span on the parenthesized form covers the
    parens. C5+ LSP-style source-preserving formatting may revisit
    this if exact original-source round-trip becomes a requirement.
11. The driver uses miette's default (fancy color) `GraphicalReport
    Handler` for human-facing errors; UI tests use
    `GraphicalTheme::none()` + 80-col width in the test runner for
    host-independent snapshots. Two separate code paths, two
    separate concerns.
12. sentinel-codegen lowers `Expr` to an LLVM IR module via inkwell
    0.5 with the `llvm18-0` feature. The emitted module defines
    `main() -> i32` whose return value is the i64 expression value
    truncated to i32 — the temporary exit-code-is-the-answer
    convention. ADR 0010 D11's `print(x)` will replace it at C0.4
    when function calls land; the truncation goes away when stdout
    is the result channel.
13. Linking lives in the driver, not in sentinel-codegen. The
    driver's `build` subcommand invokes the system `cc` on the
    emitted `.o` to produce the executable. Linking is platform
    glue (linker flags, library search paths, dynamic loader
    conventions) rather than a compiler concern; sentinel-codegen
    stays focused on IR generation. The `cc` invocation will move
    behind a more controlled interface when cross-compilation is
    in scope (C5+).
14. `.cargo/config.toml` (workspace root) sets two things: (a)
    `LLVM_SYS_180_PREFIX` via cargo's non-forcing `[env]` so
    subprocess shells (CI, automation) see what the developer's
    interactive zshrc already provides; (b) target-conditional
    `rustflags` adding `/opt/homebrew/lib` and `/usr/local/lib`
    to the link search path so the linker finds brew-installed
    `zstd` and `libxml2` that LLVM 18 references but `llvm-sys`
    does not emit search paths for. Non-existent paths are
    silently ignored. This is a macOS-specific workaround; when
    Sentinel grows beyond macOS the configuration moves to a
    build script that probes `llvm-config --libdir`.
15. ADR 0009 D7 prescribed a `sentinel-types::check() -> Result<(),
    Diagnostic>` stub at C0.2 "so the pipeline shape is right when
    C1 fills it in." C0.2 deferred this to C0.3 (or possibly C1)
    because the stub adds no value at C0.2: arithmetic has no type
    semantics, the no-op `check()` would be threaded through the
    driver as `parse -> check (noop) -> codegen`, and the driver
    pipeline is already the right shape without it. C0.3 confirmed
    the deferral — let-binding scope tracking lives in codegen
    (see 20), not in a separate types pass. ADR 0009 status line
    records the deviation.
16. `ExprKind::Var(String)` stores the bound name as an owned
    `String` rather than an interned identifier. Interning is a C1
    concern: it lives in `sentinel-resolve` where name resolution
    formalises the symbol table. C0.3's `String` allocations are
    bounded by program size and disappear at codegen time, so the
    cost is acceptable.
17. Flat per-function scoping in C0.3. There is no notion of
    nested scope yet; redeclaring an existing name in the same
    function is `CodegenError::RedeclaredVariable` rather than
    shadowing. Block-scoped shadowing arrives with nested blocks
    at C0.4 (when `if`/`else` requires them). The choice was
    deliberate at the start of C0.3 — flat is simpler to
    implement and easier to diagnose; shadowing is a useful
    feature but not load-bearing at C0.
18. `StmtKind::Let { name, name_span, value }` carries the name's
    own span in addition to the wrapping `Stmt`'s full span. This
    is so redeclaration diagnostics can point at just the
    identifier rather than the whole `let x = expr;` statement.
    The pattern generalises: future statements with prominent
    sub-spans (struct definitions, function parameters) carry
    their own field-spans alongside the statement-level span.
19. `Program { stmts, tail, span }` is the AST top-level rather
    than an `ExprKind::Block(Vec<Stmt>, Box<Expr>)`. The Block-
    expression approach would have changed the C0.1/C0.2 parser
    tests (they would read the IntLit through a Block wrapper);
    keeping `Program` distinct lets `parse_expr(src)` keep its
    "single expression, no statements" contract that those tests
    rely on. Block arrives as an expression at C0.4 when `if`/
    `else` needs nested blocks; both representations coexist —
    Program at the top, Block inside expressions.
20. Codegen name resolution: the codegen pass maintains a
    `HashMap<String, PointerValue>` from variable names to LLVM
    alloca pointers. Each `let` enters the map; each `Var`
    reference reads from it. When `sentinel-resolve` lands at C1
    (per ADR 0009 D7), this map moves out of codegen and codegen
    becomes a pure structural lowering pass against a resolved
    AST. The C0.3 arrangement is a deliberate short-term
    architectural debt — STATE.md flags it in this list so the
    refactor at C1 is obvious.
21. `Block { stmts, tail, span }` and `Program { stmts, tail,
    span }` have the same shape. Both are AST struct types, and a
    Block can be promoted to an Expr via `ExprKind::Block(Box<Block>)`.
    Program is the top-level form (no surrounding braces in source;
    implicit body of the future `main` function); Block is the
    brace-wrapped nested form. Keeping them as distinct types lets
    a reader see at a glance which level of nesting a value
    represents, at the cost of one redundant struct. At C0.5 when
    `fn main() { … }` lands, Program may collapse into a list of
    fn-defs each containing a Block body.
22. `if`/`else` codegen uses an **alloca-based result slot** rather
    than LLVM `phi` nodes. The result alloca is created in the
    current insert position; both branches `store` the branch
    value; the merge block does `load` from the slot to produce
    the if-expression's value. At -O0 this is correct and easy to
    read in the IR; mem2reg promotes the alloca to phi when
    optimization is enabled. The C0.4 plan A pinned this choice up
    front.
23. `if` condition is `cond != 0` (C-style truthy per ADR 0010
    D9). The condition is computed as i64 and compared to zero via
    `IntPredicate::NE`. When C1 introduces `bool`, ADR 0010 D9's
    Revisit clause fires and the comparison goes away — the
    condition will be `bool`-typed by then.
24. `if` is positioned at the top of `parse_expr`, not inside the
    arithmetic precedence ladder. `if x { 1 } else { 2 } + 3` is
    a parse error ("expected end of input" after the if-expr);
    `(if x { 1 } else { 2 }) + 3` works because the parens accept
    any expression. The restriction parallels Rust's; revisited
    if a real program wants the looser form.
25. sentinel-runtime is built with `crate-type = ["lib",
    "staticlib"]` so cargo produces both `libsentinel_runtime.a`
    (linked into Sentinel programs by the system cc) and
    `libsentinel_runtime.rlib` (consumable from Rust if a future
    integration test wants to call the runtime directly). The
    driver locates the staticlib via
    `current_exe().parent().join("libsentinel_runtime.a")` because
    cargo puts the snc bin and the runtime in the same target
    directory — works for both `cargo run --bin snc` and
    `CARGO_BIN_EXE_snc`-driven integration tests.
26. `CodegenCtx<'ctx, 'a>` decouples the LLVM IR lifetime ('ctx,
    bound by Context) from the ctx struct's borrow lifetime ('a,
    bound by an inner scope in `compile_to_object`). The two
    lifetimes let the ctx be scoped to drop before `module.
    verify()` and `target_machine.write_to_file(&module, …)` run
    — the borrow checker can see that the ctx's borrows end at
    the inner block's `}` and so the later `module.verify()` is
    unaliased. C0.5 dropped the `&module` field from the ctx
    because pass 1 declares every function up-front, so pass 2
    never needs to mutate the module — it only emits IR through
    the builder against pre-existing FunctionValues.
27. C0.5 top-level shape is **`Vec<FnDef>`** with a mandatory
    `main` entry point — a hard break from the C0.3-0.4 implicit-
    main `stmt* tail_expr` form. The existing 17 C0.2-0.4 pass
    fixtures were mechanically rewrapped in `fn main() { ... }`.
    The hard break was chosen at the C0.5 start because clean
    shape going into C1 was worth the one-time fixture rewrite
    over preserving two top-level forms forever.
28. Codegen is **two-pass** per the C0.5 plan A. Pass 1 declares
    every function (including the runtime `print` mapped to
    `sentinel_print`); pass 2 emits each body. Forward references
    work because all signatures are in the module before any body
    is emitted. The cost is one extra walk of `program.fns`; the
    benefit is no defined-before-use constraint on user code.
29. `main` returns i32 while every other fn returns i64. This
    matches the C ABI's `main` signature so the system linker is
    happy with no extra glue. The i64 -> i32 truncation happens
    inside `compile_fn` only for `main`; other fns build a normal
    i64 return.
30. `print` is reserved: pre-declared in pass 1 as the runtime
    `sentinel_print` symbol, so a user-defined `fn print(x)` at
    pass-1 declaration time collides with the pre-declaration and
    surfaces as `CodegenError::RedefinedFunction`. The check is
    by name in the fns table, not a special-case in the parser.
31. Function parameters become per-fn allocas in the entry block:
    on `compile_fn` we clear `vars`, then for each param we
    allocate an i64 slot and `store` the incoming parameter value
    into it. The body then reads parameters via the same
    `vars.get` path as `let`-bindings — uniform treatment. C0.5
    arity check fires from `lower_call` via
    `fn_value.count_params()`; it covers both `print` (declared
    with one param) and user-defined fns uniformly.
32. `parse_block_str(src)` is a new C0.5 public entry point that
    parses a single brace-wrapped block. It's used by the
    parser's own tests (so the C0.3-0.4 stmt+tail tests can wrap
    their input in `{ ... }` and keep their assertions) and is
    available for any future REPL or LSP completion machinery
    that wants to parse just a block.

33. (C1.0b, ADR 0011 D1) The salsa retrofit lands lex and parse but
    not yet codegen. `parse_query` does NOT depend on `lex_query`;
    it calls the pure `parse(src)` entry point directly. Pros: the
    salsa-aware queries inherit `parse`'s existing fail-fast-on-
    lex-error semantics with zero divergence; lex errors flow
    through exactly one diagnostic path (via
    `parse_error_to_diagnostic(ParseError::Lex)`, which forwards to
    `lex_error_to_diagnostic` so the diagnostic carries the lex
    stage/code); no risk of double-emitting a lex diagnostic from
    both queries when the driver collects via parse_query's
    accumulator alone. Cons: parse_query re-lexes internally
    (wasted CPU; bounded by program size); no salsa cache benefit
    between lex and parse. C1.0c+ may revisit if codegen or
    sentinel-types want both tokens and AST in the same incremental-
    rebuild scope — a `parse_from_tokens(src, &tokens)` helper plus
    a parse_query that depends on lex_query is the obvious move,
    paired with peeking-the-accumulator from inside parse_query to
    avoid double-diagnostics (or accepting that minor cost).

34. (C1.0b) Errors flow through the `#[salsa::accumulator]`
    pattern rather than tracked-struct fields. The C1.0a session
    halted on `Vec<LexError> as tracked-struct field` because
    `miette::SourceSpan` doesn't derive Hash; the C1.0b resolution
    is that tracked-function return values carry ONLY the success
    payload (`Vec<Spanned<TokenKind>>`, `Option<Program>`) and
    errors get converted at the query boundary into
    `sentinel_base::Diagnostic`s — a Hash-friendly struct with
    `(stage, code, message, span: Range<usize>)` — and pushed via
    `Accumulator::accumulate(db)`. The conversion drops per-variant
    `#[diagnostic(help(...))]` text and per-`#[label(...)]` text
    that the lex/parse error enums carried; the driver renders
    using `miette::MietteDiagnostic` constructed at runtime, which
    produces a less ornamented but still source-pointed diagnostic.
    Refining the help/label preservation is a follow-up; the
    pipeline shape is the C1.0b deliverable, not diagnostic
    polish.

35. (C1.0b) Hash derives across sentinel-ast (Spanned<T>, Block,
    Param, FnDef, Program, StmtKind, ExprKind) land prophylactically.
    C1.0b itself routes errors through the accumulator and does
    NOT use tracked structs, so strictly speaking Hash isn't
    required for the retrofit to compile. But: (a) HANDOVER §0.2
    step 1 prescribed it based on the C1.0a investigation; (b)
    Hash is a strictly additive derive (no breakage); (c) future
    sub-phases that DO want to put AST nodes into tracked-struct
    fields (e.g., a `#[salsa::tracked]` Module struct in C1.1 with
    a resolved-program field) inherit Hash for free. Cost is one
    derive per type; benefit is "salsa will not surprise us at the
    next sub-phase."

36. (C1.0b) The concrete `SentinelDatabase` struct lives in
    sentinel-driver/src/main.rs, not in sentinel-base. ADR 0011 D1
    placed the cross-crate `SentinelDb` trait in sentinel-base
    deliberately so pipeline crates (sentinel-syntax,
    sentinel-codegen, future sentinel-resolve, sentinel-types) can
    depend on the trait without depending on the concrete database
    that wires every query. The driver is the assembly point; the
    concrete DB lives there. Tests inside individual crates (e.g.,
    `query.rs`'s tests in sentinel-syntax) declare their own
    `TestDb` rather than reaching into the driver — same pattern
    as `sentinel-base`'s test module. Repeated minimal `TestDb`
    boilerplate (~12 lines per crate) is acceptable; a shared
    test-util crate would invert the dep direction and bloat
    test-build times.

38. (C1.1.1, ADR 0011 D4) sentinel-resolve uses a **parallel-tree**
    representation rather than a side-table or generic-AST scheme.
    ADR 0011 D4 specifies that `ResolvedProgram` "is the input AST
    with name references replaced by stable identifiers." Three
    representations were considered: (a) the parallel-tree approach
    (chosen) where ResolvedProgram has its own ResolvedExprKind /
    ResolvedStmtKind etc. mirroring the AST shape; (b) a side-table
    approach where the original AST stays untouched and resolution
    state lives in HashMap<Span, ID> tables; (c) a generic-AST
    approach where ExprKind<R> takes a reference type R that
    instantiates to String pre-resolve and to VarId/FnId post-
    resolve. Reasoning: (a) wins on debuggability and ergonomics
    (each variant is concrete, no generics to chase through error
    messages or type signatures); (b) was rejected because span-
    keyed maps are fragile (duplicate spans break it; future
    macro-expanded code would too) and HashMap-of-pointers is
    ergonomically awful; (c) was rejected because the generics
    parameter propagates through every AST type's signature and
    inflates the surface that callers (codegen, future types pass,
    LSP) have to track. The cost of (a) is keeping two parallel
    type hierarchies in sync as the AST grows; the discipline is
    "every AST change at C1.3+ updates the resolved tree in the
    same commit," which matches the rhythm of the codebase already.

39. (C1.1.1) `let x = expr` resolves the RHS BEFORE binding the
    name. So `let x = x` with no outer `x` is UndefinedVariable,
    not a self-reference. This matches the C0 codegen's behavior
    (lower_expr on the RHS happens before vars.insert(name)) and
    keeps the language consistent with Rust on this point. The
    behavior is locked by the
    `let_x_equals_x_errors_when_outer_x_undefined` unit test.

40. (C1.1.2) Codegen preserves source names in LLVM SSA labels by
    walking the current fn's body for the binding's source name at
    each Var load. The walk lives in `find_var_name_in_block` /
    `find_var_name_in_expr`. This is **purely IR readability** —
    semantically the VarId is the load-bearing identifier;
    codegen never depends on the name for lookup. Cost: O(fn-body
    size) per Var reference at codegen time. Acceptable at C1 scale
    (no fn body is more than ~50 lines at C0); revisit if codegen
    becomes a profiling bottleneck. An alternative would be to
    pre-build a HashMap<VarId, &str> per fn at the start of
    compile_fn; the walk-on-each-load avoids the bookkeeping and
    is the simpler default until measurements demand otherwise.

41. (C1.2.2, ADR 0012 D1-D4) Annotation grammar wires through the AST.
    Three parallel additions to sentinel-ast: `Param.ty: TypeExpr`
    (mandatory at C1.2 per ADR 0012 D1), `FnDef.return_type:
    TypeExpr` (mandatory), `StmtKind::Let.ty_annot: Option<TypeExpr>`
    (optional per ADR 0012 D2). `TypeExpr = Spanned<TypeExprKind>`
    where `TypeExprKind::Ident(String)` is the only variant at
    C1.2 — the enum is deliberately open-ended so C1.4's struct
    names, C1.5's `?T`, C2's `&T`/`@region T`, and C3's `secret T`
    all land as new variants without churning the existing
    annotation sites. Display preserves source-readable form
    (`(fn name (p: i64 q: i64) -> i64 body)`) so `snc parse` output
    remains human-debuggable.

42. (C1.2.2, ADR 0012 D10) Fixture rewrite is a Python script
    (`/tmp/annotate_fixtures.py`) committed alongside the feat for
    reproducibility. The regex `fn IDENT(PARAMS) {` captures
    bare-name params and injects `: i64` plus appends `-> i64`
    after `)`. All 22 .sentinel pass-test fixtures + the
    parse_unbalanced_paren UI fixture were rewritten in one pass;
    snapshots re-blessed. The script is "throwaway tooling"
    (the rewrite happens once); checking it in alongside the
    commit makes the rewrite reproducible if a future hard break
    needs the same shape.

43. (C1.2.3, ADR 0011 D5) sentinel-types uses a **parallel-tree**
    representation matching sentinel-resolve's pattern from
    C1.1.1. ResolvedExpr and TypedExpr have different shapes —
    ResolvedExpr is `Spanned<ResolvedExprKind>`, TypedExpr is a
    struct `{ kind, span, ty }`. The inline `ty: Type` field
    avoids `Spanned<(TypedExprKind, Type)>` ergonomic awkwardness
    and gives codegen one-hop access to the type during lowering.
    Same for TypedBlock (carries `ty: Type` == tail.ty so codegen
    can read the block's type without recursion). TypedStmt is a
    struct `{ kind, span }` (statements have no value type at C1.2,
    but the struct shape stays consistent with the rest of the
    typed tree). The cost is keeping yet-another parallel tree in
    sync; at C1.3 this is "the same maintenance cadence as
    sentinel-resolve" which the codebase already absorbs.

44. (C1.2.3) The `check()` pass at C1.2 is mostly mechanical
    because the universe is just `I64`. Real cross-type validation
    (Mismatch, ReturnTypeMismatch, CallArgMismatch) is implemented
    but unreachable at C1.2 since every annotation must say `i64`
    (anything else hits UnknownType first). C1.3 activates the
    dormant variants when `bool` and comparison operators introduce
    multi-type expressions. The implementation already has the
    right shape (env: HashMap<VarId, Type>, signatures resolved
    once into TypedFnSignature.param_types/return_type, recursive
    walk threading the env); only the Type universe + a few
    operator-typing rules need to expand at C1.3.

45. (C1.2.4) Codegen sheds zero LOC and gains zero LOC of LLVM
    logic at C1.2.4 — the lowering is identical to C1.1.2's
    because every type is still I64. The change is **only the
    input shape**: ResolvedExpr → TypedExpr, ResolvedBlock →
    TypedBlock, etc. The `ty: Type` field on TypedExpr is
    available to codegen but unused at C1.2; C1.3 will start
    reading it (e.g., to emit `i1` for bool conditions and `i32`
    for i32-typed values). This minimal-diff refactor keeps
    C1.2.4 reviewable; the substantive type-aware codegen work
    happens at C1.3.

46. (C1.3.1, ADR 0012 D9) The lexer additions for C1.3 — six
    comparison ops (`== != < <= > >=`), three logical ops (`&& ||
    !`), two boolean keywords (`true` `false`) — landed as a
    single ~15-LOC patch + 9 new tests. logos's longest-match
    guarantee made the precedence-aware lexing automatic: `==`
    beats `=`, `!=` beats `!`, `<=` beats `<`, `>=` beats `>` —
    no manual reordering tricks needed beyond listing them as
    separate `#[token]` entries. The change is isolated to
    `crates/sentinel-syntax/src/lexer.rs`; nothing downstream
    sees the new tokens at this commit because parser changes
    follow in step 2.

47. (C1.3.2-4 / cd1c0d4) New ExprKind variants `Cmp(CmpOp, l, r)`
    and `Logic(LogicOp, l, r)` are kept separate from `Binary`
    rather than overloading `BinOp` with all 12 operators. The
    motivation is exhaustive matching at the type-check and
    codegen layers — `Binary` typing is "same int type → same
    int type", `Cmp` is "same type → Bool", `Logic` is "Bool,
    Bool → Bool with short-circuit". Three distinct typing rules
    map cleanly to three distinct ExprKind arms; collapsing them
    into Binary would require a runtime dispatch on `BinOp` in
    every consumer. Same argument applies at the codegen layer:
    arithmetic uses `build_int_add` / etc., comparison uses
    `build_int_compare`, logical uses basic-block control flow
    with a PHI — three different LLVM idioms that benefit from
    being separate match arms.

48. (C1.3.2-4 / cd1c0d4) The parser's precedence ladder grew
    three new levels (or > and > cmp) between `parse_expr` and
    `parse_add`. Comparisons (`cmp_expr`) are non-associative
    per ADR 0012 D6 — a second cmp op after the first surfaces
    as the new `ParseError::ChainedComparison` rather than
    parsing as `(a < b) < c` (which would type-error anyway
    because `(a < b)` is Bool and Bool can't compare with int).
    Parser-level rejection gives a better diagnostic ("chained
    comparison is not allowed; parenthesise one side") than
    the type error would. Aligns with Rust; diverges from Python.

49. (C1.3.2-4 / cd1c0d4) Codegen drops its `i64_type` ctx field
    in favour of a type-driven `llvm_int_type(Type)` helper that
    picks between `i1`, `i32`, and `i64` based on the typed
    expression's annotation. The `vars` HashMap changes from
    `VarId → PointerValue` to `VarId → (PointerValue, Type)` so
    `build_load` can pick the right element type for each
    variable. Function signatures consult `signature.return_type`
    and `.param_types` for the LLVM fn type. The `'ctx` and `'a`
    lifetimes on `CodegenCtx` collapsed to a single `'ctx` since
    the borrowed Context and the LLVM derived values share the
    same effective scope; the two-lifetime version was a hold-
    over from C1.2 that no longer earned its keep.

50. (C1.3.2-4 / cd1c0d4) Short-circuit `&&` / `||` lower to
    PHI-based control flow: one conditional branch on the lhs,
    a separate basic block for the rhs evaluation, and a join
    block with a PHI that takes the lhs value (when the rhs is
    skipped) or the rhs value (when it ran). The pattern is
    standard for compiled languages — same shape as LLVM's
    canonical short-circuit lowering and what Rust / Swift /
    Clang produce. Unary `!` is the simpler case: `xor x, 1`
    flips an i1 bit cheaper than a `compare-EQ-with-false`
    would. The PHI versus alloca-and-branch question was
    decided in favour of PHI because the join block is short
    and SSA-natural; alloca-and-branch makes more sense for the
    `if-else` lowering which has multi-statement branches.

51. (C1.3.5, ADR 0010 D9 retirement / ba5fd9d) Retiring C-style
    truthy + rewriting the 6 if-using fixtures is a single
    commit because the type-checker rule and the fixture
    rewrites are intrinsically coupled — flipping one without
    the other fails the suite. The fixture rewrites are
    mechanical (`if x` → `if x != 0`, `if 1` → `if true`, etc.);
    the c05_go_no_go program gets the full ADR 0012 appendix
    shape with `is_positive(x)` returning bool and
    `pick(cond: bool, ...)` consuming it. Codegen sheds the
    legacy "compare-NE-zero on int condition" path since the
    type checker now guarantees `cond.ty == Bool` — a
    `debug_assert_eq` pins the invariant. ADR 0012 D9 is fully
    exercised; ADR 0010 D9 is now historically retired (the
    "deliberate temporary ugliness" loop opened by C0.4 is
    closed).

52. (C1.3.5 / ba5fd9d) Short-circuit verification fixtures
    `c13_short_circuit_and` and `c13_short_circuit_or` pin the
    PHI-based control flow against regression. They use a
    `print(99) > 0` rhs operand that, if evaluated, would emit
    "99\n" to stdout; the test asserts stdout is empty. If a
    future codegen change accidentally collapses `&&` / `||` to
    eager evaluation (e.g., `build_and(l, r)` / `build_or(l,
    r)`), the side effect surfaces and the test fails. This is
    a stronger guarantee than just verifying the result value
    is correct — many bugs that produce the right value still
    eagerly evaluate.

53. (C1.3 / cd1c0d4) i32 is in the type universe per ADR 0012
    D3 but practically thin without literal-typing
    infrastructure. Integer literals always type as I64; there
    is no `5i32` suffix, no `cast(x as i32)`, no bidirectional
    inference. So `let r: i32 = some_fn_returning_i32()` works,
    `fn add32(a: i32, b: i32) -> i32 { a + b }` works (operands
    flow from parameters), but `add32(2, 3)` fails (the literal
    `2` types as I64). Future literal-typing work (C1.5+) will
    revisit this; documented here so the C1.3 scope is clear.

54. (C1.4.1 / f34b401) The C1.4 lexer additions are just two
    tokens (`struct` keyword + `.` punctuation) per ADR 0013 D8.
    logos's longest-match is not relevant here — `.` has no
    `..` / `.=` / `...` neighbours until ranges arrive in C2+.
    The `structure` / `structured` ident-prefix-vs-keyword
    regression is verified by a dedicated test. 4 new lexer
    tests + the existing lex_all_keywords / lex_all_punctuation
    extended.

55. (C1.4.2-6 / aa8f252) C1.4's AST adds new variants
    `ExprKind::StructLit` and `ExprKind::FieldAccess` —
    separate from `ExprKind::Call` because their typing rules
    differ. StructLit takes a struct name + a set of named
    field initializers; FieldAccess is postfix `.field` on any
    expression. Both could in principle be expressed via
    existing variants (StructLit as a magical `Foo` call,
    FieldAccess as a method-call shape), but separate variants
    let the type checker enforce per-feature rules without
    runtime branching on a generic `Call` shape. Same precedent
    as C1.3's Cmp / Logic separation.

56. (C1.4.2-6 / aa8f252) The parser's `allow_struct_lit: bool`
    mode flag per ADR 0013 D3a is the first piece of context-
    sensitive parsing in Sentinel. It's set to false while
    parsing an `if` condition so `if Foo { ... }` parses as
    if-condition `Foo` (Var atom) + if-then block `{ ... }`
    rather than struct-literal `Foo { ... }` + unexpected `else`.
    Parens always restore it (the LParen arm in parse_atom saves
    and re-sets the flag inside the parenthesized subexpr).
    Bounded — just a single Boolean threaded through `parse_expr`.
    Rust has the same shape and reasoning. C2+'s `while` / `for`
    / `match` will inherit the same forbidden-position list.

57. (C1.4.2-6 / aa8f252) Field access is implemented via a
    `parse_postfix()` wrapper that runs `parse_atom()` and then
    loops on `.field` chains. This pattern generalises to other
    postfix operators — C1.6's arrays will add `[index]`
    indexing; C4's classes will add `.method()` (which is `.`
    plus call shape). The loop shape gives left-associativity
    naturally: `a.b.c` parses as `(a.b).c`. parse_unary now
    calls parse_postfix instead of parse_atom directly.

58. (C1.4.2-6 / aa8f252) The resolve crate's struct table is
    built BEFORE fn signatures so that fn signatures can
    reference struct types in their TypeExprs. Resolution
    order in `resolve()`: (0) struct decls, (1) fn signatures,
    (2) fn bodies. Without this ordering, a fn `fn f(p: Point)
    -> i64` that mentions `Point` before its declaration in
    source order would fail. Sentinel doesn't require source-
    order declaration anyway (Sentinel-Mini taught this), so
    the multi-pass approach is correct.

59. (C1.4.2-6 / aa8f252) The recursive-struct check at
    sentinel-types runs a depth-first walk on the
    struct→field-struct edges using a Color enum (White / Gray
    / Black). Cycles surface when the walker hits a Gray node
    — the current path stack is the cycle. Direct cycles
    (`struct Node { next: Node }`) and mutual cycles
    (`struct A { b: B } struct B { a: A }`) both detected.
    Lifts at C1.5 when `?T` introduces indirection that breaks
    cycles. The error includes the full cycle path for the
    diagnostic.

60. (C1.4.2-6 / aa8f252) The type checker reorders struct-
    literal fields to declaration order at check() time, so
    codegen iterates by index without needing to consult field
    names. Source order `{ y: 4, x: 3 }` becomes
    `{ x: 3, y: 4 }` in TypedExprKind::StructLit.fields,
    matching the struct declaration's field order. The
    field_index in FieldAccess is set similarly — it's the
    GEP offset that codegen consumes directly.

61. (C1.4.2-6 / aa8f252) Codegen's value type widens from
    `IntValue<'ctx>` to `BasicValueEnum<'ctx>` to accommodate
    struct values. Every lower_* function's return signature is
    updated; arithmetic / cmp / logic / unary ops call
    `.into_int_value()` on their operands (the type checker
    guarantees they're int-typed) and `.into()` on results.
    The `vars` map's `(PointerValue, Type)` shape is unchanged
    at the surface but now backs struct-typed bindings — alloca
    uses `llvm_basic_type` instead of `llvm_int_type`. The
    refactor is mechanical but pervasive (every match arm
    touched). One-shot value-type widening is much cleaner than
    threading two return types.

62. (C1.4.2-6 / aa8f252) Struct literal codegen builds via a
    chain of `build_insert_value` starting from
    `struct_type.get_undef()`. Field access codegen uses
    `build_extract_value(struct_val, field_index)`. Both are
    SSA-native (no alloca dance needed for the value-level
    operations). Pass-by-value through fn calls is transparent
    — LLVM's ABI lowering handles the small-struct-in-registers
    vs large-struct-via-pointer choice automatically. C1.4
    doesn't need to think about it.

63. (C1.4.2-6 / aa8f252) Struct equality `==` / `!=` is
    rejected at C1.4 per ADR 0013 D6 — the Cmp typing rule
    explicitly rejects struct operands. Allows `struct == struct`
    in a future ADR (C1.5+) without retroactively breaking
    programs that depend on the current rejection. Same for
    struct arithmetic (`struct + struct`) — caught by the
    existing arithmetic rule that requires int operands.

64. (C1.4.2-6 / aa8f252) The codegen lifetime situation
    settled at C1.3 as `'ctx` alone parameterising `CodegenCtx`
    holds through C1.4. The new `struct_types: HashMap<StructId,
    StructType<'ctx>>` field shares that lifetime. Pass 0 of
    `compile_to_object` (declare LLVM struct types) happens
    before the CodegenCtx is built, so the types are passed in
    by-value at construction. This avoids re-creating struct
    types per fn body.

65. (C1.5.1 / dff8642) The C1.5 lexer additions are `null`
    keyword + `?` token per ADR 0014 D8. The `?` token is
    reserved for type-position only at C1.5 — `?.` / `??` /
    `?` propagation / `x!` force-unwrap are deferred per D11
    to avoid token-role conflicts. 6 new lexer tests including
    the `nullify` / `null_value` / `nullable` ident-prefix
    regression.

66. (C1.5.2-6 / 1d0adae) **Implementation revises ADR 0014 D4**:
    the D4 spec proposed `Type::Nullable(Box<Type>)` but the
    implementation went with a flat `NullableInner` subset enum
    instead. The reason: `Box<Type>` would force `Type` to lose
    `Copy`, cascading `.clone()` additions across ~20-30 use
    sites in sentinel-types + sentinel-codegen. The subset enum
    keeps `Type` `Copy` and makes the no-nested-nullables rule
    (ADR 0014 D6) structural rather than convention-based —
    `?(?T)` is literally unrepresentable. Cost: a duplicate
    constructor list (Type and NullableInner mirror each other
    minus the Nullable case); every new Type at C1.6+ adds one
    line to NullableInner. Worth it.

67. (C1.5.2-6 / 1d0adae) **Bidirectional checking infrastructure**
    via `check_expr(expr, expected: Option<Type>, ...)`. The
    `expected` is threaded through and bottoms out at
    `coerce_to_expected(synth, expected)` which either passes
    through, widens T→?T via `WidenToNullable`, or surfaces a
    `Mismatch` if synth type doesn't fit expected. Pushed into:
    let RHS, fn body tail (only when return type is Nullable to
    preserve ReturnTypeMismatch granularity), call args (only
    when param type is Nullable to preserve CallArgMismatch
    granularity), struct-lit field values, if/else branches.
    The selective-pushdown rule prevents the more specific
    error variants from being shadowed by Mismatch in the
    non-nullable case.

68. (C1.5.2-6 / 1d0adae) `null` literal has no synthesis type.
    The check_expr handles ResolvedExprKind::NullLit before the
    main match: if expected is Some(Type::Nullable(_)), the
    NullLit types as expected; otherwise `TypeError::AmbiguousNull`.
    This is the only place in the type checker where the
    expected type is REQUIRED (everywhere else it's optional
    extra info).

69. (C1.5.2-6 / 1d0adae) The generic builtins `unwrap_or` and
    `is_some` are special-cased at the Call typing arm because
    Sentinel doesn't have real generics yet. The signature
    table holds placeholder param types (?I64 / I64 for
    unwrap_or; ?I64 for is_some); the type checker bypasses
    the standard `CallArgMismatch` path when `Call.id` is one
    of the two known IDs and applies the ADR 0014 D9
    type-from-arg inference rule instead. The machinery
    evaporates at C1.7 when real generics arrive.

70. (C1.5.2-6 / 1d0adae) Codegen's `?T` representation is the
    LLVM struct `{ i1 valid, T payload }`. The widening
    `WidenToNullable` lowers as `insert_value` into a fresh
    `get_undef()` struct (set the i1 to true, fill in payload).
    `null` literal lowers as a const struct with i1 false + T
    zero. `unwrap_or` lowers inline as `build_select(valid,
    payload, default)` — both arms are unconditionally
    evaluated because they're already on the value stack;
    short-circuit semantics aren't needed (the builtin is a
    pure projection). `is_some` is just `extract_value(0)`.
    `x == null` extracts the valid bits from both sides and
    compares them as i1.

71. (C1.5.2-6 / 1d0adae) **Forward-reference fix in codegen pass 0**:
    the original C1.4 pass-0 code computed struct field types
    INSIDE the loop that inserts the struct into the struct_types
    map. For `struct Node { next: ?Node }`, computing the
    `next` field type requires looking up `Node` in
    struct_types — which fails because Node hasn't been
    inserted yet. The fix: two sub-passes. First, declare all
    opaque struct types via `context.opaque_struct_type(name)`
    + insert into struct_types. Then, in a second loop,
    compute field types (which can now see all struct names)
    and call `set_body`. The forward-reference path works for
    LLVM because struct types can be referenced opaquely before
    their body is set.

72. (C1.5.2-6 / 1d0adae) **ADR 0014 D10 deferral**: the
    cycle-check relaxation was documented in the ADR (cycles
    via nullable edges should be accepted) but the
    implementation choice of `?T = { i1, T }` inline makes
    recursive structs infinite-sized in LLVM. So C1.5 keeps
    the C1.4 cycle check unrelaxed — recursive structs via
    `?T` still error. Proper indirection (heap allocation,
    smart pointers) is C1.6+. The detect_struct_cycle docstring
    explains the deferral. ADR 0014 D10's text says "relaxes"
    but the runtime behavior says "deferred" — this is a known
    gap, documented in the docs commit's notes.

73. (C1.5.2-6 / 1d0adae) FnId remapping: with `unwrap_or` and
    `is_some` pre-registered alongside `print`, user fns now
    start at `FnId(3)` (was `FnId(1)` at C1.4). Updated 3
    existing resolve tests that hardcoded FnId(1)/(2) for
    user fns. Same shift in sentinel-types tests that asserted
    `fn_signatures[1].name == "main"` — now FnId(3) is main.
    Future C1.7+ generics will retire the builtins, shifting
    FnIds back; tests should not hardcode IDs.

74. (C1.6.1 / 3cfd49f) The C1.6 lexer additions are just two
    punctuation tokens: `[` (LBracket) and `]` (RBracket) per
    ADR 0015 D8. They serve three roles disambiguated by the
    parser at C1.6.2-6: array type `[T]`, array literal
    `[e1, e2, ...]`, and postfix indexing `a[i]`. No new
    keywords — `len` is a registered builtin per D4, not a
    reserved word. 6 new lexer tests.

75. (C1.6.2-6 / 8c5bbbe) **ADR 0015 D6 amendment: depth-1 type
    nesting.** The ADR proposed extending NullableInner with
    `Array(ArrayElem)` and ArrayElem with `Nullable(NullableInner)`
    to represent `?[T]` / `[?T]` combinations. Rust's mutual
    enum recursion forces Box indirection somewhere, which breaks
    `Type`'s Copy and cascades through the codebase. The
    implementation cap: NullableInner and ArrayElem stay as
    primitive-only subset enums (I64/I32/Bool/Struct). `?T` and
    `[T]` each contain primitives + structs only, never each
    other. `?[T]` and `[?T]` become "not yet representable" at
    C1.6; a future ADR adds them when generics or a more
    sophisticated representation is in place.

76. (C1.6.2-6 / 8c5bbbe) **Codegen value-type representation
    split for `?T`** per ADR 0015 D11. `?primitive` (I64, I32,
    Bool) stays as the flat `{ i1 valid, T payload }` from C1.5.
    `?Struct` switches to heap-indirect `{ i1 valid, ptr payload }`
    where payload points to a `sentinel_alloc`'d struct value.
    The asymmetry is the key insight: primitives are tiny and
    inline pays for itself; structs can be arbitrarily large and
    might be recursive, so the pointer indirection both bounds
    the parent's size AND breaks recursive struct cycles. The
    detect_struct_cycle pass relaxes to walk only direct
    `Type::Struct` edges, accepting cycles through `?Struct`.

77. (C1.6.2-6 / 8c5bbbe) **Runtime additions land in C1.6.**
    sentinel-runtime gains two new C-ABI symbols:
    `sentinel_alloc(i64 size) -> *mut u8` (libc malloc wrapper
    + abort on failure) and `sentinel_panic_oob(i64 idx, i64
    len) -> never` (print + abort on out-of-bounds index).
    Codegen pass 1 declares both as external `extern "C"` fns
    in the LLVM module; the actual implementations come from
    sentinel-runtime when the program is linked. NO `free`
    exposed — arrays + nullable struct payloads leak per ADR
    0015 D9. Documented limitation; C2 introduces region-based
    resource management.

78. (C1.6.2-6 / 8c5bbbe) **The fns HashMap "dummy entries" for
    inlined builtins.** Pre-C1.6 codegen aliased ALL is_runtime
    signatures to the same `sentinel_print` LLVM symbol (a
    latent name clash that worked only because LLVM's
    add_function with a duplicate name returns the existing fn).
    C1.6 fixes this: only `print` (FnId 0) gets a real LLVM
    declaration as `sentinel_print`; the inlined builtins
    (`unwrap_or` FnId(1), `is_some` FnId(2), `len` FnId(3))
    have fns HashMap entries pointing at print_fn as a sentinel
    value (never read because codegen special-cases their
    FnIds before calling `lower_call`). The runtime symbols
    `sentinel_alloc` and `sentinel_panic_oob` get their own
    declarations — they're called by codegen helpers, not by
    user code via the Call mechanism.

79. (C1.6.2-6 / 8c5bbbe) Array codegen lowers `[T]` to LLVM
    `{ i64 len, ptr data }`. Array literal allocates
    `n * sizeof(T)` bytes via `sentinel_alloc`, stores each
    element via GEP+store, then build_insert_values the
    `{ len, data }` struct. Indexing extracts both fields,
    bounds-checks (`0 <= idx < len`), branches: ok-branch does
    GEP+load, oob-branch calls `sentinel_panic_oob` then
    `build_unreachable`. The element type for GEP comes from
    `TypedExprKind::Index.elem_ty` (cached by the type checker)
    — LLVM uses opaque pointers since LLVM 15, so we track the
    payload type at the AST level.

80. (C1.6.2-6 / 8c5bbbe) The cycle-detector relaxation that
    closes ADR 0014 D10 is a one-line change: walk only
    `Type::Struct(_)` edges, not `Type::Nullable(NullableInner::
    Struct(_))`. The codegen change is more involved (the `?T`
    representation split per decision 76) but the type-check
    relaxation is trivial — the cycle is broken at the
    representation level, so the type checker can simply
    accept it.

81. (C1.6.2-6 / 8c5bbbe) Test FnId shift: user fns now start
    at FnId(4) because we have four builtins (print at 0,
    unwrap_or at 1, is_some at 2, len at 3). Updated 2 tests
    in sentinel-resolve and 1 in sentinel-types. Same caution
    as decision 73 — tests should not hardcode FnIds.

82. (C1.7 scaffolding / c1e5083) **No new lexer tokens at C1.7.**
    ADR 0016 D5: `<` and `>` from C1.3 comparisons get reused as
    type-param / type-arg delimiters. The parser disambiguates by
    position — `parse_type` looks for `<...>` after an Ident,
    expression-position grammar is unchanged. The cost is no
    turbofish (`f::<i64>(x)`) at call sites per ADR 0016 D4 —
    type-args are inferred bidirectionally. C1.7's parser change
    is bounded: parse_type_params, parse_type_args helpers + a
    Generic branch in parse_type + optional `<T1, T2>` clause on
    parse_fn_def / parse_struct_decl.

83. (C1.7.4a / d32a9fe) **TypeParam variants on the helper enums.**
    `NullableInner::TypeParam(TypeParamId)` and
    `ArrayElem::TypeParam(TypeParamId)` were added so `?T` and
    `[T]` are representable inside generic-fn bodies. This is the
    same flat-subset pattern from C1.5 / C1.6 (preserves `Type:
    Copy`); the TypeParamId is a `Copy` `u32` newtype re-exported
    from sentinel-resolve.

84. (C1.7.4a / d32a9fe) **Builtin typing routes through generics
    uniformly.** The three C1.5/C1.6 special-cased builtins
    (unwrap_or, is_some, len) had ad-hoc Call branches in
    check_expr that pre-computed T from arg[0] then pushed it
    down to arg[1]. C1.7.4a deletes ~75 LOC of those branches and
    re-expresses the builtins' signatures with real
    `Type::TypeParam` (e.g., `unwrap_or<T>(x: ?T, default: T) -> T`).
    The new unified `check_call` handles them identically to
    user-defined generic fns via iterative bidirectional
    inference. Code-path simplification + the right framing for
    user generic fns.

85. (C1.7.4a / d32a9fe) **Iterative call-site inference.** ADR
    0016 D4: bidirectional generic-call inference is implemented
    as a fixed-point loop. Each round, for each not-yet-typed
    arg, compute its effective expected type by substituting the
    param under the current `subst` (`try_substitute`). If the
    param has unbound TypeParams AND the arg is a null literal,
    skip that round — it'll be retried after another arg has
    bound the relevant T. After all args are typed, any unbound
    TypeParam surfaces `AmbiguousTypeArg`; any untyped null arg
    surfaces `AmbiguousNull`. The substituted return type is the
    call's `ty`. Handles e.g., `unwrap_or(null, 0)` correctly:
    arg[1]=0 → I64 → bind T=I64; arg[0]=null re-checked with
    expected `?I64` → typed.

86. (C1.7.5 / ad7e10d) **Eager monomorphisation via TypedFnDef::
    substitute.** Codegen materialises each `(FnId, type_args)`
    instantiation as a deep-cloned `TypedFnDef` with every
    `Type::TypeParam` substituted to its concrete type-arg. The
    substituted def has empty `type_params` and looks no
    different from a non-generic fn to `compile_fn`'s machinery
    — so codegen's per-fn lowering path stays unchanged. The
    eager-substitute approach (vs lazy substitution at every
    Type-access site) was chosen for safety: missing a single
    site in lazy substitution would silently emit wrong code,
    whereas eager substitution surfaces any TypeParam that
    leaks through llvm_basic_type's panic arm.

87. (C1.7.5 / ad7e10d) **Worklist for transitive monomorphic
    instantiations.** A user generic fn calling another user
    generic fn (`fn first_or<T>(?T, T) -> T { unwrap_or(...) }`
    works because unwrap_or is inline, but `fn foo<T>(x: T) -> T
    { bar<T>(x) }` requires `bar<concrete>` to be emitted when
    foo is monomorphised) needs the codegen pre-pass to
    transitively discover instantiations. The implementation is a
    worklist: seed from non-generic fn bodies, pop each pending
    instance, walk its body substituting type_args under the
    current subst, queue any new instances. Substitution may
    extend `instances` table (when nested generics produce new
    `Pair<i64, bool>`-style entries).

88. (C1.7.5 / ad7e10d) **Generic-fn name mangling.** Each
    monomorphic LLVM fn gets a deterministic mangled name:
    `id__i64`, `pick__bool__i64`, etc. Per ADR 0016 D7. Format
    is `{name}__{type1}__{type2}...` where each type is rendered
    by `mangle_type` (e.g., `Box_i64` for nested generics).
    Stable across runs given the same input program — useful for
    LLVM IR inspection. Internal-only; no surface implication.

89. (C1.7.4b / 2c6c652) **Interned generic-struct instances
    preserve `Type: Copy`.** ADR 0016 D6a. The naive approach
    (`Type::GenericInstance { struct_id, args: Box<[Type]> }`)
    would break Copy and force ~30 site .clone() refactors. The
    interned-id approach wraps each unique `(struct_id, args:
    Vec<Type>)` in a `Copy` `u32` newtype (`GenericInstanceId`).
    The underlying `Vec<Type>` lives in a program-level table
    (`TypedProgram.generic_instances`) keyed by id. Mirrors the
    StructId / FnId pattern. Linear search through the table at
    intern time — fine at C1.7 program scale; HashMap interning
    is a profile-driven future optimisation.

90. (C1.7.4b / 2c6c652) **Codegen extends the instance table
    during substitution.** The type checker populates
    `program.generic_instances` with every instance it sees
    (`Pair<i64, i64>` annotations, return types after call-site
    inference, etc.). Codegen owns a *mutable copy* of this
    table and may extend it during monomorphisation when nested
    generic substitution produces new `(struct_id, args)` pairs
    not seen by the type checker (e.g., `fn f<T>() -> Pair<T, T>
    { ... }` instantiated as `f<i64>` produces `Pair<i64, i64>`
    — same instance the type checker may or may not have seen
    directly). The abstract-vs-concrete filter in pass 0
    (`arg_contains_typeparam`) skips abstract instances (those
    with TypeParam args) from LLVM struct-type emission since
    they never materialize at runtime.

91. (C1.7.4b / 2c6c652) **Generic-struct unification recurses
    into args.** `unify_one(Type::GenericInstance(p_id),
    Type::GenericInstance(a_id), ...)` doesn't shortcut on
    `p_id == a_id` equality alone (two different abstract
    instances with the same shape would have different ids).
    Instead it looks up both data entries, verifies same
    `struct_id`, then unifies each arg pairwise. This is what
    makes `fst<A, B>(p: Pair<A, B>)` called with `p: Pair<i64,
    i64>` correctly infer A=i64, B=i64.

92. (C1.7.4b / 2c6c652) **Bidirectional pushdown extended for
    generic-instance returns.** The C1.5 ADR 0014 D5 rule pushed
    the return type down only for nullable returns. C1.7.4b
    extends to generic-instance returns too — without this, the
    body of `fn make_pair<A, B>(...) -> Pair<A, B> { Pair {
    first: a, second: b } }` would hit `AmbiguousGenericStructLit`
    because the literal can't synthesize its own type args.

93. (C1.7 retrospective) **ADR 0011 D6 estimated "4-6 weeks" for
    C1.7; actual was ~1 session across 5 commits (e411ded +
    c1e5083 + d32a9fe + ad7e10d + 2c6c652).** Faster than
    estimated, in line with the C1.4 / C1.5 / C1.6 pattern
    (estimated "2-4 weeks" each, actual ~1 session each). The
    speedup came from (a) the interned-instance design choice
    (avoided the Type-loses-Copy refactor cascade), (b) the
    eager-substitute approach for codegen (avoided the
    lazy-substitution audit risk), (c) the well-established
    pattern of "scaffolding commit → typing commit → codegen
    commit → fixtures commit" inherited from C1.4/5/6.

37. (C1.0c, ADR 0011 D1 amendment) Codegen stays outside the salsa
    query graph through Phase C1.0. ADR 0011's original D1 sketch
    had "… through codegen" in the query list, suggesting
    `compile_to_object` would eventually become `#[salsa::tracked]`.
    C1.0c reconsiders and rejects that for now. Three options
    were weighed (see ADR 0011 D1 amendment for the full
    write-up); the chosen option (2: don't wrap codegen at all)
    is justified by three factors: (a) codegen gets rewritten at
    C1.2 against typed HIR anyway, so investing in a pre-types
    salsa wrapper amortises over weeks at most; (b) the LLVM
    `'ctx` lifetime woven through `Context`, `Module<'ctx>`,
    `Builder<'ctx>`, and `FunctionValue<'ctx>` doesn't trivially
    fit salsa's `'static`-ish query model — fitting it requires
    either bitcode-roundtripping or single-fn codegen, and the
    cost-benefit isn't favorable yet; (c) the C1.0b front-end
    retrofit is what LSP / `cargo check`-style tooling actually
    cares about, since those tools exit after types-but-not-
    codegen — codegen incremental rebuild has near-zero practical
    value at C0/C1 scale. The cost is a small piece of explicit
    architectural debt (driver does a direct function call from
    parse_query's Program output into non-salsa codegen). It is
    revisited automatically at C1.2 because the codegen rewrite
    for typed HIR will touch the call site. ADR 0009 D1a's pure-
    function discipline preserved through C0+C1.0 keeps the
    retrofit mechanical whenever we do choose to do it. No code
    change at C1.0c; this is a pure docs commit capturing the
    architectural decision.

---

## Conventions

### Build & Test Commands

The standard check suite for a clean tree, applied per-crate:

    cargo build -p <crate>
    cargo clippy -p <crate> --all-targets -- -D warnings
    cargo nextest run -p <crate>           # or `cargo test -p <crate>`
    cargo test -p <crate> --doc

All four must pass for any commit on `main`. Current expected counts:

  - sentinel-broker:        69 tests + 1 doctest
  - sentinel-effects-proto: 226 tests (203 lib + 23 integration) + 0 doctests
  - sentinel-syntax:        195 tests (193 lib + 2 UI integration) + 0 doctests
                            (lib at C1.6: 186 lexer/parser + 7 query;
                             C1.5 added 6 lexer + 9 parser tests;
                             C1.6 added 6 lexer + 20 parser tests for
                             `[T]` array type / `[...]` literal / `a[i]`
                             indexing / nullable+array / nested array
                             parsing / error cases)
  - sentinel-ast:           42 tests (1 smoke + Display impls + op symbols);
                            C1.5 added 3; C1.6 added 4 (ArrayLit + Index +
                            array type Display + empty array)
  - sentinel-codegen:       41 tests (1 smoke + 1 target init + 39 positive
                            compile) + 0 doctests; C1.5 added 8; C1.6
                            added 9 (array literal, index, len, empty
                            array, array as fn arg, linked list,
                            nullable-struct widening, array of struct,
                            C1.6 phase-go)
  - sentinel-resolve:       36 tests (32 from C1.1-5 + 4 new C1.6:
                            array literal pass-through, array index
                            pass-through, len builtin pre-registration,
                            redefining len rejected) + 0 doctests
  - sentinel-types:         86 tests (71 from C1.2-5 + 14 new C1.6:
                            array type resolution + array literal typing
                            + array index typing + len builtin +
                            empty-needs-annotation + IndexOnNonArray +
                            IndexNotInt + NestedArray + mixed-element
                            rejection + len-on-non-array + array in
                            struct + linked-list-unlock + direct-cycle
                            still-rejected + C1.6 phase-go;
                            recursive_nullable_struct_now_accepted
                            replaces the C1.5 deferral test)
                            + 0 doctests
  - sentinel-driver:        47 pass integration tests + 0 doctests
                            (22 from C0 + 7 c13_* + 5 c14_* + 6 c15_* +
                            7 new c16_*: array_basic, empty_array,
                            array_as_arg, array_of_struct,
                            array_in_struct, linked_list_node,
                            c16_go_no_go)
  - sentinel-runtime:       2 tests (smoke + sentinel_print_returns_zero) + 0 doctests
                            — note: sentinel_alloc + sentinel_panic_oob
                            (C1.6 / ADR 0015 D9) are exercised via the
                            c16_* pass-test integration tests, not direct
                            Rust unit tests (they abort on failure paths)
  - sentinel-base:          3 tests (salsa query runs/caches + source file accessors) + 0 doctests
  - other compiler crates:  1 scaffold smoke test each, 0 doctests
                            (sentinel-hir, -mir, -lsp)

Total active workspace tests: **744**.

### Script Convention

Every code change lands via a script under `scripts/`:

  - `NN-<phase>.sh`          primary generator/patch for a milestone
  - `NNa-`, `NNb-`, ...      follow-up patches (lint fixes, etc.)
  - `NNz-commit-<phase>.sh`  pre-commit checks + commit creation

Scripts are committed alongside source changes so contributors can
see exactly what was patched, in what order. They are also a useful
debugging aid: re-run the most recent `NNa-` under `set -x` to
inspect a patch.

Output convention: each script prints `======` delimited sections
(BUILD / CLIPPY / TESTS / DOC TESTS). When asking for help, paste
those sections back.

### Working Norms (from HANDOVER §0.1)

- Trust STATE.md (this file), not the git log.
- Terminal heredocs: single-level only. Use `cat > /tmp/script.py`
  then `python3 /tmp/script.py` instead of nested heredocs.
- Avoid `set -e` and bare `exit` in pasted scripts — they can close
  the user's terminal. Use `return 1 2>/dev/null || exit 1` and
  rely on `PIPESTATUS[0]` for return codes.
- Small patches, build between each. Land type/trait changes first,
  build, then implementations, build, then tests.
- Honest disclosure beats confident-but-wrong.
- Examples held to `-D warnings`.
- Check before overwriting docs (`p.exists()` and merge patterns).

---

*End of document. Update on every commit that changes phase
status, public API surface, or invariants.*
