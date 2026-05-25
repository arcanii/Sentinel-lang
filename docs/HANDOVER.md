# HANDOVER.md — Sentinel Bootstrap Compiler Implementation

This document is the practical handover for starting work on the Sentinel
bootstrap compiler in Rust. It assumes you have read SENTINEL_DESIGN.md
and SENTINEL_DESIGN2.md and have decided to proceed with the staged
validation approach described in Section 16.3 of the design document.

Read this top to bottom once before writing any code. Then use it as a
reference as you work through the milestones.

---

## 0. Current Implementation Status

> This section is the canonical "where the codebase is right now"
> pointer. For per-crate detail and design decisions, read
> docs/STATE.md and the ADRs under docs/decisions/.

**Phase A — sentinel-broker — complete.** Generational arenas
(bump + slab), scoped budgets, diagnostics, recording mode,
secret-memory policy. 69 active tests + 1 doctest. See STATE.md
Section A. ADR 0001 (staged validation) is the umbrella.

**Phase B — sentinel-effects-proto (Sentinel-Mini) — complete.**
Research-grade tree-walking interpreter validating Sentinel's
effect-system design before the production compiler commits.
226 tests (203 lib + 23 integration). All three HANDOVER §5.2
validation demos landed (supply-chain, async-as-effect,
password-verify). The crate is explicitly throwaway per
HANDOVER §5; deletion-eligible once C3 absorbs its lessons.
ADRs 0002-0008 are authoritative. See STATE.md Section B.

**Phase C0 — bootstrap compiler MVP — complete.** The new
production-shape crates (sentinel-syntax, sentinel-ast,
sentinel-codegen, sentinel-driver, sentinel-runtime) now ship a
full lex → parse → AST → two-pass LLVM IR → object → cc-linked
executable pipeline via the `snc` binary. The ADR 0010 appendix
go/no-go program runs:

    fn double(x) { x * 2 }
    fn pick(cond, a, b) { if cond { a } else { b } }
    fn main() {
        let x = 5;
        let y = pick(x, double(x), 0);
        print(y)
    }
    // stdout: "10\n", exit 0

Six sub-phases C0.0-C0.5 shipped across twelve feat+docs commits.
22 pass-test fixtures cover the full surface. ADRs 0009 (Phase C
kickoff) and 0010 (concrete C0 surface) are ACCEPTED. Everything
is `i64` per ADR 0009 ("no type system in C0"); `bool` arrives at
C1.3. See STATE.md Section C.

**Phase C1.0a-b — Salsa retrofit foundation + lex/parse wrapped — complete; C1.0c pending.**
Phase C1 (type system, regions, effects per HANDOVER §6.2) is in
flight per ADR 0011 (PROPOSED, 8 sub-phases, honest 5-6 month
estimate vs HANDOVER's 3-month budget). C1.0a landed the
foundation crate `sentinel-base` hosting the `#[salsa::db]`
SentinelDb trait, `#[salsa::input]` SourceFile, and
`#[salsa::accumulator]` Diagnostic (3 tests; salsa 0.18 is in
the dep graph). **C1.0b landed** — `sentinel-syntax::query`
exposes `lex_query` and `parse_query` as `#[salsa::tracked]`
queries; errors route through the Diagnostic accumulator rather
than tracked-struct fields (side-stepping the
`miette::SourceSpan: !Hash` issue that paused the C1.0a draft);
sentinel-driver instantiates a concrete `SentinelDatabase` and
collects diagnostics via `parse_query::accumulated::<Diagnostic>`.
All 22 C0 pass-test fixtures still run end-to-end through the
new query-based driver path. **C1.0c is pending** — wrap codegen
as a tracked query (LLVM context lifetimes may need a careful
design pass; deferred to keep the C1.0b deltas reviewable). See
ADR 0011 D1 status notes.

**Workspace test count**: 453 active across all crates (+7 over
C1.0a, all in sentinel-syntax: 4 positive lex/parse + 1 cross-
stage propagation + 2 cache validation). All four check-suite
checks green (cargo build --workspace, cargo clippy --workspace
--all-targets -D warnings, cargo test --workspace, cargo test
--workspace --doc). See STATE.md "Conventions" for the per-crate
breakdown.

**ADR status**:

  - 0001 staged-validation                       ACCEPTED
  - 0002 effect-rows-in-mini                     ACCEPTED
  - 0003 b1-retrospective                        ACCEPTED
  - 0004 row-representation-and-effect-surface   ACCEPTED
  - 0005 effect-inference-judgment               ACCEPTED
  - 0006 default-close-row-variables             ACCEPTED
  - 0007 effect-handlers                         ACCEPTED
  - 0008 secret-qualifier-and-constant-time      ACCEPTED
  - 0009 phase-c-kickoff-and-c0-plan             ACCEPTED (all C0
                                                 sub-phases done)
  - 0010 concrete-c0-surface-syntax              ACCEPTED (all
                                                 D-decisions exercised)
  - 0011 phase-c1-kickoff-and-type-system-plan   PROPOSED — D1
                                                 (Salsa) exercised
                                                 end-to-end at C1.0b;
                                                 fully ACCEPTED when
                                                 C1.0c lands codegen

### 0.1 Working norms (carry forward into Phase C1)

Original Phase-A norms, augmented with Phase-B and Phase-C
lessons:

- **Trust STATE.md, not the git log.** Commit messages are dense
  and miss design rationale. Always read docs/STATE.md and this
  file before doing anything; never infer state from git log alone.

- **Terminal quirk: nested heredocs break.** This developer's
  terminal mangles `<<EOF ... <<INNER ... INNER ... EOF`-style
  scripts. Use one of: (a) base64-encoded python3 -c blocks,
  (b) write a script to /tmp/ via a single non-nested heredoc and
  then execute it, or (c) single non-nested heredocs only.

- **Small patches, build between each.** The session that built
  Phase A7 took four diagnostic/fix iterations because the
  initial patch was too ambitious. Better practice: land the
  type/trait changes first, build, then add the implementations,
  build, then add the tests. Same lesson held for Phase C0:
  small sub-phase commits + cargo test after each beats one big
  commit.

- **Honest disclosure beats confident-but-wrong.** This developer
  values being told when something is uncertain or guessed at
  ("I'm not sure if BudgetScope::within_budget emits BudgetClosed
  on rejection, so I included an assertion to find out") over
  patches presented as definitely-correct that turn out not to be.
  The C1.0b pause is an example: rather than land a half-working
  retrofit, the session committed the working sentinel-base alone
  and documented C1.0b's path forward.

- **Minimal ceremony.** "go", "proceed", short replies are the
  norm. Long preambles are unwelcome.

- **Examples held to -D warnings.** Don't allow lint debt in
  examples; they're educational artifacts. Same for tests/pass/
  fixtures (Phase C).

- **Check before overwriting docs.** When patching documentation
  files via Python, always check `p.exists()` and read existing
  content first. Prefer merge/append patterns for docs/. Phase A
  hard-learned lesson on BACKLOG.md.

New norms learned during Phase B and Phase C:

- **ADR-first per phase boundary.** ADR 0002 was the Phase B
  kickoff; ADR 0009 was Phase C kickoff; ADR 0011 was Phase C1
  kickoff. Each landed PROPOSED before the first feat commit,
  became ACCEPTED at sub-phase completion. Continue the pattern
  for Phase D and beyond.

- **feat + docs commit pairs per sub-phase.** Each sub-phase
  ships as a feat commit (code + tests) followed by a docs
  commit (STATE.md refresh + ADR status updates). The docs
  commit also backfills the hash that the feat commit produced.
  See the C0.0-C0.5 history for the rhythm.

- **The pure-function pipeline discipline (ADR 0009 D1a).**
  C0 held it — `lex`, `parse`, `compile_to_object` are all
  `(input) -> (output, diagnostics)`. C1.0a starts cashing in
  the payoff by retrofitting Salsa. Keep new pipeline stages
  pure-function until salsa wrapping happens at a known
  sub-phase.

- **cargo clippy --workspace --all-targets -D warnings** is
  part of the standard four-check suite alongside build / test
  / test --doc. Don't let clippy debt accumulate; it has caused
  full re-sweep commits before (4182ff6 cleared pre-B4.0 lints).

- **No pushes from the assistant.** Commits land locally; the
  dev pushes via GitHub Desktop in batches. Never run `git push`.

- **macOS-only assumption.** `.cargo/config.toml` hardcodes brew
  paths (LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18, link
  search at /opt/homebrew/lib + /usr/local/lib). Cross-platform
  is a future concern; right now LLVM 18 must be `brew install`-ed.

- **Mode B working conventions** (from Phase B onward): paste-
  direct zsh; anchor-guarded Python patches via `/tmp/`; no
  nested heredocs; when a Python script needs multi-line Rust
  text put it in a separate `cat > /tmp/foo.txt <<'RSEOF' …`
  block and `read_text()` it from Python (triple-quoted Python
  strings inside a bash heredoc can mangle terminals); cargo
  test -p <crate> after each patch.

### 0.2 Next session opening (C1.0c)

Resume at **C1.0c**: wrap `sentinel_codegen::compile_to_object`
as a `#[salsa::tracked]` query. The C1.0b session deliberately
deferred this because (a) the LLVM `Context` and `Module` types
hold a `'ctx` lifetime that doesn't trivially fit salsa's
`'static`-ish query model, and (b) `compile_to_object` writes to
the filesystem, which makes it a side-effectful "query" — salsa
usually expects pure functions. The C1.0b retrofit gave the
front-end the incremental shape it'll want anyway; C1.0c is its
own design question.

Sketches to evaluate:

1. **Salsa wraps just the path that builds the in-memory module,
   not the object emit.** A tracked `compile_module(db, file) ->
   ??` returns IR-as-bytes (or a serialised LLVM module — bitcode
   via `module.write_bitcode_to_memory()` is the obvious carrier);
   the driver does the object-emit + cc-link outside the query
   graph. This sidesteps the lifetime issue because the
   `Context`/`Module` live only inside the query body and the
   return value is owned bytes. Cost: bitcode roundtrip per
   query rerun even when caching wins; bitcode-as-cache-key
   means salsa diffs IR (which is what we want).

2. **Salsa doesn't wrap codegen at all; the codegen call stays a
   direct fn from the driver.** Defensible if the codegen pass is
   destined to be re-architected at C1.2 (when types arrive and
   codegen becomes a structural lowering of a typed HIR rather
   than the current name-resolved-AST → LLVM lump). Cost: codegen
   doesn't get incremental rebuild; LSP / `cargo check`-style
   tools that only need types-but-not-codegen are unaffected.

3. **Salsa wraps a `lower_to_ir` step but the final object emit
   stays out.** Splits codegen into an in-memory IR-build (tracked)
   and an object-emit (untracked). Mostly an organisational win.

C1.0c's first action is to write a short note (or ADR amendment)
choosing between these, since the choice cascades into the
sentinel-types and sentinel-resolve crates' query shapes at
C1.1 / C1.2. Tentative recommendation given the C0-style pure-
function discipline that survived through C1.0b: option 2 for
the immediate session (don't wrap codegen yet; revisit when
types land and we have a richer HIR to feed it). Filing this
note as the first C1.0c artifact is itself the first commit.

After C1.0c (whichever option) flips ADR 0011 to fully ACCEPTED
for D1, **C1.1** (sentinel-resolve crate lift — move name
resolution out of codegen) is the next sub-phase per ADR 0011 D6.

**Estimated effort for C1.0c**: 1 session if option 2 (writeup
only); 2-3 sessions if option 1 (bitcode plumbing + tests).

### 0.3 Quick-status block for session start

For pasting into a fresh chat to bootstrap context:

    Continuing Sentinel-lang work. Repo: https://github.com/arcanii/Sentinel-lang
    Local HEAD: <docs-hash> (docs: C1.0b landed at 557cc60).
    [N] commits ahead of origin/main pending GitHub Desktop push.
    Working tree clean.

    Phase A (broker) + Phase B (effects-proto) + Phase C0 (bootstrap
    compiler MVP) complete. 453 active workspace tests. C0 go/no-go
    program runs at tests/pass/c05_go_no_go.sentinel: stdout "10\n",
    exit 0. ADRs 0001-0010 ACCEPTED.

    Phase C1 in flight per ADR 0011 (PROPOSED, 8 sub-phases). C1.0a
    landed sentinel-base (Salsa db trait + SourceFile + Diagnostic
    accumulator) at 09dc8c3. C1.0b landed sentinel-syntax's
    salsa-tracked lex_query/parse_query plus the driver's
    SentinelDatabase wiring; the front-end now runs through Salsa.
    C1.0c is the next step: choose how (or whether) to wrap codegen
    as a salsa query — see HANDOVER §0.2 for the three sketches.

    Read docs/HANDOVER.md §0 in full, then docs/STATE.md, then
    ADR 0011 — that's the canonical context. Resume at C1.0c per
    HANDOVER §0.2.

---

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
