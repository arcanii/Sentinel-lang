# ADR 0069: `SealedChannel<secret T>` — the AEAD-encrypted secret-cross-process path

Status: **PROPOSED — design PINNED; M2.4a + M2.4b IMPLEMENTED incl. the REAL pipe
transport (snc-side, 2026-06-30)** — the M2.4 sub-phase of the ADR 0066 roadmap,
broken out into its own ADR per **ADR 0066 D8a** ("Because this is security-critical
and crypto-bearing, M2.4 gets its own ADR"). The authenticated cross-process
`SealedChannel<secret i64>` now runs over a **real parent↔child process pipe**
end-to-end (the self-stdin/stdout builtins closed the last blocker); what remains is
M2.4c (generic `secret T` + variable-length) + the scg self-host mirror. **The security-critical decisions D3/D4/D5/D9 are PINNED (maintainer
sign-off 2026-06-30):** D3 = reuse the ssh host-key authentication model (a); D4 =
counter-nonces + per-direction HKDF keys; D5 = fixed-width frames at the
i64-minimum (padding with the later variable-length phase); D9 = add only
`SealedChannel<secret T>` in v1 (leave M2.3's raw path as bare-`Process`
builtins).

**M2.4a (D7 minimum) is now implemented snc-side** (the scg mirror is deferred,
like M2.3b): a unit `Type::SealedChannel` interner variant (the fence-as-type, D1/D9)
+ two identity-ptr **bridge builtins** `sealed_channel(Process) -> SealedChannel` /
`sealed_process(SealedChannel) -> Process` (FnId 31/32, user-fn base shift 31→33
mirrored into `selfhost/` for differential parity — **no new runtime symbol**,
abi-v1 untouched at 34) + a stdlib `std::security::sealed` module whose `seal`/`open`/
`sealed_send`/`sealed_recv` reuse the **verified-CT ssh record cipher**
(`ssh_seal`/`ssh_open_verify`/`ssh_open_payload`) — no new primitive (D2). `open`
returns `OpenResult { ok: i64, v: secret i64 }` (`?(secret i64)` is unrepresentable —
`NullableInner` has no `Secret` variant — so the public verdict is a separate field).
The fixed-width frame is 40 bytes = 5 public i64 pipe frames over the M2.3
`process_send`/`process_recv` (D5). Demonstrator `examples/lang/sealed_channel.sentinel`
(an in-process `seal`→`open` round-trip — runtime-verified, the secret re-emerges
secret and authenticated → exit 42 — plus a guarded pipe path); ui rejection
`tests/ui/c66_sealed_channel_public_element` (a non-secret element is a type error,
D1). Four-check green; both bootstrap fixed points byte-identical.

**M2.4b-crypto (D3/D4) is now implemented + verified IN-PROCESS** (snc-side; the real
cross-process pipe transport is deferred — see below). New stdlib
`std::security::sealed_kex`: the authenticated x25519 KEX (D3 — the ssh host-key model:
the initiator verifies the responder's ed25519 host signature over the exchange hash
**and** pins the host key, so an unauthenticated/MITM'd KEX yields `authed=0`) as
**transport-free core functions** (`sealed_kex_client_msg` / `sealed_kex_server` /
`sealed_kex_client_finish` take/return the wire bytes), reusing the verified-CT
`x25519`/`ed25519`/`ssh_exchange_hash`/`ssh_kdf` — **no new primitive (D2), no
compiler change.** The exchange yields the two directional keys `keyc`/`keyd`
(`ssh_kdf` letters 'C'/'D', D4); a sealed stream then uses **monotonic counter nonces**
per direction (the `seqnr` of `seal`/`open`). Demonstrator
`examples/lang/sealed_session.sentinel` runs both KEX halves in-process (the wire bytes
passed between them) → both derive matching `keyc`/`keyd`, the initiator authenticates
the host, and a 3-message counter-nonce sealed stream (seqnr 0/1/2, no reuse) re-emerges
secret → exit 42. This removes M2.4a's **fixed pre-shared key** (now an authenticated
x25519 exchange) and **single-message** (now a counter-nonce stream) caveats at the
crypto level.

**The REAL pipe transport is now implemented (2026-06-30).** The blocker — a child
could not read its own stdin — is closed by two new **self-stdin/stdout framed
builtins**: `stdin_recv() -> ?i64` + `stdout_send(v: i64) -> i64` (the child-side
twins of `process_recv`/`process_send`, runtime symbols `sentinel_stdin_recv` /
`sentinel_stdout_send`; abi-v1 34→36; user-fn FnId base 33→35, mirrored into
`selfhost/` — all 6 differentials byte-identical). A new stdlib
`std::security::sealed_pipe` drives the KEX over the actual pipe: the **parent**
(initiator/client) frames bytes with `process_send`/`process_recv` over the child's
`Process`; the spawned **child** (responder/server) frames on its own stdin/stdout via
the new builtins; both run the same transport-free `sealed_kex` core. The end-to-end
test `crates/sentinel-driver/tests/sealed_pipe.rs` spawns a child, runs the
authenticated handshake over a **real pipe** (parent pins the child's host key), seals
a `secret i64`, sends it as a record, the child `open`s it (re-emerges secret on the
verified child) and exits 42, the parent `process_wait`s and exits 42. **So the
authenticated cross-process `SealedChannel<secret i64>` is real end-to-end.** Then
M2.4c (generic `secret T` + variable-length + padding) + the scg mirror.

Date: 2026-06-30

## Related

- **0066** (threading + multi-processing roadmap): **D8** (the cross-process
  secret fence — a `secret` may not cross a process boundary over a public ABI)
  and **D8a** (the encrypted escape — `SealedChannel<secret T>` as a *third*
  sanctioned secret-cross-process path beside `declassify`). D8a fixes the
  *rule*; this ADR fixes the *mechanism*. The M2.3 / M2.3b raw typed channel
  (`process_send`/`process_recv` over a `Process`, public `T`) is the
  **un-sealed** sibling this ADR splits against.
- **0008** (secret / constant-time): the guarantee. `declassify` is the only
  sanctioned qualifier-drop; D8a makes a `SealedChannel` `seal`/`open` the
  *second* sanctioned way a `secret` may leave the verified address space — but
  it trades the machine-verified guarantee for a cryptographic one (see the
  caveats).
- **0057 / 0059** (FFI import / C-ABI export): the secret fence precedent this
  generalizes. The fence stays for foreign-C IPC; `SealedChannel` is
  **Sentinel↔Sentinel only**.
- **The verified-constant-time crypto stdlib this is built from:**
  `std::security::aead` (`chacha20poly1305_encrypt(key, nonce, aad,
  plaintext: [secret u8]) -> AeadResult` + its `_decrypt`), `std::security::x25519`
  (`x25519(scalar, u)`), and **`std::net::ssh*`** — which ALREADY runs an x25519
  KEX → per-record `chacha20-poly1305@openssh.com` encryption with per-direction
  sequence-number nonces **over a real socket** (`ssh_client_handshake` /
  `ssh_server_handshake(conn, …)`, `ssh_send_record` / `ssh_recv_record`). **A
  `SealedChannel` is that exact machine pointed at a pipe instead of a TCP
  socket** — the design's biggest de-risking fact.
- **M2.1/M2.2/M2.3** (`Process` handle, byte-pipe IPC, the typed framed channel):
  the substrate. `SealedChannel` frames its ciphertext over the same
  `sentinel_process_send`/`_recv` (or `_write`/`_read`) pipe.

## Context

The bare cross-process secret fence (ADR 0066 D8) is sound but **over-fences**:
it blocks privilege separation — the *core* reason a security language wants
processes (a key-holding daemon, a privsep signer à la OpenSSH, software-HSM
emulation). D8a's resolution: **encryption is a cryptographic `declassify`** —

    seal : secret T   × secret Key → public Ciphertext     (AEAD encrypt)
    open : public Ciphertext × secret Key → secret T        (AEAD decrypt)

Emitting the ciphertext as *public* is a recognized sound declassification
("cryptographically-masked flows", Askarov–Hedin–Sabelfeld 2006): under AEAD
(IND-CCA) security the ciphertext reveals nothing about the plaintext to a holder
of neither the key nor the plaintext. The receiver `open`s and the value
**re-emerges `secret T`**, so the type-level taint is preserved end-to-end with
**no plaintext `declassify`**, and the verified receiver's `secret_leak` keeps
constant-time intact on the far side.

This ADR turns that rule into a typed mechanism. The constraints are the same
three Sentinel invariants that shaped ADR 0066 (the lexical borrow checker,
constant-time `secret`, cross-platform `std`), plus a fourth specific to crypto:
**the silent-failure surface** — when the *type* says "secret preserved" but a
key-management or nonce mistake means it is not. Most of the decisions below
exist to make that surface visible and hard to get wrong.

## Decisions

### D1. The type split makes the fence a static property of the type.

Two distinct types over a `Process` pipe, so the compiler — not a convention —
decides what may cross:

- **`ProcessChannel<T>`** (raw): the M2.3/M2.3b path, element a **public**
  word-scalar `T`. `send`/`recv` move plaintext bytes. A `secret` element is a
  type error (the D8 fence). *(M2.3 implemented this as builtins over the bare
  `Process` handle; D1 proposes promoting it to a named `Type::ProcessChannel`
  interner variant so it is the sibling of `SealedChannel` — see D9 for whether
  to do that promotion now or keep the bare-`Process` builtins.)*
- **`SealedChannel<secret T>`** (encrypted): carries a **`secret`** element.
  `seal`/`open` AEAD-encrypt/decrypt with a session key; only the **public**
  ciphertext touches the pipe. A non-`secret` element is a type error (sealing a
  public value is pointless — use `ProcessChannel`).

The fence rule (D8a) becomes: *a `secret` may cross a process boundary only by
(a) `declassify` first, or (b) a `SealedChannel`.* A raw
`ProcessChannel<secret T>` write stays a compile error. Because the two are
distinct types, "did this secret get encrypted before it left?" is answered at
type-check, not by reading the code.

### D2. `seal`/`open` are the verified-constant-time stdlib AEAD — not new crypto.

`seal` is `std::security::aead::chacha20poly1305_encrypt` and `open` is its
`_decrypt`, with the `SealedChannel` runtime/library wiring the key + nonce +
framing. **No new cryptographic primitive is written** — the whole point of D8a
is that the verified-CT machine already exists (`aead` over `chacha20`/`poly1305`,
exercised end-to-end by the ssh stack). The plaintext is `secret T` serialized to
`[secret u8]` (the M2.3b encode, but staying *secret*); the ciphertext + tag are
**public** `[u8]` framed over the M2.2/M2.3 pipe; `open` authenticates the tag
(reject on failure — a typed error, never a panic) and deserializes back to
`secret T`.

### D3. Key exchange — an authenticated x25519 handshake over the pipe. **PINNED: (a) reuse the ssh host-key model.**

The session key must be established **without ever putting key material in
`env`/`argv`** (D8a caveat 2). For Sentinel's **spawn-exec** process model (M2.1
`process_spawn`; Sentinel does not `fork`), the parent and child run a
**handshake over the pipe itself** before any sealed message — exactly what
`std::net::ssh_{client,server}_handshake` already does over a socket, reused
verbatim with the pipe (`Process` stdin/stdout) as the transport. That gives an
x25519 ECDH → an authenticated session key + a session id, with the wire-public
values `declassify`d to send and widened back to `[secret u8]` on receipt (K
never crossing the wire — the established ssh pattern).

**The authentication question is the sign-off point.** Unauthenticated x25519 is
MITM-able. The ssh stack authenticates with a host key (`ssh-ed25519`) +
publickey userauth. Options for `SealedChannel`:
- **(a) Reuse the ssh host-key model** — the child presents an ed25519 host key
  the parent pins (the parent spawned the child, so it can ship/pin the expected
  key). *Recommended* — maximal reuse, and the parent-pins-child trust matches
  the spawn relationship.
- **(b) A pre-shared key** seeded at spawn via a *file descriptor / pipe* (never
  argv/env), then HKDF to per-direction keys. Simpler, no asymmetric handshake,
  but needs a secure seeding channel.
- **(c) Out-of-scope for v1**: ship the unauthenticated KEX only behind a
  `// SAFETY: trusted local child` and defer authentication — **rejected** as a
  default (an unauthenticated "sealed" channel is the silent-failure trap D8a
  warns about).

**PINNED (maintainer 2026-06-30): (a) — reuse the ssh host-key model.** v1
authenticates (no unauthenticated default). The child presents an `ssh-ed25519`
host key the parent pins; the x25519 KEX runs over the pipe via the existing
`ssh_{client,server}_handshake` (parent = client, child = server, or vice versa),
yielding the authenticated session key + session id.

### D4. Nonce discipline — fresh nonce per message, per direction. **PINNED.**

Nonce reuse under ChaCha20-Poly1305 is catastrophic (it breaks confidentiality
*and* authentication). The discipline, mirroring the ssh record cipher:
**per-direction monotonic counter nonces** (a `secret i32[3]` derived from a
64-bit message sequence number that starts at 0 and increments per sealed
message, never reused, never wrapping in practice). Two independent counters
(parent→child, child→parent) keyed by two HKDF-derived directional keys (`C`/`D`,
as the ssh data phase does). **No random nonces** (no RNG dependency on the hot
path; counters are simpler to audit for non-reuse). **PINNED (maintainer 2026-06-30):
counter-nonces + per-direction HKDF keys.**

### D5. Length / padding — fixed-width frames at the minimum. **PINNED.**

Ciphertext length leaks plaintext length (D8a caveat 1), which for true CT wants
padded / fixed-length messages. **At the i64-minimum this is free:** the plaintext
is always an 8-byte `secret i64`, so every sealed frame is the same length
(8-byte ciphertext + 16-byte tag) — **no length leak**. For the later
variable-length payloads (`secret [u8]`), a padding policy is required (pad to a
block multiple, or a fixed max) — **deferred with the variable-length phase**.
**PINNED (maintainer 2026-06-30): fixed-width i64-minimum for v1; variable-length
+ padding is a later sub-phase (M2.4c).**

### D6. Scope — Sentinel↔Sentinel only; foreign-C IPC stays `declassify`-fenced.

`SealedChannel` requires **both ends verified Sentinel** (D8a caveat 3): a foreign
C peer would `open` and could then branch on the plaintext, so encryption buys
nothing there — the `declassify` fence stays the only path for Sentinel↔foreign
IPC. The type enforces this: `SealedChannel` only connects two Sentinel programs
that ran the D3 handshake.

### D7. The minimum — `SealedChannel<secret i64>`, mirroring M2.3's i64-minimum.

v1 ships the smallest useful slice: seal a `secret i64`, frame the public
ciphertext over the pipe, `open` on the other end → the value re-emerges
`secret i64`, and the receiver's `secret_leak` keeps CT. This proves the whole
spine (type split + handshake + AEAD + the fence-as-type) end-to-end with a
fixed-width frame (D5) and the existing word-scalar serialization (M2.3b, kept
secret). Generic `secret T` elements + variable-length `secret [u8]` (with D5
padding) are later sub-phases.

### D8. Threat-model honesty (carried verbatim from D8a — must stay in the design).

- It is a **different *kind* of guarantee**: confidentiality-in-transit under
  cryptographic assumptions + correct key management, **not** machine-verified
  constant-time. The CT property is preserved by the verified receiver
  *independently*; `SealedChannel` must **never** be documented as "extending
  machine-verified CT across processes."
- **Key management is the whole ballgame** and the silent-failure surface (D3/D4).
- On one machine in one trust domain, a local attacker who can read the pipe can
  often `ptrace` the plaintext too → same-trust-local sealing is mostly
  **defense-in-depth** (swap, core dumps). The **decisive** win is across a
  **trust boundary** (different UID, a sandbox, the network) — which is exactly
  the privilege-separation case the bare fence was blocking, so this *resolves*
  the D8 tension rather than merely softening it.

### D9. Open: promote `ProcessChannel<T>` to a named type now, or later?

D1 wants `ProcessChannel<T>` and `SealedChannel<secret T>` as sibling types.
M2.3/M2.3b implemented the raw path as builtins over the bare `Process` handle
(no `ProcessChannel` type). Two routes:
- **Promote now**: add `Type::ProcessChannel(elem)` + `Type::SealedChannel(elem)`
  interner variants (the `Channel<T>` pattern), and re-express M2.3's
  `process_send`/`recv` over `ProcessChannel<T>`. Cleaner type story; more churn
  (an FnId/typing migration of the existing builtins).
- **Add only `SealedChannel`**: leave M2.3's raw path as bare-`Process` builtins,
  add `SealedChannel<secret T>` as the one new type. Less churn; the "sibling
  types" symmetry is partial. *Recommended for v1* (minimize the migration; the
  fence is still a static type property because `SealedChannel` is its own type).
**PINNED (maintainer 2026-06-30): add only `SealedChannel<secret T>` in v1;
the raw path stays the M2.3 bare-`Process` builtins (no migration).**

## Consequences

### Positive
- Restores **privilege separation across a process boundary without abandoning
  the secret type** — the core security pattern the bare fence blocked — reusing
  the already-verified-CT `aead`/`x25519`/ssh crypto (no new primitive).
- The fence is a **static property of the type** (D1): "was this secret sealed
  before it crossed?" is a compile-time fact.
- Reuses the ssh stack's proven KEX + per-record-AEAD machine almost verbatim.

### Negative
- A **different, weaker, more assumption-laden guarantee** than in-process
  `secret` (D8): cryptographic + key-management, not machine-verified. The
  silent-failure surface (nonce reuse, unauthenticated KEX, length leak) is real
  and is why D3/D4/D5 need sign-off and a careful implementation.
- More surface: a new type, a handshake, key/nonce state on the pipe.

### Neutral
- `SealedChannel` is one more interner type variant (the `Channel`/`Task`
  pattern). No `Type: Copy` regression (an id is `u32`).

## Revisit
- **D3/D4/D5 are the security-critical forks** — any weakness is a
  confidentiality regression; if a leak is found, report **privately** (the
  CONTRIBUTING.md vulnerability process), never a public issue/PR.
- Revisit the authentication model (D3) if a sandboxing use case wants a
  different trust root; revisit padding (D5) when variable-length payloads land.

## Estimated footprint (per sub-phase)
| Sub-phase | Deliverable | Rough LOC |
|-----------|-------------|-----------|
| M2.4a | `Type::SealedChannel` + `seal`/`open` over `secret i64` + the pipe framing (raw ciphertext) | ~400 |
| M2.4b | The x25519 handshake over the pipe (reuse `ssh_*_handshake`) + per-direction keys/nonces (D3/D4) | ~300 (mostly wiring) |
| M2.4c | Generic `secret T` elements + variable-length `secret [u8]` + the D5 padding policy | ~400 |
| (scg) | self-host mirror | deferred (snc-side first, like M2.3b) |

---

**▶ SIGN-OFF COMPLETE (maintainer 2026-06-30):** D3 = (a) reuse the ssh host-key
model (v1 authenticates); D4 = counter-nonces + per-direction HKDF keys; D5 =
fixed-width i64-minimum now, padding later; D9 = add only `SealedChannel<secret T>`
in v1. **Implementation may proceed: M2.4a (the `secret i64` minimum) first,
snc-side (like M2.3b), ADR-rhythm.**
