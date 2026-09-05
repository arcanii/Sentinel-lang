# SENTINEL_DESIGN2.md

**Sentinel** is a C-like, class-based systems programming language designed
around a security-first thesis: it is the systems language built for the
threats of the 2030s, not just the memory-safety threats of the 1990s.

Memory safety is the floor, not the ceiling. Sentinel treats supply-chain
attacks, side channels, secret disclosure, untrusted-code execution, and
information flow as first-class concerns of the language, enforced jointly
by the compiler and a queryable runtime memory broker.

This document supersedes SENTINEL_DESIGN.md. It preserves the core type
system (regions, ownership, nullability), the broker model, the effects
system, and the path to self-hosting, while sharpening the language's
positioning and adding a coherent set of security primitives.

---

## 1. Thesis and Positioning

### 1.1 The Security Landscape Has Moved

The bug classes that dominated CVE counts in the 1990s and 2000s — buffer
overflows, use-after-free, null dereferences, data races — are real and
still account for the majority of vulnerabilities in C and C++ code.
Rust addressed them. That fight is largely settled.

The bug classes that dominate the 2020s and will dominate the 2030s are
different. Supply-chain compromises (xz-utils, event-stream, SolarWinds),
cryptographic side channels (timing attacks, Spectre-class leaks),
secret disclosure through memory leaks, untrusted-code execution
(plugins, serverless, smart contracts), and information-flow violations
(GDPR, HIPAA, multi-tenant isolation) are where current production
languages offer little structural help.

Sentinel is designed for that landscape.

### 1.2 Positioning

Sentinel does not try to replace Rust as the default systems language.
It targets a more specific constituency: security-critical systems where
memory safety is just the starting point. Cryptographic libraries, TLS
implementations, confidential computing platforms, regulated-data
processors, plugin and sandboxing architectures, and HSM-adjacent
systems are the natural fit. These users have real budget, real pain
that current languages don't solve, and tolerance for a smaller
ecosystem in exchange for stronger guarantees.

### 1.3 The Core Principle

*The runtime is a peer to the compiler.* The memory broker, the effects
system, and the type system together enforce guarantees that no current
production language enforces jointly. Each individual feature exists in
research languages; Sentinel's contribution is integrating them into a
coherent, shippable systems language.

---

## 2. Guarantees

Sentinel provides the following guarantees as language-level invariants
when used outside `unsafe` blocks:

  - No out-of-bounds memory access.
  - No null pointer dereference.
  - No data races across threads or processes.
  - No use-after-free, including across arena drops.
  - No silent capability escalation in transitive dependencies.
  - No accidental disclosure of values tagged `secret` through logs,
    serialization, network channels, or swap.
  - No branching on secret data without explicit declassification.
  - No leaked tasks, deadlocked cross-process locks, or unhandled
    cancellations in structured concurrency scopes.

The guarantees are enforced by a combination of static checks (the
compiler refuses programs that violate them) and runtime checks (the
broker traps violations rather than allowing corruption to proceed).
Where static enforcement is possible the runtime cost is zero; where it
is not, the runtime cost is bounded and predictable.

---

## 3. Surface Syntax

Sentinel reads like a modernized C with explicit type qualifiers.

    module geometry;

    import std::io;
    import std::mem;
    import std::sync;

    class Point {
        pub let x: f64;
        pub let y: f64;

        pub init(x: f64, y: f64) {
            self.x = x;
            self.y = y;
        }

        pub fn distance(other: &Point) -> f64 {
            return sqrt((self.x - other.x)^2 + (self.y - other.y)^2);
        }
    }

Every declaration may carry up to five orthogonal qualifiers, each of
which is part of the type:

  - **Region**: `@stack`, `@heap`, `@arena<A>`, `@static`, `@shared`,
    `@gpu`, `@numa(n)` — where the value lives.
  - **Ownership**: `own`, `&`, `&mut`, `rc`, `arc` — how it is held.
  - **Nullability**: `T` for non-nullable, `?T` for optional.
  - **Secrecy**: `secret T` for values that must not leak.
  - **Integrity**: `authenticated T` for values whose origin is
    cryptographically verified (post-1.0, see Section 11).

Effects appear in function signatures rather than as type qualifiers,
because they describe what a function *does* rather than what its values
*are*. See Section 7.

---

## 4. The Type System

### 4.1 Regions Replace Lifetimes

Sentinel uses named, visible regions in place of Rust's inferred
lifetime parameters. References default to second-class — usable as
parameters and locals but not storable in heap structures or returnable
past their source — which eliminates region annotations from the
majority of code. First-class references that escape their scope use an
explicit `'esc` binding and an explicit region parameter.

    fn longest(a: &str @r, b: &str @r) -> &str @r {
        return if a.len() > b.len() { a } else { b };
    }

Error messages reference concrete regions (arenas, stack frames, shared
segments) rather than inference variables, making lifetime-class errors
substantially more comprehensible.

### 4.2 Ownership

`own T` is unique ownership. `&T` and `&mut T` are shared and exclusive
borrows with the familiar shared-XOR-mutable rule. `rc T` and `arc T`
are reference-counted, with `arc` using atomic counts and being the
only form sendable across threads. The `Send` marker extends across
process boundaries for `@shared` data.

### 4.3 Nullability

There is no `null` keyword. The optional case lives in the algebraic
type `?T` and must be matched before use.

### 4.4 Bounds Safety

Slices are fat references carrying `(ptr, len, region_tag)`. Indexing
compiles to zero overhead when provably in range and to a single
compare-and-trap branch otherwise. Raw pointer arithmetic is forbidden
outside `unsafe` blocks; pointers escaping `unsafe` must be rewrapped as
slices with explicit length, which the broker records.

### 4.5 The `secret` Qualifier

`secret T` propagates through expressions and constrains what
operations are legal on the value. Comparing two `secret [u8; 32]`
arrays compiles to a constant-time comparison. Branching on `secret
bool` is a compile error unless the value is explicitly declassified.
Allocations of `secret` data are locked into RAM (never swapped),
excluded from core dumps, zeroed on free, and forbidden from appearing
in logs, debug prints, or default serializers.

    fn verify_token(provided: secret &[u8; 32],
                    expected: secret &[u8; 32]) -> bool {
        return constant_time_eq(provided, expected);
    }

The qualifier composes with regions and ownership. `secret own [u8; 32]
@heap` is a uniquely owned, heap-allocated, secret value with all the
protections of each qualifier.

---

## 5. The Memory Broker

The broker is Sentinel's most distinctive feature. Every allocation
flows through it, every live object is registered, and the broker is
queryable, programmable, and visible to tooling.

### 5.1 Queries

    let broker = mem::broker();
    let stats  = broker.stats();
    let region = broker.where_is(&some_obj);
    let alive  = broker.is_live(handle);
    let owners = broker.owners_of(handle);

### 5.2 Arenas and Generational Handles

Every handle into an arena carries a generation tag. Dropping the arena
atomically invalidates all outstanding handles; subsequent access
raises `UseAfterFreeFault` rather than touching reused memory.

    let arena = broker.create_arena(name: "request-42", size: 4.MiB);
    let req   = arena.alloc<Request>();
    arena.drop();   // O(1) bulk free, all handles invalidated safely

### 5.3 Programmable Strategies

Allocation strategies are first-class objects. A function may declare
that all its allocations route to a thread-local bump arena that resets
on return; the compiler statically verifies no allocation escapes and
eliminates per-allocation overhead within that scope.

### 5.4 Memory Budgets

Structured budgets are a language primitive. Allocations exceeding the
budget return a typed error rather than aborting the process, enabling
graceful degradation in plugins, request handlers, and embedded
contexts.

    within budget(8.MiB) {
        let result = parse_untrusted_input(bytes)?;
        respond(result);
    } on_exceed { return Error::Overbudget; }

### 5.5 Secret Memory Policies

The broker treats `secret`-tagged allocations as a distinct policy
domain. Secret memory is mlock'd, excluded from core dumps, zeroed on
free, and isolated from non-secret allocations to reduce the blast
radius of memory-disclosure bugs. Allocator metadata for secret memory
lives in a separate structure to prevent metadata reads from leaking
secret layout.

### 5.6 Memory-Hard Secret Storage (Research)

For ultra-sensitive secrets — root keys, master credentials, signing
keys — the broker offers an opt-in `secret @hardened T` policy that
applies Argon2id-derived memory-hard storage. The secret is not held as
contiguous plaintext but as the result of memory-hard derivation over a
larger scratch space, with the scratch continuously shuffled.

This raises the cost of memory-disclosure attacks: an attacker who
reads even a substantial fraction of process memory does not
immediately recover the secret. The cost is significant per-access
computation, so this policy is opt-in for values where the threat model
justifies it (HSM-adjacent systems, TLS root keys, cryptocurrency
wallets, confidential computing).

This is a research-grade feature and is not committed to 1.0.

### 5.7 Time-Travel Recording

Running with `--record` produces a causally complete trace of every
allocation, handle invalidation, and cross-thread transfer. The trace
is deterministically replayable, enabling time-travel debugging
without external instrumentation.

---

## 6. Cross-Process Safety

The `@shared` region is backed by a broker-managed shared-memory
segment. Handles into it are process-relative offsets, valid across
`fork` and across cooperating processes that map the same segment.
Cross-process mutexes use robust futexes; if a holder dies, the next
acquirer receives a typed `LockPoisoned` error rather than deadlocking.

    let seg  = mem::shared_segment("/myapp/cache", size: 64.MiB);
    let map  = seg.root_or_init(|| PMap::<str, Entry>::new());
    let lock = sync::pmutex("/myapp/cache.lock");

    {
        let g = lock.acquire()?;
        map.insert("k", entry);
    }

Generational handles extend across process boundaries. Partial segment
unmapping is detected by the broker, and dangling cross-process handles
trap on access.

---

## 7. Effects and Capabilities

Sentinel uses an algebraic effects system in place of treating async,
errors, and capabilities as separate features. An effect is a
capability a function declares it may use; effects propagate through
call sites unless explicitly handled.

    fn read_config(path: &str) uses io, throw -> Config {
        let bytes = fs::read(path)?;
        return parse(bytes)?;
    }

### 7.1 Async as an Effect

A function using the `await` effect can be called from synchronous code
that installs a blocking handler, from asynchronous code that installs
a scheduler handler, or from tests that install a mock handler — all
without changing the function's source. Function coloring is
eliminated.

### 7.2 Capabilities and Supply-Chain Security

Effects double as capabilities. Code that does not declare the
`network`, `filesystem`, `subprocess`, `env`, or `unsafe_ffi` effects
cannot perform those operations, enforced transitively by the compiler.

A dependency's effect manifest is part of its public interface.
Importing a JSON parser that newly declares the `network` effect would
be a compile error, not a silent compromise. Build policies can enforce
rules like "no transitive dependency may declare `subprocess` without
explicit allowlisting."

This blocks an entire class of supply-chain attacks. The xz-utils
backdoor of 2024 would have failed at compile time, because the
malicious build script required capabilities that a compression library
has no business declaring.

### 7.3 Sandboxing Untrusted Code

Untrusted modules can be imported with an explicit effect mask that
restricts what they are permitted to do. Combined with memory budgets
and arena isolation, this provides language-level sandboxing that is
lighter than a process boundary and stronger than current language-
level attempts.

    let plugin = import untrusted "user_script.snt"
        with effects = { compute }
        with memory  = budget(16.MiB)
        with arena   = isolated;

The use cases are significant: editor and database plugins, serverless
function runtimes, smart-contract execution, and confidential computing
workloads where untrusted code processes sensitive data under tight
constraints.

---

## 8. Concurrency

### 8.1 Structured Concurrency

Every concurrent task has a parent scope and cannot outlive it.
Cancellation, error propagation, and resource cleanup follow the scope
hierarchy. Leaked tasks are impossible by construction.

    scope concurrent {
        let a = spawn fetch_user(id);
        let b = spawn fetch_orders(id);
        return Profile { user: a.await, orders: b.await };
    }

### 8.2 Actors

Actor types declare a statically checked message protocol. The mailbox
accepts only declared message variants, and the compiler tracks which
actors may communicate with which. Cross-process actors use the same
syntax, with the broker handling serialization and transport.

---

## 9. Side-Channel Hardening

### 9.1 Constant-Time Operations

Operations on `secret`-tagged data compile to constant-time
implementations where the target architecture supports them.
Comparisons, conditional selects, and table lookups all use branchless
or hardened code paths. The compiler refuses to generate
data-dependent branches on secret values without explicit
declassification.

### 9.2 Speculation Safety

Code operating on `secret` data automatically receives speculation
barriers, hardened branches, and the architecture-appropriate
mitigations (LFENCE, CSDB, speculative-load-hardening). Code that does
not touch secret data pays no cost. This addresses Spectre-class
leaks at the granularity of the data they would actually leak, rather
than imposing uniform mitigation across the entire program.

### 9.3 Cryptographic Primitives

⚠ **This section described an intended library, not the shipped one, and it was
read as a capability list** — a downstream consumer designed a password-KDF around
its Argon2id sentence before checking (register D55). Corrected 2026-09-05 against
`sentinel_library/std/security/`; what follows is what EXISTS.

The standard library exposes the SHA-2 family (SHA-256, SHA-512), SHA-3, HMAC and
HKDF; the AEAD constructions ChaCha20-Poly1305 and AES-GCM, plus a keyed,
sequence-numbered record sealer; SipHash; and the Edwards/Montgomery curves
Curve25519 and Curve448 (X25519, X448, Ed25519, Ed448). All operate on
`secret`-tagged buffers.

**Not implemented, and not currently planned in this document:** Argon2id or any
password-hashing KDF, BLAKE2, the NIST prime curves (P-256), and the post-quantum
selections. Argon2id in particular is ecosystem/future work — see
`docs/decisions/0030-go-no-go-tls-handshake.md`, which has always been the accurate
statement. For password-based derivation today the closest shipped primitive is
HKDF, which is a key-DERIVATION function and explicitly not a password hash;
PBKDF2-HMAC-SHA256 is requested but not present (`docs/inbound-requests.md`, R7).

⚠ **The broker's secret-memory policy does NOT cover these.** mlock + zero-on-drop
applies to `Shared<secret T>` / `Mutex<secret T>` cells and only over a scalar
payload. Every buffer in the list above is `[secret u8]`-shaped, and array-shaped
secrets do not enter that path at all — they are neither locked nor scrubbed
(register D55; the capability itself is requested as R1).

---

## 10. Classes Without Inheritance

Classes are structs plus methods plus trait implementations. There is
no class-based inheritance; composition with explicit delegation
replaces it.

    class LoggingWriter {
        delegate inner: FileWriter to Writer;
        let log: Logger;

        pub fn write(self: &mut Self, data: &[u8]) -> Result<usize> {
            self.log.info("writing {} bytes", data.len());
            return self.inner.write(data);
        }
    }

Construction goes through `init`, which must definitely assign every
field before returning. Half-constructed objects cannot be observed.

Polymorphism is provided by traits with named implementations. When two
implementations of the same trait for the same type exist, the call
site selects which to use, eliminating Rust's orphan rule without
sacrificing coherence within a given scope.

---

## 11. Information Flow Control (Post-1.0)

Beyond capability-based effects, Sentinel reserves syntax for explicit
information-flow labels. A value can carry a flow label like `phi`,
`pii`, `tenant<T>`, or `public`, and the type system tracks where data
labeled one way is permitted to flow. A `phi`-labeled medical record
cannot reach a function whose output is `public` without an explicit
declassification step that the compiler records.

This is opt-in for regulated domains (healthcare, financial services,
multi-tenant SaaS) rather than mandatory, because the ergonomics
require careful design. The 1.0 language reserves the syntax and
defines the semantic skeleton; full information-flow checking is a
post-1.0 deliverable.

---

## 12. Hardware Awareness

Execution location is part of the type system the way memory region is.
A value tagged `@gpu` lives in GPU memory; a function tagged `@simd<8>`
operates on eight-lane vectors; a type tagged `@numa(node=2)` is pinned
to a specific NUMA node. The broker manages transfers between locations
with the same rigor it manages transfers between threads, and the
compiler rejects operations that mix locations incompatibly.

    fn matmul(a: &Matrix @gpu, b: &Matrix @gpu) -> own Matrix @gpu { ... }

This positions Sentinel for heterogeneous compute as a first-class
concern rather than a library afterthought.

---

## 13. Reproducible Builds and Attestation

### 13.1 Reproducibility

Sentinel commits to bit-for-bit reproducible builds as a language-level
guarantee. No nondeterminism in code generation, no timestamps in
binaries, no path-dependent symbol mangling. Combined with the stable
ABI commitment (Section 14), this enables a software supply chain where
binary artifacts can be cryptographically tied to source code with high
assurance.

### 13.2 Attestation

The broker supports cryptographic memory attestation: a running program
can produce a signed statement about what code it is executing and what
data it has loaded, verifiable by a remote party. This is the
foundation of confidential computing (SGX, SEV, TDX) and currently
requires significant manual work in other languages. In Sentinel,
attestation is a broker primitive that integrates with the standard
library's cryptographic types.

---

## 14. Compilation Model

### 14.1 Generics

Generics compile once against witness-table dispatch by default, with
opt-in monomorphization via `@specialize` for hot paths. This reduces
compile times and binary sizes substantially compared to
all-monomorphization, at a small runtime cost most code does not
notice.

### 14.2 Stable ABI

`extern` declarations have a stable, versioned ABI from day one.
Dynamic linking of Sentinel libraries is a supported, intended use
case. Plugin architectures, shared system libraries, and
language-bridging at the binary level all work without rebuilding the
world on every compiler update.

### 14.3 Compiler as Library

The compiler is built as a salsa-style query engine and exposed as a
public library API. The language server, linters, refactoring tools,
formatters, fuzzers, and code generators all sit on the same query
engine, with incremental recompilation as a foundational property.

### 14.4 Integrated Verification

Pre- and post-conditions written in the language as `requires` and
`ensures` clauses can be checked at runtime, used to guide fuzzers, or
discharged statically by a verification backend. Every effect handler
is naturally a mock point, so property tests get effect injection for
free. Every `secret` value can be replaced with a fuzzer-controlled
value automatically. The broker's recording mode produces deterministic
replay, so any fuzzing crash is reproducible by construction.

---

## 15. Scope Discipline

A language with this many features is at high risk of becoming
academically correct but practically forbidding. The 1.0 language is
deliberately restricted to a subset that composes cleanly and ships in
a reasonable timeframe.

### 15.1 In 1.0

Core type system (regions, ownership, nullability, bounds safety).
The memory broker with arenas, generational handles, budgets, and
recording. The `secret` qualifier with constant-time operations and
speculation safety. Effects with async, errors, and capability-based
supply-chain security. Structured concurrency with actors.
Cross-process safety via `@shared`. Classes with delegation and named
trait implementations. Witness-table generics with opt-in
monomorphization. Stable ABI. Reproducible builds. Standard
cryptographic primitives.

⚠ **(register D55) This clause read "…including Argon2id", and 1.0 has since
shipped, so it was a claim about the delivered product rather than a plan.**
There is no Argon2 implementation anywhere in the tree. §9.3 above lists what the
crypto library actually contains. The rest of this section is the pre-1.0 scope
plan as written and has not been re-audited against the shipped compiler — read
[`STATE.md`](STATE.md) for delivered status, not this list.

### 15.2 Research Track (Post-1.0)

Memory-hard secret storage (`secret @hardened`). Information flow
control with flow labels. Hardware-awareness for GPU, SIMD, and NUMA.
Cryptographic attestation primitives. Integrated formal verification
backend. Memory-pressure proof-of-work as an adversarial-load defense.

### 15.3 Explicitly Out of Scope

Garbage collection as a fallback. Source compatibility with C or C++
headers. Class-based inheritance. Implicit conversions. Operator
overloading beyond a small fixed set. A standard ORM, web framework,
or other application-layer libraries — these belong in the ecosystem,
not the language.

---

## 16. Implementation Strategy

### 16.1 Bootstrap in Rust

The first compiler is written in Rust. The pipeline is a query-based
flow: lexer, parser, CST-to-AST lowering, name resolution, type and
region checking producing a typed HIR, effect inference, lowering to
SSA-form MIR, Sentinel-specific optimizations (bounds-check elision,
region escape analysis, constant-time verification), and LLVM codegen
via `inkwell`. The broker is a separate Rust crate linked into emitted
programs.

### 16.2 Path to Self-Hosting

Stage zero: bootstrap reaches feature completeness for Sentinel 1.0.
Stage one: lexer and parser ported to Sentinel.
Stage two: type checker and HIR ported.
Stage three: back end and broker bindings ported.
Stage four: fixed point — the stage-three compiler compiles its own
source to a binary that, fed its own source, produces a byte-identical
binary.

The Rust bootstrap is retained indefinitely as a reproducibility
anchor. Each Sentinel release pins which prior language version its
compiler is written in, avoiding chicken-and-egg upgrade traps.

### 16.3 Validation Before Full Commitment

Two prototypes are built before the full language is committed to.
First, the broker as a Rust crate with generational arenas,
programmable budgets, and recording mode — three to six months,
useful regardless of whether Sentinel proceeds. Second, an effects
system as a research compiler targeting interpretation — six to twelve
months, publishable as a research artifact even if abandoned. Only if
both prototypes validate the ideas does the full language project
proceed.

---

## 17. What Sentinel Gives Up

Sentinel imposes small runtime costs Rust does not: fat slices,
generational handle checks, broker bookkeeping, witness-table dispatch
for unspecialized generics, constant-time operations on secret data,
and speculation barriers in secret-handling code. The ecosystem starts
at zero and will take years to approach Rust's breadth. The combined
type system — regions, ownership, nullability, secrecy, effects — is
more complex than Rust's, and error messages must be carefully designed
to remain comprehensible.

These costs are real. Sentinel justifies them by addressing
security threats that Rust does not address structurally: supply-chain
attacks, cryptographic side channels, secret disclosure, untrusted-code
execution, and cross-process safety. For the constituencies that face
these threats — cryptographic libraries, confidential computing
platforms, regulated-data processors, plugin and sandboxing
architectures — the tradeoff is favorable.

---

## 18. Open Questions

The interaction between effects and traits, specifically whether trait
methods can declare effects polymorphically without recreating the
monad-transformer trap, remains open. The exact semantics of
cross-process generational handles under partial segment unmapping
need specification. The performance characteristics of memory-hard
secret storage on realistic workloads need measurement before
committing to a default policy. The ergonomics of named trait
implementations need user testing; the design space between Rust's
orphan rule and Scala's implicit search is wide. The integration of
information-flow labels with the effect system must be designed
carefully to avoid combinatorial explosion in type signatures.

---

## 19. Summary

Sentinel is a systems language built for the security threats of the
2030s. It keeps C's surface familiarity, adopts Rust-style ownership
with friendlier ergonomics through named regions and second-class
references, and adds a queryable memory broker, algebraic effects with
capability-based supply-chain security, the `secret` qualifier with
constant-time operations and speculation safety, structured concurrency
with cross-process safety, first-class sandboxing for untrusted code,
reproducible builds, and a stable ABI.

It does not try to replace Rust. It serves the constituency for whom
memory safety is the floor rather than the ceiling: cryptographic
systems, confidential computing platforms, regulated industries,
plugin and sandboxing architectures, and the security-critical layer of
the software stack.

The runtime is a peer to the compiler. That is the thesis. Everything
else follows.

*End of document.*
