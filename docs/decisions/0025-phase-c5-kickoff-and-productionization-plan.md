# ADR 0025: Phase C5 kickoff — productionization to 1.0 (broker integration, constant-time codegen, actors, stable ABI, tooling)

Status: PROPOSED — to flip to ACCEPTED (or ACCEPTED-WITH-
AMENDMENTS) as Phase C5's sub-phases land. This ADR opens the
final bootstrap-compiler phase per HANDOVER §6.2's month-15-18
budget: "broker integration, cross-process safety, reproducible-
build guarantees, stable ABI definition, LSP and tooling polish."
It also absorbs the items prior phases deferred *to* C5: actors
(ADR 0021 D10), cross-process / location qualifiers (ADR 0021
D12), and constant-time secret codegen (ADR 0019 D12 / ADR 0008).
**Phase C5 close = Phase C close = Sentinel 1.0.**

Date: 2026-05-29
Related:
  - **0001** (staged validation — ACCEPTED): the umbrella. C5 is
    the last validation stage before Phase D self-hosting.
  - **0009** (Phase C kickoff — ACCEPTED): the query-based
    pipeline + `snc` driver. C5 fills the pipeline's middle
    (HIR/MIR) that C0-C4 skipped (codegen consumes `TypedProgram`
    directly today).
  - **0011 / 0017 / 0019 / 0021** (C1-C4 kickoffs): the
    parallel-tree pattern, interner-table-with-`Copy`-`Type`
    invariant, salsa pipeline, and ADR-first + feat/docs commit
    norms all carry forward.
  - **0008** (secret qualifier + constant-time — ACCEPTED): the
    constant-time *codegen* it specified is implemented at C5
    (D3); C3.1 shipped only the type-level rejection.
  - **0019** (Phase C3 kickoff — ACCEPTED-WITH-AMENDMENTS): D12
    deferred constant-time codegen + speculation barriers. C5 D3
    closes it.
  - **0021** (Phase C4 kickoff — ACCEPTED-WITH-AMENDMENTS): D10
    deferred actors → C5 (D5 here); D12 deferred cross-process /
    GPU / `@numa` → C5 (D6 here).
  - **0018** (Polonius migration — PROPOSED): an available C2
    follow-on whose trigger ("empirical friction") may surface
    during C5's larger programs; not C5-gating.
  - **Phase A broker** (ADRs 0001 + the A-milestone history):
    generational arenas (bump + slab), scoped budgets, recording
    mode, secret-memory policy. C5 D4 wires it into codegen.

## Context

Phases C0-C4 built a single-process, single-file Sentinel
compiler that already enforces the memory-safety + secret-typing +
effect-system + class/trait/concurrency surface end-to-end:

  - C0: lex → parse → AST → LLVM → linked binary (`snc`).
  - C1: full type system (structs, generics, nullable, arrays).
  - C2: references + mutability + lexical borrow check + RAII drop.
  - C3: effect rows + secret *typing* + handler runtime.
  - C4: classes + traits + named impls + delegation + structured
    concurrency (thread-per-spawn).

What stands between that and a 1.0 toolchain — the C5 gap:

  1. **The pipeline middle is missing.** ADR 0009 §6.1 specifies
     `sentinel-hir` (typed HIR, qualifiers resolved) and
     `sentinel-mir` (SSA: escape analysis, bounds-check elision,
     constant-time verification). Both crates are scaffold stubs;
     codegen lowers `TypedProgram` directly. The deferred
     security + perf passes have nowhere to live.
  2. **`secret` has no runtime teeth.** C3.1 type-rejects the four
     leak shapes (secret branch, secret divisor, secret index,
     secret-ref deref) but codegen lowers `secret T` identically
     to `T` (ADR 0019 D12). No branch-free `select`/`cmov`, no
     speculation barriers. Sentinel's headline promise is
     unfulfilled at runtime.
  3. **No broker integration.** Codegen allocates via
     `sentinel_alloc`/`sentinel_free` (libc malloc/free wrappers).
     The Phase A broker — arenas, scoped budgets, secret-memory
     policy, recording mode — is unused. Sentinel's region/scope
     model doesn't yet map onto it.
  4. **Single-file only.** `snc build <one-file>`; no module
     system, no separate compilation, no stable ABI between units.
  5. **No actors, no cross-process.** Deferred from C4.
  6. **Tooling is a stub.** `sentinel-lsp` has one smoke test; the
     "reuse the salsa query engine for the LSP" payoff (ADR 0009
     §6.1) is unrealised. UI tests assert `stderr.contains(code)`
     rather than blessed snapshots (§6.4 wants `insta` + `nextest`).
  7. **No determinism or perf gates.** Reproducible builds (a
     Phase D self-hosting prerequisite — §7 wants a byte-identical
     fixed point) and the §6.5 perf targets are unmeasured.

The C5 close bar is the **Phase C go/no-go** (HANDOVER §6 /
SENTINEL_DESIGN2 §15): the compiler compiles a non-trivial program
— a TLS handshake or an HTTP server — that exercises all 1.0
features and runs correctly. That program is the forcing function
for which C5 workstreams are 1.0-minimum vs post-1.0.

C5 is the largest and highest-novelty Phase C sub-effort. Unlike
C4 ("reasonable language-design plumbing"), C5's constant-time
codegen (D3), cross-process safety (D6), and stable ABI (D7)
carry genuinely novel work. This ADR's job is to **sequence** that
work, make minimum-viable scoping calls, and defer detailed
surface to per-sub-phase ADRs (0026+) — mirroring how ADR 0021
deferred to 0022/0023/0024. It does NOT fully specify any
workstream.

The C4.5 lexer/state going into C5: the full keyword + punctuation
set from C0-C4 (class/trait/impl/init/delegate/scope/spawn/await/
Self/self/as/for + the C1-C3 set). C5 adds keywords only as its
sub-phase ADRs require (e.g., `actor`/`receive` for D5, `mod`/`use`
for a module system if D9 lands).

## Decision

Fourteen D-numbered scoping decisions. Each is a *proposed* call
for a PROPOSED ADR; detailed surface + the hard design lands in
per-sub-phase ADRs 0026+.

### D1. Phase C5 scope + the 1.0 close bar.

C5 is the final bootstrap-compiler phase. Its close criterion is
the Phase C go/no-go: **compile + correctly run a TLS handshake or
HTTP-server-shaped program exercising all 1.0 features** (the
SENTINEL_DESIGN2 §15 subset). The chosen go/no-go program (D13) is
picked at C5.0 open and pins the 1.0-minimum scope of every other
D — a workstream is C5-minimum iff that program needs it.

Workstreams: HIR/MIR (D2), constant-time codegen (D3), broker
integration (D4), actors (D5), cross-process (D6), stable ABI
(D7), reproducible builds (D8), modules / separate compilation
(D9), LSP + tooling (D10), test/diagnostics infra (D11).

### D2. Stand up the HIR + MIR pipeline stages.

Per ADR 0009 §6.1 the pipeline is supposed to be `types → hir →
mir → codegen`; C0-C4 short-circuited `types → codegen`. C5
introduces the middle so the security + perf passes have a home:

  - **`sentinel-hir`**: a typed HIR with all qualifiers resolved —
    desugars the parallel-tree `TypedProgram` into a flatter form
    (monomorphic instances materialised, method/trait dispatch
    resolved to concrete callees, drops/scope-exits explicit).
  - **`sentinel-mir`**: SSA lowering hosting the analyses — escape
    analysis, bounds-check elision, and the constant-time
    verification of D3.

**Decision (proposed):** introduce both as salsa-tracked stages.
**Minimum:** a thin MIR sufficient for the D3 constant-time pass +
basic bounds-check elision; the full escape-analysis/optimisation
suite is post-1.0. **Risk:** this is a large refactor of the
codegen boundary; if it threatens the C5 budget, an amendment may
fold the constant-time pass into a codegen sub-pass on
`TypedProgram` instead (no new crate) — the *capability* (D3)
matters more than the pipeline shape.

### D3. Constant-time secret codegen (closes ADR 0019 D12 + ADR 0008).

The security core of C5. `secret T` must be constant-time at
runtime, not just type-rejected:

  - Secret-dependent **branches** → branch-free `select`/`cmov`.
  - Secret-dependent **memory indexing** → already type-rejected
    (C3.1); codegen must not reintroduce it via optimisation.
  - **Speculation barriers** (lfence-class) inserted per ADR 0008
    where a secret crosses a speculation boundary.
  - A MIR **verification pass** that proves no secret value feeds a
    branch/index/timing-variable operation — a belt-and-suspenders
    check that the codegen actually achieved constant-time, with a
    diagnostic on failure.

**Decision (proposed):** implement on the D2 MIR. **Minimum:** the
operations the C3.1 secret surface already admits (arithmetic,
compare-to-produce-`secret bool`, `declassify`) lowered constant-
time + the verification pass. Architecture-specific barrier
selection is x86-64/aarch64 (the brew-LLVM18 targets); other
targets post-1.0.

### D4. Broker integration.

Route Sentinel heap allocation through the Phase A broker instead
of raw libc malloc/free:

  - Map the C2 region/scope model + the C4 `scope concurrent`
    onto broker **arenas** (bump for scoped, slab for churn).
  - Honor **scoped budgets** — a Sentinel scope can declare a
    memory budget the broker enforces (`BudgetClosed` on overrun).
  - Use the broker's **secret-memory policy** for `secret T`
    payloads (zero-on-free, no-swap hints) — complements D3.

**Decision (proposed):** **minimum** = back `sentinel_alloc`/
`sentinel_free` with a process-wide broker arena (drop-in, proves
the wiring + unlocks budgets/recording). **Richer** = per-region /
per-scope arenas mapping the lexical region tree onto an arena
tree — deferred unless the go/no-go program needs it. Recording
mode (deterministic replay) supports D8.

### D5. Actors (deferred from ADR 0021 D10).

Declared message protocols + mailbox routing. Single-process
first. Surface (`actor`/`receive`/send syntax) is deferred to a
per-sub-phase ADR; this kickoff only fixes: **single-process
mailboxes at C5 minimum**, built on the C4 structured-concurrency
runtime (thread-per-actor or thread-pool); cross-process actors are
D6. Actors are "language-design plumbing" like C4 — the risk is
volume, not novelty.

### D6. Cross-process safety (deferred from ADR 0021 D12).

Cross-process actors + arenas via the broker, with **capability
enforcement at the process boundary** (ADR 0009 §6.2's C3 line,
realised here). **Decision (proposed):** scope to the go/no-go
program's needs; if the 1.0 program is single-process (a TLS
handshake plausibly is), cross-process becomes a **post-1.0
follow-on** and C5 ships the single-process subset. `@numa` / GPU
location qualifiers stay out of scope (D12). This is the riskiest
integration (broker × actors × OS boundary); de-risk by keeping it
optional for the 1.0 bar.

### D7. Stable ABI definition.

Define + document + version a stable ABI for compiled artifacts:
calling convention, the in-memory layout of every `Type`
constructor (struct/array/nullable/ref/class/Task — today ad-hoc
in codegen), name mangling, and the runtime-symbol contract
(`sentinel_*`). 1.0 needs this so separately-compiled units link
and the runtime is independently versionable; Phase D self-hosting
needs it stable. **Decision (proposed):** write an ABI spec doc +
freeze it at `abi-v1`; a layout-stability test suite (extending the
existing `SentinelKont`/`SentinelTask` size asserts) pins it.

### D8. Reproducible-build guarantees.

Byte-identical artifacts for identical inputs. Salsa already gives
stable IDs (source-order interning); the remaining nondeterminism
risks are HashMap iteration order in codegen, embedded
timestamps/paths, and LLVM nondeterminism. **Decision (proposed):**
audit + pin determinism (switch codegen-iteration-order-sensitive
`HashMap`s to `BTreeMap`/sorted, strip timestamps, set LLVM to
deterministic mode); add a "compile twice, diff the object"
regression test. This is a **hard prerequisite for Phase D**'s
byte-identical fixed-point go/no-go, so it lands early (C5.0).

### D9. Module system / separate compilation. (Scoping-open.)

ADR 0009 §6.1 lists "name resolution, module graph"; C0-C4 is
single-file. A real 1.0 program likely spans files. **OPEN
QUESTION (decide at C5.0 with the go/no-go program):** does the
chosen 1.0 program need a module system, or can it be a single
(large) file? If needed: `mod`/`use` surface + a module graph in
resolve + separate-compilation units keyed to the D7 ABI. If a
single file suffices for the go/no-go, modules become a **post-1.0
follow-on**. Flagged honestly rather than pre-decided — this is the
biggest scope swing in C5.

### D10. LSP + tooling polish.

Populate `sentinel-lsp` (today a stub) by reusing the salsa query
engine — the ADR 0009 §6.1 payoff. **Minimum:** diagnostics
(stream the existing accumulator) + go-to-definition, hitting the
§6.5 <50ms p95 target. Hover / completion / rename are post-1.0.
Plus `snc` polish: better CLI, `--emit` flags (IR/asm), a
`check`-only mode.

### D11. Diagnostics + snapshot testing infrastructure.

Adopt the §6.4 testing discipline: migrate `tests/ui/` from
ad-hoc `stderr.contains(code)` checks to **`cargo-insta` blessed
snapshots** (so diagnostic regressions surface in diffs); run via
**`cargo nextest`**. Honor the §6.3 "15% of engineering time on
error quality" budget — every C5 diagnostic answers what/why/how.
Lands at C5.0 (unblocks every later sub-phase's tests).

### D12. Out-of-scope at C5 (post-1.0).

Deferred beyond 1.0: the Cranelift fast-debug backend (ADR 0009
§6.1's optional second backend — LLVM-only at 1.0); full IDE LSP
(hover/completion/rename/refactor); GPU + `@numa` location
qualifiers; multi-architecture beyond x86-64/aarch64; the full
MIR optimisation suite beyond what D3 needs; and **Phase D
self-hosting** (its own multi-month phase after C5).

### D13. Phase-go program.

The C5 phase-go IS the Phase C / 1.0 go/no-go: a TLS-handshake or
HTTP-server-shaped Sentinel program exercising all 1.0 features
(secrets with constant-time, regions/budgets via the broker,
effects + handlers, classes/traits, structured concurrency, and —
if D9 lands — modules). Picked concretely at C5.0 open. Smaller
per-workstream phase-gos (`c5_secret_ct`, `c5_broker_arena`,
`c5_actor_*`, etc.) land with each sub-phase, as in C1-C4.

### D14. Per-sub-phase ADRs.

Like ADR 0021 → 0022/0023/0024, each substantive C5 sub-phase gets
its own PROPOSED-at-open ADR: ADR 0026 (HIR/MIR + constant-time —
D2+D3, the security core), ADR 0027 (broker integration — D4),
ADR 0028 (actors — D5), ADR 0029 (stable ABI + reproducible builds
— D7+D8), and further ADRs for cross-process (D6) / modules (D9) /
LSP (D10) as those land. Numbers are indicative, not binding.

## Sub-phase split

Proposed sequence (refined per revisit triggers + the D13 program
choice). Foundational/low-risk work first; the security core and
the riskiest integrations gated behind it.

| Sub  | Title                                                        | Risk   | Est.        |
|------|--------------------------------------------------------------|--------|-------------|
| C5.0 | Pick the 1.0 go/no-go program (D1/D13); test infra (insta +  | low    | 1 session   |
|      | nextest, D11); reproducible-build audit (D8). Decide D9.     |        |             |
| C5.1 | HIR + MIR stages (D2) — salsa-tracked, minimum viable.       | high   | 2-4 sessions|
| C5.2 | Constant-time secret codegen + verification (D3) on the MIR. | high   | 2-3 sessions|
| C5.3 | Broker integration (D4) — alloc → arenas + budgets.          | medium | 1-2 sessions|
| C5.4 | Stable ABI definition + layout-stability suite (D7).         | medium | 1-2 sessions|
| C5.5 | Actors, single-process (D5).                                 | medium | 2-3 sessions|
| C5.6 | (Conditional) modules / separate compilation (D9) and/or     | high   | 2-4 sessions|
|      | cross-process (D6) — only if the go/no-go program needs them.|        |             |
| C5.7 | LSP minimum + `snc`/tooling polish (D10); perf benchmark     | medium | 1-2 sessions|
|      | harness (§6.5).                                              |        |             |
| C5.8 | The 1.0 go/no-go program (D13); Phase C close; ADR 0025 +    | —      | 1-2 sessions|
|      | ADR 0009 flips; 1.0 tag.                                     |        |             |

Total: ~13-23 sessions — the widest band of any Phase C sub-effort,
dominated by C5.1/C5.2 (the novel MIR + constant-time work) and the
conditional C5.6. The HANDOVER §6.2 "month 15-18" budget is 3
months; the band reflects genuine uncertainty in the MIR refactor
and the D9/D6 scope swing.

## Reasoning

**Why an HIR/MIR stage now (D2), after skipping it for C0-C4.**
The constant-time verification (D3), bounds-check elision, and
escape analysis all want an SSA-shaped IR with explicit control
flow; bolting them onto the parallel-tree `TypedProgram` would be
fragile. C0-C4 correctly skipped the middle to prove the
end-to-end pipeline fast (the rustc approach, ADR 0009 §6.2); C5 is
where the deferred structure pays for itself. The escape hatch
(fold constant-time into a codegen sub-pass) keeps the *security
capability* from being hostage to the *refactor*.

**Why constant-time (D3) is the headline.** Secret typing that
only type-checks is a paper guarantee; the value proposition is
runtime constant-time. It was deferred twice (ADR 0008 → 0019 D12);
C5 is the last responsible place to land it before calling the
language 1.0. It is also the most novel C5 work, hence sequenced
early-but-after-infra (C5.2, behind C5.0/C5.1).

**Why D9 (modules) and D6 (cross-process) are scoping-open.** Both
are large and both may be unnecessary for the *chosen* 1.0 program.
Pre-committing would risk building a module system a single-file
TLS handshake never exercises. The honest move is to pick the
go/no-go program first (C5.0) and let it decide — the kickoff
records the decision *procedure*, not a guess.

**Why infra + determinism lead (C5.0).** `insta`/`nextest` (D11)
and reproducible builds (D8) unblock and de-risk everything after,
and D8 is a Phase D prerequisite. Cheap, foundational, no design
novelty — exactly what belongs first.

## Consequences

### Positive

- Sentinel reaches 1.0: secrets have runtime teeth, the broker is
  wired, the ABI is stable, tooling is real, builds are
  reproducible.
- The pipeline finally matches its ADR 0009 §6.1 architecture
  (HIR/MIR populated), readying Phase D self-hosting.
- Per-sub-phase ADRs keep the largest phase legible — each lands
  PROPOSED-at-open, ACCEPTED-at-close, like C1-C4.

### Negative

- Largest, least-certain phase. The MIR refactor (C5.1) and the
  D6/D9 scope swing could blow the 3-month budget; the band
  (13-23 sessions) is honest about that.
- The constant-time pass (D3) couples to target architecture — the
  brew-LLVM18 x86-64/aarch64 assumption (working norm) becomes
  load-bearing for a *security* property, not just convenience.

### Neutral

- No surface-syntax churn for the existing C0-C4 features; C5 is
  mostly *below* the surface (IR, codegen, runtime, ABI, tooling)
  plus the additive actor/module surface.
- The broker integration (D4) is invisible to user programs (same
  region/scope surface; different allocator underneath) — except
  where a scope opts into a budget.

## Alternatives considered

- **Skip HIR/MIR; constant-time as a codegen sub-pass (D2 escape
  hatch as the primary plan).** Tempting for budget, but
  bounds-check elision + escape analysis genuinely want SSA, and
  Phase D will want the MIR too. Kept as the documented fallback if
  C5.1 over-runs, not the default.
- **Defer constant-time to post-1.0.** Rejected: it is *the*
  differentiating guarantee; shipping "1.0" without it
  misrepresents the language. Deferring it a third time is not
  responsible.
- **Commit to a full module system up front.** Rejected: couples
  C5 to a design the chosen 1.0 program may not need; D9 makes it
  conditional instead.
- **Big-bang C5 (no sub-phases).** Rejected: contradicts the
  ADR-first + feat/docs-per-sub-phase norm that compounded through
  C1-C4; the phase is far too large for one ADR to spec.

## Revisit

PROPOSED until Phase C5 closes. Per-D revisit triggers:

- **D1/D13**: revisit at C5.0 once the concrete go/no-go program is
  chosen — it re-scopes D6/D9 and pins 1.0-minimum.
- **D2**: revisit at C5.1 — if the MIR refactor threatens the
  budget, invoke the escape hatch (codegen sub-pass) via amendment.
- **D6/D9**: revisit at C5.0/C5.6 — in vs post-1.0 per the program.
- **D7**: revisit when the first multi-unit/separate-compilation
  need arises; the ABI must be frozen before Phase D.

## Appendix: estimated implementation footprint

Indicative, by workstream (a kickoff estimate; per-sub-phase ADRs
refine):

| Workstream                              | LOC estimate |
|-----------------------------------------|--------------|
| HIR stage (D2)                          | ~600-1000    |
| MIR stage + SSA + analyses (D2)         | ~1000-2000   |
| Constant-time codegen + verify (D3)     | ~600-1200    |
| Broker integration (D4)                 | ~300-600     |
| Actors (D5)                             | ~800-1500    |
| Cross-process (D6, conditional)         | ~500-1500    |
| Stable ABI spec + tests (D7)            | ~200 + docs  |
| Reproducible-build audit (D8)           | ~150-400     |
| Modules / separate compilation (D9)     | ~800-2000    |
| LSP minimum + tooling (D10)             | ~600-1200    |
| Test/diagnostics infra (D11)            | ~200-400     |
| 1.0 go/no-go program + fixtures (D13)   | ~300         |
| **Total (band, D6/D9 conditional)**     | **~6000-12000** |

Comparable to all of C1-C4 combined — appropriate for the final
phase to 1.0. The wide band reflects the D2 refactor + D6/D9 scope
uncertainty; both narrow at C5.0.
