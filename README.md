# Sentinel-lang

**A security-first systems programming language for the threats of the 2030s.**

Sentinel is a memory-safe, capability-bounded systems language being built by [Anie Ltd.](https://aniesolutions.ai) to address the bug classes that dominate modern security incidents — supply-chain attacks, cryptographic side channels, secret disclosure, untrusted-code execution, and information-flow violations — none of which are structurally addressed by any production language today.

For the short-form pitch, see [`docs/SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md).
For the full design, see [`docs/SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) and
[`docs/SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md).
**To learn Sentinel programming today, start with the
[Programming Guide](docs/PROGRAMMING_GUIDE.md).**

> **Is the compiler written in Sentinel?** **Now yes — it self-hosts.** The
> original compiler (`snc`) is a **Rust** bootstrap compiler (~15 crates) that
> lowers Sentinel to native code via LLVM. In **Phase D** the entire compiler
> pipeline — lexer → parser → resolve → type-check → effect-check →
> borrow-check → MIR + constant-time verify → codegen — has been **rewritten in
> Sentinel** (`selfhost/*.sentinel`, ~20k lines), each stage differentially
> validated *byte-for-byte* against the Rust `snc`, and it reaches the
> **bootstrap fixed point**: the Sentinel-built compiler compiles its own
> source to LLVM IR identical to the Rust oracle, and the resulting binary
> reproduces itself. The Rust `snc` remains the bootstrap seed and the
> differential reference oracle; full-corpus codegen parity is the remaining
> work (see Phase D below).

## Status

Sentinel is in active development. This is a multi-year research project; nothing here is production-ready. The implementation is staged into four phases (see [`docs/HANDOVER.md`](docs/HANDOVER.md) for the full rationale):

- ✅ **Phase A — Memory Broker prototype** (Rust crate, complete)
  - Generational arenas, two allocation strategies (bump + slab), scoped budgets, stats and diagnostics, recording mode, secret-memory policy (mlock + zero-on-free).
- ✅ **Phase B — Sentinel-Mini effects prototype** (tree-walking interpreter, complete)
  - Hindley-Milner inference, row-polymorphic effect tracking, deep effect handlers, `secret T` with a constant-time check. Validates the design before the production compiler commits.
- ✅ **Phase C — Bootstrap compiler** (production Rust implementation, targets LLVM) — **complete; closed at Sentinel 1.0 (2026-05-30).** C0–C4 + C5 (productionization):
  - ✅ **C0** — end-to-end pipeline: lex → parse → AST → two-pass LLVM IR → object → linked executable.
  - ✅ **C1** — type system + name resolution + Salsa retrofit + witness-table generics (generic fns + structs + monomorphisation).
  - ✅ **C2** — references + mutability + lexical borrow checker (shared-XOR-mutable, move semantics) + RAII drop.
  - ✅ **C3** — `secret` typing + effect rows + the algebraic-effect **handler runtime** (`effect` / `perform` / `handle … with`, deep handlers, continuations).
  - ✅ **C4** — classes + methods + `init` + traits + impls + **delegation** auto-forwarders + **structured concurrency** (`scope`/`spawn`/`await`).
  - 🟢 **C5 — productionization toward 1.0:** an HIR/MIR analysis pipeline; **constant-time `secret` codegen, delivered and *machine-verified*** (a MIR pass rejects any `secret` reaching a branch, a memory index, or a division divisor — `sentinel::mir::secret_leak`); bitwise operators `& | ^`; the **Phase A broker wired into compiled programs** as per-scope bump arenas (scope-exit bulk free); a **defined, frozen, layout-tested ABI** (`abi-v1`); and the **1.0 acceptance program — a constant-time TLS-1.3-handshake-shaped go/no-go that passes the verification** (the close bar). **Sentinel 1.0 is declared** (ADR 0025 + 0030 → ACCEPTED) — the bootstrap-compiler milestone, *not* a production release (still single-process, single-file, research-stage).
- 🟢 **Phase D — Self-hosting** (the Sentinel compiler rewritten in Sentinel) — **the bootstrap fixed point is reached; the Sentinel compiler compiles itself.** Two movements:
  - ✅ **Movement 1 — language + stdlib build-out** (ADR 0031–0037): the 1.0 language couldn't self-host (no sum types/`match`, strings, growable collections, file I/O, loops, or modules — all of which a compiler needs), so Phase D first grew the language. **D.1** sum types + `match`, **D.2** strings + `u8`, **D.3** growable `Vec<T>`, **D.4** file I/O, **D.5** `while`/`break`/`continue`, **D.6** modules / multi-file (`use`) — all landed.
  - 🟢 **Movement 2 — the port** (ADR 0038–0045): every stage of `snc` is reimplemented in Sentinel (`selfhost/*.sentinel`) and validated **byte-for-byte** against the Rust `snc` via a per-stage dump oracle (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`/`llvm`) over the whole corpus. The whole pipeline through codegen is ported, and **`scg` (the Sentinel-built compiler) discovers, merges, and lowers its own multi-module source to LLVM IR byte-identical to the Rust oracle — then `cc`-ing that IR yields a binary that re-emits the same IR (a true fixed point), leak-free.** Remaining: full-corpus codegen parity (the exotic constructs the corpus exercises but the self-hosting compiler doesn't itself use — effects/handlers, generics, classes, nullable — are being covered slice by slice) → ADR 0045 ACCEPTED. The Rust bootstrap stays as the seed + oracle. (The per-unit separate-compilation back end from ADR 0037 is an independent deferred track.)

**1476 tests across the workspace**, four-check green (build · `cargo nextest` · doctests · `clippy -D warnings`). For the authoritative, per-crate state of the codebase — and the per-crate test breakdown — see [`docs/STATE.md`](docs/STATE.md). When STATE.md and any other document disagree, STATE.md wins.

## What works today

**The Phase A broker crate** is feature-complete and runnable:

```bash
cargo run -p sentinel-broker --example token_bucket
cargo run -p sentinel-broker --example request_pipeline
cargo run -p sentinel-broker --example credential_store
```

The `credential_store` demo is a concrete demonstration of Sentinel's security thesis: it allocates credentials into a slab arena with `mlock` + zero-on-free active, hex-dumps the raw memory before and after `free()`, and verifies the slot is fully zeroed when the credential is released.

**The Phase C bootstrap compiler** compiles and runs real Sentinel programs via LLVM. `snc` is the driver binary; 21 `*_go_no_go.sentinel` fixtures (C0 through C5) exercise the surface end-to-end, each compiling + running and asserting its exit code / stdout:

```bash
cargo build --workspace
./target/debug/snc build tests/pass/c4_go_no_go.sentinel -o /tmp/c4 && /tmp/c4; echo $?   # 42
./target/debug/snc build tests/pass/c5_go_no_go.sentinel -o /tmp/c5 && /tmp/c5; echo $?   # 42
```

### The headline capability: constant-time `secret`, machine-verified

Sentinel's most novel guarantee is delivered. A `secret`-typed value may not influence control flow, memory addressing, or division — the timing channels behind cryptographic side-channel attacks — and the compiler **proves it** during `snc build` (a MIR pass lowers the program and rejects any leak before codegen). Branch-free `secret` computation, using the sanctioned bitwise/arithmetic primitives, passes:

```sentinel
// A constant-time equality over secrets: XOR each pair, OR-reduce, then
// declassify the accumulator (0 iff every pair matched). Branch-free, so
// it PASSES the constant-time verification — no secret reaches a branch,
// a memory index, or a divisor. This is the real shape of a TLS
// `Finished` MAC verify.
fn ct_eq(a0: secret i64, a1: secret i64, b0: secret i64, b1: secret i64) -> i64 {
    let d0 = a0 ^ b0;
    let d1 = a1 ^ b1;
    declassify(d0 | d1)
}

fn main() -> i64 {
    let x0: secret i64 = 42;
    let x1: secret i64 = 7;
    let diff = ct_eq(x0, x1, x0, x1);   // compare (x0,x1) to itself → 0
    42 - diff                            // exit 42
}
```

Writing `if declassify(...)` is fine; writing `if (someSecretBool)`, indexing `a[someSecret]`, or dividing by a secret is a compile error. The **1.0 acceptance program** (`tests/pass/c5_go_no_go.sentinel`) builds this up into a TLS-1.3-handshake-*shaped* program — a state-machine class + a cipher-suite trait + I/O-as-effects + a constant-time Montgomery-ladder step, HKDF-shaped key derivation, and the `Finished` verify above — that compiles, runs, and passes the constant-time check end-to-end.

As of C1.0+, the front-end runs through `#[salsa::tracked]` queries with diagnostics flowing through a salsa accumulator — the incremental-recompilation foundation for editor tooling.

### Self-hosting: the compiler compiles itself

The `selfhost/` directory holds the compiler **rewritten in Sentinel** — every stage, ~20k lines of `.sentinel`. Each stage carries a Rust dump-oracle subcommand (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`/`llvm`) and a differential test asserts the Sentinel stage produces **byte-identical** output to the Rust `snc` over the whole `tests/pass` + `tests/ui` corpus — a mismatch means the port is wrong, and the Rust stage is ground truth:

```bash
# Each stage matches its Rust oracle byte-for-byte over the corpus:
cargo nextest run -p sentinel-driver sentinel_typer_matches_oracle_on_corpus
cargo nextest run -p sentinel-driver sentinel_codegen_matches_oracle_on_corpus

# The capstone: the Sentinel-built compiler lowers its OWN multi-module source
# to LLVM IR identical to the oracle, then reproduces itself — a fixed point.
cargo nextest run -p sentinel-driver \
  sentinel_codegen_self_merges_the_compiler_and_reaches_fixed_point
```

The Rust `snc` is still what bootstrap-builds the Sentinel compiler the first time and serves as the differential reference; the LLVM toolchain (`clang`/`llc`) is external to both. Full-corpus codegen parity — covering constructs the corpus exercises but the self-hosting compiler doesn't itself use — is the remaining work toward closing Phase D.

## Build

Requirements: **macOS on Apple Silicon** (the toolchain is pinned to `aarch64-apple-darwin`), **Rust stable** (pinned via `rust-toolchain.toml`), and **LLVM 18** (`brew install llvm@18` — inkwell builds against `llvm18-0`; `.cargo/config.toml` points at the Homebrew prefix). `cargo-nextest` recommended.

```bash
git clone https://github.com/arcanii/Sentinel-lang.git
cd Sentinel-lang
cargo build --workspace
cargo nextest run --workspace        # or `cargo test --workspace`
cargo test --doc --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Repository layout

- `crates/sentinel-broker/` — Phase A deliverable (generational arenas, budgets, secret-memory policy). Now also backs compiled programs as scope arenas (C5.4).
- `crates/sentinel-effects-proto/` — Phase B: Sentinel-Mini interpreter, complete.
- `crates/sentinel-base/` — shared Salsa db trait + `SourceFile` input + `Diagnostic` accumulator.
- `crates/sentinel-ast/` — AST types.
- `crates/sentinel-syntax/` — lexer + recursive-descent parser + salsa-tracked `lex_query` / `parse_query`.
- `crates/sentinel-resolve/` — name resolution; `VarId`/`FnId`/`StructId`/`ClassId`/`ImplId`/… stable identifiers.
- `crates/sentinel-types/` — type checking; produces a `TypedProgram` (the `Type` interner covers I64/I32/Bool/Struct/Nullable/Array/TypeParam/GenericInstance/Ref/Secret/Kont/Task/Class/TraitSelf).
- `crates/sentinel-borrow-check/` — lexical borrow checker; emits the `DropPlan` for scope-exit drops.
- `crates/sentinel-effect-check/` — effect-row inference + annotation check + the `fn main`-must-be-effect-free invariant.
- `crates/sentinel-hir/` — the typed→codegen seam (`lower_to_hir`); the thick HIR desugar is a post-1.0 follow-on.
- `crates/sentinel-mir/` — minimal SSA/CFG (`lower_to_mir`) hosting the **constant-time verification pass** (`verify_constant_time`), wired into `snc`.
- `crates/sentinel-codegen/` — LLVM IR lowering via inkwell: structs/arrays/nullables/generics/refs/secret, RAII drop, the handler runtime, classes/traits/delegation, structured concurrency, and broker scope-arena routing. Freezes the `abi-v1` layout/mangling/symbol contract (layout-stability tests).
- `crates/sentinel-runtime/` — the runtime linked into compiled binaries (`sentinel_print`/`alloc`/`free`/`panic_oob`, the broker scope-arena C-ABI, the handler-continuation runtime, the structured-concurrency scheduler).
- `crates/sentinel-driver/` — the `snc` compiler driver binary + the pass/UI/repro test suites.
- `crates/sentinel-lsp/` — editor-integration scaffold (stub; ADR 0025 D10).
- `docs/` — design, status, and process documents:
  - [`PROGRAMMING_GUIDE.md`](docs/PROGRAMMING_GUIDE.md) — intro to writing Sentinel programs today
  - [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md) — one-page pitch
  - [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) / [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) — full design
  - [`HANDOVER.md`](docs/HANDOVER.md) — implementation plan and working norms
  - [`STATE.md`](docs/STATE.md) — current implementation state (source of truth)
  - [`abi-v1.md`](docs/abi-v1.md) — the frozen, tested `abi-v1` artifact contract
  - [`SECRETS_LIFECYCLE.md`](docs/SECRETS_LIFECYCLE.md) — secret-memory design
  - [`borrow-check-limitations.md`](docs/borrow-check-limitations.md) — known borrow-check over-rejections + the partial-move soundness gap
  - [`decisions/`](docs/decisions/) — architecture decision records (45 so far)
- `selfhost/` — **the compiler rewritten in Sentinel** (Phase D): `lexer`/`parser`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`/`codegen`/`merge` `.sentinel` stages, each differentially validated byte-for-byte against the Rust `snc`.
- `tests/pass/` — pass-test fixtures (`.sentinel` programs that should compile + run to a known exit/stdout).
- `tests/ui/` — UI-test fixtures (`.sentinel` programs that should produce specific diagnostics; `insta` snapshots).

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate for security-critical products targeting banks, governments, and regulated industries. Sentinel is open-source; the products built on top of it are Anie's commercial work.

## What this is not

- **Not production-ready.** "Sentinel 1.0" is the *bootstrap-compiler* milestone (Phase C close): the full language — types, generics, borrow check + RAII, secret + effect typing, the handler runtime, classes/traits/delegation, structured concurrency — compiles and runs, with machine-verified constant-time `secret`. It was **not** a production release: single-process, single-file, loop-free-by-design, no standard library at the 1.0 close. **Phase D has since grown the language (sum types, strings, `Vec`, I/O, loops, modules) and ported the compiler to Sentinel — it now self-hosts** (see Status above); production-hardening and tooling (LSP) remain pending.
- **Not stable.** Every API can change. The `abi-v1` *compiled-artifact* contract is frozen and layout-tested; the Rust crate APIs are not.
- **Constant-time `secret` is delivered and machine-verified** — the compiler rejects secret-dependent control flow, memory indexing, and division. What remains future/ecosystem work: explicit speculation-barrier / `cmov` *emission* (a branch-free program already passes verification, so this was scoped out of the 1.0 minimum), `[secret T]` arrays, and real cipher suites (libraries belong in the ecosystem, not the language).
- **Single-file, single-process, loop-free-by-design *at the 1.0 close bar*.** Those were 1.0 scope limits, not permanent ones: **Phase D's language build-out has since added** sum types + `match`, strings + a `u8` byte type, growable `Vec<T>`, file I/O, `while`/`break`/`continue` loops, and modules / multi-file (`use`) — the features a compiler needs to self-host. Still single-process; cross-process capabilities, actors, and true per-unit separate compilation remain deferred follow-ons.
- **Not accepting general contributions yet.** The design is still fluid.

## License

MIT — see [`LICENSE.md`](LICENSE.md).
