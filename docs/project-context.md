---
project_name: 'Sentinel-lang'
user_name: 'Bryan'
date: '2026-06-27'
sections_completed: ['technology_stack', 'authority_and_adrs', 'pipeline_and_architecture', 'secret_constant_time', 'effects_runtime_ffi', 'testing', 'workflow', 'footguns']
existing_patterns_found: 28
status: 'complete'
rule_count: 28
optimized_for_llm: true
---

# Project Context for AI Agents

_This file contains critical rules and patterns that AI agents must follow when implementing code in this project. Focus on unobvious details that agents might otherwise miss._

Sentinel is a security-focused language whose compiler is a 15-crate Rust workspace. Its reason to exist is **machine-verified constant-time `secret`** handling. Most rules below exist to protect that guarantee or the self-hosting bootstrap — breaking either is a serious regression, not a style nit.

---

## Technology Stack & Versions

- **Language:** Rust **2021 edition**, `rust-version = 1.80`, **stable channel only** — pinned in `rust-toolchain.toml`. No nightly features. Components: rustfmt, clippy, rust-analyzer, rust-src.
- **Workspace:** 15 crates; dependency versions are inherited via `package.workspace = true` from root `[workspace.dependencies]`. **Never pin a dependency version inside a member crate** — add/bump it in the root `Cargo.toml` only (stated reason: prevent version drift).
- **Codegen back ends:** LLVM **18** via `inkwell 0.5` (feature `llvm18-0`) for release; **Cranelift 0.111** for fast debug builds. LLVM 18 is a hard requirement.
- **Query engine:** `salsa 0.18`. **Lexer:** `logos 0.14`.
- **Arenas / interners:** `bumpalo 3.16`, `typed-arena 2.0`, `rustc-hash` (FxHashMap), `indexmap`, `smallvec`.
- **Diagnostics:** `miette 7.2` (feature `fancy`) + `thiserror 1.0`.
- **Testing:** `insta 1.40` (snapshots), `proptest 1.5`, `criterion 0.5`, `serial_test 3.1`; run via **`cargo nextest`**.
- **Release profile:** `lto = "thin"`, `codegen-units = 1`.

## Critical Implementation Rules

### Authority & where truth lives

- **`docs/STATE.md` is the source of truth** for current status. When STATE.md, `docs/HANDOVER.md`, or `CONTRIBUTING.md` disagree, STATE.md wins. Read it first.
- The architecture is governed by an **ADR trail** in `docs/decisions/`. The "why" of any non-obvious decision is an ADR — find and read it before changing the behavior it ratifies.
- Current status: **Sentinel 1.0 reached** (Phase C closed 2026-05-30); the compiler **self-hosts** (Phase D bootstrap fixed point reached); per-unit separate compilation is functionally complete. `sentinel-lsp` is a post-1.0 stub.

### Compiler pipeline & crate architecture

- Pipeline order: **`syntax` → `ast` → `resolve` → `types` → `borrow-check` → `effect-check` → `hir` → `mir` → `codegen` → `runtime`/`driver`.** Note **borrow-check precedes effect-check.** `sentinel-base` underlies all (salsa DB + diagnostics accumulator).
- `sentinel-broker` is the memory subsystem (generational arenas; secret-memory policy: `mlock` + zero-on-free). `sentinel-effects-proto` is a **frozen Phase-B research interpreter — do NOT wire new features through it.**
- Phases are **salsa queries** that chain by passing one query's output into the next. Keep query functions pure — emit diagnostics through the accumulator, don't mutate shared state.
- **Codegen lives outside salsa** (LLVM `'ctx` lifetimes are incompatible with salsa storage). Don't try to make LLVM IR generation a tracked query.
- **HIR is a thin seam, not a full desugar.** Codegen consumes the *typed program* directly; MIR is lowered from the typed program **only for verification**. Do NOT assume HIR performs monomorphization, dispatch resolution, or explicit-drop rewriting — those are deferred post-1.0.
- `Type` is **`Copy + Hash`**, and that is load-bearing (interners are value tables, not Arc-wrapped). Keep `Type` Copy; don't add non-Copy fields.

### The `secret` / constant-time discipline (highest-stakes rules)

The **type system is the taint oracle**: a value is secret iff its `Type` is `Secret(..)`. The MIR pass **`sentinel::mir::secret_leak`** rejects any secret reaching a branch, a memory index, or a divisor. It runs **pre-LLVM-optimization** — it constrains the *program*, not the optimized machine code (see README for the precise boundaries).

- A secret must **never** reach: an `if`/branch condition, an array index, or a divisor → these are the `SecretBranch` / `SecretIndex` / `SecretDivisor` rejections. Never add a code path that lets a secret through them.
- `secret f64` is a **type error** — floats are a public-only domain (float ops aren't constant-time: subnormals, NaN, div microcode all leak timing).
- `declassify(e)` is the **only** sanctioned way to drop the secret qualifier (it's a special form, not a function). Don't add other escape hatches.
- A change that makes the borrow checker accept use-after-free / double-free, or lets a `secret` reach a branch/index/divisor **without** a `secret_leak` rejection, is a **security bug** → report privately (GitHub "Report a vulnerability" or the maintainer email in `CONTRIBUTING.md`), do **not** open a public PR/issue.
- Don't let docs over-claim beyond the README's stated constant-time boundaries — over-claiming the guarantee is itself a bug.

### Effect handlers, runtime & FFI/ABI

- Effects are reified (free-monad style) into continuations. **Continuations are one-shot** — resuming a `kont` twice panics by design (multi-shot is a deferred post-1.0 upgrade). Handlers are **deep** (the handler re-wraps the tail).
- Runtime symbols are crate-prefixed C-ABI (`sentinel_perform_op`, `sentinel_kont_resume`, …). Frame chains are heap (malloc) linked lists, not arena-allocated.
- The callable ABI is **`abi-v1`** and is **stable at 1.0** — no breaking changes to emitted symbol names/shapes. Cross-module / separate-compilation linking relies on module-qualified `abi-v1` symbols. The **type-tag table** (`docs/abi-v1.md` §4) is complete over every `Type` variant and every tag is **structural** — derived from the type's shape, never from an interner index, because three independent back ends must derive the same name. A new `Type` variant needs a tag in all three (`mangle_type` in `sentinel-codegen` and in `sentinel-driver/src/llvm_dump.rs`, `cg_mangle_to` in `selfhost/types/cg.sentinel`); both Rust matches are exhaustive on purpose, so **never add a `_ =>` arm** (ADR 0016 A1).
  - ADR 0016 A1 did move some inkwell tags (`shared0` → `shared_i64`, …). That is an amendment rather than an `abi-v2` bump on one evidenced ground, not on principle: those tags are reachable only through a phantom generic argument or a generic fn's mono key, and **sweeping all 339 corpus programs showed zero emitted names change**. A tag change with any corpus reach would be an `abi-v2` matter — run that sweep before claiming otherwise.
- `extern "C"` import and `export "C"` export resolve by **symbol name**; the value ABI is public scalars (`i64`/`f64`) only. A **secret fence** rejects `secret` arguments crossing FFI — keep it.
- Driver build modes: `snc build --separate` (per-unit objects), `--lib` (static archive), `--shared` (`.dylib`/`.so`), `--emit-header` (C header).

### Testing rules

- Two corpora: **`tests/pass/`** = Sentinel programs that must compile-and-run (asserted on exit code / stdout); **`tests/ui/`** = programs that must be **rejected**, with `insta`-snapshotted diagnostics.
- Fixtures are named **`c<phase>_<description>.sentinel`** (e.g. `c02_arithmetic.sentinel`, `c52_secret_leak.sentinel`); major milestones carry a `c##_go_no_go.sentinel`.
- Update snapshots with **`just bless`** (`INSTA_UPDATE=always cargo nextest run`). Review the diff — never bless blindly.
- nextest skips doctests — use **`just test-all`** (or `cargo test --doc`) to include them.
- `examples/` programs double as feature tests and are built **both** via `--separate` and the merge path, asserted in `crates/sentinel-driver/tests/examples.rs`.

### Development workflow

- **Every change lands four-check green:** `cargo build --workspace` · `cargo nextest run --workspace` · `cargo test --doc --workspace` · `cargo clippy --workspace --all-targets -- -D warnings`. Shortcut: **`just check-all`**. Clippy warnings are errors.
- Use the **`just`** recipes: `just build`, `just test`, `just snc <args>` (runs the `snc` driver = `crates/sentinel-driver`), `just bless`, `just lint`, `just fmt`.
- **Oracle-moving changes** (anything that alters `snc`'s stage dumps or emitted IR) follow a fixed rhythm: **ADR PROPOSED → Rust `snc` + fixtures → re-bless the per-stage differential → mirror into `selfhost/*.sentinel` → both bootstrap fixed points green → ADR ACCEPTED.**
- **Both bootstrap fixed points must stay green:** the Rust `snc` (oracle) and the self-hosted **`scg`** (built from `selfhost/*.sentinel`) must emit **byte-identical LLVM IR** for every module. If you change codegen in Rust, mirror it into `selfhost/` or you break self-host parity.
- **No new dependencies without discussion;** match the surrounding code's style. The repo is **not accepting unsolicited feature/refactor PRs** — design via an issue + ADR first.

### Critical don't-miss footguns

- Don't pin dep versions in member crates — use root `[workspace.dependencies]`.
- Don't add a feature only to Rust `snc` and forget `selfhost/` — it breaks the self-host fixed point.
- The borrow checker is **lexical at 1.0 and over-rejects** safe programs (`docs/borrow-check-limitations.md`). Fix a false rejection by scoping the borrow in an inner block — **not** by weakening the checker. Polonius migration is post-1.0.
- Don't route new work through `sentinel-effects-proto` (frozen) or assume `sentinel-lsp` is functional (stub).
- Don't introduce nightly Rust features — stable 1.80 only.
- Don't `unwrap()` / `panic!` on user-program input — surface a `miette` diagnostic through the accumulator.

---

## Usage Guidelines

**For AI agents:**
- Read this file before implementing any code in this repo.
- Follow all rules exactly; when in doubt, prefer the more restrictive option (it usually protects the constant-time guarantee or self-host parity).
- This file is a pointer, not a replacement: for current status read `docs/STATE.md`, and for the "why" behind a rule read the relevant ADR in `docs/decisions/`.

**For humans (maintenance):**
- Keep this file lean and focused on what agents get wrong — not a tutorial.
- Update it when the stack changes, an ADR ratifies a new convention, or a recurring agent mistake surfaces.
- Re-verify the version pins and pipeline order against `Cargo.toml` / `STATE.md` periodically; prune rules that become obvious.

_Last updated: 2026-06-27_

