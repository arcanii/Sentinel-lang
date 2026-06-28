# ADR 0061: Code signing & supply-chain trust — signed artifacts, a consumer trust manifest, capability-bounded keys

Status: **ACCEPTED-WITH-AMENDMENTS — v1 IMPLEMENTED (2026-06-28, Windows-verified).**
The first concrete, v1-scoped realization of **BACKLOG2 §2** ("Cryptographic Signatures
and Supply-Chain Provenance") + the **AI_TOOLING §7.1** mechanism ("a dependency not in
the trust manifest fails to compile"). **Not oracle-moving** (see *Self-host*); does
**not** touch the constant-time machinery (see *Constant-time guarantee*).

**v1 shipped** (new `sentinel-trust` crate + `snc` subcommands):
- **Verify (D4)** — in-process **Ed25519 + SHA-512** in Rust, the TweetNaCl twin of
  `std::security::ed25519`, KAT-validated on RFC 8032 (verify lives inside `snc`'s trust
  boundary — chosen over a shell-out helper). Cross-checked **byte-identical** to the
  Sentinel signer.
- **Format (D2/D3)** — `canonical_payload = domain‖algo‖pubkey‖grants‖SHA512(body)`
  (Rust-only — the Sentinel signer signs opaque bytes, so no second format impl) + the
  detached / in-file-`//`-block carrier + `verify_signed`. **Comments are signed**;
  **grants are bound** (both tested).
- **Sign (dogfood)** — `tools/trust/{sign,keygen}_core.sentinel` (the verified-constant-time
  `std::security::ed25519`) + `snc sign` / `snc keygen` / `snc verify` (Rust orchestration).
- **Gate (D7)** — `snc build --require-signatures off|warn|strict` + `--trust <manifest>`
  (the consumer `sentinel-trust.toml`, D5); runs over the module graph after discovery.
- **Capability bounding (D6)** — a trusted module's used capabilities ⊆ its key's grants;
  v1 enforces **`ffi`** (an `extern "C"` block).

**Amendments from the design (v1 pragmatics):**
- **A1.** The canonical carrier is the **detached `.sig`** (the in-file `//` block parses
  via the same parser, but the build-time gate consumes detached carriers; the in-file
  block's body-exclusion split is a follow-up).
- **A2.** Verify is a **Rust twin** of `std::security::ed25519` (owner-chosen "twin oracle"
  over the ADR 0059 link-in, which stays the north star for making verify *literally*
  Sentinel). Sign **is** Sentinel (the dogfooded cores); `snc sign`/`keygen` shell out to
  them (acceptable on the author side — a tampered signer only yields signatures verify
  rejects).
- **A3.** The signed-object metadata is bound as a **rigid canonical byte payload**, not a
  TOML serialization (no canonicalization TCB); the human-readable carrier is hex, no
  base64 dep.
- **A4.** Capability taxonomy is **`ffi`-first** (from `program.externs`, no AST walk);
  `network`/`filesystem`/… (builtin-call scanning) extend from here.
- **A5.** `keygen` entropy is `std::sys::random` — `getentropy` on unix, `RtlGenRandom`
  (advapi32) on Windows, selected by ADR 0062 conditional compilation. **`snc keygen` now
  runs on all three targets** (the initial POSIX-only limitation is resolved; `c230ff3`).

**v1 deferred** (unchanged from *Phasing*): the in-file-block gate, `--separate`/`--lib`
gating, TOFU/issuer policies, the keystore + hardware keys, rotation, revocation,
build-env attestation, multi-party signing, and the per-language binding generators.

## Context

ADR 0037 point 12 (`--lib-path` / `SNC_LIB_PATH`) lets a program `use` a library from
anywhere on disk. That answers *where* a dependency's source is found — but nothing
answers *who wrote it* or *whether the bytes are the ones their author vouched for*. A
library consumed via the search path is today fully trusted by default, which is exactly
the supply-chain gap Sentinel exists to close: the value proposition is *"the code you
build is the code you reviewed, signed by an authorized party"* (BACKLOG2 §2.1).

Sentinel is unusually well-positioned to make this a **language** property rather than a
packaging afterthought, because two of the load-bearing pieces already exist:

- **Verified constant-time Ed25519** (`std::security::ed25519`, sign + verify, RFC 8032) —
  the compiler can verify signatures with its *own* crypto.
- **Effects in signatures** (the effect/capability system) — trust can be **bounded by
  capability**, not just identity, for free (BACKLOG2 §2.3). This is the distinguishing
  feature over every existing package manager.

This ADR scopes a v1 that is small, decentralized (no mandatory third party), and
forward-compatible with the full §2 roadmap (keystore, rotation, revocation, attestation).

### Threat model

**Defends against:** tampering with a dependency's bytes in transit or at rest;
substitution of a malicious dependency for a trusted one; and — via capability bounds —
a **compromised maintainer key** pivoting to capabilities it was never granted (e.g. a
crypto library's key suddenly publishing code that opens a network socket).

**Does NOT defend against (out of scope / honest limits):** a *trusted author who is
themselves malicious* (trust is trust — capability bounds limit the blast radius but a
key granted `network` can abuse `network`); a compromised build machine signing with a
software key (mitigated later by hardware-backed keys, §2.4); or a consumer who pins the
wrong key (key distribution is out-of-band — see D5).

## Decision (proposed)

### D1. Signed ≠ trusted — two distinct layers.

Detection of "is this signed" is near-useless on its own: **anyone can generate a keypair
and sign anything.** A signature establishes only *authenticity + integrity* — "the
holder of key `K` vouches for exactly these bytes." **Trust** is a separate decision the
*consumer* makes: "is `K` a key I have chosen to rely on, and for what?" The compiler
therefore evaluates two independent layers, in order:

1. **Authenticity/integrity** (this ADR's crypto): do the bytes verify under the key the
   artifact claims?
2. **Trust** (this ADR's policy): is that key trusted by the consumer's manifest, under a
   policy, and does the code stay within the capabilities that key was granted (D6)?

A correct signature by an untrusted key is **untrusted**, exactly like no signature.

### D2. What is signed — the raw bytes, comments included; never a parsed projection.

The signed content is the artifact's **literal bytes**, hashed as-is. **Comments are
signed.** The only region excluded from the body is the signature container itself (D3),
and that container's own security-relevant fields are themselves signed (D3) — so nothing
an attacker can reach is left unsigned. Three reasons this is non-negotiable:

1. **Comments carry security-relevant meaning, uniquely so in Sentinel.** Every
   `declassify` — the one sanctioned escape from the `secret` discipline — is justified by
   a comment (`// SAFETY: declassifying only the public AEAD tag, never key material`).
   License, authorship/attribution, and any future comment-borne directive live there too.
   If comments were mutable post-signature, an attacker could **forge the appearance of
   review** — rewrite the justification a human signed off on — without breaking the
   signature. The reviewer signed what they *read*; they read the comments.

2. **"Strip comments, then verify" would make the lexer attacker-controlled TCB.** Signing
   a *projection* of the source (comments removed, whitespace normalized, the AST, …)
   forces a parse step on untrusted input *ahead of* the trust decision. Any disagreement
   between the signer's and verifier's notion of "what is a comment" — a `//` inside a
   string literal, a marker at a token boundary, char-literal edge cases — either
   false-rejects or, worse, validates a signature over content X while the compiler
   compiles content Y (the XML-dsig / JSON-canonicalization wrapping-attack class). The
   invariant is **verify the exact bytes you compile, with zero transformation between
   verification and compilation.**

3. **It is simpler and stricter.** Hash raw bytes; any byte change fails. (The rejected
   alternative — signing the canonical AST/token stream so cosmetic edits survive — also
   ties every signature to the compiler version and breaks "reviewer reads text, signer
   signs text." See *Reasoning*.) The cost — re-signing after a comment typo — is paid in
   **tooling** (`snc sign` is one command), not in a weaker guarantee.

### D3. The signed object — a manifest/header that commits to content hashes and signs its own metadata.

The thing actually signed is a small, fixed-shape **signed object** (a "manifest" for a
library; an in-file "header block" for a single file) containing:

- `version` (e.g. `sentinel-sig-v1`) and `algorithm` (D4 — agility field),
- the signer **public key** + a short `key-id`,
- the **capability grants** (D6),
- a **content commitment**: the SHA-512 digest of each artifact's raw bytes (the whole
  source file for single-file; each source file and/or compiled object for a library),
- the **signature**: Ed25519 over a canonical serialization of all of the above *except
  the signature field itself*.

This binds the body via its hash *and* binds the metadata (algorithm, key, grants) under
the same signature — so neither the code nor the declared capabilities are malleable.
Verification is: recompute each artifact's hash from the bytes on disk, check it matches
the manifest, then Ed25519-verify the manifest over a candidate key. The only thing
canonicalized is the small metadata blob **we** define the format of — never
attacker-controlled source — so the canonicalization TCB is minimal and bounded.

Three carriers, same signed-object shape:

- **Detached** (`<file>.sig`) — canonical; signs the exact raw bytes, no self-reference.
- **In-file header block** — ergonomic for single-file programs/scripts. A run of `//`
  line comments at **byte 0**, delimited exactly (`// @sentinel-signature v1 …
  // @end-signature`); the body is every byte after the terminating delimiter. It is a
  *comment* (the lexer already skips it — see *Self-host*), the metadata fields are signed
  per the rule above, and the body (all code + **all other comments**) is hashed.
- **Library manifest** (`<lib>.sentinel-manifest` + detached sig) — the primary path for
  the pre-built-library rock: one signed object committing to every file and/or the
  compiled object + its Sentinel interface descriptor. Scales past one file.

(All three filenames/markers are provisional — open to fold into a future package
manifest. Algorithm agility (D4) and the rotation/capability fields (D6, §2.5) are
reserved now so v1 artifacts stay forward-verifiable.)

### D4. Signature scheme — Ed25519, dogfooded; domain-separated; algorithm-agile.

Ed25519 (RFC 8032), verified by Sentinel's own constant-time `std::security::ed25519`.
The signed message is **domain-separated** — prefixed with a context string
(`"sentinel-sig-v1"` + a separator) — so a Sentinel signature can never be replayed as a
signature in another protocol, and vice versa. Large/binary artifacts are bound by their
SHA-512 content digest (D3), so the message Ed25519 signs is always small. `algorithm` is
an explicit field (not assumed) so a future migration (e.g. to a post-quantum or hybrid
scheme) is a manifest change, not a format break — and "algorithm deprecation" is already
a typed revocation reason (§2.6).

### D5. The consumer trust manifest — the actual authority; pluggable policy.

The root of trust is **the consumer's own manifest** (e.g. `sentinel-trust.toml` at the
project root), not an external CA. It declares, per dependency, which key(s) are
acceptable and under which policy (BACKLOG2 §2.2):

```toml
[dependencies.crypto-primitives]
sig    = "ed25519:RWQ…"          # the trusted key (or its fingerprint)
policy = "exact-key"             # exact-key | trust-on-first-use | issuer
grants = ["secret", "constant_time", "alloc"]
forbids = ["network", "filesystem", "subprocess"]
```

Three policies cover the realistic cases (§2.2): **exact-key** (this key, this version,
refuse any change — strictest), **trust-on-first-use** (pin on first sight, alarm on
change — the reasonable default), and **issuer** (any key with a valid cert chain to a
named CA — for orgs that *want* an internal authority). **v1 ships exact-key + TOFU
only**; issuer/CA is a later add. The honest answer to "what authority?" is therefore
*the consumer is the root of trust*, bootstrapped by **out-of-band** key publication (an
author posts their fingerprint in their repo / on their site) — the SSH `authorized_keys`
model. A transparency-log / identity layer (Sigstore-style) is the credible north star to
layer on later but needs online infra; v1 is not gated on it.

### D6. Capability-bounded keys — trust composes with the effect system.

A trusted key is trusted to publish code within a **declared maximum effect/capability
set**, not for everything (BACKLOG2 §2.3). The consumer's `grants` / `forbids` (D5) — and,
symmetrically, a `grants` bound the *author* self-declares in the signed object (D3) — are
checked against the dependency's **actual effect row** (which the type/effect system
already computes). Code whose effects exceed the bound **fails to compile**, exactly as an
unhandled effect does today. This is the answer to task 2's open question *"how is
untrusted bounded"*: unknown/untrusted code runs under a **minimal capability mask** plus
the existing FFI secret-fence; **trust raises the ceiling**. A compromised maintainer key
thus cannot pivot to a capability it was never granted — the blast radius is bounded by
the trust declaration, least-privilege applied to the signing infrastructure itself.

### D7. Compiler policy & where the gate runs.

A per-project policy controls strictness: `require_signatures = off | warn | strict`.
**off** = today's behavior (everything trusted). **warn** = compile, but surface unsigned
/ untrusted-key / unknown-provenance dependencies as diagnostics. **strict** = a
dependency that is unsigned, signed by an untrusted key, or exceeding its capability bound
**fails to compile** (the AI_TOOLING §7.1 contract). The check is a **driver + resolve
gate** wrapped around module discovery (it slots naturally onto the ADR 0037 module-graph
walk + the `--lib-path` search): for each discovered unit, extract its signed object from
raw bytes, verify (D2–D4), resolve trust (D5), and bound capabilities against the effect
row (D6) — before the unit's bytes enter the existing pipeline. It changes *which units
are admitted*, not how an admitted unit lexes/types/lowers.

## Reasoning

- **Raw bytes over AST/canonical-form.** Signing semantics (the typed program / IR) would
  survive cosmetic edits, but it drags the whole front-end into the verification TCB,
  reintroduces parser-differential risk, ties every signature to the compiler version, and
  breaks "the reviewer reads text; the signer signs text." Raw bytes keep the human and
  the machine looking at the same artifact and shrink the TCB to a hash function.
- **Consumer manifest over a mandatory CA.** A CA hierarchy is centralized, brings CA-
  compromise + revocation complexity, and is philosophically at odds with Sentinel's
  "you control your trust." The consumer manifest is decentralized, needs no third party,
  and still *permits* a CA (the `issuer` policy) for orgs that want one.
- **Capability bounding is the differentiator.** Identity-only signing (minisign, PGP)
  proves *who*; it does nothing to limit *what*. Binding trust to the effect set turns a
  key compromise from "publish anything" into "publish within the declared envelope" — and
  Sentinel gets it almost for free because effects are already in signatures.

## Consequences

**Positive.** Supply-chain integrity becomes a language property; the `--lib-path` /
pre-built-library paths gain a trust gate; Sentinel dogfoods its own Ed25519; capability
bounds give a real least-privilege story no mainstream package manager has.

**Negative.** Re-signing is required after any byte change (mitigated by `snc sign`). A
keystore + key management is genuinely security-critical and easy to get wrong (deferred,
§2.4, but its absence in v1 means software keys only). Trust-manifest maintenance is new
consumer-side work.

**Neutral.** No new runtime `sentinel_*` symbols and no emitted-IR change — signing is a
build/trust concern, not a codegen one. The `secret`/CT machinery is untouched.

## Phasing

- **v1 (this ADR's MVP):** Ed25519 detached sig + in-file header block + library manifest
  (D3/D4); `snc sign` / `snc verify`; the consumer trust manifest with exact-key + TOFU
  (D5); capability bounds checked against the effect row (D6); `require_signatures`
  off/warn/strict (D7). Software keys in a local file; verification via
  `std::security::ed25519`.
- **Later (already in BACKLOG2 §2.4–2.8):** a separate keystore with hardware-backed keys
  (YubiKey/Secure Enclave, physical-touch signing); first-class **key rotation** (signed
  handoff statements); typed **revocation** lists; **issuer/CA** policy; **build-environment
  attestation**; **multi-party reproducible-build** threshold signatures (credible because
  per-unit codegen is already reproducible — ADR 0045 / `repro.rs`); a transparency log.

## Self-host

**Not oracle-moving.** The signature gate is a **driver + resolve** concern that admits or
rejects whole units; it does not alter the emitted LLVM IR or any stage dump for an
admitted unit. The in-file header block is a **`//` comment**, which the lexer already
skips — the *driver* extracts it from raw bytes, the lexer never learns a new token — so
`snc lex` / `ast` and the downstream dumps are byte-identical. No `selfhost/*.sentinel`
mirror is required; both bootstrap fixed points are unaffected. (Stated explicitly per the
repo's oracle discipline; to be confirmed on the differential.)

## Constant-time guarantee

**Untouched.** Signing/trust is orthogonal to the `secret` type rules and the
`sentinel::mir::secret_leak` pass. If anything it *reinforces* the discipline: the
verifier uses constant-time Ed25519, and capability bounds (D6) make `declassify`-bearing
or FFI-bearing dependencies require explicit, signed authority. The CT property constrains
the program pre-LLVM and is unaffected by admitting or rejecting a unit at the trust gate.

## Non-goals

- A package **registry** / network fetch — this is trust of artifacts you already have on
  disk (via `--lib-path` or vendoring), not a distribution system.
- A mandatory **CA / PKI hierarchy** or a **web of trust** — both are at most opt-in
  (`issuer` policy), never required.
- The **keystore**, **hardware-backed keys**, **rotation**, **revocation**, **build-env
  attestation**, and **multi-party** signing — all deferred to post-v1 (BACKLOG2 §2.4–2.8).
- Signing the **compiler/toolchain** itself (a distinct concern from signing user code).

## Open questions

- **Carrier default:** detached `.sig` (cleanest) vs the in-file block (friendliest) as the
  canonical single-file form. Recommendation: detached canonical, in-file block as a
  convenience; library manifest for everything multi-file/compiled.
- **Trust-manifest home:** a standalone `sentinel-trust.toml` now vs folding it into a
  future unified package manifest. Recommendation: standalone now, designed to merge later.
- **Does the signed object bind the file path?** Binding it gives rename-integrity but
  breaks on legitimate relocation; not binding it lets a signed file be moved freely.
  Recommendation: bind the *module path* in a library manifest, not the filesystem path.
- **Author-declared `grants` vs consumer-declared `forbids` precedence** when both are
  present (D6) — recommendation: enforce the **intersection** (both must permit).
- **TOFU storage:** where the first-use pin is recorded (a lockfile) and how a key change
  is surfaced + explicitly acknowledged.

## References

BACKLOG2 §2 (the full provenance vision this v1 realizes incrementally — §2.1 identity,
§2.2 manifest policies, §2.3 capability bounds, §2.4 keystore, §2.5 rotation, §2.6
revocation, §2.7 build-env attestation, §2.8 multi-party); AI_TOOLING §7.1 ("a dependency
not in the trust manifest fails to compile"); SENTINEL_DESIGN2 §2 (supply-chain thesis),
§7.3 (sandboxing untrusted code with an effect mask), §13.2 (attestation);
ADR 0037 (module graph + `--lib-path` — the gate's host); ADR 0057 (FFI secret-fence — the
floor under untrusted code); ADR 0008 (the `secret`/constant-time discipline the effect
bounds protect); `std::security::ed25519` (the verifier); ADR 0045 / `repro.rs`
(reproducible builds — the basis for multi-party signing later); HANDOVER §0 (task 2).
