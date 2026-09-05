# Inbound capability requests — disposition ledger

Requests filed **against** this compiler by downstream consumers, and what this repo
decided about each. It records the **response**, not the request: the request text stays
canonical in the filer's own repo, and duplicating it here would drift the way
`BACKLOG.md`/`BACKLOG2.md` already have.

**Rule:** closing a row updates it. This file is what the filer reads to know where they
stand, so a stale row is worse than an empty one.

---

## Source: Sentinel-IDE (`arcanii/Sentinel-IDE`)

**Document:** `G:\SentinelIDE\docs\Sentinel-lang_request.md` — R1–R15, filed 2026-07-19,
framing revised 2026-09-04, requests unchanged since filing.

**Standing:** Sentinel-IDE is a native Win32 C++ IDE that ships **1,503 lines of Sentinel**
in its released binary as a `snc build --lib` C-ABI static library — every file reader it
has, plus its only network input. So this is a consumer with production traffic through
the C-ABI path, not a hypothetical one. Their target is `src/core/Seal.h`: an AES-256-GCM
+ PBKDF2-HMAC-SHA256 crypto core they want to rebuild in Sentinel.

⚠ **The document is unusually careful and it is worth reading before acting on any row.**
It states what was tried, what was measured, and what was done instead; it reports its
method; and it removes four of its own initial conclusions that an adversarial pass
refuted, rather than filing them. Treat its measurements as real.

### Triage, 2026-09-05 (verified against the live tree at `2b32b78`)

Every request was re-checked against current source — the doc is seven weeks old and this
repo moves. **None had been satisfied in the meantime**; thirteen are still true as
written, two are partly true, none were wrong.

| R# | Ask | Claim today | Disposition | Size |
|---|---|---|---|---|
| **R1** | secure-zero for `[secret u8]` / `Vec<secret u8>` | STILL-TRUE | **NEEDS-ADR** | medium |
| **R2** | host-controllable failure path for runtime aborts | STILL-TRUE | **NEEDS-ADR** | small |
| ~~R3~~ | `--emit-header` marks `&mut [u8]` params `const` | **DONE** | D54, landed | small |
| R4 | `chacha20poly1305_open` in the stdlib | STILL-TRUE | BACKLOG | small |
| R5 | bulk `[secret u8]` throughput (opt-in `-O`, LTO) | PARTLY-TRUE | NEEDS-ADR | large |
| R6 | hoist `k_constants()` out of the SHA-256 hot path | **PARTLY-TRUE** | BACKLOG | small |
| R7 | PBKDF2-HMAC-SHA256 + HMAC midstate | STILL-TRUE | BACKLOG | medium |
| ~~R8~~ | emit the required Windows system libraries | **DONE** | D56 / ADR 0059 A12 | small |
| R9 | a `std::fs`-shaped stdlib module | STILL-TRUE | NEEDS-ADR | medium |
| R10 | `snc build --shared` on Windows | STILL-TRUE | BACKLOG | small |
| R11 | `?[T]` nullable arrays, or generic enums | STILL-TRUE | NEEDS-ADR | medium |
| R12 | capturing closures | STILL-TRUE | BACKLOG | large |
| R13 | AES-256 key schedule + `pub` GF(2⁸)/GHASH | STILL-TRUE | BACKLOG | small |
| R14 | chunked / streaming AEAD | STILL-TRUE | BACKLOG | small |
| ~~R15~~ | documentation corrections | **DONE** | D55, landed | small |

### What to do first, and why

**▶ R3, R8 and R15 are DONE (2026-09-05).** The rest of this section is the case for what
remains.

**R3 was the cheapest real win in the list — about four lines.** `emit_c_header`
([`main.rs:1697`](../crates/sentinel-driver/src/main.rs)) pushes `"const uint8_t*"`
unconditionally and never reads `RefData.mutable`, so a generated header advertises a
read-only pointer to memory Sentinel writes through. The filer reproduced an access
violation with no compiler diagnostic. Three source comments asserted the const rendering for BOTH forms and were corrected
with it. **Register D54, landed** — `fill(&mut [u8], i64)` now emits
`int64_t fill(uint8_t*, int64_t, int64_t);` while `total(&[u8])` keeps
`const uint8_t*`, pinned by `export_header_const_qualifies_shared_byte_slices_only`
(header text only, so unlike its sibling export tests it runs on Windows).

**R8 is DONE, in its STRONGER form** — the header now carries both the plain-comment
listing and a `#pragma comment(lib, …)` block guarded by
`#if defined(_MSC_VER) && !defined(SENTINEL_NO_AUTOLINK)`. Their "done looks like" accepted
either; the pragma means a host following the header alone links with **no system libraries
named**, which is what the criterion actually asks for. Verified end-to-end on Windows: a C
host naming none of the six links and runs; built with `/DSENTINEL_NO_AUTOLINK` it fails
with `__imp_closesocket`, `__imp_NtReadFile`, `__imp_getaddrinfo`, `__imp_WSAGetLastError`
— the class they reported. The escape hatch is real, because the pragma injects
`/DEFAULTLIB:` and a host with a deliberate CRT model should keep control.
**Register D56, ADR 0059 A12.**

**R2's cheapest form is authorised by the request itself:** "*even a hook that is required
to terminate afterwards would be a large improvement over the current silence*". That
variant is runtime-only — funnel the 17 `std::process::abort()` sites through one
`runtime_abort()` that calls a host-registered callback and then aborts unconditionally.
It moves no emitted byte, so the bootstrap fixed point is untouched; it adds one C-ABI
symbol (the pinned count at `runtime/src/lib.rs:2917` goes 51 → 52).

**R6 should be answered with a number, not a patch.** The hoist is real, but the payoff
the filer expects is not there: measured on this box, hoisting the table gives **1.14× on a
single-block hash and 1.02× on 17 blocks**, because the table is ~12% of a 2.97 µs call and
the per-block message-schedule `Vec` ([`sha256.sentinel:123`](../sentinel_library/std/security/sha256.sentinel))
is what actually dominates. Their acceptance bar asks for a "multiple-× improvement", which
this cannot meet. Sending them that measurement redirects their budget better than the
patch does — by their own R5 data the opt-in optimiser is worth far more.

**R15's most important item is one it does not list.** [`docs/STATE.md`](STATE.md) — the
repo's own source of truth, which outranks every other doc here — still lists as "Still
deferred: the caller-provides-buffer convention …, the Linux `cc -shared` path". Both
ship: the Linux shared-object path is implemented at
[`main.rs:1652`](../crates/sentinel-driver/src/main.rs) ("Linux: a PIC shared object with
a soname (ADR 0060 Phase 2)"), and `&mut [u8]` export parameters demonstrably work — R3
exists precisely because they work and the generated header mis-declares them. A wrong
capability claim in STATE.md is the highest-authority wrong claim in the tree.
**Filed as register D55.**

### Standing notes

- **R1 is the one that decides the port.** The filer states it is the only item that would
  make them abandon rather than work around: without a way to wipe a `[secret u8]`, moving
  key handling out of CNG makes their product *less* secure than the code it replaces.
  Automatic scrubbing today fires only at `Shared<secret T>` / `Mutex<secret T>` last drop
  and only over a *scalar* payload; array-shaped secrets never enter that path. This is an
  ADR, and it lands in the `secret` discipline, so it is high-stakes by definition.
- **R5 separates cleanly** and the separation is the finding: opt-in `-O` via
  `Module::run_passes` is non-oracle-moving (`snc llvm` is a deliberately separate text
  backend), so it needs no `selfhost/` mirror. Bounds-check elision and LTO do not share
  that property.
- **R11's two halves cannot be separated.** `is_some` and `unwrap_or` are the only
  operations over `?T`, so a `?[u8]` whose `unwrap_or` is refused is constructible,
  testable and unreadable — a write-only type. Note this interacts with register D47,
  which just refused `unwrap_or` on heap-indirected payloads.
- **R12's near-term need is documentation, not the feature** — the filer says so: "*we
  raise it now so we can design our interfaces around its absence*". The cheapest useful
  step is publishing the actual `Fn` envelope, because the workaround their doc assumes
  does not type-check.
- **R14 may already be closed for them.** `std/security/sealed.sentinel` has a keyed,
  sequence-numbered, fixed-width AEAD record sealer that borrows its key. Point them at it
  before building anything.
