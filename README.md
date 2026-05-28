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
  - Generational arenas, two allocation strategies (bump + slab), scoped budgets, stats and diagnostics, recording mode, secret-memory policy (mlock + zero-on-free), fallible builders + structured OS-error detail.
- ✅ **Phase B — Sentinel-Mini effects prototype** (tree-walking interpreter, complete)
  - Hindley-Milner type inference, `letrec`, row-polymorphic effect tracking, deep effect handlers (`handle … with …`), `secret T` qualifier with constant-time check. Three HANDOVER §5.2 validation demos live as integration tests (supply-chain, async-as-effect, password-verify).
- 🟡 **Phase C — Bootstrap compiler** (production Rust implementation of full Sentinel, targets LLVM)
  - ✅ **C0** — end-to-end pipeline: lex → parse → AST → two-pass LLVM IR → object → cc-linked executable. Six sub-phases C0.0–C0.5. Everything is `i64` (no type system yet).
  - ✅ **C1** — type system, name-resolution lift, Salsa retrofit. Eight sub-phases C1.0 through C1.7. Type universe at C1 close: `{ I64, I32, Bool, Struct, Nullable, Array, TypeParam, GenericInstance }`. ADRs 0011 + 0012 + 0013 + 0014 + 0015 + 0016 all ACCEPTED.
  - ✅ **C2** — references + mutability + lexical borrow checker + RAII drop. Six sub-phases C2.0.1 through C2.5. Type universe widens with `Ref(RefId)`. ADR 0017 ACCEPTED-WITH-AMENDMENTS; ADR 0018 (Polonius migration plan) PROPOSED.
  - ✅ **C3 typing layer** — secret typing + effect rows + new `sentinel-effect-check` crate. Six sub-phases C3.0(a) through C3.3. Type universe widens with `Secret(SecretId)` (sixth interner-table ADR running). ADR 0019 ACCEPTED-WITH-AMENDMENTS.
  - 🟡 **C3 runtime layer** — handler runtime + `perform` lowering per ADR 0020 (PROPOSED). Sub-phases C3.4–C3.7. **In flight.**
- ⬜ **Phase D — Self-hosting** (Sentinel compiler written in Sentinel)

Current test coverage (**1008 active tests + 1 doctest** across the workspace):

- `sentinel-broker`:        69 tests + 1 doctest, clippy clean under `-D warnings`
- `sentinel-effects-proto`: 226 tests (203 lib + 23 integration), clippy clean
- `sentinel-syntax`:        274 tests (272 lib + 2 UI integration) — full C0/C1/C2/C3.0 lexer + parser surface
- `sentinel-ast`:           50 tests
- `sentinel-resolve`:       50 tests — incl. C2 mutability + C3.2 effect-decl resolution
- `sentinel-types`:         147 tests — full coverage of the C3.1 type universe with `Type::Secret(SecretId)`
- `sentinel-borrow-check`:  79 tests — lexical shared-XOR-mutable + move semantics + DropPlan
- `sentinel-effect-check`:  8 tests — fixed-point effect inference + annotation check + `fn main` invariant
- `sentinel-codegen`:       62 tests — incl. C2.5(a) recursive struct-field drop + C3.1 Secret/declassify lowering
- `sentinel-driver`:        63 pass-test fixtures (each compiles + runs a `.sentinel` program)
- `sentinel-runtime`:       4 tests — `sentinel_alloc` + `sentinel_panic_oob` + `sentinel_free`
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
demos — supply-chain (effect-as-capability rejects an auditing
mismatch), async-as-effect (same library function, two handlers, two
results), and constant-time password-verify (branching on a secret
fails to type-check) — all live as integration tests:

```bash
cargo test -p sentinel-effects-proto pipeline_b42_
```

**The Phase C bootstrap compiler** compiles and runs real Sentinel
programs via LLVM. `snc` is the driver binary; **thirteen** go/no-go
fixtures exercise the full C0 + C1 + C2 + C3 surface end-to-end:

```bash
cargo build --workspace
for f in c05 c14 c15 c16 c17 c20 c21 c22 c23 c24 c25 c31 c32 c33; do
  cargo run --bin snc --quiet -- build tests/pass/${f}_go_no_go.sentinel -o /tmp/${f}
  /tmp/${f}
done
# c05→"10", c14→"7", c15→"142", c16→"15", c17→"42",
# c20→"53", c21→"168", c22→"35", c23→"100", c24→"160",
# c25→"190", c31→"100", c32→"42", c33→"42"
```

Each go/no-go demonstrates a sub-phase's delta:

| Fixture | Sub-phase | Surface                                                   | Stdout |
|---------|-----------|-----------------------------------------------------------|--------|
| c05     | C0        | fn definitions, `if`/`else`, `let`, arithmetic, `print`  | `10`   |
| c14     | C1.4      | structs with named fields, field access                  | `7`    |
| c15     | C1.5      | `?T` nullables, `null`, `unwrap_or` / `is_some`          | `142`  |
| c16     | C1.6      | heap-backed arrays, indexing, `len`, recursive structs via `?T` | `15` |
| c17     | C1.7      | generic fns + generic structs + monomorphisation         | `42`   |
| c20     | C2.0.2    | `&T` / `&mut T` refs, `let mut`, `*r` deref, assignment  | `53`   |
| c21     | C2.1      | shared-only lexical borrow check                         | `168`  |
| c22     | C2.2      | `&mut` + shared-XOR-mutable rule                         | `35`   |
| c23     | C2.3      | move semantics + use-after-move                          | `100`  |
| c24     | C2.4      | RAII / drop + `sentinel_free` (closes the C1.6+ heap leak) | `160` |
| c25     | C2.5      | recursive struct-field drop + full Phase C2 surface       | `190`  |
| c31     | C3.1      | `Type::Secret` + `declassify` + operator-secret-preserving | `100` |
| c32     | C3.2      | effect declarations + annotated fn chain                 | `42`   |
| c33     | C3.3      | full Phase C3 typing-layer surface                       | `42`   |

The c33 phase-go program is the most compact demonstration of the
full Sentinel surface available today:

```sentinel
effect Io {
    log(msg: i64) -> i64;
}

fn maybe_log(x: i64) -> i64 ! { Io } {
    x + 1
}

fn process(stored: secret i64, guess: secret i64) -> i64 {
    let doubled: secret i64 = stored + stored;
    let matched: secret bool = stored == guess;
    if declassify(matched) {
        declassify(doubled) - 42
    } else {
        0
    }
}

fn main() -> i64 {
    let s: secret i64 = 42;
    let g: secret i64 = 42;
    print(process(s, g))
}
// stdout "42", exit 0
```

It exercises (a) top-level `effect Io { ... }` declaration, (b) postfix `! { Io }` effect annotation on `maybe_log` (not called from main, so `fn main` stays effect-free per ADR 0019 D13), (c) implicit `T → secret T` widening at let-annotation boundaries, (d) operator-secret-preserving (`secret i64 + secret i64 → secret i64`, `secret == secret → secret bool`), (e) `declassify(e)` stripping the secret wrapper to enable `if`-branching, and (f) the full salsa pipeline `parse → resolve → check → effect_check → borrow_check → codegen` with diagnostics flowing through.

As of C1.0+, the front-end stages run through `#[salsa::tracked]` queries with diagnostics flowing through a salsa accumulator — the incremental-recompilation foundation that LSP and refactoring tooling will build on.

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
- `crates/sentinel-ast/` — Phase C: AST types (`Program`, `FnDef`, `StructDecl`, `EffectDecl`, `TypeParam`, `Block`, `Stmt`, `Expr`, `TypeExpr`, …).
- `crates/sentinel-syntax/` — Phase C: lexer + hand-written recursive-descent parser + salsa-tracked `lex_query` / `parse_query`.
- `crates/sentinel-resolve/` — C1.1: name resolution; produces a `ResolvedProgram` with `VarId`/`FnId`/`StructId`/`TypeParamId`/`EffectId` stable identifiers.
- `crates/sentinel-types/` — C1.2+: type checking; produces a `TypedProgram` with `Type` on every expression, plus interned `generic_instances` / `refs` / `secrets` / `effect_decls` tables.
- `crates/sentinel-borrow-check/` — C2.1+: lexical borrow checker; emits a `DropPlan` for codegen's scope-exit drop emission.
- `crates/sentinel-effect-check/` — C3.2+: effect-row inference + annotation check + `fn main`-must-be-effect-free invariant.
- `crates/sentinel-codegen/` — Phase C: LLVM IR lowering via inkwell; C1.5+ heap-backed `?Struct` indirection; C1.7 monomorphic emission for generic fns + generic structs; C2.4+ scope-exit drop; C3.1+ secret-as-identity lowering.
- `crates/sentinel-runtime/` — Phase C: minimal runtime (`sentinel_print`, `sentinel_alloc`, `sentinel_panic_oob`, `sentinel_free`) linked into compiled binaries.
- `crates/sentinel-driver/` — Phase C: the `snc` compiler driver binary.
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
  - [`borrow-check-limitations.md`](docs/borrow-check-limitations.md) — known C2 borrow-check over-rejections + the partial-move-through-field-projection soundness gap
  - [`decisions/`](docs/decisions/) — architecture decision records (20 so far)
- `tests/pass/` — workspace-root pass-test fixtures (`.sentinel` programs that should compile and run).
- `tests/ui/` — workspace-root UI-test fixtures (`.sentinel` programs that should produce specific diagnostics).
- `scripts/` — patch scripts that built each milestone, named `NN-<phase>.sh`.

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate for security-critical products targeting banks, governments, and regulated industries. Sentinel is open-source; the products built on top of it are Anie's commercial work.

## What this is not

- **Not production-ready.** The broker is a working Rust crate and the bootstrap compiler runs Sentinel programs with a full type system, references + borrow check + RAII drop (Phase C2), and secret + effect typing (Phase C3 typing layer). Still ahead: Phase C3 runtime layer (handler dispatch per ADR 0020), Phase C4 traits + structured concurrency, Phase D self-hosting. Sentinel's constant-time codegen + speculation-barrier insertion are deferred to a post-C3 codegen ADR; the C3 static check only catches the typing-layer cases.
- **Not stable.** Every API in this repository can change at any time. No semver guarantees, no public release.
- **Not accepting general contributions yet.** The design is still fluid; a contributor onboarding process will come once the core shape stabilises after Phase C.
- **Not making security claims today.** Sentinel will eventually enforce strong security properties at the language layer. The constant-time check ships at the typing layer (rejects `if` on `secret bool`, division by a secret, etc.); the codegen-layer guarantees (branch-free comparisons, `cmov` selection, speculation barriers) are a follow-on ADR.

## License

MIT — see [`LICENSE.md`](LICENSE.md).
