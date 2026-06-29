# BACKLOG.md — Sentinel Post-1.0 Backlog and Research Directions

This document captures capabilities and tracked items in two layers:

- **Section 0 (Phase A carry-over)**: short-term, tactical items
  deferred from the broker implementation. Address before the broker
  is considered 1.0-stable.
- **Sections 1+ (post-1.0 research)**: long-term strategic directions
  that are not part of Sentinel 1.0 but should remain visible.

---

## 0. Phase A Carry-Over (broker tech debt)

Deferred from Phase A0-A8 (commits 9c7474d through b4412d8). None block
Phase B; all are nice-to-haves or known sharp edges.

### 0.1 Broker API hardening

- **Builder panics on SecretStrategy::wrap failure.** The current
  `ArenaBuilder::bump()` and `.slab()` use `.expect()` when wrapping
  in SecretStrategy. mlock failure (typical on macOS dev machines
  without `ulimit -l unlimited`) panics. The `credential_store.rs`
  example works around this with `std::panic::catch_unwind`, but the
  fix is to add `try_bump() -> Result<ArenaHandle, BrokerError>`
  and `try_slab(...) -> Result<...>` variants. Originating context:
  Phase A8, commit b4412d8.

- **slot_size_hint is None for bump strategies.** The
  `Arena::__raw_slot_bytes_for_diagnostics` accessor returns None
  for bump arenas because bump slots are variable-size. Either
  track per-slot sizes in bump (via the existing SlotInfo struct,
  which already has a `size` field — currently dead-code), or
  document that diagnostics are slab-only. The SlotInfo.size field
  is annotated `#[allow(dead_code)]` and could be returned by a
  bump-side override of slot_size_hint as a per-call lookup.

- **BrokerError::SecretMemory loses OS error detail.** The `reason`
  field was changed to `&'static str` to keep `BrokerError: Copy`.
  The OS error is logged via `tracing::warn!` before the variant is
  returned, but consumers who only see the error get no specifics.
  Options: drop Copy from BrokerError (most variants don't need it),
  or add a separate `os_errno: Option<i32>` field.

- **Probe-arena pattern in credential_store wastes one alloc cycle.**
  To detect STRICT availability, the demo creates a 1-slot probe
  arena, destroys it, then builds the real vault. With `try_bump`/
  `try_slab` (above) this becomes a single direct attempt.

### 0.2 Broker stability

- **Recording event field stability.** Event enum variants currently
  carry detailed payloads (arena id, name, kind, capacity, slot,
  generation, at_ns). If consumers serialize these for persistent
  logs, future field additions are breaking changes. Consider
  marking Event `#[non_exhaustive]` and committing to a stable
  on-disk format separately.

### 0.3 Phase B prerequisites (likely)

- Decide whether the broker's `parking_lot::Mutex` is acceptable in
  no_std contexts, or whether Phase B needs an alternative. (Note:
  this overlaps with Section 1.1 `no_runtime` Compilation Mode below.)
- Decide whether the broker should be re-exported from a higher-level
  crate (sentinel-core?) or stay accessible directly.
- Cargo workspace currently has stub crates (sentinel-syntax,
  sentinel-ast, sentinel-vm, etc.) — confirm these names are still
  the intended Phase B layout before populating them.

---

### 0.4 Phase B1 carry-over (effects-proto)

Items noticed during B1 implementation that are too small to be ADRs
but worth tracking. None block B2.

- **`EvalError` variants carry no spans.** Eval errors are rare
  post-type-check (div-by-zero, the `LetRecUninitialised` internal
  invariant, application of a Closure to the wrong-typed value once
  the type system gets richer), but when they do fire, `MiniError::render`
  falls back to the terse one-line form. Add `Span` fields to the
  three variants that can plausibly point at source (`Unbound` is
  unreachable post-type-check; `DivByZero` and `Type` should carry
  the offending expression's span; `NotAFunction` likewise; the
  `LetRecUninitialised` is a bug-class error and may stay span-less).
  Originating context: B1.7 dispatch, commit e6b06cd.

- **Multi-line span rendering clips to first line.** `diag::render`
  intentionally degrades on cross-line spans; B2's `effect` and
  `handle` blocks are likely to introduce multi-line constructs
  that make this annoying. Either teach `render` to emit a multi-line
  excerpt with carets on each line, or adopt `miette` here rather
  than waiting for Phase C.

- **`let rec` RHS-must-be-lambda restriction.** Currently a parser
  rule. May need relaxing in B3 when effect handlers arrive
  (handlers as recursive bindings). Re-evaluate when designing the
  handler surface; see ADR 0003.

---

## 1. Privileged-Mode and Bare-Metal Sentinel

The 1.0 language assumes an OS underneath it. Substantial new territory
opens up if Sentinel can run with no runtime, no OS, or as a privileged
component itself.

### 1.1 `no_runtime` Compilation Mode

A mode analogous to Rust's `no_std` where the broker and standard
library's OS-dependent components are removed. The language operates
on bare metal or inside another runtime (kernel, hypervisor, TEE).

Foundational work needed in 1.0: the broker must be defined as a trait
or interface, with the default implementation in a separate crate, so
alternative brokers can be supplied. Standard library functions must
declare which broker capabilities they require. Position-independent
freestanding binary output must be a supported codegen target from
day one.

### 1.2 Privilege-Level Region Tags

Region qualifiers like `@ring0`, `@ring3`, `@vmx_root`, `@vmx_nonroot`,
`@el2`, `@el1`, `@secure_world`, `@smm`. The compiler refuses to
dereference a pointer in the wrong privilege context, preventing the
confused-deputy bugs that have produced CVEs in Xen, KVM, Hyper-V,
and many ARM TrustZone implementations.

Foundational work in 1.0: regions must be extensible by the standard
library, not hardcoded. The region-checking machinery must support
user-defined region kinds with declared subtyping relationships.

### 1.3 Architectural Resource Broker

A kernel-mode or hypervisor-mode broker that manages CPU architectural
resources (MSRs, control registers, debug registers, performance
counters, VMCS regions, EPT/NPT hierarchies, IOMMU contexts,
exception vectors) with the same generational-handle safety the
default broker provides for memory. Use-after-free of a VMCS becomes
a typed error rather than a security breach.

### 1.4 Architecture-Specific Libraries

`sentinel-x86_64` and `sentinel-aarch64` standard-library crates
exposing architectural state through safe wrappers over `unsafe`
primitives. Includes VMX/SVM intrinsics, ARMv9 virtualization
extensions (EL2 control), SMMU and IOMMU abstractions, and the
exception-vector and trap-frame machinery.

### 1.5 Specific Targets

  - **Userspace introspection library** (most accessible). A debugger
    or profiler that uses OS APIs (`ptrace`, `process_vm_readv`,
    `task_for_pid`, ETW) to inspect other processes' CPU state with
    safety guarantees over the broker handles.
  - **Driver framework**. Sentinel drivers for Linux or for a
    Sentinel-native microkernel, getting region-tracked pointer safety
    that Linux's C drivers conspicuously lack.
  - **Trusted execution module**. Sentinel code running in SGX
    enclaves, TrustZone secure world, or SEV-SNP guests. The
    constrained TEE environment maps unusually well onto Sentinel's
    capability-based effects.
  - **Type-1 hypervisor**. A bare-metal hypervisor written entirely
    in Sentinel, comparable in ambition to seL4 or Bareflank but with
    Sentinel's broader safety story.
  - **Unikernel framework**. Application-specific single-address-space
    OSes built from Sentinel libraries, with the broker providing the
    memory management normally delegated to the kernel.

---

## 2. Memory-Hard and Cryptographic Storage

### 2.1 Memory-Hard Secret Storage

The `secret @hardened T` policy from SENTINEL_DESIGN2.md Section 5.6.
Ultra-sensitive secrets (root keys, master credentials, signing
keys, wallet seeds) are stored using Argon2id-derived memory-hard
schemes so that an attacker who reads a fraction of process memory
does not immediately recover the secret. Significant per-access cost,
so opt-in for the highest-value secrets only.

### 2.2 Cryptographic Type System

Beyond `secret`, additional qualifiers reflecting cryptographic
properties of data: `authenticated T` for values whose origin is
verified, `forward_secret T` for values whose compromise does not
compromise past sessions, `committed T` for values cryptographically
bound to a context. The type system enforces that, for example,
`decrypt()` returns `secret authenticated T` and operations requiring
authenticated input refuse unauthenticated values.

### 2.3 Cryptographic Memory Attestation

The broker can produce signed statements about what code is executing
and what data is loaded, verifiable by a remote party. Foundation of
confidential computing scenarios (SGX, SEV-SNP, TDX, CCA on ARMv9).
Sentinel exposes attestation as a broker primitive, not a third-party
library.

### 2.4 Post-Quantum Primitives as First-Class

Standard library includes the NIST-selected post-quantum algorithms
(ML-KEM, ML-DSA, SLH-DSA) with the same `secret` integration and
broker-managed key material as the classical primitives. Migration
helpers for hybrid classical/PQ key exchange and signatures.

### 2.5 Misuse-Resistant Cryptographic APIs

The cryptographic standard library is designed so that misuse is hard.
Nonce reuse for AEAD is prevented by the type system (nonces are
linear types that cannot be used twice). Key rotation is a
first-class operation with type-tracked epochs. Algorithm choice is
opinionated; deprecated algorithms are not exposed in the safe API
surface at all.

---

## 3. Information Flow Control

### 3.1 Flow Labels

The opt-in label system reserved in SENTINEL_DESIGN2.md Section 11.
Values carry labels like `phi`, `pii`, `tenant<T>`, `public`,
`classified<L>`. The compiler tracks where labeled data is permitted
to flow and refuses violations. Declassification is an explicit
operation recorded for audit.

### 3.2 Lattice-Based Policy

A general-purpose lattice over labels so that policies can be
expressed declaratively. The compiler verifies that all flows respect
the lattice. The lattice itself is project-defined, not hardcoded, so
regulated industries can express their own classification schemes.

### 3.3 Integration with Effects

Flow labels and effects interact: a function declaring the `network`
effect cannot receive data labeled `phi` unless it also declares
`declassify_phi`. The combinatorial space is large; careful design
needed to keep signatures readable.

### 3.4 Compliance Outputs

Tooling produces machine-checkable artifacts that map source-level
declarations to regulatory frameworks (HIPAA, GDPR, PCI-DSS, FedRAMP).
The compiler is the source of truth for "does this code handle PHI
correctly," not a separate static analysis pass.

---

## 4. Speculation and Side-Channel Hardening

### 4.1 Per-Function Mitigation Granularity

Compiler-inserted speculation barriers, hardened branches, and cache
flushes apply only to functions that touch `secret` data, with the
exact mitigations tunable per target. Code that does not touch
secrets pays no cost.

### 4.2 Constant-Time Verification

A verification pass that confirms `secret`-handling functions do not
contain data-dependent timing variation visible at the architectural
level. This is necessarily incomplete (cache effects, branch
predictors, prefetchers are not architectural), but catches the
software-level mistakes that produce most real attacks.

### 4.3 Cache Partitioning Awareness

When the target hardware supports cache partitioning (Intel CAT,
ARM MPAM), the broker exposes partition allocation so that
`secret`-handling code can run in a dedicated cache way, reducing
cache-based side channels.

### 4.4 Microarchitectural Data Sampling Defenses

Integration with vendor-provided MDS mitigations (VERW on Intel,
DSB+ISB on ARM) at the type-system-visible boundaries between secret
and public data. The language inserts the flush instructions; the
programmer does not have to remember to.

### 4.5 What Sentinel Will Not Promise

Sentinel will not claim to make insecure hardware secure. Side-
channel hardening reduces the attack surface for software-introduced
leaks. It does not eliminate side channels rooted in shared hardware
state (LLC contention, ring-bus probing, power analysis, EM
emanations). The documentation must be honest about this limit.

---

## 5. Sandboxing and Confined Execution

### 5.1 Process-Lite Sandboxing

Language-level sandboxing for untrusted code that is lighter than a
process boundary but stronger than current language-level attempts.
Combines effect masking (no capabilities), memory budgets (no
exhaustion), arena isolation (no leakage), and timing limits (no
livelock).

### 5.2 WebAssembly as a Target

Sentinel compiles to Wasm with full safety guarantees preserved. The
effect system maps onto WASI capability declarations. Sentinel
becomes a natural source language for Wasm-based plugin systems,
serverless runtimes, and smart-contract platforms.

### 5.3 Cross-Language Sandboxing

Sentinel-hosted runtimes for executing untrusted code written in
other languages (Lua, JS, Python) with the host enforcing capability
and resource limits the guest language cannot. The broker mediates;
the guest sees a normal language but cannot exceed its sandbox.

### 5.4 Confidential Computing Integration

First-class support for running Sentinel code inside SGX, SEV-SNP,
TDX, and CCA enclaves with attestation primitives, sealed storage,
and enclave-aware memory regions. The threat model where the host OS
is hostile is supported natively.

---

## 6. Formal Verification and Property Checking

### 6.1 Pre/Post Conditions in the Language

`requires` and `ensures` clauses on functions, checked at runtime in
debug builds, optionally discharged statically by a verification
backend. Modeled on Dafny and Verus rather than full Coq-style proof
obligations; the goal is incremental verification of critical paths.

### 6.2 Refinement Types

Types like `i32 where x > 0 && x < 1000` checkable at compile time
where SMT can decide, runtime where it cannot. Useful for index
safety, protocol state machines, and cryptographic preconditions.

### 6.3 Property-Based Testing as First-Class

Fuzzing and property tests share infrastructure with the type system.
Every effect handler is a mock injection point. Every `secret` value
can be replaced with a fuzzer-controlled value. The broker's
recording mode produces deterministic replay for any crash.

### 6.4 Verified Subsets

A defined subset of Sentinel that is fully verifiable against a
formal semantics, with a verified compiler. Comparable in ambition to
CompCert or CakeML but for a contemporary safety-focused language.
Verified Sentinel is the natural target for safety-critical systems
(avionics, medical devices, automotive control).

---

## 7. Concurrency and Distribution

### 7.1 Distributed Actors

Actors transparent over process and machine boundaries. The
type-checked message protocol becomes a network protocol;
serialization is generated; the broker manages identity, routing, and
delivery semantics. Comparable to Erlang but with static type
checking and Sentinel's broader safety story.

### 7.2 Deterministic Concurrency

Opt-in deterministic schedulers for testing and replay. Combined with
the broker's recording mode, this gives full deterministic replay of
concurrent executions, which is otherwise nearly impossible.

### 7.3 Software Transactional Memory

STM as a built-in effect rather than a library. Transactional regions
are statically tracked; the compiler refuses operations within a
transaction that cannot be rolled back (I/O without compensation,
externally-visible effects).

### 7.4 GPU and Accelerator Offload

The `@gpu` region and `@simd<n>` qualifiers from SENTINEL_DESIGN2.md
Section 12 receive full treatment. Sentinel kernels for CUDA, Metal,
Vulkan compute, and emerging AI accelerators. Heterogeneous memory
managed through the broker with the same generational-handle safety
as CPU memory.

### 7.5 NUMA-Aware Allocation

`@numa(n)` region tags with broker-enforced placement and migration
policies. Useful for high-performance servers where memory locality
dominates throughput.

---

## 8. Tooling and Ecosystem

### 8.1 Time-Travel Debugger

The broker's recording mode integrated with a graphical debugger
that supports stepping backward through execution. Built on the LSP
infrastructure from 1.0.

### 8.2 Effect Inference Visualization

IDE tooling that shows the inferred effects at every call site,
making capability requirements visible without forcing the
programmer to annotate everything.

### 8.3 Supply-Chain Audit Tools

`snc audit` produces a report of every effect declared by every
transitive dependency, with diffs against the previous lockfile.
Surprising capability acquisitions (a JSON parser suddenly needing
`network`) are flagged as security incidents.

### 8.4 Reproducible Build Verification

A separate tool that, given a Sentinel binary and its source, verifies
that the binary is the byte-exact compilation of the source. The
foundation of a verifiable software supply chain.

### 8.5 Package Registry with Capability Policy

A package registry that enforces effect policies at publication time.
Packages declare effects in their manifests; the registry refuses
publication of packages whose declared effects exceed their actual
needs (measured by the compiler).

### 8.6 Cross-Language FFI Generation

Automated generation of safe bindings to Sentinel from C, C++, Rust,
Python, and other languages. The stable ABI from 1.0 makes this
realistic in a way it is not for Rust.

---

## 9. Embedded and Resource-Constrained Targets

### 9.1 Microcontroller Support

Sentinel for Cortex-M, RISC-V embedded, and similar targets. The
`no_runtime` mode is required. The broker is replaced with static
allocation strategies known at compile time. Effects are extended
with hardware-specific capabilities (`gpio`, `dma`, `interrupt`).

### 9.2 Real-Time Guarantees

Worst-case execution time analysis as a compiler pass. Functions can
declare timing bounds; the compiler verifies them against the target
architecture's documented timing. Useful for hard real-time control
systems.

### 9.3 Safety-Critical Certification

A defined subset of Sentinel suitable for DO-178C, ISO 26262, and
IEC 62304 certification. Verified compilation, deterministic
semantics, no dynamic allocation in the certified subset, and
traceability from requirements through code to tests.

### 9.4 C Transpilation Backend (legacy / portability)

An alternative codegen backend that emits portable **C** instead of
LLVM IR, so Sentinel can target platforms the primary LLVM path does
not serve — legacy systems, exotic or older architectures, and
toolchains where only a C compiler is available (the "C as portable
assembly" route taken by Nim, V, and mrustc). The self-host port's
codegen (ADR 0045) emits textual LLVM `.ll`; a C backend would be a
**parallel emission target reusing the same `TypedProgram + DropPlan`
walk**, not a rewrite. **Research-gated — establish value/requirement
before committing:** which concrete legacy targets actually demand it,
and — the load-bearing risks — whether the `abi-v1` layout + calling
convention (ADR 0029) and the constant-time `secret` guarantee (no
compiler-introduced branches or timing variation) survive translation
through C *and its optimizer*, which a C backend cannot pin the way the
LLVM path does. Lower priority until a target need is identified.

---

## 10. AI and Numerical Computing

### 10.1 Tensor Types

First-class tensor types with shape checking at compile time.
Shape mismatches are compile errors, not runtime exceptions. Builds
on the refinement-type machinery from Section 6.2.

### 10.2 Automatic Differentiation

Differentiation as a compiler transformation rather than a library
overlay. The compiler can produce forward-mode and reverse-mode
derivatives of declared-differentiable functions. Avoids the runtime
overhead of tape-based AD.

### 10.3 Accelerator Targeting

Compilation of numerical kernels to GPU, TPU, and NPU targets while
preserving Sentinel's safety guarantees. The `@gpu` region work from
Section 7.4 generalizes.

### 10.4 Privacy-Preserving Computation

Type-system support for differentially-private computation, secure
multi-party computation, and homomorphic encryption primitives.
Combines with the cryptographic type system from Section 2.2.

---

## 11. Language Evolution

### 11.1 Versioning and Editions

Like Rust's edition system: language changes that would break
backward compatibility are gated behind a per-crate edition
declaration. The compiler supports all editions simultaneously.
Critical for a language committed to a stable ABI.

### 11.2 Macro System

A hygienic macro system, ideally procedural rather than purely
syntactic, with access to the typed HIR rather than just the AST.
Carefully designed to compose with effects and regions.

### 11.3 Reflection

Compile-time reflection over types, with first-class type values that
can be examined and manipulated. Useful for serialization frameworks,
ORM-equivalents, and code generation tools.

### 11.4 Dependent Types in Limited Form

A constrained form of dependent typing for cases where refinement
types are insufficient. The full power of dependent types is
out of scope; the goal is supporting common patterns like
length-indexed vectors and protocol state machines.

### 11.5 Async/Await Refinement

As real usage patterns emerge, the effect-based async model will
need refinement: cancellation semantics, structured concurrency
primitives, async drop, async traits, async closures. None of
these are 1.0 blockers but all will need attention.

### 11.6 Explicit `return` as the Canonical Function-Return Form

Today a function's return value falls out of its trailing **tail expression**
(Rust-style, expression-oriented): `fn add(a, b) -> i64 { a + b }`. ADR 0065
added an explicit `return expr` (primarily for early return). A proposal
(2026-06-29) is to make the explicit **`return value;`** the canonical form and
emit a **deprecation warning** for the implicit "floating" / "hanging"
tail-expression return — the clarity argument being that, for a security
language, "the last expression is silently the return value" is an avoidable
foot-gun, and an explicit `return value;` is unambiguous.

Feasible without new infrastructure: `sentinel-base` already has a non-fatal
`Severity::Warning` (rendered by the driver), currently unused — a deprecation
warning would be the first emitter.

**Step 0 (the bounded, do-first fix — confirmed gap, 2026-06-29):** a trailing
`return value;` (WITH the semicolon) **does not parse today** — `fn main() ->
i64 { return 42; }` errors `blocks must end with an expression`. Only `return
42` (no semicolon, return-as-tail) or a *hanging* tail value works; `return x;`
is valid mid-block (followed by a tail) but not as the block terminator. So the
form the proposal calls "correct" isn't even writable. The first, separable fix
(an ADR 0065 follow-up, NO deprecation) is to let a **value-block end with a
divergent statement** — `return …;` (and `break;` / `continue;`) — with no
trailing tail expression, since the block diverges before any tail is reached.
This reuses ADR 0065's `block_diverges` machinery; it is a parser + block-typing
change (and the selfhost mirror + a differential fixture if it enters the
corpus), with NO IR change (`return 42;`-as-terminator lowers identically to
`return 42`-as-tail). This step alone makes `return value;` canonical-capable;
the deprecation below is the separate, larger piece.

The sharp edges the deprecation ADR must resolve **before any warning is
emitted**:

- **Scope precisely.** The warning must target ONLY a *function-body* implicit
  tail return — NOT expression tails. `if c { 42 } else { 0 }`, `match` arms,
  and `scope` / block values are sub-expression results, not returns, and the
  language stays expression-oriented there. The deprecated case is narrowly "a
  function whose value comes from an implicit tail rather than an explicit
  `return`."
- **The bootstrap + corpus.** Every function in `selfhost/*.sentinel`, every
  `examples/` program, and every fixture uses implicit tail returns — a naive
  warning floods all of them. Decide: warn-only indefinitely, or warn → later
  error behind an edition (11.1) with a mechanical migration to explicit
  `return` that stays **byte-identical at both bootstrap fixed points**
  (`return tail;` as the last statement must lower identically to the implicit
  epilogue — verify, since ADR 0065's `return` drops-to-floor should coincide
  with the normal epilogue's drops for a tail-position return).
- **Philosophical call.** This nudges the language from Rust-style
  expression-oriented returns toward C/Go-style statement-oriented returns — a
  language-identity decision, not a lint.

Recommendation: ADR-first; pin scope + migration + bootstrap handling before
emitting anything. Low urgency relative to the threading / concurrency track.
(Filed in BACKLOG.md §11 because BACKLOG2.md has no Language-Evolution section.)

**Action (deferred, maintainer-confirmed 2026-06-29):** draft a dedicated ADR
covering **Step 0 + the deprecation as one design** (the bounded `return value;`
terminator fix and the implicit-tail deprecation, with the warn→error migration
and bootstrap rewrite). Not now — after the threading / concurrency track.

### 11.7 Cross-Module Classes (`pub class`)

Today a Sentinel `class` (ADR 0022) is module-LOCAL: **`pub class` is rejected**
(`pub` is allowed only on `fn` / `struct` / `enum` / `trait` / `effect`), so a
class cannot cross a module / separate-compilation boundary. The cross-module
form of "a type with behaviour" is a `pub struct` + `pub fn`s, or a `pub trait`
+ `impl` (see BACKLOG2 §10.9 + `examples/modules/rect_demo.sentinel`). A
2026-06-29 ask for a cross-FILE class example surfaced this; making classes
exportable is a real language feature that **needs an ADR** (maintainer-confirmed
2026-06-29).

The ADR must decide:

- **Whether to do it at all.** Is module-local `class` intentional — structs the
  cross-module *data* type, classes the local *encapsulation* type — or a gap to
  close? If kept local, document the rule and the struct/trait alternative.
- **Visibility surface.** `pub class` to export the type; methods are already
  `pub fn`; FIELDS presumably stay private across the boundary (construct via
  `init`, access via methods — encapsulation preserved).
- **The cross-unit ABI.** A class `init` (the `out_ptr` convention) and its
  methods (the `self_ptr` convention) are `abi-v1`; a cross-unit class needs
  module-qualified symbols for `init` + each method, exactly as struct / trait /
  impl methods already cross (ADR 0037 D7 mangling). Pin the mangled names.
- **Delegation across modules.** A `delegate field: T to Trait` whose `T` is an
  imported class (ADR 0021 D6) — does the synthesized `impl` cross cleanly?
- **The bootstrap.** The self-hosted compiler must handle cross-module classes
  too (both fixed points byte-identical), and `--separate` / `--lib` /
  `--shared` must link them.

Low urgency relative to the threading / concurrency track.

### 11.8 Multi-File Modules + Explicit Module Declarations (self-host modularization)

> ✅ **DONE (2026-06-30, ADR 0067 ACCEPTED-WITH-AMENDMENTS).** Multi-file modules
> implemented in both compilers; `selfhost/types.sentinel` split **13,718 → 3,371
> lines** + 5 parts (`types/{interner,infer,borrow,cg,mir}.sentinel`), both bootstrap
> fixed points byte-identical. Realized as directory = module + a `part` manifest
> (read by path — no new builtin), module-wide private, entry exempt from the decl
> check. The original problem statement is kept below for the record.

`selfhost/types.sentinel` is **13.7k lines — 62% of the self-host, ~5× the next
file** — holding the type interner, generic-fn inference, borrow-move analysis,
the codegen (`cg`) text emitter, AND the MIR dump in one file. Every
oracle-moving change touches it in many scattered places; it is the self-host's
chief maintainability liability and should be split into focused modules
(maintainer-flagged 2026-06-29).

The blocker is the module model. ADR 0037 is **one file = one module**, the
module name IMPLICIT from the file path, with per-item `use` (e.g. `use
parser::Expr;`; the self-host already uses this across lexer/parser/resolve/
effects/merge/types). Splitting types.sentinel under that model is possible but
ugly: only **2 of its 313 fns are `pub`**, so a split would force pub-ifying
~hundreds of internal helpers and FRAGMENT the `types::` namespace that
importers (`use types::run;`) depend on; and the one shared mutable `TyCtx`
struct couples the typer / cg / mir tightly. (Classes are NOT the issue —
types.sentinel uses none, §11.7; `TyCtx` is a struct, which crosses fine.)

The clean enabler (the 2026-06-29 proposal) is **explicit module declarations +
multi-file modules**: a file declares its module name (decoupling identity from
path), and SEVERAL files may declare the SAME module name to form one logical
module. types.sentinel then splits into focused files (e.g. `types_interner`,
`types_infer`, `types_borrow`, `types_cg`, `types_mir`) that together ARE the
`types` module — internal helpers stay module-private (visible across the
module's files, not pub-exported), and the public API (`run`) + the `types::`
namespace importers use are UNCHANGED. This is the Rust `mod` / C++-namespace
model.

The ADR must decide:
- **Syntax + semantics** of the declaration (`module types;`?), and how multiple
  files merge into one module — visibility across the files, ordering, and
  whether a private item in file A is visible in file B of the same module.
- **Keep ADR 0037's implicit path-based modules** as the default (so existing
  single-file modules / the `std` library are unchanged), with the explicit
  declaration as an opt-in for multi-file modules.
- **Byte-identical at both bootstrap fixed points** — the hard part. This is a
  bootstrap RESTRUCTURING, not just a language feature: the merge must produce
  identical IR + symbols after the split (FnId numbering, `__spawn_wrapper_<id>`,
  and mangled names must not shift, or shift identically on the Rust + selfhost
  sides). Sequence it ADR → mechanism (multi-file modules) → re-bless → split
  types.sentinel file-by-file, differential green at each step.

Orthogonal to the threading / concurrency track; high maintainability value.
ADR-first.

---

## 12. What Sentinel Will Never Do

The backlog includes items that have been explicitly rejected. This
list exists to prevent revisiting decisions that have already been
made.

  - **Tracing garbage collection** as a fallback for the safe subset.
    Sentinel commits to deterministic destruction; GC would
    undermine the broker's ownership model and the predictability
    that systems programmers require.

  - **Source compatibility with C or C++ headers.** FFI to C is
    supported through explicit declarations, not header inclusion.
    C's preprocessor and template machinery cannot be safely
    interpreted.

  - **Class-based inheritance.** Replaced by delegation and named
    trait implementations. This is settled.

  - **Implicit numeric conversions.** Every conversion is explicit.

  - **A monolithic standard library covering web frameworks, ORMs,
    GUI toolkits, or other application-layer concerns.** These
    belong in the ecosystem.

  - **Promising to make insecure hardware secure.** Sentinel's safety
    guarantees apply at the language layer. Hardware vulnerabilities
    require hardware fixes; Sentinel will help apply vendor-prescribed
    mitigations but will not claim to substitute for them.

  - **Replacing Rust as the default systems language.** Sentinel
    serves a more specific constituency. Mission creep toward "be
    everything to everyone" has killed more languages than any
    technical limitation.

---

## 13. Process for This Backlog

This document is revisited annually. Each item gets one of four
dispositions:

  - **Promoted to roadmap.** Scheduled for the next release cycle.
  - **Kept in backlog.** Still relevant, not yet scheduled.
  - **Demoted to research.** Interesting but unlikely to ship; a
    research artifact may be produced.
  - **Removed.** No longer worth pursuing; reasoning recorded in
    `docs/decisions/`.

New items are added freely. The backlog being large is not a
problem; the backlog being out of date is.

Decisions to promote require a written design proposal showing how
the item integrates with the existing language and the broker, what
foundational changes (if any) are required, and what the migration
story for existing code looks like.

---

## 14. The Through-Line

Every item in this backlog ties back to Sentinel's purpose: a
flexible, high-security, high-safety framework that catches security
bugs the programmer did not intend to introduce. The features differ
in scope and ambition, but they share a common test: does this make
ordinary code more secure by default, or does it only help when the
programmer is already thinking about security?

Sentinel's bet is on the former. Languages that only reward
careful programmers do not move the security needle, because the
careful programmers were already writing secure code in C. The
languages that move the needle are the ones that make insecure code
fail to compile. That is the standard every item here must meet.

*End of document.*
