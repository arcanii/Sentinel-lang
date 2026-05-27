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
- 🟡 **Phase C — Bootstrap compiler** (production Rust implementation of full Sentinel, targets LLVM) — C0 + all of C1 (C1.0 through C1.7) functionally complete
  - ✅ C0: end-to-end pipeline — lex → parse → AST → two-pass LLVM IR → object → cc-linked executable; six sub-phases C0.0–C0.5; everything is `i64` (no type system yet); the ADR 0010 go/no-go program runs end-to-end (`tests/pass/c05_go_no_go.sentinel`: stdout `10`, exit 0)
  - ✅ C1: type system, name-resolution lift, Salsa retrofit — eight sub-phases C1.0 through C1.7 all landed per ADR 0011
    - ✅ C1.0: Salsa retrofit — `sentinel-base` foundation (`SentinelDb` + `SourceFile` + `Diagnostic` accumulator); `lex_query` / `parse_query` as `#[salsa::tracked]` queries; codegen intentionally stays outside the query graph (ADR 0011 D1 amended)
    - ✅ C1.1: `sentinel-resolve` crate lift; name resolution out of codegen; driver pipeline = `parse_query → resolve_query → codegen`
    - ✅ C1.2: lexer `:` + annotation grammar + `sentinel-types::check()` real; pipeline becomes `parse_query → resolve_query → check_query → codegen`; type universe is `I64` only
    - ✅ C1.3: `bool` + `i32` primitives, comparison and logical operators, ADR 0010 D9 C-style truthy retires; c05 go/no-go rewritten in the new shape
    - ✅ C1.4: structs (`struct Point { x: i64, y: i64 }`) + field access + struct literals; codegen value type widens from `IntValue` to `BasicValueEnum`; c14 go/no-go (`stdout "7"`)
    - ✅ C1.5: `?T` nullable types + `null` literal + `unwrap_or` / `is_some` builtins + bidirectional checking; c15 go/no-go (`stdout "142"`)
    - ✅ C1.6: arrays (`[T]`) + indexing (`a[i]`) + `len` builtin + heap-allocation runtime (`sentinel_alloc` / `sentinel_panic_oob`); recursive structs via `?T` heap indirection unlock the ADR 0014 D10 deferral; c16 go/no-go (`stdout "15"`)
    - ✅ C1.7: witness-table generics (generic fns + generic structs + monomorphisation); generic builtins re-route through the unified inference path; c17 go/no-go (`stdout "42"`)
- ⬜ **Phase D — Self-hosting** (Sentinel compiler written in Sentinel)

Current test coverage (798 active tests + 1 doctest across the workspace):

- `sentinel-broker`:        69 tests + 1 doctest, clippy clean under `-D warnings`
- `sentinel-effects-proto`: 226 tests (203 lib + 23 integration), clippy clean under `-D warnings`
- `sentinel-syntax`:        214 tests (212 lib + 2 UI integration) — includes lexer + parser tests for the full C0/C1.0-C1.7 surface
- `sentinel-ast`:           42 tests
- `sentinel-resolve`:       43 tests (positive paths + each error variant + salsa query smoke + C1.7 type-param tracking)
- `sentinel-types`:         101 tests — full coverage of the C1.7 type universe `{ I64, I32, Bool, Struct, Nullable(NullableInner), Array(ArrayElem), TypeParam, GenericInstance }`
- `sentinel-codegen`:       41 tests
- `sentinel-driver`:        53 pass-test fixtures (each compiles + runs a `.sentinel` program)
- `sentinel-runtime`:       2 tests
- `sentinel-base`:          3 tests (salsa machinery verification)
- `sentinel-{hir,mir,lsp}`: 1 stub-smoke test each

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

**The Phase C bootstrap compiler** compiles and runs real Sentinel
programs via LLVM. `snc` is the driver binary; five go/no-go
fixtures exercise the full C0 + C1 surface end-to-end:

```bash
cargo build --workspace
for f in c05 c14 c15 c16 c17; do
  cargo run --bin snc --quiet -- build tests/pass/${f}_go_no_go.sentinel -o /tmp/${f}
  /tmp/${f}     # c05→"10", c14→"7", c15→"142", c16→"15", c17→"42"
done
```

The C1.7 phase-go (ADR 0016 D12) exercises every C1 type-system
feature — generic structs, generic fns, type-arg inference at call
sites, bidirectional struct-literal context, field access through
type-arg substitution, and codegen monomorphisation:

```sentinel
struct Pair<A, B> { first: A, second: B }

fn make_pair<A, B>(a: A, b: B) -> Pair<A, B> {
    Pair { first: a, second: b }
}

fn fst<A, B>(p: Pair<A, B>) -> A { p.first }
fn snd<A, B>(p: Pair<A, B>) -> B { p.second }

fn pick_int(use_first: bool, p: Pair<i64, i64>) -> i64 {
    if use_first { fst(p) } else { snd(p) }
}

fn main() -> i64 {
    let p: Pair<i64, i64> = make_pair(7, 35);
    print(pick_int(true, p) + pick_int(false, p))
}
// stdout "42", exit 0
```

Each C1 sub-phase has its own go/no-go demonstrating the
delta:

- **c05** (C0): fn definitions, `if`/`else`, `let`, arithmetic,
  `print`; everything `i64`.
- **c14** (C1.4): structs with named fields, field access.
- **c15** (C1.5): `?T` nullables, `null` literal, `unwrap_or` /
  `is_some` builtins, bidirectional checking.
- **c16** (C1.6): heap-backed arrays, indexing, `len` builtin,
  recursive structs through `?T` heap indirection.
- **c17** (C1.7): generic fns + generic structs +
  monomorphisation.

As of C1.0, the lex / parse / resolve / check pipeline stages run
through `#[salsa::tracked]` queries with diagnostics flowing
through a salsa accumulator — the incremental-recompilation
foundation that LSP and refactoring tooling will build on.

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
- `crates/sentinel-base/` — C1.0a: shared Salsa db trait + `SourceFile` input + `Diagnostic` accumulator.
- `crates/sentinel-ast/` — Phase C: AST types (`Program`, `FnDef`, `StructDecl`, `TypeParam`, `Block`, `Stmt`, `Expr`, `TypeExpr`, …).
- `crates/sentinel-syntax/` — Phase C: lexer + hand-written recursive-descent parser + salsa-tracked `lex_query` / `parse_query`.
- `crates/sentinel-codegen/` — Phase C: LLVM IR lowering via inkwell; C1.5+ heap-backed `?Struct` indirection; C1.7 monomorphic emission for generic fns + generic structs.
- `crates/sentinel-runtime/` — Phase C: minimal runtime (`sentinel_print`, `sentinel_alloc`, `sentinel_panic_oob`) linked into compiled binaries.
- `crates/sentinel-driver/` — Phase C: the `snc` compiler driver binary.
- `crates/sentinel-resolve/` — C1.1: name resolution; produces a `ResolvedProgram` with `VarId`/`FnId`/`StructId`/`TypeParamId` stable identifiers.
- `crates/sentinel-types/` — C1.2+: type checking; produces a `TypedProgram` with `Type` on every expression, plus an interned `generic_instances` table for C1.7 generic-struct instances.
- `crates/sentinel-{hir,mir,lsp}/` — scaffolds for later phases (HIR for trait/protocol work, MIR for optimization passes, LSP for editor integration).
- `docs/` — design, status, and process documents:
  - [`PROGRAMMING_GUIDE.md`](docs/PROGRAMMING_GUIDE.md) — intro to writing Sentinel programs today
  - [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md) — one-page pitch
  - [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) and [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) — full design
  - [`HANDOVER.md`](docs/HANDOVER.md) — implementation plan and working norms
  - [`STATE.md`](docs/STATE.md) — current implementation state (source of truth)
  - [`BACKLOG.md`](docs/BACKLOG.md) — post-1.0 backlog and research directions
  - [`SECRETS_LIFECYCLE.md`](docs/SECRETS_LIFECYCLE.md) — secret-memory design
  - [`TIERED_RELEASES.md`](docs/TIERED_RELEASES.md) — release tiers
  - [`decisions/`](docs/decisions/) — architecture decision records (16 so far)
- `tests/pass/` — workspace-root pass-test fixtures (`.sentinel` programs that should compile and run).
- `tests/ui/` — workspace-root UI-test fixtures (`.sentinel` programs that should produce specific diagnostics).
- `scripts/` — patch scripts that built each milestone, named `NN-<phase>.sh`.

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate for security-critical products targeting banks, governments, and regulated industries. Sentinel is open-source; the products built on top of it are Anie's commercial work.

## What this is not

- **Not production-ready.** The broker is a working Rust crate and
  the C0 + C1 bootstrap compiler runs Sentinel programs (with a
  full type system) end-to-end, but Phase C2's region work
  (references, mutability, lifetimes) and Phase C3's effect-system
  integration are still ahead — none of Sentinel's
  effect-as-capability or constant-time security properties exist
  at the production-compiler layer today (they're validated in the
  Phase B Sentinel-Mini interpreter).
- **Not stable.** Every API in this repository can change at any time.
  No semver guarantees, no public release.
- **Not accepting general contributions yet.** The design is still
  fluid; a contributor onboarding process will come once the core
  shape stabilises after Phase C.
- **Not making security claims today.** Sentinel will eventually
  enforce strong security properties at the language layer. None of
  those properties exist yet for end-user code in the bootstrap
  compiler; only the broker's internal invariants and Sentinel-
  Mini's effect / secret-qualifier checks are tested and enforced.

## License

MIT — see [`LICENSE.md`](LICENSE.md).
