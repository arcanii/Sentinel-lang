# Sentinel-lang

**A security-first systems programming language for the threats of the 2030s.**

Sentinel is a memory-safe, capability-bounded systems language being built by [Anie Ltd.](https://aniesolutions.ai) to address the bug classes that dominate modern security incidents — supply-chain attacks, cryptographic side channels, secret disclosure, untrusted-code execution, and information-flow violations — none of which are structurally addressed by any production language today.

For the short-form pitch, see [`docs/SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md).
For the full design, see [`docs/SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) and
[`docs/SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md).

## Status

Sentinel is in active early-stage development. This is a multi-year research project; nothing here is production-ready. The implementation is staged into four phases (see [`docs/HANDOVER.md`](docs/HANDOVER.md) for the full rationale):

- [x] **Phase A — Memory Broker prototype** (Rust crate, complete)
  - [x] A0–A8: generational arenas, two allocation strategies, scoped budgets, stats and diagnostics, recording mode, secret-memory policy (mlock + zero-on-free), validation example programs
  - [x] A9: fallible builders, structured OS-error detail in `BrokerError`
- [x] **Phase B — Sentinel-Mini effects prototype** (tree-walking interpreter, complete)
  - [x] B0: lexer + recursive-descent parser + evaluator for a pure expression calculus (no types, no effects yet)
  - [x] B1: Hindley-Milner type inference, `letrec`, span-tracked diagnostics
  - [x] B2: effect rows and effect declarations
  - [x] B3: effect handlers (`handle … with …`)
  - [x] B4: `secret T` qualifier with constant-time check (B4.0 surface, B4.1 typing, B4.2 demos)
- [ ] **Phase C — Bootstrap compiler** (production Rust implementation of full Sentinel, targets LLVM)
- [ ] **Phase D — Self-hosting** (Sentinel compiler written in Sentinel)

Current test coverage:

- `sentinel-broker`:        69 tests + 1 doctest, clippy clean under `-D warnings`
- `sentinel-effects-proto`: 226 tests (203 lib + 23 integration), clippy clean under `-D warnings`

For the authoritative state of the codebase, see
[`docs/STATE.md`](docs/STATE.md). When STATE.md and any other document
disagree, STATE.md wins.

## What works today

The broker crate is feature-complete for Phase A and runnable. Three
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

The effects-proto crate (Sentinel-Mini) is complete through Phase B
and demonstrates the language-level security thesis: a tree-walking
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
- `crates/sentinel-effects-proto/` — Phase B: Sentinel-Mini interpreter.
- `crates/sentinel-{syntax,ast,resolve,types,hir,mir,codegen,driver,runtime,lsp}/` — Phase C scaffolds (stub crates).
- `docs/` — design, status, and process documents:
  - [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md) — one-page pitch
  - [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) and [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) — full design
  - [`HANDOVER.md`](docs/HANDOVER.md) — implementation plan and working norms
  - [`STATE.md`](docs/STATE.md) — current implementation state (source of truth)
  - [`BACKLOG.md`](docs/BACKLOG.md) — post-1.0 backlog and research directions
  - [`SECRETS_LIFECYCLE.md`](docs/SECRETS_LIFECYCLE.md) — secret-memory design
  - [`TIERED_RELEASES.md`](docs/TIERED_RELEASES.md) — release tiers
  - [`decisions/`](docs/decisions/) — architecture decision records (8 so far)
- `scripts/` — patch scripts that built each milestone, named `NN-<phase>.sh`.

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate for security-critical products targeting banks, governments, and regulated industries. Sentinel is open-source; the products built on top of it are Anie's commercial work.

## What this is not

- **Not production-ready.** The broker is a working Rust crate;
  Sentinel-the-language does not yet compile any real programs.
- **Not stable.** Every API in this repository can change at any time.
  No semver guarantees, no public release.
- **Not accepting general contributions yet.** The design is still
  fluid; a contributor onboarding process will come once the core
  shape stabilises after Phase B.
- **Not making security claims today.** Sentinel will eventually
  enforce strong security properties at the language layer. None of
  those properties exist yet for end-user code; only the broker's
  internal invariants are tested and enforced.

## License

MIT — see [`LICENSE.md`](LICENSE.md).
