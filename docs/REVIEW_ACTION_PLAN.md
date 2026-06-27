# External Review Action Plan — June 2026

Four independent external reviews of the repository were received on
2026-06-09 (labeled **CO**, **D**, **G**, **Gr** after their filenames;
source documents live outside the repo). All four are broadly positive
about the engineering methodology — the differential-validation
self-host, the ABI discipline, the ADR trail — and converge on a
consistent set of concerns. This document consolidates those concerns,
records what was verified against the codebase on 2026-06-10, and lays
out a prioritized plan.

When this plan and STATE.md disagree, STATE.md wins.

---

## 0. Status update (2026-06-27)

The plan below is the original June 2026 document; its §1 status column is
frozen at the 2026-06-10 verification. Most of the plan has since shipped.
Current band status (see STATE.md / the commit log for detail):

- **P0 — DONE (all of it).** README constant-time reword (P0.1) + soundness-gap
  bullet (P0.2) + CONTRIBUTING.md (P0.3) + SECRETS_LIFECYCLE vision banner
  (P0.4) + the stale closure pointer fix (P0.5) + the CI doctest step (P0.6).
- **P1 — DONE.** Bar B reached full-corpus codegen parity → **ADR 0045
  ACCEPTED** (P1.1); the partial-move soundness gap (F1) is closed in BOTH
  `snc` and `scg` → **ADR 0046 ACCEPTED** (P1.2). The headline UB hole is fixed.
- **P2 — DONE except P2.4.** `docs/ct-model.md` (P2.1) + the secret-flow
  conformance suite (P2.2) + the `lookup_var` loud fallback guard (P2.3) + the
  front-end panic-freedom fuzzer, which found + fixed two real bugs (P2.5).
  **REMAINING: P2.4 (Linux CI)** — needs CI infrastructure; cannot be validated
  on the macOS dev box.
- **P3 — P3.1 + P3.2 DONE.** PROGRAMMING_GUIDE.md rewritten to current Sentinel
  (P3.1); the STATE.md chronology split out to `HISTORY.md` (P3.2). **REMAINING:
  P3.3 (DESIGN/DESIGN2 disposition), P3.4 (stale-reference sweep), P3.5 (minimal
  LSP), P3.6 (diagnostics quality pass).**
- **P4 — REMAINING / deferred** (external CT review, perf profile, Polonius +
  field-disjoint borrows, platform/distribution niceties, the CT-emission
  research track) — outreach + deferred ergonomics, post-core by design.

Beyond the review plan, the active track since has been examples-as-tests +
core libraries (the `std/security` constant-time crypto suite, a networked
`sshd`, the data/text trio, and the `f64` / `extern "C"` FFI / C-ABI export
language-gap rocks); see STATE.md.

---

## 1. Consolidated findings

Each row was re-verified against the repo before being accepted as
actionable (a review claim that turned out stale is marked).

| # | Concern | Raised by | Verified status in repo (2026-06-10) |
|---|---------|-----------|--------------------------------------|
| F1 | Borrow-check **partial-move soundness gap** (UAF / double-free via field-by-value move; drop fires twice) | CO, D, G | OPEN. `docs/borrow-check-limitations.md` documents it honestly; fix has **no ADR** (the doc's "ADR 0019" pointer is stale — 0019 was consumed by the C3 kickoff). Drops are fully active since C2.4/C2.5, so the gap is live UB. |
| F2 | **Constant-time claim outruns the guarantee** — README says the compiler "proves it"; actual property is "no secret-dependent branch/index/divisor *at the MIR level, pre-LLVM-optimization*, with the type system as the taint oracle" | CO (hardest), G | CONFIRMED. README line 69 carries the flat claim. The code itself is honest (`verify_constant_time` doc comment + ADR 0026 D5 amendment admit type-as-oracle and the post-optimization gap); the README does not. |
| F3 | **Opaque paths are load-bearing and untested adversarially** — method calls, struct/array literals, handler bodies, enum/match all funnel through `MirOp::Opaque`; no enumeration of precise vs conservative constructs; no secret-flow fuzzing | CO, Gr | CONFIRMED. ~15 constructs funnel to `Opaque` in `sentinel-mir/src/lib.rs`. No conformance suite routes secrets through each path. |
| F4 | `lookup_var` unbound fallback silently emits empty `Opaque` → a resolver bug would *false-negative* the CT check | CO | CONFIRMED at `crates/sentinel-mir/src/lib.rs:318`. |
| F5 | **macOS-only** build + CI; Linux x86-64 is the actual deployment target of the stated audience | CO, D, Gr | CONFIRMED. `.github/workflows/ci.yml` runs `macos-14` only. Mitigating: no hardcoded target triples in crates; `.cargo/config.toml` already documents the Linux env passthrough; `mlock` use is confined to the broker (POSIX). The port is shallower than reviewers assumed. |
| F6 | **PROGRAMMING_GUIDE.md dangerously stale** — still describes C0 ("one type: i64") | D, Gr | CONFIRMED. Guide is pre-C1; the language now has enums/match, strings, Vec, I/O, loops, modules, classes/traits, effects, secret, concurrency. |
| F7 | **STATE.md density / README tone** — banner is a ~1500-word shorthand paragraph; README superlatives read as persuasion | CO (Gr dissents — calls STATE.md "a masterclass") | CONFIRMED (5,934 lines; banner is one dense paragraph). Split preserves both views: stable capabilities section + append-only journal. |
| F8 | **No CONTRIBUTING.md** / unclear path for bug reports & security findings | CO, D, Gr | CONFIRMED. Absent. |
| F9 | **SECRETS_LIFECYCLE.md scope** — reads as roadmap; should be explicitly marked vision | CO, D | PARTIAL. Header says "proposed extension"; reviewers want an unambiguous not-on-the-roadmap banner. |
| F10 | **LSP is a stub** | D, G, Gr | CONFIRMED (`sentinel-lsp`, ADR 0025 D10). Salsa foundation is in place. |
| F11 | **No fuzzing / property-based testing** | CO, D, Gr | CONFIRMED. No cargo-fuzz targets anywhere. |
| F12 | **Finish Phase D Bar B** (full-corpus codegen parity) | G, Gr | IN FLIGHT — this is the active track. 110 of ~123 mode-4 fixtures emitting; remaining: c35d → c35e → c36a/c36b → phase-go → ADR 0045 ACCEPTED. |
| F13 | DESIGN vs DESIGN2 split is confusing | D | CONFIRMED — both live side by side with no banner. |
| F14 | Polonius migration + field-disjoint borrows (over-rejection ergonomics) | G | OPEN by design. ADR 0018 still PROPOSED. CO explicitly agrees these are deferrable; they do not gate safety claims. |
| F15 | Post-LLVM assembly verification + speculation-barrier emission | G (CO: at least stop claiming it) | Future work, already scoped out in README "What this is not"; needs honest framing now (F2) and a named research track later. |
| F16 | scg-vs-snc performance profile undocumented; broker extraction; binary releases; devcontainer/Nix; dependency audit; stale-docs sweep | Gr | All confirmed absent; all low-risk, post-core items. |
| F17 | External cryptographer / PL-security review of the CT claim | CO | Not yet sought. Sequence after F2 calibration so the *calibrated* claim is what gets reviewed. |
| F18 | CI runs only three of the four checks — `cargo test --doc` is missing from ci.yml (the four-check norm includes it) | (found during verification) | CONFIRMED. `justfile check-all` also omits doctests (`test-all` has them). |

---

## 2. Sequencing rationale

Three constraints drive the ordering:

1. **Bar B is mid-flight.** Full-corpus parity (F12) is slice-by-slice
   differential work against a byte-exact oracle. Any change that
   moves the oracle's emitted IR mid-bar multiplies re-validation
   work. So: nothing that changes `snc`'s output lands until ADR 0045
   is ACCEPTED.
2. **The soundness fix moves the oracle.** The F1 fix changes the
   `DropPlan` (skip moved fields) and adds rejections — that changes
   emitted IR and corpus membership, and must then be mirrored in
   `selfhost/` under the established lock-step discipline. It is the
   *first* post-Bar-B engineering, not parallel work.
3. **Docs-only calibration is zero-risk and addresses the largest
   stated risk.** CO's core point — overclaiming on exactly the
   property the target audience would audit — is fixable this week
   without touching code. The code's own honesty just has to be
   copied up into the README.

Priority bands: **P0** = now, alongside Bar B (docs-only).
**P1** = the active milestone + first post-milestone engineering.
**P2** = verification hardening + platform. **P3** = docs overhaul +
tooling. **P4** = outreach + deferred ergonomics.

---

## 3. The plan

### P0 — Claim calibration & posture (docs-only; this week; no oracle risk)

- **P0.1 README constant-time reword (F2).**
  Replace the flat "proves it" framing with the precise property:
  *"secret-dependent control flow, memory addressing, and division
  are statically rejected — machine-checked on the compiler's MIR."*
  State the two boundaries the code already admits: (a) the taint
  oracle is the type system's secret-propagation rules (no
  independent dataflow pass yet — ADR 0026 D5 amendment); (b)
  verification runs before LLVM optimization: it constrains the
  program, not the optimized machine code, and constant-time
  *emission* (cmov forcing, speculation barriers, post-codegen asm
  verification) is explicitly future work. Keep the feature — it is
  real and novel — but let the claim match the mechanism.
- **P0.2 README Status surfaces the soundness gap (F1).**
  One bullet in Status linking `borrow-check-limitations.md`:
  Move-typed struct fields passed by value are a known UB hole;
  closure plan is P1.2 below. Do not let it live only in a sub-doc.
- **P0.3 CONTRIBUTING.md (F8).**
  Short: not accepting general PRs yet (and why); what *is* welcome
  (bug reports, security-relevant findings + how to report them,
  docs fixes, test cases); pointer to the ADR process and to
  STATE.md for current status.
- **P0.4 SECRETS_LIFECYCLE.md vision banner (F9).**
  Top-of-file status line: *vision / design exploration — not on the
  implementation roadmap; the shipped surface is `secret T` +
  constant-time verification.* No L1–L4 work is scheduled (explicit
  non-action, per CO + D).
- **P0.5 Fix the stale closure pointer in borrow-check-limitations.md (F1).**
  "ADR 0019" → "a new ADR (next free number at write time)"; 0019 was
  reused for the C3 kickoff.
- **P0.6 CI gains the missing doctest step (F18).**
  Add `cargo test --doc --workspace` to ci.yml (and to `just
  check-all`) so CI actually enforces the four-check norm.

### P1 — Finish the milestone, then close the soundness gap

- **P1.1 Complete Bar B → ADR 0045 ACCEPTED (F12).** Unchanged active
  track: c35d (embedded perform) → c35e (chained effecting lets) →
  c36a/c36b (return-arm + nested handles) → full-corpus phase-go.
  Two reviewers independently endorse finishing this before anything
  structural.
- **P1.2 Partial-move soundness fix (F1) — new ADR, first post-Bar-B
  engineering.** Per the limitations doc's closure plan:
  per-`(VarId, FieldPath)` move state in `sentinel-borrow-check`;
  `UseAfterMove` keyed on the field path for reads of moved fields;
  `DropPlan` statically skips moved fields (static rewrite preferred
  over dynamic drop flags — matches the doc; dynamic flags only if
  conditional moves force them). Sequence inside the slice:
  1. ADR PROPOSED (decision detail: projection depth, `match`
     bindings interaction, Vec/String fields, arrays of Move values).
  2. Implement in Rust `snc` (the oracle) + UB-shape fixtures: the
     doc's reproducer becomes a `tests/ui` rejection; sound variants
     (whole-struct move, re-init) become `tests/pass` fixtures.
  3. Re-bless differential dumps; mirror the analysis in
     `selfhost/borrow.sentinel` (+ codegen if the DropPlan shape
     changed); both fixed-point paths re-validated.
  4. Update `borrow-check-limitations.md`, README Status (drop the
     P0.2 caveat), and STATE.md.
  This is the hard blocker for any external "memory-safe" claim —
  3 of 4 reviewers, and the project's own doc, agree.

### P2 — Verification hardening & second platform

- **P2.1 Constant-time model doc (F3).** `docs/ct-model.md` (or a
  SECRETS_LIFECYCLE-adjacent doc): enumerate every construct and its
  modeling status — *precise* (branch/index/divisor sinks,
  declassify), *conservative via Opaque* (method calls, struct/array
  literals, enum construction, match), *not on the 1.0 CT path*
  (handler arm bodies). This is the contract P2.2 tests against, and
  the artifact an external reviewer (P4.1) reads first.
- **P2.2 Secret-flow conformance suite (F3).** Adversarial fixtures
  routing a `secret` through **each** Opaque-funneled construct into
  each sink, asserting the leak is still rejected (taint survives the
  funnel). Start hand-written (one per construct × sink); extend with
  property-based generation later. Closes CO's "the soundness
  argument rests on the type checker being correct everywhere"
  objection with tests instead of trust.
- **P2.3 `lookup_var` loud fallback (F4).** `debug_assert!` (or
  `cfg(debug_assertions)` panic) on the unbound-variable path in
  `crates/sentinel-mir/src/lib.rs:318`; release builds keep the total
  fallback. A resolver bug must not silently un-taint a secret.
- **P2.4 Linux CI, observe-only first (F5).** Add an
  `ubuntu-24.04` job (apt `llvm-18`, `LLVM_SYS_180_PREFIX` via env)
  with `continue-on-error: true`. Groundwork already exists (env
  passthrough documented in `.cargo/config.toml`; no hardcoded
  triples). Expect and triage: linker flags, allocator strictness
  (glibc will surface heap bugs the macOS allocator masks — that is
  the point), runtime ABI assumptions. Promote to a required check
  once green; record platform notes in `abi-v1.md`. Do this **before**
  abi-v1 ossifies further.
- **P2.5 Fuzzing seed (F11).** `cargo-fuzz` targets for lexer + parser
  (panic-freedom / no-hang on arbitrary bytes) — the most attackable
  surface and the cheapest to stand up. Phase 2: differential fuzzing
  (generated programs → compare `snc` stage dumps vs `scg` mode
  outputs), which the per-stage dump oracles make unusually cheap.
  Phase 3: property tests for `verify_constant_time` (P2.2's
  generator reused).

### P3 — Documentation overhaul & developer experience

- **P3.1 PROGRAMMING_GUIDE.md rewrite (F6) — highest-priority docs
  fix.** Restructure as a tour of *current* Sentinel: types &
  structs, enums + `match`, strings/`u8`/`Vec<T>`, file I/O, loops,
  modules (`use`/`pub`), references & the borrow checker (including
  current limitations), `secret`/`declassify` + the CT rules,
  effects/handlers, classes/traits/delegation, structured
  concurrency. Pull examples from `tests/pass/` fixtures (already
  verified by CI). Keep the C0 text as a historical appendix. Can
  proceed in parallel with anything (no code coupling).
- **P3.2 STATE.md split (F7).** STATE.md becomes: short banner +
  stable per-crate "current capabilities"; the per-slice progress
  journal moves to an append-only `docs/JOURNAL.md` (or
  STATE-archive). Nothing is deleted — CO gets a readable summary,
  Gr keeps the masterclass log.
- **P3.3 Design-doc disposition (F13).** SENTINEL_DESIGN2.md is the
  design of record (absorb DESIGN.md's still-unique content:
  implementation strategy, open questions); SENTINEL_DESIGN.md gets a
  superseded banner. README links updated.
- **P3.4 Stale-reference sweep (F16).** One pass over docs/ for
  pre-Phase-D claims stated as current ("single-file", "no loops",
  "C0 only") outside clearly historical context.
- **P3.5 Minimal LSP (F10).** Wire the existing salsa pipeline into
  `sentinel-lsp`: open/save → run lex→…→ctverify queries →
  `publishDiagnostics`. No completion/goto in v0 — error-as-you-type
  alone moves the DX needle 3 reviewers flagged. Follow-on:
  tree-sitter grammar for highlighting (cheap, independent).
- **P3.6 Diagnostics quality pass (F16).** Audit the highest-traffic
  diagnostics (borrow, effects, CT `secret_leak`) for
  actionability — expected/found, the *why*, and the workaround
  (e.g. the lexical-borrow inner-block idiom from
  borrow-check-limitations.md).

### P4 — Outreach & deferred ergonomics

- **P4.1 External review of the CT property (F17).** After P0.1 +
  P2.1 land, seek a cryptographer / PL-security reviewer for the
  calibrated claim + the ct-model doc + the conformance suite.
  Self-validation catches divergence, not shared conceptual error
  (CO's framing — correct).
- **P4.2 scg vs snc performance profile (F16).** Measure self-build
  wall-clock + memory for both paths; document in STATE.md/README.
  No optimization work scheduled until measured.
- **P4.3 Polonius step .b + field-disjoint borrows (F14).** Real
  ergonomics, explicitly deferrable (CO concurs; G wants it).
  Schedule after P1.2 + P2.4; the ADR 0018 plan stands.
- **P4.4 Platform/distribution niceties (F16).** devcontainer or Nix
  flake (after P2.4 proves Linux); prebuilt binaries; broker
  crates.io extraction (independently useful; needs its own
  API-stability pass); dependency audit (`cargo audit` in CI).
- **P4.5 Constant-time emission research track (F15).** Named
  post-Phase-D track, referenced from README future-work: cmov/CSDB
  emission, post-codegen assembly verification of secret-handling
  functions, valgrind-style dynamic checking on the runtime side.
  Until it exists, the README claim stays at the P0.1 calibrated
  level.

### Explicit non-actions

- **SECRETS_LIFECYCLE L1–L4 implementation** — stays parked (vision
  doc per P0.4). Two reviewers independently recommend this.
- **Cross-process capabilities / actors** — stays deferred
  (post-Phase-D, as already documented).
- **Thick-HIR migration / codegen-consumes-MIR** — unchanged
  post-1.0 stance; not raised as a risk by any reviewer.

---

## 4. Review-point → action mapping

| Review point | Action | Band |
|---|---|---|
| CT claim overreach (CO, G) | P0.1, P2.1, P4.5 | P0/P2/P4 |
| Borrow soundness gap (CO, D, G) | P0.2, P0.5, P1.2 | P0/P1 |
| Opaque-path trust + secret-flow fuzzing (CO, Gr) | P2.1, P2.2, P2.5 | P2 |
| `lookup_var` silent fallback (CO) | P2.3 | P2 |
| macOS lock-in (CO, D, Gr) | P2.4, P4.4 | P2/P4 |
| Stale programming guide (D, Gr) | P3.1 | P3 |
| STATE.md density / README tone (CO) | P0.1, P3.2 | P0/P3 |
| Contribution posture (CO, D, Gr) | P0.3 | P0 |
| SECRETS_LIFECYCLE scope (CO, D) | P0.4 | P0 |
| Finish Bar B (G, Gr) | P1.1 | P1 |
| LSP stub (D, G, Gr) | P3.5 | P3 |
| Compiler fuzzing (D) | P2.5 | P2 |
| DESIGN/DESIGN2 (D) | P3.3 | P3 |
| Polonius + field-disjoint (G) | P4.3 | P4 |
| External CT review (CO) | P4.1 | P4 |
| Perf profile, broker spin-out, releases, audit (Gr) | P4.2, P4.4 | P4 |
| Doctest gap in CI (internal finding) | P0.6 | P0 |

---

## 5. Process notes

- P0 items ship as docs-only commits (plus the one-line CI yaml
  change) and do not disturb Bar B.
- P1.2 and every subsequent oracle-moving change follows the
  established rhythm: ADR PROPOSED → Rust `snc` + fixtures →
  differential re-bless → selfhost mirror → both fixed-point paths
  green → ADR ACCEPTED → STATE/HANDOVER refresh.
- Every item lands four-check green (build · nextest · doctests ·
  clippy `-D warnings`); P0.6 makes CI enforce the same.
