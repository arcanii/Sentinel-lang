# ADR 0009: Phase C kickoff and C0 plan

Status: ACCEPTED (Phase C in flight; C0.0/C0.1/C0.2 at 8f37381/7e32e8c/0b07931 exercise D1 defer-Salsa, D1a pure-function discipline, D2 LLVM-only via inkwell, D3 miette+thiserror+insta, D4 logos lexer + hand-written-RD parser, D5 tests/ui+tests/pass harnesses, D7 sentinel-{syntax,ast,codegen,driver} populated, D6 sub-phase rows C0.0-C0.2; D8 closed via ADR 0010 at 72e29d1; D7 sentinel-types-stub-at-C0.2 deferred to C1 per STATE.md C.3; D6 C0.3-C0.5 still ahead)
Date: 2026-05-25
Related: 0001 (staged-validation umbrella), 0002 (Phase B kickoff
template; pay-once-vs-defer asymmetry), 0008 (latest ADR D-numbered
sub-decision format)

## Context

Phase B is complete as of `94d752e` — 226 tests in
sentinel-effects-proto plus 69 in sentinel-broker, all four
check-suite checks green (`cargo build`, `cargo clippy --all-targets
-D warnings`, `cargo test`, `cargo test --doc`). ADRs 0001-0008 are
all ACCEPTED. The Phase B lessons (effect rows, default-close,
handlers, secret qualifier) live in those ADRs and in
sentinel-effects-proto's source as the structural reference for
Phase C; the crate itself is throwaway per HANDOVER §5 and will
become deletion-eligible once C3 absorbs its lessons.

Phase C is the production bootstrap compiler in Rust, targeting the
1.0 subset defined in SENTINEL_DESIGN2.md §15.1. HANDOVER §6
estimates twelve to eighteen months across six sub-phases C0-C5.
C0 (month 1-3) is the first: an end-to-end pipeline for the
smallest possible language subset (`let`, arithmetic, `if`, function
calls), producing a runnable binary via LLVM, with no type system —
everything is `i64`. The goal of C0 is not language coverage but
plumbing: prove the pipeline shape works before any phase has to
fight for room inside it.

The scaffold crates listed in HANDOVER §3.2 already exist as 20-line
stubs (`crate_name()` plus a smoke test). Workspace `Cargo.toml`
pins the dependencies HANDOVER §6.1 prescribes (`logos`, `salsa`,
`inkwell`, `cranelift`, `miette`, `insta`). `justfile`,
`rust-toolchain.toml`, and `.github/workflows/` are in place. There
is no `tests/ui/` or `tests/pass/` directory at the workspace root
yet; C0.0 creates them.

Several Phase C decisions have enough cross-cutting consequences
that discovering them during coding would lead to churn. This ADR
pins down the up-front commitments for C0 in particular and for
Phase C generally where the answer is already clear; decisions
that depend on having a parser or a type system to argue against
are deliberately deferred to follow-up ADRs.

## Decision

Eight D-numbered sub-decisions for Phase C / C0.

### D1. Pipeline framework: defer Salsa to C1+; C0 uses direct function calls.

HANDOVER §6.1 prescribes a Salsa-based query pipeline because
"incremental recompilation is foundational, not retrofitted." That
guidance is not rejected — it is timed.

At C0 there is exactly one input (one source file) and the simplest
possible pipeline (lex → parse → codegen). The Salsa query graph
would be trivial and the query *boundaries* would be guesses,
because the forces that shape good query boundaries — the type
system's invalidation needs, the borrow checker's cross-procedural
queries, the effect inferer's row-unification work — do not exist
yet. Choosing boundaries before those forces exist is the same
mistake ADR 0002 rejected for "row-ready B1."

Additionally, `salsa = "0.18"` is pinned in `Cargo.toml` but the
Salsa ecosystem in 2026 includes rust-analyzer's fork and at least
one active alternative. Deferring to C1 buys a real decision point
on which engine to adopt, informed by what C0 actually produced.

#### D1a. Discipline rider — C0 pipeline stages are pure functions.

The "defer Salsa" call is only safe if C0 does not accumulate
shared mutable state that Salsa adoption would later need to
unwind. C0 holds the following discipline:

  - Each pipeline stage is a pure function: `Input -> (Output,
    Vec<Diagnostic>)` (or `Result<Output, Diagnostic>` where the
    stage is fail-fast).
  - No `Compiler` god-struct holding the AST arena, the source map,
    and the diagnostic sink. No shared mutable diagnostic sink.
  - The driver (`snc`) composes stages by direct calls. The driver
    is the only place state-threading happens.

If this discipline holds, the C1 retrofit is essentially "wrap each
stage in `#[salsa::tracked]` and decide intern boundaries." If the
discipline drifts, the retrofit cost is from the bad shared-state
design, not from late Salsa adoption — diagnose accordingly.

### D2. Codegen backend: LLVM only via `inkwell`. No Cranelift in C0.

HANDOVER §6 mentions Cranelift as a fast-debug-build option.
`cranelift` and friends remain pinned in `Cargo.toml`
[workspace.dependencies] for future use but are not pulled into any
member crate in C0. Dual-backend infrastructure is C5+ work; C0's
job is to prove the LLVM path end-to-end.

### D3. Diagnostics: `miette` + `thiserror` from day one; snapshots via `cargo-insta`.

Direct HANDOVER §6.3 prescription. Every diagnostic answers what is
wrong, why it is wrong, and what to do. Diagnostic regressions
caught by snapshot tests (HANDOVER §6.4). Allocate the 15%
engineering-time budget HANDOVER §6.3 calls for.

The lexer in C0.0 has just enough error surface to exercise the
diagnostic pipeline (e.g., an unterminated string or unexpected
character). The first `tests/ui/` test is a snapshot of one such
error.

### D4. Parser: hand-written recursive descent; lexer via `logos`. No CST in C0.

HANDOVER §3.3 explicitly recommends "hand-written recursive
descent for better error messages." Sentinel-effects-proto's parser
is the structural reference: precedence-climbing for expressions,
spanned tokens, span-bearing AST nodes.

C0 has no CST/AST split. The AST is a direct enum (Rust `enum`
with `Span` on each node). The rationale for splitting CST from
AST in production compilers (rust-analyzer, rowan) is that LSP
wants to preserve original-source positions for refactoring; C0
has no LSP and `miette` spans on AST nodes suffice for diagnostic
quality. The split is reconsidered at C5 (LSP polish) at the
earliest.

### D5. Test harness: `tests/ui/` + `tests/pass/` + `cargo-insta`, created in C0.0.

rustc-style. `tests/ui/<name>.sentinel` paired with
`tests/ui/<name>.stderr` snapshots; `tests/pass/<name>.sentinel`
paired with `tests/pass/<name>.stdout` snapshots. `cargo-insta`
manages all snapshot files. `cargo nextest` is the runner per
HANDOVER §6.4.

`just check-all` extends to invoke the new layers. The full
check-suite from C0.0 onward is:

  - `cargo build --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace`
  - `cargo test --doc --workspace`

The Phase B four-check pattern (`cargo build`, `cargo clippy`,
`cargo test`, `cargo test --doc`) carries forward; `nextest`
replaces `cargo test` for the unit/integration layers because UI
tests benefit from per-test isolation. Doctests still run via
`cargo test --doc` because nextest does not own them.

### D6. C0 sub-phase split: six commits, one per milestone.

The Phase B rhythm of feat-commits-per-milestone + docs-commit-at-
phase-boundary carries forward. Each sub-phase produces a `feat`
commit (or chained `feat` + small follow-up commits where useful),
adds tests, and updates the STATE.md tracker row. A docs commit at
the end of C0 marks the phase landed and updates this ADR's status
to ACCEPTED.

| Sub  | Scope                                                        | Crates touched                                          |
|------|--------------------------------------------------------------|---------------------------------------------------------|
| C0.0 | Tokens + lexer + `tests/ui/` harness + 1 lex-error UI test   | sentinel-syntax, sentinel-driver, root `tests/`         |
| C0.1 | Hand-written parser + AST (int literal, arithmetic, parens); `snc parse` dumps AST | sentinel-syntax, sentinel-ast, sentinel-driver         |
| C0.2 | LLVM codegen for C0.1 AST; first runnable binary; 1 `tests/pass/` test  | sentinel-codegen, sentinel-driver, sentinel-runtime    |
| C0.3 | `let` bindings + variable references (i64 everywhere)        | sentinel-ast, sentinel-codegen                          |
| C0.4 | `if`/`else` + function calls (forward-declared, fixed signatures) | sentinel-ast, sentinel-syntax, sentinel-codegen     |
| C0.5 | `fn` definitions + `main` entry; **C0 go/no-go**: a multi-fn program with let/if/arithmetic produces correct stdout | all of the above |

Six commits is acceptable density; further sub-splitting (C0.2a
literal codegen + C0.2b arithmetic codegen) is at the implementer's
discretion if a milestone turns out to be larger than expected and
deserves a checkpoint.

### D7. Scaffold crates kept as-is; populated in pipeline order.

No restructure of `crates/`. Crates are populated in pipeline
order across C0:

  - sentinel-syntax: lexer (C0.0), parser (C0.1)
  - sentinel-ast: AST nodes (C0.1)
  - sentinel-types: stub `check() -> Result<(), Diagnostic>`
    returning `Ok` in C0.2 so the pipeline shape is right when C1
    fills it in
  - sentinel-codegen: LLVM lowering (C0.2 onward)
  - sentinel-driver: `snc` binary, wires the pipeline (C0.0 onward)
  - sentinel-runtime: minimal print/exit stub linked into emitted
    binaries (C0.2)

sentinel-resolve, sentinel-hir, sentinel-mir, sentinel-lsp remain
at 20-line stubs through C0. Each gets populated at its
corresponding sub-phase (resolve in C1, hir in C1/C2, mir in C2,
lsp in C5).

sentinel-effects-proto (Sentinel-Mini) is **not touched** in C0.
Its source remains as the structural reference for C3. Its
deletion-eligibility after C3 is flagged here but the actual
delete is a future ADR.

### D8. Concrete C0 surface syntax: deferred to ADR 0010.

The 1.0 surface target is SENTINEL_DESIGN2.md §15.1, but C0 needs
a *concrete*, strictly-subsetted surface: which integer literal
forms (decimal only? hex? underscores?), which arithmetic
operators (`+ - * /` only? remainder? unary minus?), which `if`
shape (statement vs expression), which function-call syntax.

These questions are easier to argue once a lexer and parser exist
to point at. ADR 0010 is written after C0.0 ships the lexer, so it
can argue against real token productions rather than against a
sketch. ADR 0010 lands before C0.1's parser commit.

## Reasoning

The decisions cluster around two recurring themes:

**Decide-now-or-later asymmetry.** D1 (Salsa), D4 (CST), and D8
(concrete syntax) are all "defer until forces exist." The
asymmetry is the same one ADR 0002 documented for effect rows: if
you commit early and guess wrong, you pay twice; if you defer and
the migration is painful, you pay once with full intermediate
lessons. D1 in particular adds a discipline rider (D1a) because
Salsa is a framework, not a data shape — frameworks are harder to
back out of, and only the pure-function discipline keeps the door
open. D2 (LLVM-only), D3 (miette), and D5 (test harness) go the
other way: these are decisions where the cost of deferring is
visible right away (you'd write the harness twice, you'd write
diagnostics twice) and the design space is well-understood.

**Pipeline shape over feature coverage.** D6 and D7 together
encode the rustc strategy HANDOVER §6.2 prescribes: end-to-end
plumbing for the smallest subset, then expand. Each C0 sub-phase
proves a new joint in the pipeline rather than adding a new
feature in isolation. C0.0 proves "source → tokens → driver
output." C0.1 adds parsing. C0.2 adds the LLVM joint and produces
the first runnable binary. C0.3-0.5 thicken the language behind
that already-working pipeline. This is the same shape Phase B
took (B0 surface, B1 types, B2 effects, B3 handlers, B4 secret) —
the lesson transferred from validating it in B is that pipeline-
first remains the right approach in C.

## Consequences

### Positive

- C0.0 has zero new dependencies beyond what is already pinned in
  `Cargo.toml`. The first commit is small and reviewable.

- C1's first sub-phase (call it C1.0) can be either "adopt Salsa"
  or "add type checking, defer Salsa further" — the choice stays
  live because of D1a. The decision point exists.

- The test harness from C0.0 onward catches diagnostic regressions
  starting from the very first lex error. No "we'll add tests
  later" debt accumulates.

- The Phase B documentation rhythm transfers cleanly: feat commits
  per sub-phase, docs commit at phase boundary, STATE.md tracker
  row backfilled with hash. No new conventions to learn.

### Negative

- No `tests/` directory exists at workspace root. C0.0 creates
  it along with the lexer. This is a slightly larger first commit
  than "just lexer" would be, but the alternative is creating an
  empty harness in a separate commit that does nothing.

- The pure-function discipline (D1a) is a real constraint. C0
  cannot reach for ergonomic shortcuts like a shared diagnostic
  sink threaded through a `&mut Cx`. If the discipline drifts, the
  C1 retrofit gets painful. This must be enforced in review.

- Deferring the concrete-syntax decision to ADR 0010 means C0.1
  starts with a brief design lull while 0010 is written. This is
  fine: the parser commit takes long enough that one ADR-writing
  pause in front of it is rounding error.

- sentinel-types::check() returning unconditional `Ok` is a
  shape-only stub. It does not validate that the pipeline is wired
  correctly to a real type pass (because there isn't one yet). C1
  is where it earns its place.

### Neutral

- The six-commit C0 split (D6) is denser than Phase B's typical
  three-to-five commits per phase (e.g., B4.0a/b/c + B4.1a/b +
  B4.2). C0's coverage is wider per commit because each sub-phase
  thickens a different pipeline joint rather than adding to one
  feature. Acceptable.

- sentinel-effects-proto stays in the workspace through C3.
  `cargo build --workspace` and `cargo test --workspace` continue
  to build/run it; that is by design (it remains the structural
  reference for effects work in C3) and costs about three seconds
  of CI time per check.

## Alternatives considered

- **Adopt Salsa at C0.0.** Documented and rejected above: the
  query boundaries would be guesses, the API churn risk is real,
  and the asymmetry from ADR 0002 transfers directly. The strongest
  counter-argument is "frameworks are sticky, so the retrofit cost
  is harder than the row-shape migration was" — addressed by D1a's
  pure-function discipline.

- **Dual-backend codegen from C0.** Rejected: LLVM is the
  production target, Cranelift is fast-debug-build infrastructure
  that has no payoff at C0 (debug builds of the prototype compile
  in seconds anyway). Revisit at C5 if developer iteration time
  becomes a real bottleneck.

- **Skip the test harness in C0.0, add it later.** Rejected: every
  HANDOVER guidance on diagnostics (§6.3) and testing (§6.4)
  emphasizes that snapshot tests catch the regressions worth
  catching. Building the diagnostic pipeline without snapshot
  coverage is exactly the way diagnostic quality drifts.

- **CST/AST split from C0.** Rejected: the split's payoff is for
  LSP refactoring and source-preserving formatters, neither of
  which exists before C5. Carrying the CST cost from C0 onward is
  premature.

- **Hand-rolled query system instead of Salsa.** Not rejected, just
  deferred — D1's "defer to C1" deliberately preserves this option.
  ADR at C1 will choose between Salsa 0.18, rust-analyzer's fork,
  and a hand-rolled approach with the C0 codebase in evidence.

- **Single C0 commit ("the prototype compiler")** instead of the
  six-sub-phase split. Rejected: the Phase B rhythm validated
  that small commits with tests at each milestone catch regressions
  early and produce a readable git log. The same rhythm transfers
  to C0.

## Revisit

This ADR is **PROPOSED** until C0.0 lands. At that point it
becomes ACCEPTED and the status line gets a hash-stamped
confirmation note, in the ADR 0008 style.

D1 is revisited at C1.0 (or whichever sub-phase first wants
caching). Trigger conditions for revisiting before then:

- C0 accumulates shared mutable state despite D1a, suggesting the
  discipline is not actually holding.
- A C0 milestone surfaces a query-graph requirement that direct
  function calls cannot express cleanly (e.g., needing to
  invalidate downstream stages from an upstream change without
  recomputing everything).

D4 is revisited at C5 (LSP polish) when the CST/AST decision
matters. Trigger condition for revisiting earlier: a C2/C3
diagnostic needs original-source byte-exact positioning that the
AST `Span` cannot provide.

D8 is closed when ADR 0010 lands (before C0.1's parser commit).

The "Sentinel-Mini deletion eligibility" pointer in D7 is
revisited as a separate ADR after C3 completes. Until then the
crate is preserved untouched.
