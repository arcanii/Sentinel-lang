# BACKLOG2.md — Sentinel Post-1.0 Backlog, Second Revision

This document supersedes BACKLOG.md. It preserves the structure and
intent of the original while adding a substantial new section on
cryptographic signatures and supply-chain provenance (Section 2) and
several smaller additions throughout. The framing, process, and
discipline of "what we will never do" remain unchanged.

## Framing

Sentinel's purpose is to be a flexible, high-security, high-safety
framework for systems programming. Its job is to eliminate as many
unintentionally introduced security bugs as the language layer can
reach: memory-safety errors, confused-deputy bugs, secret disclosure,
unchecked capability escalation, race conditions, side channels
expressible in software, supply-chain compromises through dependencies,
and provenance failures where the code that runs is not the code that
was reviewed.

Sentinel does not guard against hardware-level CPU state
vulnerabilities. Spectre, Meltdown, MDS, Foreshadow, Downfall,
fault-injection attacks, hardware backdoors, and microcode bugs all
operate below the language layer. Sentinel can insert vendor-prescribed
mitigations (speculation barriers, cache flushes, register clearing)
and can express which data is sensitive so the mitigations apply where
they matter, but it cannot make insecure silicon secure.

What Sentinel can do is ensure that when a programmer writes ordinary
code without thinking about security, the language and runtime catch
the security mistakes anyway. That is the bar every item in this
backlog must meet.

---

## 1. Privileged-Mode and Bare-Metal Sentinel

The 1.0 language assumes an OS underneath it. Substantial new territory
opens up if Sentinel can run with no runtime, no OS, or as a privileged
component itself.

### 1.1 `no_runtime` Compilation Mode

A mode analogous to Rust's `no_std` where the broker and the OS-
dependent components of the standard library are removed. The language
operates on bare metal or inside another runtime (kernel, hypervisor,
TEE).

Foundational work needed in 1.0: the broker must be defined as a trait
or interface, with the default implementation in a separate crate, so
alternative brokers can be supplied. Standard-library functions must
declare which broker capabilities they require. Position-independent
freestanding binary output must be a supported codegen target from
day one.

### 1.2 Privilege-Level Region Tags

Region qualifiers like `@ring0`, `@ring3`, `@vmx_root`, `@vmx_nonroot`,
`@el2`, `@el1`, `@secure_world`, `@smm`. The compiler refuses to
dereference a pointer in the wrong privilege context, preventing
confused-deputy bugs that have produced CVEs in Xen, KVM, Hyper-V, and
many ARM TrustZone implementations.

### 1.3 Architectural Resource Broker

A kernel-mode or hypervisor-mode broker that manages CPU architectural
resources (MSRs, control registers, debug registers, performance
counters, VMCS regions, EPT/NPT hierarchies, IOMMU contexts, exception
vectors) with the same generational-handle safety the default broker
provides for memory.

### 1.4 Architecture-Specific Libraries

`sentinel-x86_64` and `sentinel-aarch64` standard-library crates
exposing architectural state through safe wrappers over `unsafe`
primitives. Includes VMX/SVM intrinsics, ARMv9 virtualization
extensions, SMMU and IOMMU abstractions, and the exception-vector and
trap-frame machinery.

### 1.5 Specific Targets

  - **Userspace introspection library** (most accessible). A debugger
    or profiler that uses OS APIs to inspect other processes' CPU
    state with broker-managed handle safety.
  - **Driver framework**. Sentinel drivers for Linux or for a
    Sentinel-native microkernel, with region-tracked pointer safety
    that C drivers conspicuously lack.
  - **Trusted execution module**. Sentinel code in SGX enclaves,
    TrustZone secure world, or SEV-SNP guests. The constrained TEE
    environment maps unusually well onto Sentinel's capability-based
    effects.
  - **Type-1 hypervisor**. A bare-metal hypervisor written entirely
    in Sentinel.
  - **Unikernel framework**. Application-specific single-address-space
    OSes built from Sentinel libraries.

---

## 2. Cryptographic Signatures and Supply-Chain Provenance

This section is the principal addition over BACKLOG.md. It covers the
infrastructure that makes "the code you build is the code you reviewed,
signed by an authorized party, with verifiable provenance" a first-
class language property rather than a tooling afterthought.

### 2.1 Signatures as Part of Module Identity

A module's identity includes its signature. Two modules with the same
name and version but different signing keys are different modules, the
way two functions with the same name in different namespaces are
different functions. The compiler treats signature mismatches as
identity mismatches, not as warnings.

Source files carry a detached signature block so signatures can be
regenerated without modifying source. Build artifacts carry their own
signature over compiled output. The manifest declares which signing
keys are acceptable for each dependency. The compiler refuses to
compile against a dependency whose signature does not verify.

### 2.2 Manifest Trust Declarations

The package manifest expresses trust per dependency:

    [dependencies]
    json-parser = {
        version = "2.4.x",
        sig     = "ed25519:7a9f...c2e1",
        issuer  = "company-internal-ca",
        policy  = "exact-key"
    }

    crypto-primitives = {
        version      = ">=3.0",
        sig          = "minisign:RWQ...",
        policy       = "trust-on-first-use",
        pinned-since = "2026-01-15"
    }

Three trust policies cover realistic cases:

  - **Exact key.** "This specific key signed this specific version,
    refuse any change." Strictest, for security-critical
    dependencies.
  - **Trust on first use.** "First time we saw this package the key
    was X, refuse if it ever changes without explicit
    acknowledgment." Reasonable default for most dependencies.
  - **Issuer trust.** "Any key with a valid certificate chain to this
    CA is acceptable." For internal corporate dependencies where many
    developers can publish under one organizational identity.

### 2.3 Capability-Bounded Signatures

Trust declarations bound not just identity but capability. A signing
key is trusted to publish code with a declared maximum effect set; any
attempt to publish code exceeding that set fails verification.

    crypto-library = {
        sig     = "ed25519:...",
        grants  = [ alloc, secret, constant_time ],
        forbids = [ network, filesystem, subprocess ]
    }

If a maintainer's key is compromised, the attacker cannot pivot to
capabilities the key was never authorized to grant. The damage is
bounded by the original trust declaration. This is the principle of
least privilege applied to the signing infrastructure itself, and it
is the strongest distinguishing feature over current package managers.

### 2.4 The Keystore

Tooling requires a keystore. Its design is more security-critical than
casually appreciated, because it is the root of trust for the entire
build system.

Design constraints:

  - **Separate component.** The keystore is not embedded in the
    compiler or package manager. It exposes a narrow interface (look
    up key by ID, verify signature, optionally perform hardware-
    backed operations).
  - **Multiple backends.** Local file for individual developers,
    network service for organizations, TPM or Secure Enclave for
    high-security workstations, HSM for production signing.
  - **Hardware-backed by default.** Developer signing keys live on a
    YubiKey, Secure Enclave, or equivalent. Sentinel tooling supports
    this natively. Signing operations require physical confirmation,
    raising the bar against malware that has compromised the
    developer's machine but cannot trigger a physical touch.
  - **Audit log.** Every signing operation produces an entry in a
    tamper-evident log that can be cross-referenced against published
    artifacts.

### 2.5 Key Rotation

Key rotation must be a first-class operation. The manifest format
supports historical key sequences: "key A was trusted from version 1.0
through 2.7, key B is trusted from 2.8 onward, A authorized B in a
signed rotation statement." A rotation produces a signed statement
from the old key authorizing the new key, verified against the
historical trust record.

Rotation is the same pattern as TLS certificate rotation but applied
to source code. The infrastructure must support emergency rotation
(old key compromised, no signed handoff possible) through an out-of-
band trust update that all consumers must explicitly accept.

### 2.6 Revocation

Signed revocation lists distributed through the same channels as
packages, checked at build time, cached locally so builds remain
possible offline. Revocations apply going forward; historical artifacts
remain verifiable against the trust state at the time they were built.

Revocation reasons are typed: key compromise, maintainer departure,
algorithm deprecation, voluntary rotation. Build tooling presents
revocation reasons to developers, who must explicitly migrate.

### 2.7 Build-Environment Attestation

Beyond signing the source and the artifact, the build process signs
the *environment*: compiler version, build flags, dependency tree, OS,
and the hardware where available (Intel TDX, AMD SEV-SNP, ARM CCA can
attest to the bare-metal environment). A binary's full provenance
becomes "this source, built by this compiler, on this attested
platform, by this signing identity." Tampering at any layer is
detectable.

### 2.8 Multi-Party Reproducible Builds

A release of a Sentinel package can be signed not by one party but by
several independent builders who each compile the source and confirm
they produced the same output. The release signature is a threshold
signature over those independent attestations. This is what the
Bootstrappable Builds and Reproducible Builds projects have pursued
for a decade; Sentinel bakes it into the language and tooling.

Combined with the bootstrap story from HANDOVER.md, this gives Sentinel
a verifiable trust chain from the Rust source of the bootstrap
compiler all the way to a production binary, with every stage
cryptographically attestable.

### 2.9 Bill of Materials as a Language Primitive

Every Sentinel binary produces, on demand, a complete software bill of
materials listing every dependency, every version, every signing key,
every effect declaration, and the build environment that produced it.
The compiler is the only entity that knows the full dependency graph
with cryptographic precision; making it the canonical SBOM source
removes the SBOM-drift problem entirely.

This satisfies the increasingly common regulatory and contractual SBOM
requirements (US Executive Order 14028, EU Cyber Resilience Act) as a
language feature rather than a build-system afterthought.

### 2.10 Capability Attenuation Across the Dependency Graph

The 1.0 design lets each package declare its effects. A stronger model
lets the importer *restrict* effects further when re-exporting. If you
import a parsing library that declares `alloc, throw`, your code can
re-export it to *your* consumers with only `pure` exposed, and the
compiler enforces the restriction at the boundary.

This lets library authors offer broad capabilities while application
authors lock them down to the minimum their code actually exercises.
Attenuation is monotonic: capabilities can only be removed across
boundaries, never added.

### 2.11 Time-Bounded Trust

Trust declarations can include expiration dates. A dependency signature
trusted today expires in (default) one year unless explicitly renewed,
forcing periodic re-evaluation of the trust graph. This addresses the
slow drift where old trust decisions accumulate and become forgotten
attack surface. Renewal is part of normal dependency update workflows;
expiration produces a build warning thirty days in advance and a build
error on the expiration date.

### 2.12 Cryptographic Agility

The signature format supports multiple algorithms simultaneously. Ed25519
is the 1.0 default; the format reserves space for post-quantum
signatures (ML-DSA, SLH-DSA) and supports hybrid signatures during the
transition. Algorithm identifiers are part of the signature; algorithm
confusion attacks (the JWT `alg: none` class of bug) are prevented by
treating the algorithm as part of the verifying key's identity, not as
a separate field.

### 2.13 Mandatory Disclosure of `unsafe`

Every `unsafe` block in Sentinel, including transitively in
dependencies, surfaces as a security-relevant data point. The build
tool produces a report: "your binary contains 47 unsafe blocks across
12 dependencies, audited at these commits." Combined with signatures,
this enables statements like "this unsafe block was reviewed by Alice
on 2026-04-12, signed by her key, and has not changed since."

A package can carry signed `unsafe` audits: each `unsafe` block has an
optional reviewer signature, and the manifest can require all
transitively-included `unsafe` to be audited by signers from a trusted
set. This formalizes the auditing that the `cargo-crev` and
`cargo-vet` communities currently do informally.

### 2.14 Adoption Tiers

Mandatory signing has a real adoption cost. The defaults are tiered:

  - **Published packages.** Signing mandatory. Unsigned packages
    cannot be published to the registry.
  - **Production builds.** Signature verification mandatory. The
    build refuses to produce a binary if any dependency is unsigned
    or fails verification.
  - **Development mode.** Signature checks can be disabled with a
    visible warning on every build. Used for local iteration only,
    never for distribution.

The right thing must be the easy thing. Defaults make signed,
verified builds the path of least resistance, with bypasses
deliberately visible.

---

## 3. Runtime Integrity

### 3.1 Anti-Tampering for Running Binaries

Once running, the binary can periodically verify its own code segments
against signed expected hashes. The broker's memory-attestation
primitives extend to runtime integrity checks. An attacker who modifies
the binary after deployment, or who hot-patches running code through
process injection, can be detected.

### 3.2 Memory Poisoning Mode

A hardened broker mode where freed memory is poisoned with detectable
patterns and protected pages, and any access to freed memory traps.
This is a runtime cost paid by security-critical deployments wanting
belt-and-suspenders defense even within `unsafe` code.

### 3.3 Egress Filtering at the Language Level

A function declaring the `network` effect can also declare *which*
destinations it contacts. The runtime enforces the constraint: a
function declared to talk to `api.payment-provider.com` cannot
exfiltrate data to `evil.example.com`. This pushes firewall-style
egress filtering into the language, where it is much more precise
than network-layer enforcement.

### 3.4 Side-Channel Information Budgets

For systems handling user data, the broker enforces statistical
budgets: how many bits of information about a secret have been emitted
through public channels (logs, network traffic, error messages, timing
side channels). Once the budget is exceeded, further operations on the
secret fail. This is research-grade work but directly addresses the
category of bugs where individually-innocent emissions combine to leak
secrets.

### 3.5 Resource-Use Anomaly Detection

The broker maintains baseline profiles of normal allocation and effect
patterns. Significant deviations (a parser suddenly allocating gigabytes,
a logging function suddenly opening sockets) raise typed runtime
alerts. Anomaly detection is opt-in for sensitive deployments.

---

## 4. Memory-Hard and Cryptographic Storage

### 4.1 Memory-Hard Secret Storage

The `secret @hardened T` policy from SENTINEL_DESIGN2.md Section 5.6.
Ultra-sensitive secrets are stored using Argon2id-derived memory-hard
schemes so an attacker reading a fraction of process memory does not
immediately recover the secret. Significant per-access cost; opt-in
for the highest-value secrets.

### 4.2 Cryptographic Type System

Beyond `secret`, additional qualifiers reflecting cryptographic
properties: `authenticated T` for values whose origin is verified,
`forward_secret T` for values whose compromise does not compromise
past sessions, `committed T` for values cryptographically bound to a
context. The type system enforces that `decrypt()` returns
`secret authenticated T` and that operations requiring authenticated
input refuse unauthenticated values.

### 4.3 Cryptographic Memory Attestation

The broker produces signed statements about what code is executing
and what data is loaded, verifiable by a remote party. Foundation of
confidential computing scenarios (SGX, SEV-SNP, TDX, CCA on ARMv9).
Attestation is a broker primitive, not a third-party library.

### 4.4 Post-Quantum Primitives as First-Class

Standard library includes NIST-selected post-quantum algorithms (ML-KEM,
ML-DSA, SLH-DSA) with the same `secret` integration and broker-managed
key material as classical primitives. Hybrid classical/PQ key exchange
and signature helpers for the migration period.

### 4.5 Misuse-Resistant Cryptographic APIs

The cryptographic standard library is designed so misuse is hard.
Nonces for AEAD are linear types that cannot be used twice. Key
rotation is a first-class operation with type-tracked epochs.
Deprecated algorithms are not exposed in the safe API surface at all.

---

## 5. Information Flow Control

### 5.1 Flow Labels

The opt-in label system reserved in SENTINEL_DESIGN2.md Section 11.
Values carry labels like `phi`, `pii`, `tenant<T>`, `public`,
`classified<L>`. The compiler tracks where labeled data is permitted
to flow and refuses violations. Declassification is an explicit
operation recorded for audit.

### 5.2 Lattice-Based Policy

A general-purpose lattice over labels so policies can be expressed
declaratively. The lattice is project-defined, not hardcoded, so
regulated industries can express their own classification schemes.

### 5.3 Integration with Effects and Signatures

Flow labels, effects, and signing capabilities interact. A function
declaring the `network` effect cannot receive `phi`-labeled data unless
it also declares `declassify_phi`, and a signing key may or may not be
authorized to grant `declassify_phi`. The combinatorial space is large;
careful design is needed to keep signatures readable.

### 5.4 Compliance Outputs

Tooling produces machine-checkable artifacts mapping source-level
declarations to regulatory frameworks (HIPAA, GDPR, PCI-DSS, FedRAMP).
The compiler is the source of truth for "does this code handle PHI
correctly," not a separate static analysis pass.

---

## 6. Speculation and Side-Channel Hardening

### 6.1 Per-Function Mitigation Granularity

Compiler-inserted speculation barriers, hardened branches, and cache
flushes apply only to functions touching `secret` data, with the exact
mitigations tunable per target. Code that does not touch secrets pays
no cost.

### 6.2 Constant-Time Verification

A verification pass that confirms `secret`-handling functions contain
no data-dependent timing variation visible at the architectural level.
Necessarily incomplete (cache effects, predictors, prefetchers are
not architectural) but catches the software-level mistakes that
produce most real attacks.

### 6.3 Cache Partitioning Awareness

Where hardware supports cache partitioning (Intel CAT, ARM MPAM), the
broker allocates partitions so `secret`-handling code can run in a
dedicated cache way, reducing cache-based side channels.

### 6.4 Microarchitectural Data Sampling Defenses

Integration with vendor-provided MDS mitigations (VERW on Intel, DSB+ISB
on ARM) at the type-system-visible boundaries between secret and public
data. The language inserts the flush instructions automatically.

### 6.5 What Sentinel Will Not Promise

Sentinel will not claim to make insecure hardware secure. Side-channel
hardening reduces the attack surface for software-introduced leaks. It
does not eliminate side channels rooted in shared hardware state (LLC
contention, ring-bus probing, power analysis, EM emanations).
Documentation is honest about this limit.

---

## 7. Sandboxing and Confined Execution

### 7.1 Process-Lite Sandboxing

Language-level sandboxing for untrusted code that is lighter than a
process boundary but stronger than current language-level attempts.
Combines effect masking, memory budgets, arena isolation, and timing
limits.

### 7.2 WebAssembly as a Target

Sentinel compiles to Wasm with full safety guarantees preserved. The
effect system maps onto WASI capability declarations. Sentinel becomes
a natural source language for Wasm-based plugin systems, serverless
runtimes, and smart-contract platforms.

### 7.3 Cross-Language Sandboxing

Sentinel-hosted runtimes for executing untrusted code in other
languages (Lua, JS, Python) with the host enforcing capability and
resource limits the guest language cannot. The broker mediates.

### 7.4 Confidential Computing Integration

First-class support for running Sentinel code inside SGX, SEV-SNP,
TDX, and CCA enclaves with attestation primitives, sealed storage, and
enclave-aware memory regions. The threat model where the host OS is
hostile is supported natively.

---

## 8. Formal Verification and Property Checking

### 8.1 Pre/Post Conditions in the Language

`requires` and `ensures` clauses checked at runtime in debug builds,
optionally discharged statically by a verification backend. Modeled on
Dafny and Verus; the goal is incremental verification of critical
paths, not full Coq-style proof obligations.

### 8.2 Refinement Types

Types like `i32 where x > 0 && x < 1000` checked at compile time where
SMT can decide, runtime where it cannot. Useful for index safety,
protocol state machines, and cryptographic preconditions.

### 8.3 Property-Based Testing as First-Class

Fuzzing and property tests share infrastructure with the type system.
Every effect handler is a mock injection point. Every `secret` value
can be fuzzer-controlled. The broker's recording mode produces
deterministic replay for any crash.

### 8.4 Verified Subsets

A defined subset of Sentinel fully verifiable against a formal
semantics, with a verified compiler. Comparable to CompCert or CakeML
but for a contemporary safety-focused language. The natural target for
safety-critical systems (avionics, medical, automotive).

---

## 9. Concurrency and Distribution

### 9.1 Distributed Actors

Actors transparent over process and machine boundaries. The
type-checked message protocol becomes a network protocol;
serialization is generated; the broker manages identity, routing, and
delivery semantics.

### 9.2 Deterministic Concurrency

Opt-in deterministic schedulers for testing and replay. Combined with
the broker's recording mode, full deterministic replay of concurrent
executions.

### 9.3 Software Transactional Memory

STM as a built-in effect. Transactional regions are statically tracked;
the compiler refuses operations within a transaction that cannot be
rolled back.

### 9.4 GPU and Accelerator Offload

The `@gpu` region and `@simd<n>` qualifiers from SENTINEL_DESIGN2.md
Section 12 receive full treatment. Sentinel kernels for CUDA, Metal,
Vulkan compute, and AI accelerators. Heterogeneous memory managed
through the broker.

### 9.5 NUMA-Aware Allocation

`@numa(n)` region tags with broker-enforced placement and migration
policies.

---

## 10. Tooling and Ecosystem

### 10.1 Time-Travel Debugger

The broker's recording mode integrated with a graphical debugger
supporting backward stepping. Built on the LSP infrastructure from 1.0.

### 10.2 Effect Inference Visualization

IDE tooling showing inferred effects at every call site, making
capability requirements visible without forcing annotation everywhere.

### 10.3 Supply-Chain Audit Tools

`snc audit` produces a report of every effect declared by every
transitive dependency, with diffs against the previous lockfile.
Surprising capability acquisitions are flagged as security incidents.
Integrates with the signature infrastructure from Section 2.

### 10.4 Reproducible Build Verification

A separate tool that, given a Sentinel binary and its source, verifies
the binary is the byte-exact compilation of the source. Foundation of
the verifiable supply chain.

### 10.5 Package Registry with Capability and Signature Policy

The package registry enforces effect and signature policies at
publication time. Packages declare effects in manifests; the registry
refuses publication of packages whose declared effects exceed actual
need (measured by the compiler). All published packages are signed.

### 10.6 Cross-Language FFI Generation

Automated generation of safe bindings to Sentinel from C, C++, Rust,
Python, and other languages. The stable ABI from 1.0 makes this
realistic.

### 10.7 Audit Trail Aggregation

Tooling aggregates signed audit statements across an organization's
dependency graph: who reviewed what, when, against which commit. The
output is a queryable database supporting compliance reporting and
security incident response.

---

## 11. Embedded and Resource-Constrained Targets

### 11.1 Microcontroller Support

Sentinel for Cortex-M, RISC-V embedded, and similar. The `no_runtime`
mode is required. The broker is replaced with static allocation
strategies known at compile time. Effects are extended with
hardware-specific capabilities (`gpio`, `dma`, `interrupt`).

### 11.2 Real-Time Guarantees

Worst-case execution time analysis as a compiler pass. Functions can
declare timing bounds; the compiler verifies them against the target
architecture's documented timing. Useful for hard real-time control
systems.

### 11.3 Safety-Critical Certification

A defined subset of Sentinel suitable for DO-178C, ISO 26262, and IEC
62304 certification. Verified compilation, deterministic semantics, no
dynamic allocation in the certified subset, full traceability.

---

## 12. AI and Numerical Computing

### 12.1 Tensor Types

First-class tensor types with shape checking at compile time. Shape
mismatches are compile errors. Builds on the refinement-type machinery
from Section 8.2.

### 12.2 Automatic Differentiation

Differentiation as a compiler transformation rather than a library
overlay. The compiler produces forward-mode and reverse-mode
derivatives of declared-differentiable functions.

### 12.3 Accelerator Targeting

Compilation of numerical kernels to GPU, TPU, and NPU targets while
preserving Sentinel's safety guarantees. The `@gpu` region work
generalizes.

### 12.4 Privacy-Preserving Computation

Type-system support for differentially-private computation, secure
multi-party computation, and homomorphic encryption primitives.
Combines with the cryptographic type system from Section 4.2.

---

## 13. Language Evolution

### 13.1 Versioning and Editions

Per-crate edition declarations gate language changes that would
otherwise break backward compatibility. The compiler supports all
editions simultaneously. Critical for a language committed to a stable
ABI.

### 13.2 Macro System

A hygienic macro system, ideally procedural with access to the typed
HIR rather than just the AST. Carefully designed to compose with
effects, regions, and signatures.

### 13.3 Reflection

Compile-time reflection over types, with first-class type values that
can be examined and manipulated. Useful for serialization frameworks
and code generation tools.

### 13.4 Dependent Types in Limited Form

A constrained form of dependent typing for cases where refinement
types are insufficient. The full power of dependent types is out of
scope; the goal is supporting common patterns like length-indexed
vectors and protocol state machines.

### 13.5 Async/Await Refinement

As real usage patterns emerge, the effect-based async model will need
refinement: cancellation semantics, structured concurrency primitives,
async drop, async traits, async closures.

---

## 14. What Sentinel Will Never Do

This list exists to prevent revisiting decisions already made.

  - **Tracing garbage collection** as a fallback for the safe subset.
    GC would undermine the broker's ownership model and the
    predictability systems programmers require.

  - **Source compatibility with C or C++ headers.** FFI to C is
    supported through explicit declarations, not header inclusion.

  - **Class-based inheritance.** Replaced by delegation and named
    trait implementations.

  - **Implicit numeric conversions.** Every conversion is explicit.

  - **A monolithic standard library covering web frameworks, ORMs,
    or GUI toolkits.** These belong in the ecosystem.

  - **Promising to make insecure hardware secure.** Hardware
    vulnerabilities require hardware fixes; Sentinel will apply
    vendor-prescribed mitigations but will not substitute for them.

  - **Optional signature verification in production builds.** Once
    signing is part of the language, bypassing it in production is
    not a supported configuration.

  - **A central trusted authority.** No single party signs all
    packages or all keys. Trust is federated by design; the
    infrastructure must work without a global root.

  - **Replacing Rust as the default systems language.** Sentinel
    serves a more specific constituency. Mission creep has killed
    more languages than any technical limitation.

---

## 15. Process

This document is revisited annually. Each item gets one of four
dispositions: promoted to roadmap, kept in backlog, demoted to
research, or removed. New items are added freely; the backlog being
large is not a problem, but being out of date is.

Promotion requires a written design proposal showing how the item
integrates with the existing language, the broker, and the signature
infrastructure; what foundational changes (if any) are required; and
what the migration story for existing code looks like.

The signature infrastructure in Section 2 deserves special process
attention. Signing decisions are extraordinarily hard to reverse once
deployed because every pinned dependency becomes a migration problem.
A formal design review with external cryptographic expertise should
precede any implementation work on Section 2.

---

## 16. The Through-Line

Every item in this backlog ties back to Sentinel's purpose: a
flexible, high-security, high-safety framework that catches security
bugs the programmer did not intend to introduce. The features differ
in scope and ambition, but they share a common test: does this make
ordinary code more secure by default, or does it only help when the
programmer is already thinking about security?

Sentinel's bet is on the former. Languages that only reward careful
programmers do not move the security needle, because the careful
programmers were already writing secure code in C. The languages that
move the needle are the ones that make insecure code fail to compile,
fail to sign, or fail to run. That is the standard every item here
must meet.

The addition of cryptographic signatures and supply-chain provenance
in Section 2 represents the strongest such addition since the original
effect system: it pushes provenance and authorization from being a
matter of operational discipline into being a property the language
itself can verify. Combined with the existing effect system, the
secrecy qualifiers, the broker, and the reproducible-build commitment,
Sentinel becomes the first systems language where the question "did
the code I reviewed actually become the binary I'm running, signed by
the people I trust, with only the capabilities I authorized?" has a
yes-or-no answer the compiler can give.

*End of document.*
