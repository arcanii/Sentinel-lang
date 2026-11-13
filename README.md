# Sentinel-lang

**A security-first systems programming language for the threats of the 2030s.**

Sentinel is a memory-safe, capability-bounded systems language built by
[Anie Ltd.](https://aniesolutions.ai) for the bug classes that dominate modern security
incidents and that no production language addresses structurally: cryptographic side
channels, secret disclosure, supply-chain attacks, untrusted-code execution, and
information-flow violations.

Its first delivered guarantee is **machine-verified constant-time handling of `secret`
values**. The compiler is written in Rust and **self-hosts**: the same compiler, rewritten
in Sentinel, emits byte-identical LLVM IR to the Rust original and rebuilds itself.

- To write Sentinel today, start with the [Programming Guide](docs/PROGRAMMING_GUIDE.md).
- For the pitch, read [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md); for the design,
  [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) (the design of record;
  [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) is the superseded original, kept for provenance).
- For what is actually built, [`docs/STATE.md`](docs/STATE.md) is the source of truth.
  When it and any other document, this one included, disagree, STATE.md wins.

Sentinel is a multi-year research project in active development. **Nothing here is
production-ready**; see [Limitations](#limitations).

## A taste: constant-time `secret`, machine-verified

A `secret`-typed value may not influence control flow, memory addressing, or division, the
timing channels behind cryptographic side-channel attacks. `snc build` **statically
rejects** any program where one does. Branch-free computation over secrets, using the
ordinary bitwise and arithmetic operators, passes:

```sentinel
// Constant-time equality over secrets: XOR each pair, OR-reduce, then declassify
// the accumulator (0 iff every pair matched). No secret reaches a branch, an index
// or a divisor, so the constant-time check PASSES. This is the real shape of a TLS
// `Finished` MAC verify.
fn ct_eq(a0: secret i64, a1: secret i64, b0: secret i64, b1: secret i64) -> i64 {
    let d0 = a0 ^ b0;
    let d1 = a1 ^ b1;
    declassify(d0 | d1)
}

fn main() -> i64 {
    let x0: secret i64 = 42;
    let x1: secret i64 = 7;
    let diff = ct_eq(x0, x1, x0, x1);   // compare (x0,x1) to itself -> 0
    42 - diff                            // exit 42
}
```

`if declassify(...)` is fine. `if someSecretBool`, `a[someSecret]`, and dividing by a
secret are compile errors (`sentinel::mir::secret_leak`). `declassify(e)` is the only way to
drop the qualifier, and `secret f64` is a type error, because float operations are not
constant-time.

**What the check is, and its two boundaries**, so the claim matches the mechanism:

- It is **checked on the compiler's MIR, with the type system as the taint oracle**: a value
  is secret iff its type says so. The type checker's secret-preserving operator rules compute
  the taint; there is no separate dataflow pass. So the verifier is exactly as sound as the
  type checker's secret propagation.
- It runs **before LLVM optimization**: it constrains the program you wrote, not the
  optimized machine code, and it does **not force** constant-time *emission*. A branch-free
  program passes, but `cmov` forcing, speculation barriers, and post-codegen assembly
  verification are explicitly future work.

[`docs/ct-model.md`](docs/ct-model.md) is the precise contract: which constructs the
verifier models exactly, which conservatively, and which are not on the constant-time path.
The bootstrap acceptance program, [`tests/pass/c5_go_no_go.sentinel`](tests/pass/c5_go_no_go.sentinel),
builds the sample above into a TLS-1.3-handshake-*shaped* program (a state-machine class, a
cipher-suite trait, I/O as effects, a constant-time Montgomery-ladder step, HKDF-shaped key
derivation and the `Finished` verify) that compiles, runs and passes the check end to end.

## Status

The implementation is staged into four phases ([`docs/HANDOVER.md`](docs/HANDOVER.md) has
the rationale; the full chronology is archived in [`docs/HISTORY.md`](docs/HISTORY.md)).

| Phase | What it is | State |
|---|---|---|
| **A — Memory broker** | Rust crate: generational arenas, bump + slab allocation, scoped budgets, recording mode, secret-memory policy (`mlock` + zero-on-free). Now also backs compiled programs as per-scope arenas. | Complete |
| **B — Sentinel-Mini** | Tree-walking interpreter that validated the design: Hindley-Milner inference, row-polymorphic effects, deep handlers, `secret T` with a constant-time check. Frozen. | Complete |
| **C — Bootstrap compiler** | The production Rust compiler `snc`, targeting LLVM. Types, witness-table generics, references + lexical borrow checker + RAII, `secret` + effect typing, the algebraic-effect handler runtime, classes/traits/delegation, structured concurrency, the MIR constant-time pass, a frozen `abi-v1`. | Closed at the bootstrap milestone, 2026-05-30 |
| **D — Self-hosting** | The language grew what a compiler needs (sum types + `match`, strings + `u8`, `Vec<T>`, file I/O, loops, modules), then every stage of `snc` was reimplemented in Sentinel and validated byte-for-byte against it. | Fixed point reached; active |

**Phase D in more detail.** The compiler under [`selfhost/`](selfhost/) (about 37k lines of
`.sentinel`) reimplements every stage: lexer, parser, resolve, type-check, borrow-check,
effect-check, MIR + constant-time verify, codegen and module merge. Each stage has a Rust
dump-oracle subcommand (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`/`llvm`),
and a differential test asserts the Sentinel stage produces **byte-identical** output over
the whole `tests/pass` + `tests/ui` corpus; a mismatch means the port is wrong, and the Rust
stage is ground truth. The capstone is the **bootstrap fixed point**: `scg`, the
Sentinel-built compiler, discovers, merges and lowers its own multi-module source to LLVM IR
identical to the Rust oracle, and the binary built from that IR re-emits the same IR. Codegen
parity covers every corpus fixture the oracle emits IR for, including constructs the compiler
does not itself use (effects, generics, classes, nullables, concurrency). The Rust `snc`
remains the bootstrap seed and the reference oracle.

Layered on top since, and held to the same byte-identity bar in `scg`:

- **Concurrency and shared state** (ADR 0066, 0069, 0070, 0071): OS threads with typed
  `Channel<T>`; runtime-refcounted `Shared<T>` and `Mutex<T>` with a scope-bound guard that
  unlocks on drop, plus an opt-in deadlock detector; first-class function values.
- **Multi-processing**: `process_spawn`, byte pipes, typed framed channels, and a
  `SealedChannel`, an authenticated x25519 + AEAD channel that carries a `secret` between
  processes, with the value re-emerging `secret` on the far side. `secret` containers
  (`Shared<secret T>`, `Mutex<secret T>`) carry per-cell `mlock` + zero-on-last-drop. **A
  secret may cross a thread boundary but not a process one**; ADR 0066 D8 fences the second
  and deliberately not the first.
- **Per-unit separate compilation** (ADR 0037 (a)): each module compiles to its own object;
  cross-module references resolve at link time via module-qualified `abi-v1` symbols; generic
  instances dedup across importers via origin-qualified `linkonce_odr` symbols; rebuilds are
  incremental. Opt-in behind `snc build --separate`; the whole-graph merge path stays the
  default. Remaining tail items are minor (see STATE.md).

The workspace holds about 2,200 tests and lands every change four-check green (build,
`cargo nextest`, doctests, `clippy -D warnings`). The per-crate state and test breakdown are
in [`docs/STATE.md`](docs/STATE.md).

## What works today

**Compile and run a program.** `snc` is the driver. The 21 `*_go_no_go.sentinel` fixtures
exercise the surface end to end, each compiling, running and asserting its exit code or
stdout:

```bash
cargo build --workspace
./target/debug/snc build tests/pass/c4_go_no_go.sentinel -o /tmp/c4 && /tmp/c4; echo $?   # 42
./target/debug/snc build tests/pass/c5_go_no_go.sentinel -o /tmp/c5 && /tmp/c5; echo $?   # 42
```

**Modules and separate compilation.** A file is a module, `use a::b::Item;` imports across
modules, and `pub` controls visibility. Every `pub` item kind crosses a module boundary: fns,
structs, enums, generics, traits and effects, including a library that `perform`s an effect
the entry `handle`s.

```bash
# A two-module project:
#   util/math.sentinel  ->  pub fn add(a: i64, b: i64) -> i64 { a + b }
#   main.sentinel       ->  use util::math::add;  fn main() -> i64 { add(40, 2) }
snc build main.sentinel --separate -o app && ./app; echo $?   # 42

# Rebuild with nothing changed: every unit is reused from its cached object.
snc build main.sentinel --separate -o app    # prints: snc: fresh `main`  /  snc: fresh `util::math`
```

**Self-hosting.** Each stage matches its Rust oracle over the corpus, and the Sentinel-built
compiler reproduces itself:

```bash
cargo nextest run -p sentinel-driver sentinel_typer_matches_oracle_on_corpus
cargo nextest run -p sentinel-driver sentinel_codegen_matches_oracle_on_corpus

# The capstone: scg lowers its OWN multi-module source to IR identical to the oracle,
# then the binary built from it reproduces that IR, a fixed point.
cargo nextest run -p sentinel-driver \
  sentinel_codegen_self_merges_the_compiler_and_reaches_fixed_point
```

**The memory broker** is runnable on its own:

```bash
cargo run -p sentinel-broker --example token_bucket
cargo run -p sentinel-broker --example request_pipeline
cargo run -p sentinel-broker --example credential_store
```

`credential_store` demonstrates the secret-memory thesis directly: it allocates credentials
into a slab arena with `mlock` + zero-on-free active, hex-dumps the raw memory before and
after `free()`, and verifies the slot is fully zeroed on release.

**Libraries and examples.** [`sentinel_library/`](sentinel_library/) holds the first-party
libraries (`std/` and the `Sentinel/` core); [`examples/`](examples/) holds programs that use
them, each built both via `--separate` and the merge path and asserted on its exit code.

## Build

Requirements: **Rust stable** (pinned via `rust-toolchain.toml`) and **LLVM 18** (inkwell
builds against `llvm18-0`). Two host platforms are known to work:

- **macOS on Apple Silicon**, the primary target. `brew install llvm@18`;
  `.cargo/config.toml` points at the Homebrew prefix.
- **Windows x86-64 (MSVC)**: builds, links, and runs the full self-host differential and both
  bootstrap fixed points. Needs `LLVM_SYS_180_PREFIX` set and the MSVC environment loaded
  (`vcvars64.bat`) for anything that links, because a Unix `link` from Git/MSYS otherwise
  shadows MSVC's `link.exe`. 18 tests fail there for platform reasons, not compiler ones:
  POSIX-only FFI examples (`getppid`/`getuid`/`getentropy`), the socket examples,
  `dlopen`-based export tests, three `linkonce` tests that fail inside MSVC `link.exe`, and
  one harness that hardcodes Unix paths.

`cargo-nextest` is recommended but not required; `cargo test` is the substitute.

```bash
git clone https://github.com/arcanii/Sentinel-lang.git
cd Sentinel-lang
cargo build --workspace
cargo nextest run --workspace        # or `cargo test --workspace`
cargo test --doc --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Repository layout

The compiler pipeline runs `syntax` → `ast` → `resolve` → `types` → `borrow-check` →
`effect-check` → `hir` → `mir` → `codegen`, with `base` under all of it and `runtime` +
`driver` at the end.

- `crates/sentinel-broker/` — Phase A: generational arenas, budgets, secret-memory policy. Also backs compiled programs as scope arenas.
- `crates/sentinel-effects-proto/` — Phase B: the Sentinel-Mini interpreter, frozen.
- `crates/sentinel-base/` — shared Salsa db trait, `SourceFile` input, `Diagnostic` accumulator.
- `crates/sentinel-ast/` — AST types.
- `crates/sentinel-syntax/` — lexer + recursive-descent parser, salsa-tracked `lex_query` / `parse_query`.
- `crates/sentinel-resolve/` — name resolution; `VarId`/`FnId`/`StructId`/`ClassId`/`ImplId`/… stable identifiers.
- `crates/sentinel-types/` — type checking; produces a `TypedProgram`. The `Type` interner covers 27 variants (scalars I64/I32/U8/U128/F64/Ptr/Bool, compounds Struct/Enum/Nullable/Array/Vec/Ref/Secret/GenericInstance/TypeParam, handles Kont/Task/Class/TraitSelf/Fn/Channel/Process/SealedChannel/Shared/Mutex/Guard), each with a structural `abi-v1` tag ([`abi-v1.md`](docs/abi-v1.md) §4).
- `crates/sentinel-borrow-check/` — lexical borrow checker; emits the `DropPlan` for scope-exit drops.
- `crates/sentinel-effect-check/` — effect-row inference, annotation check, the `fn main`-must-be-effect-free invariant.
- `crates/sentinel-hir/` — the typed→codegen seam (`lower_to_hir`); a thick HIR desugar is a post-bootstrap follow-on.
- `crates/sentinel-mir/` — minimal SSA/CFG (`lower_to_mir`) hosting the **constant-time verification pass** (`verify_constant_time`).
- `crates/sentinel-codegen/` — LLVM IR lowering via inkwell: structs, arrays, nullables, generics, refs, secret, RAII drop, the handler runtime, classes/traits/delegation, structured concurrency, broker scope-arena routing. Freezes the `abi-v1` layout/mangling/symbol contract.
- `crates/sentinel-runtime/` — the runtime linked into compiled binaries: `sentinel_print`/`alloc`/`free`/`panic_oob`, the broker scope-arena C-ABI, the handler-continuation runtime, the concurrency scheduler.
- `crates/sentinel-driver/` — the `snc` driver binary (single-file, `--separate` per-unit, and `merge` paths) plus the pass/UI/repro/modules test suites.
- `crates/sentinel-lsp/` — editor-integration scaffold, a stub (ADR 0025 D10).
- `selfhost/` — **the compiler rewritten in Sentinel** (Phase D), one directory per stage, each differentially validated byte-for-byte against the Rust `snc`.
- `sentinel_library/` — first-party libraries: `std/` (batteries) and `Sentinel/` (the core base, ADR 0064).
- `examples/` — programs using those libraries, each doubling as a feature test. `demos/` holds platform demos (Win32 GUI via FFI); `tools/trust/` holds the signing and key-generation tooling.
- `tests/pass/` — programs that must compile and run to a known exit code or stdout.
- `tests/ui/` — programs that must be rejected with specific diagnostics (`insta` snapshots).
- `docs/` — design, status and process:
  - [`PROGRAMMING_GUIDE.md`](docs/PROGRAMMING_GUIDE.md) — writing Sentinel programs today
  - [`SENTINEL_SUMMARY.md`](docs/SENTINEL_SUMMARY.md) — one-page pitch
  - [`SENTINEL_DESIGN2.md`](docs/SENTINEL_DESIGN2.md) — full design (design of record); [`SENTINEL_DESIGN.md`](docs/SENTINEL_DESIGN.md) — superseded original
  - [`STATE.md`](docs/STATE.md) — current implementation state (source of truth)
  - [`HANDOVER.md`](docs/HANDOVER.md) — implementation plan and working norms
  - [`HISTORY.md`](docs/HISTORY.md) — archived milestone logs (provenance, not maintained)
  - [`ct-model.md`](docs/ct-model.md) — the constant-time contract, construct by construct
  - [`abi-v1.md`](docs/abi-v1.md) — the frozen, tested `abi-v1` artifact contract
  - [`SECRETS_LIFECYCLE.md`](docs/SECRETS_LIFECYCLE.md) — secret-memory design
  - [`borrow-check-limitations.md`](docs/borrow-check-limitations.md) — known over-rejections and workarounds
  - [`decisions/`](docs/decisions/) — the architecture decision records (ADRs) that govern every non-obvious choice

## Limitations

- **Not production-ready.** The Phase C close (2026-05-30) was the bootstrap-compiler
  milestone, the proof that the design works. It was called "Sentinel 1.0" at the time, which
  was aggressive; [ADR 0076](docs/decisions/0076-compiler-versioning.md) retired that name and
  reserved `1.0.0` for the production bar. Production hardening and tooling (an LSP) are
  pending; `sentinel-lsp` is a stub.
- **Not stable.** Every API can change. The `abi-v1` compiled-artifact contract is frozen and
  layout-tested; the Rust crate APIs are not.
- **The constant-time guarantee stops at the MIR.** It is delivered and machine-checked, but
  it is not a proof about the optimized machine code (see the two boundaries above). Still
  future or ecosystem work: speculation-barrier / `cmov` emission, post-codegen assembly
  verification, an independent secret-dataflow oracle, `[secret T]` arrays, and real cipher
  suites (libraries belong in the ecosystem, not the language).
- **The borrow checker is conservative, not flow-precise.** It is lexical (pre-Polonius), so
  it *over-rejects* some safe programs, such as a borrow held past its last use or a
  field-disjoint borrow through a parent, each with a documented workaround in
  [`docs/borrow-check-limitations.md`](docs/borrow-check-limitations.md). The one historical
  *under*-rejection (a Move-typed struct field passed by value could double-free at drop) is
  closed by ADR 0046 in both compilers. The remaining limitations are all over-rejections,
  sound by construction.
- **Deferred language features.** Actors and the capability model from the design are
  follow-ons. The bootstrap close's limits (single-file, single-process, loop-free, no
  standard library) have since been lifted by Phase D, as described under Status.
- **Not accepting general contributions yet.** The design is still fluid. Bug reports,
  security-relevant findings, docs fixes, and test cases are welcome; see
  [`CONTRIBUTING.md`](CONTRIBUTING.md), including how to report a security issue privately.

## Who's building this

Sentinel is being built by [Anie Ltd.](https://aniesolutions.ai) as the language substrate
for security-critical products targeting banks, governments, and regulated industries.
Sentinel is open-source; the products built on top of it are Anie's commercial work.

## Versioning

`snc --version` answers "what version of Sentinel do I have?" in two parts:

```
snc 0.2.0 (0x8a33152df5471198)
```

- **`0.2.0`** is the semantic version of the compiler and the language it accepts. Pre-1.0,
  so the MINOR is the breaking slot: a new feature, a changed flag, a new runtime symbol or a
  rule that now rejects a program that used to compile all bump the minor; a fix with no
  user-visible surface change bumps the patch.
- **`0x8a33…`** identifies the build. It is not a commit hash: it is derived from the
  compiler binary itself, so it moves whenever `snc` is rebuilt, which is what the
  separate-compilation cache needs in order not to reuse an older compiler's objects.

**`1.0.0` is a destination, not a past milestone.** Two other versions move on their own
clocks and are deliberately not tied to this one: **`abi-v1`**, the compiled-artifact
contract, frozen and layout-tested; and **`sentinel-interface v1`**, the `.sif` descriptor
format. See [ADR 0076](docs/decisions/0076-compiler-versioning.md).

## License

MIT — see [`LICENSE.md`](LICENSE.md).
