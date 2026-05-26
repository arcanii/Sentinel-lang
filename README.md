# Sentinel-lang

**A security-first systems programming language for the threats of the 2030s.**

Sentinel is a memory-safe, capability-bounded systems language being built by [Anie Ltd.](https://aniesolutions.ai) to address the bug classes that dominate modern security incidents — supply-chain attacks, cryptographic side channels, secret disclosure, untrusted-code execution, and information-flow violations — none of which are structurally addressed by any production language today.

For the short-form pitch, see [`docs/SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md).
For the full design, see [`docs/SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) and
[`docs/SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md).
**To learn Sentinel programming today, start with the
[Programming Guide](docs/PROGRAMMING_GUIDE.md).**

## Status

Sentinel is in active early-stage development. This is a multi-year research project; nothing here is production-ready. The implementation is staged into four phases (see [`docs/HANDOVER.md`](docs/HANDOVER.md) for the full rationale):

- ✅ **Phase A — Memory Broker prototype** (Rust crate, complete)
  - ✅ A0–A8: generational arenas, two allocation strategies, scoped budgets, stats and diagnostics, recording mode, secret-memory policy (mlock + zero-on-free), validation example programs
  - ✅ A9: fallible builders, structured OS-error detail in `BrokerError`
- ✅ **Phase B — Sentinel-Mini effects prototype** (tree-walking interpreter, complete)
  - ✅ B0: lexer + recursive-descent parser + evaluator for a pure expression calculus (no types, no effects yet)
  - ✅ B1: Hindley-Milner type inference, `letrec`, span-tracked diagnostics
  - ✅ B2: effect rows and effect declarations
  - ✅ B3: effect handlers (`handle … with …`)
  - ✅ B4: `secret T` qualifier with constant-time check (B4.0 surface, B4.1 typing, B4.2 demos)
- 🟡 **Phase C — Bootstrap compiler** (production Rust implementation of full Sentinel, targets LLVM) — C0 + C1.0 + C1.1 complete; C1.2+ in flight
  - ✅ C0: end-to-end pipeline — lex → parse → AST → two-pass LLVM IR → object → cc-linked executable; six sub-phases C0.0–C0.5; everything is `i64` (no type system yet); the ADR 0010 go/no-go program runs end-to-end (`tests/pass/c05_go_no_go.sentinel`: stdout `10\n`, exit 0)
  - 🟡 C1: type system, regions, name-resolution lift, Salsa retrofit — in flight per ADR 0011 (8 sub-phases)
    - ✅ C1.0a: `sentinel-base` foundation crate (`SentinelDb` trait + `SourceFile` input + `Diagnostic` accumulator)
    - ✅ C1.0b: `lex_query` and `parse_query` as `#[salsa::tracked]` queries; driver instantiates concrete `SentinelDatabase`
    - ✅ C1.0c: codegen-salsa decision — codegen stays outside the query graph until C1.2+ typed HIR (ADR 0011 D1 amended)
    - ✅ C1.1: `sentinel-resolve` crate lift; name resolution out of codegen; driver pipeline = parse_query → resolve_query → codegen
    - 🟡 ADR 0012: concrete C1 surface syntax — annotation grammar (D1-D4) + bool/comparisons/logicals (D5-D8); PROPOSED (decision-only commit before C1.2/C1.3 implementation)
    - ⬜ C1.2: lexer `:` + annotation parsing + `sentinel-types::check()` real (per ADR 0012 D1-D4)
    - ⬜ C1.3: `bool`, `i32`, comparison + logical operators; retires ADR 0010 D9 C-style truthy (per ADR 0012 D5-D8)
    - ⬜ C1.4–C1.7: structs, `?T` nullability, arrays, generics
- ⬜ **Phase D — Self-hosting** (Sentinel compiler written in Sentinel)

Current test coverage (468 active tests across the workspace):

- `sentinel-broker`:        69 tests + 1 doctest, clippy clean under `-D warnings`
- `sentinel-effects-proto`: 226 tests (203 lib + 23 integration), clippy clean under `-D warnings`
- `sentinel-syntax`:        92 tests (90 lib + 2 UI integration) — includes 7 salsa-query tests at C1.0b
- `sentinel-ast`:           21 tests
- `sentinel-resolve`:       21 tests (positive paths + each error variant + 4 salsa query tests)
- `sentinel-codegen`:       8 tests (positive lowering only; name-resolution rejection tests moved to sentinel-resolve at C1.1.2)
- `sentinel-driver`:        22 pass-test fixtures (each compiles + runs a `.sentinel` program)
- `sentinel-runtime`:       2 tests
- `sentinel-base`:          3 tests (salsa machinery verification)

For the authoritative state of the codebase, see
[`docs/STATE.md`](docs/STATE.md). When STATE.md and any other document
disagree, STATE.md wins.

## What works today

**The Phase A broker crate** is feature-complete and runnable. Three
example programs exercise the full surface:

```bash
cargo run -p sentinel-broker --example token_bucket
cargo run -p sentinel-broker --example request_pipeline
cargo run -p sentinel-broker --example credential_store
```

The `credential_store` demo is the most concrete demonstration of
Sentinel's security thesis available today. It allocates credentials
into a slab arena with `mlock` + zero-on-free policy active, hex-dumps
the raw memory before and after `free()`, and verifies that the
64-byte slot is fully zeroed when the credential is released.

**The Phase B effects-proto crate (Sentinel-Mini)** is complete and
demonstrates the language-level security thesis: a tree-walking
interpreter with Hindley-Milner inference, row-polymorphic effect
tracking, deep effect handlers, and a `secret T` qualifier with a
static constant-time check. HANDOVER §5.2's three Phase B validation
deliverables — a supply-chain demo (effect-as-capability rejects an
auditing mismatch), an async-as-effect demo (same library function,
two handlers, two results), and a constant-time password-verify demo
(naively branching on a secret comparison fails to type-check) — all
live as integration tests:

```bash
cargo test -p sentinel-effects-proto pipeline_b42_
```

See [`crates/sentinel-effects-proto/tests/integration.rs`](crates/sentinel-effects-proto/tests/integration.rs)
for the demo programs and their commentary. The crate is intentionally
library-only and has no `examples/` directory; the demos are
regression-pinned by the test runner.

**The Phase C0 bootstrap compiler** compiles and runs real (if
small) Sentinel programs via LLVM. `snc` is the driver binary:

```bash
cargo build --workspace
cargo run --bin snc -- build tests/pass/c05_go_no_go.sentinel -o /tmp/c05_go_no_go
/tmp/c05_go_no_go     # stdout: "10", exit 0
```

The go/no-go program (the ADR 0010 acceptance fixture) exercises
function definitions, parameters, forward references, `if`/`else`,
`let`-bindings, arithmetic, and `print`:

```sentinel
fn double(x) { x * 2 }
fn pick(cond, a, b) { if cond { a } else { b } }
fn main() {
    let x = 5;
    let y = pick(x, double(x), 0);
    print(y)
}
```

Everything is `i64` (no type system yet); `bool` and friends arrive
at C1.3. As of C1.0b, the lex and parse pipeline stages run through
`#[salsa::tracked]` queries, with diagnostics flowing through a
salsa accumulator — the incremental-recompilation foundation that
the type-system work will sit on top of.

## Build

Requirements: Rust stable (1.80+), `cargo-nextest` recommended.

```bash
git clone https://github.com/arcanii/Sentinel-lang.git
cd Sentinel-lang
cargo build --workspace
cargo nextest run --workspace        # or `cargo test --workspace`
cargo clippy --workspace --all-targets -- -D warnings
```

## Repository layout

- `crates/sentinel-broker/` — Phase A deliverable, complete.
- `crates/sentinel-effects-proto/` — Phase B: Sentinel-Mini interpreter, complete.
- `crates/sentinel-base/` — Phase C1.0a: shared Salsa db trait + inputs + diagnostic accumulator.
- `crates/sentinel-ast/` — Phase C: AST types (`Program`, `FnDef`, `Block`, `Stmt`, `Expr`, …).
- `crates/sentinel-syntax/` — Phase C: lexer + hand-written recursive-descent parser + salsa-tracked queries.
- `crates/sentinel-codegen/` — Phase C: LLVM IR lowering via inkwell.
- `crates/sentinel-runtime/` — Phase C: minimal runtime (`sentinel_print`) linked into compiled binaries.
- `crates/sentinel-driver/` — Phase C: the `snc` compiler driver binary.
- `crates/sentinel-resolve/` — Phase C1.1: name resolution; produces a `ResolvedProgram` with `VarId`/`FnId` stable identifiers.
- `crates/sentinel-{types,hir,mir,lsp}/` — Phase C1.2+ scaffolds (stub crates; populated in pipeline order).
- `docs/` — design, status, and process documents:
  - [`PROGRAMMING_GUIDE.md`](docs/PROGRAMMING_GUIDE.md) — intro to writing Sentinel programs today (C0 surface)
  - [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md) — one-page pitch
  - [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) and [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) — full design
  - [`HANDOVER.md`](docs/HANDOVER.md) — implementation plan and working norms
  - [`STATE.md`](docs/STATE.md) — current implementation state (source of truth)
  - [`BACKLOG.md`](docs/BACKLOG.md) — post-1.0 backlog and research directions
  - [`SECRETS_LIFECYCLE.md`](docs/SECRETS_LIFECYCLE.md) — secret-memory design
  - [`TIERED_RELEASES.md`](docs/TIERED_RELEASES.md) — release tiers
  - [`decisions/`](docs/decisions/) — architecture decision records (12 so far)
- `tests/pass/` — workspace-root pass-test fixtures (`.sentinel` programs that should compile and run).
- `tests/ui/` — workspace-root UI-test fixtures (`.sentinel` programs that should produce specific diagnostics).
- `scripts/` — patch scripts that built each milestone, named `NN-<phase>.sh`.

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate for security-critical products targeting banks, governments, and regulated industries. Sentinel is open-source; the products built on top of it are Anie's commercial work.

## What this is not

- **Not production-ready.** The broker is a working Rust crate and
  the C0 bootstrap compiler runs small Sentinel programs end-to-end,
  but the language has no type system yet (C1 brings it up) and
  none of Sentinel's security properties exist at the language
  layer today.
- **Not stable.** Every API in this repository can change at any time.
  No semver guarantees, no public release.
- **Not accepting general contributions yet.** The design is still
  fluid; a contributor onboarding process will come once the core
  shape stabilises after Phase C.
- **Not making security claims today.** Sentinel will eventually
  enforce strong security properties at the language layer. None of
  those properties exist yet for end-user code; only the broker's
  internal invariants are tested and enforced.

## License

MIT — see [`LICENSE.md`](LICENSE.md).
