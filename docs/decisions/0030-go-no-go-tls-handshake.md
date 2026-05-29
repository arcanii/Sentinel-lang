# ADR 0030: The 1.0 go/no-go — a TLS-1.3-handshake-shaped program (D13)

Status: **ACCEPTED-WITH-AMENDMENTS — the close bar is met; Sentinel 1.0 is
declared (2026-05-30).** The C5 **close-bar** sub-phase ADR under ADR 0025
(Phase C5 kickoff) D1/D13, opened after a readiness/scoping pass against
the current surface. The program runs + **passes the D5 constant-time
check**, so 1.0 is declared. Amendments: **A1** — the connection actor was
descoped from 1.0 (D3, developer-endorsed). **A2** — bytes/labels are
modelled as `i64`, secret material as `secret` scalars, iteration as
recursion (D4); a `sec` widening helper lifts public constants into the
secret domain (C3.1b makes a mixed secret/public op a type error). **A3**
— the shift-wave prerequisite (D5) was **not** needed: the reduced
primitives use only `+ - * ^ & |`. **(3/N) — DELIVERED (2026-05-30):**
Sentinel 1.0 declared; ADR 0025 → ACCEPTED. Per ADR 0025 D14's
per-sub-phase ADR pattern; numbers indicative.

Date: 2026-05-30
Related:
  - **0025** (Phase C5 kickoff): D1/D13 name the go/no-go; the **C5.0
    resolution** pinned it to a single-process, single-file TLS 1.3
    handshake and scoped the crypto to "handshake-*shaped*, not a
    production cipher suite." This ADR records the program structure + the
    decisions the scoping pass surfaced, and **deviates from C5.0 on one
    point — actors (D3 below).**
  - **0026 D5** (constant-time verification): the program must **pass**
    `verify_constant_time` — the decisive 1.0 validation (D6).
  - **0027** (bitwise `& | ^`; A1 deferred `<< >> ~`): the constant-time
    verify uses `^`/`|`; **shifts are a conditional just-in-time
    prerequisite** (D5) if a reduced primitive needs them.
  - **0028 / 0029** (broker scope arenas / stable ABI): the runtime
    substrate the program links against — both shipped.

**(1/N) update (2026-05-30) — DELIVERED.** The skeleton (D9 1/N) shipped:
`tests/pass/c5_go_no_go.sentinel` composes the state-machine class + the
`Kdf` cipher-suite trait/impl (receiver-typed dispatch) + the `Net` I/O
effect + handler + the 4-stage flow with **stubbed** crypto, and runs
end-to-end to exit 42. It **compiled on the first try** — empirical
confirmation of the scoping verdict that the go/no-go is an assembly of
proven patterns. +1 test (1232), four-check green. **Stays PROPOSED** —
(2/N) fills the constant-time primitives over `secret` scalars and makes
the program **pass D5** (D6); closing that declares 1.0.

**(2/N) update (2026-05-30) — DELIVERED; the close bar is MET.** The
stubs were replaced with real constant-time crypto over `secret` scalars
— a Montgomery-ladder step + branch-free `cswap` (`mask = sec(0) - bit`),
an HKDF-expand-shaped mix via the `Kdf` trait, and the `c53_ct_eq`
`Finished` verify — and the program **passes the D5 constant-time check**
(`verify_constant_time` gates the build) and runs to exit 42. Every
secret op is `+ - * ^ & |` (no D5 sink); the lone `declassify` is the
`Finished` accumulator (D6 satisfied by construction; verified the secret
typing is live — a deliberate secret array index is rejected at
type-check). One ergonomic finding, recorded for the spec: **C3.1b makes
a mixed secret/public operation a type error** (no in-expression
widening), so constant-time code lifts public constants/labels into the
secret domain first — here via a `sec(x) { let s: secret i64 = x; s }`
helper (widening happens only at a `let` with a `secret` annotation, not
at a return). +0 net tests (the `c5_go_no_go` fixture evolved from the
1/N skeleton; 1232). **The D8 close bar (runs + passes D5) is now met.**
**(3/N) — declaring Sentinel 1.0 + flipping ADR 0025 → ACCEPTED — is the
developer's call and is intentionally left to them**; this ADR stays
PROPOSED until that decision.

## Context — the scoping pass (2026-05-30)

Before opening this sub-phase, the handshake was sketched against the
**current** surface to find gaps. Result: **the go/no-go is an assembly
of already-proven patterns plus deliberate modelling choices, not a
dependency on big new machinery.**

**Proven and reusable (each has a passing fixture):**
  - constant-time `Finished`/MAC verify — `c53_ct_eq` is *literally* this
    shape (XOR-accumulate over `secret` scalars → `declassify`), and it
    **passes D5**;
  - state machine + cipher-suite trait + delegation — `c4_go_no_go`
    (class + trait + impl + delegation + concurrency in one program);
  - socket I/O + errors as effects/handlers — `c37_go_no_go`
    (`effect Io { log(msg) -> i64; }`, `perform`, `handle … with { … }`);
  - bounded iteration — **recursion works** (verified: `fact`,
    mutual `is_even`/`is_odd`), substituting for the absent loop surface;
  - broker secret-memory + scoped budget (C5.4); a frozen, linkable
    `abi-v1` (C5 D7).

**Gaps + resolutions (none block the reduced program):**
  - **No loops.** Use recursion for bounded/fixed iteration. The C5.0
    crypto scoping (fixed-size compares, a Montgomery-ladder *step*) was
    chosen to fit this; deep ladders stay out (reduced primitives).
  - **No bytes/strings** (literals are Int/Bool/Null only). Model
    records / HKDF labels as `i64` words / int constants (D4). A byte
    type is post-1.0 polish.
  - **No shifts `<< >> ~` / modulo `%`** (`%` isn't even lexed). The
    reduced primitives (XOR-accumulate, fixed `+ * ^` mixes) avoid them;
    anything hash/rotate-heavy would need ADR 0027 A1 (shifts) — a
    conditional JIT prerequisite (D5), not a blanket dependency.
  - **No `[secret T]` arrays.** Model key material as `secret` *scalars*
    (the `c53_ct_eq` pattern), not secret arrays.
  - **Actors not built** — see D3.

## Decision

### D1. Goal — the close bar.

A single-process, single-file Sentinel program, TLS-1.3-handshake-shaped
(server-flavoured: **accept → ECDHE → HKDF key schedule → `Finished` MAC
verify**), that (a) **compiles + links + runs** to a correct result and
(b) **passes the D5 constant-time verification**. It must exercise the
1.0 surface: `secret` + constant-time, classes/traits/delegation,
effects + handlers, witness-table generics, the broker (secret-memory +
a scoped budget), and the nullability/bounds/region core. **Closing it
declares Sentinel 1.0** and flips ADR 0025 → ACCEPTED.

### D2. Reduced, handshake-shaped crypto (recap of C5.0, made concrete).

The four stages, each over `secret` scalars at fixed sizes:
  - **accept** — an `effect Net { accept() -> i64; recv() -> i64;
    send(b) -> i64; }`, handled by a stub that feeds canned record
    words (I/O-as-effect, the `c37` shape);
  - **ECDHE** — a **Montgomery-ladder step**: a constant-time conditional
    swap (`cswap`) + the ladder's field add/mul **shaped** over a few
    `secret i64` words (not a full 255-bit ladder, not real `X25519`);
  - **HKDF key schedule** — an **`expand`-shaped** fixed mix of the
    shared secret + a label word → traffic-key `secret` words (a fixed
    `+ * ^` mixing function, not real HMAC-SHA256);
  - **`Finished` verify** — the `c53_ct_eq` constant-time compare of the
    computed vs received MAC words → `declassify` the OR-accumulator
    (`0` iff equal). **This is the decisive constant-time sink.**

The language-level forcing function is "can Sentinel *express*
constant-time crypto and *prove* it constant-time," which these reduced
primitives satisfy (C5.0). Optimised AES/X25519/SHA-256 + Argon2id are
**ecosystem** work (§15.3), not the language bar.

### D3. **Descope the connection actor (deviation from ADR 0025 C5.0 D5).**

C5.0 kept "a connection actor … part of the go/no-go's single-process
mailbox surface." **This ADR proposes dropping it from the 1.0 go/no-go.**
Rationale: a single-process, single-file, **sequential** handshake needs
no mailbox — it is fully drivable by the class state machine + recursion
+ I/O-as-effects (and `scope`/`spawn` is available if any step wants
independent crypto). C5.0 itself filed the actor under "*naturally
exercises*," not "requires." Dropping it removes the **single largest
remaining language-design sub-phase** (`actor`/`receive`/`send` surface +
mailbox runtime) from the 1.0 critical path — the same scope-reduction
discipline C5.0 applied to modules (D9) and cross-process (D6). **Actors
become a post-1.0 (or post-go/no-go) sub-phase** if the surface is
wanted; the 1.0 concurrency story is the C4.4 structured-concurrency
surface already shipped. *(Developer's call — flagged as a deviation.)*

### D4. Modelling choices (no new surface).

  - **Bytes/records/labels → `i64` words / int constants.** No byte/`u8`
    type, no string literals at 1.0.
  - **Secret key material → `secret` scalars.** No `[secret T]` arrays.
  - **Bounded iteration → recursion.** No loop surface.
  - Fixed sizes throughout (the reduced primitives are fixed-width).

### D5. Shifts (`<< >> ~`) — a conditional JIT prerequisite.

The reduced primitives are chosen to need only `+ - * & | ^` (all
shipped). **If** the Montgomery/HKDF shaping turns out to need bit shifts
or rotates, land **ADR 0027 A1** (the shift wave, with the
`>>`/nested-generic-close split) as a small prerequisite sub-phase first,
then resume. Recorded as a conditional dependency so it is not a
surprise; expected to be avoidable for the reduced shapes.

### D6. The constant-time bar (the decisive validation).

The program **must pass `verify_constant_time`** (ADR 0026 D5): no
`secret` value reaches a conditional branch, a memory index/address, or a
division divisor. The `cswap`, the HKDF mix, and the `Finished` compare
are all written branch-free over secrets (the `c53_ct_eq` discipline).
This is *the* thing the go/no-go exists to prove at runtime — an HTTP
server would leave it unexercised (C5.0 §1).

### D7. Out of scope (post-1.0 / ecosystem).

Real cipher suites (AES-GCM, X25519, SHA-256, Argon2id — ecosystem,
§15.3); a `u8`/byte type + string/byte literals; the `actor`/`receive`
surface (D3); modules / multi-file (ADR 0025 D9); cross-process
(ADR 0025 D6); loops (the surface is loop-free by design through 1.0,
recursion substituting). Each is a named follow-on, not a 1.0 gap.

### D8. Phase-go + fixture.

`tests/pass/c5_go_no_go.sentinel` (the TLS-handshake-shaped program):
compiles, links, runs to a correct exit code (handshake completes + the
`Finished` MAC verifies), **and** passes the D5 constant-time check
(asserted by the build succeeding past `verify_constant_time`). Its green
run + D5 pass **is** the 1.0 close bar; flip ADR 0025 → ACCEPTED.

### D9. Sub-phase split.

| Sub        | Title                                                          | Risk   | Est.        |
|------------|----------------------------------------------------------------|--------|-------------|
| (1/N)      | Skeleton — handshake state machine (class) + cipher-suite      | medium | 1 session   |
|            | trait + `Net`/error effects + the 4-stage control flow, with   |        |             |
|            | **stubbed** crypto; compiles + links + runs end-to-end.        |        |             |
| (2/N)      | Fill the constant-time primitives (`cswap` + Montgomery step + | medium | 1-2 sessions|
|            | HKDF mix + `Finished` verify over `secret` scalars); **passes  |        |             |
|            | D5**. Land ADR 0027 A1 (shifts) first **iff** a primitive      |        |             |
|            | needs it (D5).                                                 |        |             |
| (3/N)      | Close-out: `c5_go_no_go` fixture green + D5-clean; flip ADR     | low    | <1 session  |
|            | 0025 → ACCEPTED; **declare Sentinel 1.0**.                     |        |             |

## Reasoning

**Why the go/no-go now.** It is the only sub-phase that *closes* Phase C;
the three building blocks it needs — constant-time `secret`, broker scope
arenas, a frozen ABI — are all shipped, and the scoping pass showed the
rest is assembly + modelling, not new machinery. It is also the
integration test that will surface any remaining ergonomic gaps.

**Why descope actors.** The riskiest, largest remaining surface is
off the sequential-handshake critical path; dropping it mirrors the
modules/cross-process reductions C5.0 already made, and keeps 1.0 focused
on its headline guarantee (constant-time `secret`) rather than a second
concurrency surface.

**Why reduced crypto is the *right* bar, not a cop-out.** The 1.0
question is "can Sentinel express + *prove* constant-time crypto," not
"does Sentinel ship a crypto library" (§15.3 — libraries are ecosystem).
The reduced primitives force the constant-time codegen + verification
(D6) exactly; real cipher suites add bit-twiddling volume, not a new
language requirement.

## Consequences

### Positive
- Closing it ships **Sentinel 1.0** and validates the constant-time
  guarantee end-to-end at runtime.
- No new language machinery on the critical path (modulo a conditional
  shift sub-phase) — the productionization work (C5.1–C5 D7) pays off.

### Negative
- The reduced crypto is not a real cipher suite (intentional; §15.3) —
  1.0 proves *expressibility + verification*, not performance.
- Modelling bytes as `i64` is ergonomically rough; a byte type is a
  visible post-1.0 follow-on.

### Neutral
- Actors, loops, bytes, modules, cross-process all remain coherent
  post-1.0 follow-ons with their scope already analysed.

## Revisit

PROPOSED until the program runs + passes D5. Triggers:
- **D5**: if a reduced primitive needs shifts, pause for ADR 0027 A1,
  then resume (amendment).
- **D3**: if the developer wants the actor surface in 1.0, re-add it as a
  sub-phase before close (reverts this deviation).
- **D4**: if `i64`-modelled bytes prove unworkable for the skeleton, a
  minimal `u8` type becomes a prerequisite (a scope expansion to record).
