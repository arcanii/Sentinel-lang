# Sentinel — One-Page Summary

**A security-first systems programming language for the threats of the 2030s.**

## The Thesis

Memory safety is the floor, not the ceiling. Rust solved the bug classes
that dominated the 1990s and 2000s. The bug classes that dominate now —
supply-chain attacks, cryptographic side channels, secret disclosure,
untrusted-code execution, information-flow violations — remain
structurally unaddressed by every production language. Sentinel is built
for that gap.

The core principle: **the runtime is a peer to the compiler**. A
queryable memory broker, an algebraic effect system, and a region-based
type system jointly enforce security guarantees that no current
production language enforces together.

## Market Gaps Sentinel Addresses

| Gap | Current State | Sentinel's Approach |
|---|---|---|
| Supply-chain attacks (xz-utils class) | Dependencies fully trusted once imported | Effects-as-capabilities; a JSON parser declaring `network` fails to compile |
| Cryptographic side channels | Library discipline (`subtle`, `zeroize`) | `secret T` type qualifier with enforced constant-time and speculation hardening |
| Secret lifecycle management | Spreadsheets and runbooks | Typed rotation policies, dependency tracking, broker-enforced expiration |
| Untrusted code execution | Heavy process/VM isolation | Language-level sandboxing via effect masking, memory budgets, arena isolation |
| Cross-process safety | Manual `memmap2` + hand-rolled sync | First-class `@shared` region, generational cross-process handles, robust mutexes |
| Code provenance | Optional package signatures, broken in practice | Signatures as part of module identity; capability-bounded trust per signing key |
| Information flow (GDPR, HIPAA) | Application discipline + logging | Opt-in flow labels enforced by the compiler |
| Compile-time bug prevention beyond memory | Static analyzers bolted on | Effects, regions, secrecy, nullability all in the type system |

## Distinguishing Features

**The Memory Broker.** A queryable, programmable runtime that manages
allocation, enforces budgets, records causally complete execution
traces for time-travel debugging, and treats `secret` memory as a
distinct policy domain. Generational handles eliminate use-after-free
even in arena and cross-process patterns.

**Regions Instead of Lifetimes.** Named, visible regions
(`@stack`, `@heap`, `@arena`, `@shared`, `@gpu`) replace inferred
lifetime parameters. References are second-class by default, eliminating
annotations from most code while preserving safety where it matters.

**Algebraic Effects, Unifying Async and Capabilities.** Effects
declare what a function may do (`io`, `network`, `await`, `throw`).
Async stops being a parallel universe with its own rules. Function
coloring is eliminated. Dependencies that try to escalate capabilities
fail at compile time.

**The `secret` Qualifier.** Sensitive data propagates through the type
system. Comparisons are constant-time. Branches on secrets require
explicit declassification. Memory is mlock'd, no-core-dump, zeroed on
free, excluded from default serialization.

**Cryptographic Signatures as Module Identity.** Two crates with the
same name and different signing keys are different modules. Trust
declarations bound capabilities per key: a compromised maintainer key
cannot grant capabilities it was never authorized to grant.

**No Inheritance, No Null, No Hidden Allocation.** Composition by
delegation, optional types via `?T`, every allocation through the
broker. The boring choices, made consistently, prevent entire bug
classes.

**Stable ABI from Day One.** Dynamic linking is a supported, intended
use case. Plugin architectures and language-bridging work without
rebuilding the world on every compiler update.

## Key Benefits Over Rust

Rust solved memory safety. Sentinel adds: capability-based supply-chain
security at compile time, first-class secret handling with side-channel
hardening, programmable runtime introspection, cross-process safety as
a language primitive, region-based ergonomics that eliminate most
lifetime annotations, and a stable ABI.

Rust will not adopt these structurally because they conflict with its
zero-cost abstraction commitment, its commitment to library-based
solutions, and its design freeze on lifetimes. Sentinel's small runtime
overhead (1-3% in typical code, larger for `secret`-handling paths) buys
guarantees Rust cannot offer.

## Target Constituencies

Sentinel does not try to replace Rust. It serves users for whom memory
safety is the starting point, not the goal: cryptographic library
authors, confidential computing platforms (SGX, SEV-SNP, TDX, CCA),
HSM-adjacent systems, regulated-industry processors (HIPAA, PCI-DSS,
GDPR), plugin and sandboxing architectures, smart-contract and
serverless runtimes, and the security-critical layer of larger
systems.

## Honest Costs

A new language ecosystem starts at zero and takes years to mature. Small
runtime overhead per `secret` operation and per cross-region check.
Steeper initial learning curve than Rust due to the additional type
dimensions (regions, effects, secrecy). Combined error messages need
careful design to remain comprehensible. No guarantees against
hardware-level vulnerabilities (Spectre-class, fault injection, microcode
bugs); those belong to the silicon and the operator.

## Implementation Status

Built, not just specified. The staged validation ran its course: the
Phase A memory broker (a Rust crate) and the Phase B effect-system
prototype (a research interpreter) validated the design, and the
**Phase C bootstrap compiler** then lowered the full language —
types, generics, borrow check + RAII, `secret` + effect typing, the
algebraic-effect handler runtime, classes/traits/delegation, structured
concurrency — to native code via LLVM, closing at **Sentinel 1.0**
(2026-05-30) with **machine-verified constant-time `secret`**.

**Phase D self-hosts.** The language grew the features a compiler needs
(sum types + `match`, strings, growable `Vec`, file I/O, loops, modules),
and the entire compiler pipeline was rewritten in Sentinel
(`selfhost/*.sentinel`), validated byte-for-byte against the Rust
bootstrap and reaching the **bootstrap fixed point** — the
Sentinel-built compiler compiles its own source. Multi-file modules with
per-unit **separate compilation** and incremental rebuilds are in.
Production-hardening and tooling (LSP) remain. The README and
`docs/STATE.md` carry the authoritative, current state.

## The Strategic Position

Memory safety is the security story of the 1990s through 2010s. Supply
chain, side channels, sandboxing, and information flow are the security
story of the 2020s and 2030s. A language explicitly designed for the
newer threat model has a clearer reason to exist than one competing on
memory safety alone — a fight Rust already won.

Sentinel's bet is that the constituencies who feel current security
pain (cryptographic engineers, confidential computing platforms,
regulated industries, security-critical infrastructure) are large
enough and motivated enough to adopt a new language that addresses
their actual problems. They have budget, they have pain, and current
languages do not solve it.

**The pitch in one sentence: Sentinel is the systems language where
"did the code I reviewed actually become the binary I'm running, signed
by the people I trust, with only the capabilities I authorized, handling
secrets correctly, free of race conditions and side channels?" has a
yes-or-no answer the compiler can give.**

---

*Companion documents: SENTINEL_DESIGN.md, SENTINEL_DESIGN2.md,
HANDOVER.md, BACKLOG.md, BACKLOG2.md, DESIGN_IDEAS.md,
SECRETS_LIFECYCLE.md, FROMJAVA.md.*
