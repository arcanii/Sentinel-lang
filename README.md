# Sentinel-lang

**A security-first systems programming language for the threats of the 2030s.**

Sentinel is a memory-safe, capability-bounded systems language being built by [Anie Ltd.](https://aniesolutions.ai) to address the bug classes that dominate modern security incidents — supply-chain attacks, cryptographic side channels, secret disclosure, untrusted-code execution, and information-flow violations — none of which are structurally addressed by any production language today.

For the short-form pitch, see [`docs/SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md).
For the full design, see [`docs/SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md)
(the design of record; [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) is the
superseded original vision, kept for provenance).
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
> differential reference oracle, and **full-corpus codegen parity is reached**
> (all 177 corpus fixtures the oracle emits for are byte-identical) — the self-host
> port's goal is met. The active track is now the per-unit **separate-compilation** back end
> (multi-file modules → independent objects → linked, with incremental
> rebuilds; see *What works today* and Phase D below).

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
  - 🟢 **C5 — productionization toward 1.0:** an HIR/MIR analysis pipeline; **constant-time `secret` verification, delivered and *machine-checked on the MIR*** (a MIR pass rejects any `secret` reaching a branch, a memory index, or a division divisor — `sentinel::mir::secret_leak`; the type system is the taint oracle and the check runs pre-LLVM — see the headline section for the precise property + its boundaries); bitwise operators `& | ^`; the **Phase A broker wired into compiled programs** as per-scope bump arenas (scope-exit bulk free); a **defined, frozen, layout-tested ABI** (`abi-v1`); and the **1.0 acceptance program — a constant-time TLS-1.3-handshake-shaped go/no-go that passes the verification** (the close bar). **Sentinel 1.0 is declared** (ADR 0025 + 0030 → ACCEPTED) — the bootstrap-compiler milestone, *not* a production release (still single-process, single-file, research-stage).
- 🟢 **Phase D — Self-hosting** (the Sentinel compiler rewritten in Sentinel) — **the bootstrap fixed point is reached; the Sentinel compiler compiles itself.** Two movements, then the separate-compilation back end:
  - ✅ **Movement 1 — language + stdlib build-out** (ADR 0031–0037): the 1.0 language couldn't self-host (no sum types/`match`, strings, growable collections, file I/O, loops, or modules — all of which a compiler needs), so Phase D first grew the language. **D.1** sum types + `match`, **D.2** strings + `u8`, **D.3** growable `Vec<T>`, **D.4** file I/O, **D.5** `while`/`break`/`continue`, **D.6** modules / multi-file (`use`) — all landed.
  - 🟢 **Movement 2 — the port** (ADR 0038–0045): every stage of `snc` is reimplemented in Sentinel (`selfhost/*.sentinel`) and validated **byte-for-byte** against the Rust `snc` via a per-stage dump oracle (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`/`llvm`) over the whole corpus. The whole pipeline through codegen is ported, and **`scg` (the Sentinel-built compiler) discovers, merges, and lowers its own multi-module source to LLVM IR byte-identical to the Rust oracle — then `cc`-ing that IR yields a binary that re-emits the same IR (a true fixed point), leak-free.** Full-corpus codegen parity is **reached** — all 177 of the 227 `tests/pass` + `tests/ui` fixtures that the oracle emits IR for are byte-identical (the exotic constructs the corpus exercises but the self-hosting compiler doesn't itself use — effects/handlers, generics, classes, nullable, concurrency — were covered slice by slice), so **ADR 0045 is ACCEPTED-WITH-AMENDMENTS**. The Rust bootstrap stays as the seed + oracle.
  - 🟢 **Concurrency, shared state, and multi-processing** (ADR 0066, 0069, 0070, 0071) — layered on top of the self-host: OS **threads** with typed `Channel<T>`s; **shared state** via runtime-refcounted `Shared<T>` and `Mutex<T>` with a scope-bound guard that unlocks on drop (plus an opt-in deadlock detector); first-class **function values**; **multi-processing** — `process_spawn`, byte pipes and typed framed channels; and a **`SealedChannel`**, an authenticated x25519 + AEAD channel that carries a `secret` between processes, with the secret re-emerging `secret` on the far side. `secret` containers (`Shared<secret T>` / `Mutex<secret T>`) carry per-cell `mlock` + zero-on-last-drop. **A secret may cross a thread boundary but not a process one** — ADR 0066 D8 fences the second and deliberately not the first. All of it is mirrored into the self-hosted `scg` and held to the same byte-identity bar.
  - 🟢 **The per-unit separate-compilation back end** (ADR 0037 (a), the active track) — each module compiles to its **own** object, with cross-module references resolved at link time via module-qualified `abi-v1` symbols. Every `pub` item kind crosses a module boundary (fns, structs, enums, generics, traits, effects — including cross-*unit* `perform`/`handle`); generic instances dedup across importers via origin-qualified `linkonce_odr` symbols; and rebuilds are **incremental** (an unchanged unit reuses its cached object). Opt-in behind `snc build --separate`; the whole-graph merge path and both bootstrap fixed points stay green alongside it. Remaining tail items are minor (see [`docs/STATE.md`](docs/STATE.md)).

**1841 tests across the workspace**, four-check green (build · `cargo nextest` · doctests · `clippy -D warnings`). For the authoritative, per-crate state of the codebase — and the per-crate test breakdown — see [`docs/STATE.md`](docs/STATE.md). When STATE.md and any other document disagree, STATE.md wins.

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

Sentinel's most novel guarantee is delivered. A `secret`-typed value may not influence control flow, memory addressing, or division — the timing channels behind cryptographic side-channel attacks — and `snc build` **statically rejects** any program where one does: a MIR pass propagates `secret` taint and flags any secret value that reaches a branch condition, a memory index or address, or a division divisor (`sentinel::mir::secret_leak`).

**What the check is, and its two boundaries** — so the claim matches the mechanism:

- It is **machine-checked on the compiler's MIR**, with the **type system as the taint oracle**: a value is secret iff its type says so (the type checker's operator-secret-preserving rules compute the taint; there is no separate dataflow pass — a standalone propagation only becomes necessary once MIR is lowered from *post-optimization* code, a recorded post-1.0 refinement). So the verifier is exactly as sound as the type checker's secret propagation.
- It runs **before LLVM optimization**: it constrains the program you wrote, not the optimized machine code, and it does **not force** constant-time *emission*. A branch-free program passes — but `cmov` forcing, speculation barriers, and post-codegen assembly verification are explicitly future work (which is why emission was scoped out of the 1.0 minimum, not why it is claimed).

Branch-free `secret` computation, using the sanctioned bitwise/arithmetic primitives, passes:

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

The Rust `snc` is still what bootstrap-builds the Sentinel compiler the first time and serves as the differential reference; the LLVM toolchain (`clang`/`llc`) is external to both. Full-corpus codegen parity — covering constructs the corpus exercises but the self-hosting compiler doesn't itself use — is **reached**: of the 227 fixtures under `tests/pass` + `tests/ui`, the Rust oracle emits IR for 177, and the Sentinel-built `scg` is byte-identical on **all 177** (the other 50 are `tests/ui` rejections and snc-only constructs the oracle refuses before lowering, which the differential skips). The self-host port's goal is met; the per-unit separate-compilation back end below is the active Phase D track.

### Modules + separate compilation

A program can span multiple files: a file is a module, `use a::b::Item;` imports across modules, and `pub` controls visibility. `snc build --separate` compiles each module to its **own** object and links them — cross-module references bind at link time via module-qualified `abi-v1` symbols, so a rebuild only recompiles the modules that changed (and their importers); unchanged units are reused from their cached objects.

```bash
# A two-module project:
#   util/math.sentinel  ->  pub fn add(a: i64, b: i64) -> i64 { a + b }
#   main.sentinel       ->  use util::math::add;  fn main() -> i64 { add(40, 2) }
snc build main.sentinel --separate -o app && ./app; echo $?   # 42

# Rebuild with nothing changed: every unit is reused from its cached object.
snc build main.sentinel --separate -o app    # prints: snc: fresh `main`  /  snc: fresh `util::math`
```

Every `pub` item kind crosses a module boundary — fns, structs, enums, generics (deduplicated across importers via origin-qualified `linkonce_odr` symbols), traits, and effects, including a library that `perform`s an effect the entry `handle`s. The whole-graph merge path (`snc merge`, used by `snc build`) stays the default; `--separate` is opt-in until it reaches full parity.

## Build

Requirements: **Rust stable** (pinned via `rust-toolchain.toml`) and **LLVM 18** (inkwell builds against `llvm18-0`). Two host platforms are known to work:

- **macOS on Apple Silicon** — the primary target. `brew install llvm@18`; `.cargo/config.toml` points at the Homebrew prefix.
- **Windows x86-64 (MSVC)** — builds, links, and runs the full self-host differential and both bootstrap fixed points. Needs `LLVM_SYS_180_PREFIX` set and the MSVC environment loaded (`vcvars64.bat`) for anything that links, because a Unix `link` from Git/MSYS otherwise shadows MSVC's `link.exe`. **18 tests fail there for platform reasons, not compiler ones** — POSIX-only FFI examples (`getppid`/`getuid`/`getentropy`), the socket examples, `dlopen`-based export tests, three `linkonce` tests that fail inside MSVC `link.exe`, and one harness that hardcodes Unix paths.

`cargo-nextest` recommended (not required — `cargo test` is the substitute).

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
- `crates/sentinel-types/` — type checking; produces a `TypedProgram` (the `Type` interner covers 27 variants: the scalars I64/I32/U8/U128/F64/Ptr/Bool, the compounds Struct/Enum/Nullable/Array/Vec/Ref/Secret/GenericInstance/TypeParam, and the handles Kont/Task/Class/TraitSelf/Fn/Channel/Process/SealedChannel/Shared/Mutex/Guard — every one of which needs a structural `abi-v1` tag, see [`abi-v1.md`](docs/abi-v1.md) §4).
- `crates/sentinel-borrow-check/` — lexical borrow checker; emits the `DropPlan` for scope-exit drops.
- `crates/sentinel-effect-check/` — effect-row inference + annotation check + the `fn main`-must-be-effect-free invariant.
- `crates/sentinel-hir/` — the typed→codegen seam (`lower_to_hir`); the thick HIR desugar is a post-1.0 follow-on.
- `crates/sentinel-mir/` — minimal SSA/CFG (`lower_to_mir`) hosting the **constant-time verification pass** (`verify_constant_time`), wired into `snc`.
- `crates/sentinel-codegen/` — LLVM IR lowering via inkwell: structs/arrays/nullables/generics/refs/secret, RAII drop, the handler runtime, classes/traits/delegation, structured concurrency, and broker scope-arena routing. Freezes the `abi-v1` layout/mangling/symbol contract (layout-stability tests).
- `crates/sentinel-runtime/` — the runtime linked into compiled binaries (`sentinel_print`/`alloc`/`free`/`panic_oob`, the broker scope-arena C-ABI, the handler-continuation runtime, the structured-concurrency scheduler).
- `crates/sentinel-driver/` — the `snc` compiler driver binary (single-file, `--separate` per-unit, and `merge` paths) + the pass/UI/repro/modules test suites.
- `crates/sentinel-lsp/` — editor-integration scaffold (stub; ADR 0025 D10).
- `docs/` — design, status, and process documents:
  - [`PROGRAMMING_GUIDE.md`](docs/PROGRAMMING_GUIDE.md) — intro to writing Sentinel programs today
  - [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md) — one-page pitch
  - [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) — full design (design of record); [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) — superseded original vision
  - [`HANDOVER.md`](docs/HANDOVER.md) — implementation plan and working norms
  - [`STATE.md`](docs/STATE.md) — current implementation state (source of truth)
  - [`HISTORY.md`](docs/HISTORY.md) — archived milestone-by-milestone running logs (provenance, not maintained)
  - [`abi-v1.md`](docs/abi-v1.md) — the frozen, tested `abi-v1` artifact contract
  - [`SECRETS_LIFECYCLE.md`](docs/SECRETS_LIFECYCLE.md) — secret-memory design
  - [`borrow-check-limitations.md`](docs/borrow-check-limitations.md) — known borrow-check over-rejections + the partial-move soundness gap
  - [`decisions/`](docs/decisions/) — architecture decision records (73 so far)
- `selfhost/` — **the compiler rewritten in Sentinel** (Phase D): `lexer`/`parser`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`/`codegen`/`merge` `.sentinel` stages, each differentially validated byte-for-byte against the Rust `snc`.
- `tests/pass/` — pass-test fixtures (`.sentinel` programs that should compile + run to a known exit/stdout).
- `tests/ui/` — UI-test fixtures (`.sentinel` programs that should produce specific diagnostics; `insta` snapshots).

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate for security-critical products targeting banks, governments, and regulated industries. Sentinel is open-source; the products built on top of it are Anie's commercial work.

## What this is not

- **Not production-ready.** "Sentinel 1.0" is the *bootstrap-compiler* milestone (Phase C close): the full language — types, generics, borrow check + RAII, secret + effect typing, the handler runtime, classes/traits/delegation, structured concurrency — compiles and runs, with machine-verified constant-time `secret`. It was **not** a production release: single-process, single-file, loop-free-by-design, no standard library at the 1.0 close. **Phase D has since grown the language (sum types, strings, `Vec`, I/O, loops, modules) and ported the compiler to Sentinel — it now self-hosts** (see Status above); production-hardening and tooling (LSP) remain pending.
- **Not stable.** Every API can change. The `abi-v1` *compiled-artifact* contract is frozen and layout-tested; the Rust crate APIs are not.
- **Constant-time `secret` is delivered and machine-checked** — the compiler statically rejects secret-dependent control flow, memory indexing, and division, verified on the MIR with the type system as the taint oracle (the headline section above states the precise property and its two boundaries). It is *not* a proof about the optimized machine code: the check runs before LLVM optimization and does not yet *force* constant-time emission. What remains future/ecosystem work: explicit speculation-barrier / `cmov` *emission* and post-codegen assembly verification (a branch-free program already passes verification, so this was scoped out of the 1.0 minimum), an independent secret-dataflow oracle, `[secret T]` arrays, and real cipher suites (libraries belong in the ecosystem, not the language).
- **The borrow checker is conservative, not yet flow-precise.** It is lexical (pre-Polonius), so it *over-rejects* some safe programs — a borrow held past its last use, a field-disjoint borrow through a parent — each with a documented workaround in [`docs/borrow-check-limitations.md`](docs/borrow-check-limitations.md). The one historical *under*-rejection (a Move-typed struct field passed by value could double-free at drop) is **closed** — ADR 0046, in both the Rust `snc` and the self-hosted `scg`. The remaining limitations are all over-rejections (ergonomics, deferred to the Polonius migration), which are sound by construction.
- **Single-file, single-process, loop-free-by-design *at the 1.0 close bar*.** Those were 1.0 scope limits, not permanent ones: **Phase D's language build-out has since added** sum types + `match`, strings + a `u8` byte type, growable `Vec<T>`, file I/O, `while`/`break`/`continue` loops, and modules / multi-file (`use`) — the features a compiler needs to self-host. **True per-unit separate compilation has since landed** (modules → independent objects → link, with incremental rebuilds; `snc build --separate`), and so has **concurrency beyond a single thread**: OS threads with typed channels, shared state (`Shared<T>` / `Mutex<T>` with runtime refcounting and a scope-bound guard), **multi-processing** (`process_spawn` + framed pipes), and an authenticated, AEAD-encrypted `SealedChannel` for moving a `secret` between processes. Actors and a capability model remain deferred follow-ons.
- **Not accepting general contributions yet.** The design is still fluid — but bug reports, security-relevant findings, docs fixes, and test cases are welcome; see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

MIT — see [`LICENSE.md`](LICENSE.md).
