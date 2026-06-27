# SENTINEL_DESIGN.md

> **Status: SUPERSEDED — original vision, kept for provenance.** The design of
> record is **[`SENTINEL_DESIGN2.md`](SENTINEL_DESIGN2.md)** — it preserves this
> document's core model (type system, memory broker, effects, the path to
> self-hosting) and sharpens the security positioning. Where the two disagree,
> DESIGN2 wins; for what is actually *shipped*, [`STATE.md`](STATE.md) is
> authoritative.

**Sentinel** is a C-like, class-based systems programming language designed
around a single thesis: *the runtime is part of the language*. The compiler
and a queryable runtime memory broker jointly enforce memory safety,
concurrency safety, and cross-process safety, while exposing fine-grained
control over allocation, regions, and effects to the programmer.

This document sketches the language's goals, type system, runtime model,
concurrency story, implementation strategy, and roadmap to self-hosting.

---

## 1. Goals and Non-Goals

### 1.1 Goals

Sentinel aims to provide three hard guarantees as language-level invariants
rather than as programmer discipline: no out-of-bounds memory access, no
null pointer dereference, and no data races across threads or processes.
It aims to keep the surface syntax and mental model close to C — curly
braces, explicit types, predictable performance, no hidden allocation —
while offering a runtime that can be queried and programmed the way most
languages let you query and program the type system.

It aims to be ergonomic for the patterns systems programmers actually
write: arena allocation, request-scoped lifetimes, cross-process shared
memory, plugin sandboxing, structured concurrency, and heterogeneous
compute. It aims for fast compilation, a stable ABI, and a tooling story
where the compiler is a library from day one.

### 1.2 Non-Goals

Sentinel is not a garbage-collected language and will not add tracing GC
as a fallback. It is not source-compatible with C or C++ headers; FFI
is explicit and crosses a checked boundary. It is not trying to replace
Rust in every niche — it competes specifically where runtime
introspection, region-based memory control, and cross-process safety
matter more than absolute zero-cost abstraction.

---

## 2. Surface Syntax

Sentinel reads like a modernized C with explicit type qualifiers. Classes
exist but inheritance is replaced by delegation; traits provide
polymorphism.

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

Every declaration carries up to three orthogonal qualifiers: a region
(`@stack`, `@heap`, `@arena<A>`, `@static`, `@shared`, `@gpu`), an
ownership mode (`own`, `&`, `&mut`, `rc`, `arc`), and a nullability marker
(`T` for non-nullable, `?T` for optional). These compose:

    let buf:   own [u8; 1024] @heap        = mem::alloc_array<u8>(1024);
    let view:  &[u8]                       = &buf[0..512];
    let maybe: ?&Point                     = registry.lookup("origin");

---

## 3. The Type System

### 3.1 Regions Replace Lifetimes

Where Rust threads inferred lifetime parameters through types, Sentinel
uses named, visible regions. A reference's region is part of its type and
appears in error messages as a concrete entity — an arena, a stack frame,
a shared segment — rather than as an inference variable.

    fn longest(a: &str @r, b: &str @r) -> &str @r {
        return if a.len() > b.len() { a } else { b };
    }

References default to *second-class*: they may be passed as parameters
and used locally but cannot be stored in heap structures or returned past
their source unless explicitly promoted to first-class with an `'esc`
region binding. This eliminates region annotations from the large
majority of code, where references never escape, and confines the
complexity to the rare cases that genuinely need it.

### 3.2 Nullability

There is no `null` keyword in safe Sentinel. The optional case lives in
the algebraic type `?T` and must be matched before use. Sugar `?.` and
`??` exist for propagation and defaulting, but the underlying type
remains explicit.

    match registry.lookup("alice") {
        some(user) => io::print(user.name),
        none       => io::print("not found"),
    }

### 3.3 Ownership

`own T` denotes unique ownership and moves on assignment. `&T` and
`&mut T` are shared and exclusive borrows respectively, with the
familiar shared-XOR-mutable rule enforced statically. `rc T` and `arc T`
are reference-counted, with `arc` using atomic counts and being the only
form sendable across threads. The compiler enforces sendability through
a `Send` marker analogous to Rust's, extended across process boundaries
for `@shared` data.

### 3.4 Bounds Safety

Arrays and slices are fat references: every `&[T]` carries `(ptr, len,
region_tag)`. Indexing compiles to zero overhead when the compiler can
prove the index is in range, and to a single compare-and-trap branch
otherwise. The trap raises a recoverable `BoundsFault` rather than
corrupting memory. Raw pointer arithmetic is forbidden in safe code;
`unsafe { ... }` blocks exist for FFI and intrinsics, and any pointer
escaping them must be rewrapped as a `Slice<T>` with an explicit length
that the broker records.

---

## 4. The Memory Broker

The broker is Sentinel's most distinctive feature. Every allocation
flows through it; every live object is registered; the broker is
queryable, programmable, and visible to tooling.

### 4.1 Queries

    let broker = mem::broker();
    let stats  = broker.stats();
    let region = broker.where_is(&some_obj);
    let alive  = broker.is_live(handle);
    let owners = broker.owners_of(handle);

### 4.2 Arenas and Generational Handles

Arenas allocate by bumping a pointer and free in bulk. Every handle into
an arena carries a generation tag; dropping the arena atomically
invalidates all outstanding handles, and subsequent access raises
`UseAfterFreeFault` rather than touching reused memory.

    let arena = broker.create_arena(name: "request-42", size: 4.MiB);
    let req   = arena.alloc<Request>();
    arena.drop();   // O(1) bulk free, all handles invalidated safely

### 4.3 Programmable Strategies

Allocation strategies are first-class objects. A function may declare
that all its allocations route to a thread-local bump arena that resets
on return; the compiler statically verifies no allocation escapes and
eliminates per-allocation overhead within that scope.

### 4.4 Budgets

Structured memory budgets are a language primitive. A `within budget(N)`
scope guarantees that allocations exceeding `N` return a typed error
rather than aborting the process, enabling graceful degradation in
plugins, request handlers, and embedded contexts.

    within budget(8.MiB) {
        let result = parse_untrusted_input(bytes)?;
        respond(result);
    } on_exceed { return Error::Overbudget; }

### 4.5 Time-Travel Recording

Running with `--record` produces a causally complete trace of every
allocation, handle invalidation, and cross-thread transfer. The trace is
deterministically replayable, enabling time-travel debugging without
external instrumentation.

---

## 5. Effects

Sentinel uses an algebraic effects system instead of treating async,
errors, and capabilities as separate features. An effect is a capability
a function declares it may use; effects propagate through call sites
unless explicitly handled.

    fn read_config(path: &str) uses io, throw -> Config {
        let bytes = fs::read(path)?;
        return parse(bytes)?;
    }

Async becomes one effect among many. A function using the `await` effect
can be called from synchronous code that installs a blocking handler,
from asynchronous code that installs a scheduler handler, or from tests
that install a mock handler — all without changing the function's
source. Function coloring is eliminated.

Effects double as capabilities: code that does not declare the `network`
effect cannot make network calls, enforced transitively by the compiler.
This provides capability-based sandboxing as a natural consequence of
the type system.

---

## 6. Concurrency

### 6.1 Structured Concurrency

Every concurrent task has a parent scope and cannot outlive it.
Cancellation, error propagation, and resource cleanup follow the scope
hierarchy. Leaked tasks are impossible by construction.

    scope concurrent {
        let a = spawn fetch_user(id);
        let b = spawn fetch_orders(id);
        return Profile { user: a.await, orders: b.await };
    }

### 6.2 Actors

Actor types declare a statically checked message protocol. The mailbox
accepts only declared message variants, and the compiler tracks which
actors may communicate with which. Cross-process actors use the same
syntax, with the broker handling serialization and transport.

### 6.3 Cross-Process Safety

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

---

## 7. Classes Without Inheritance

Classes in Sentinel are structs plus methods plus trait implementations.
There is no class-based inheritance; composition with explicit
delegation replaces it.

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

Polymorphism is provided by traits with named implementations: when two
implementations of the same trait for the same type exist (typically
from different modules), the call site selects which to use, eliminating
Rust's orphan rule without sacrificing coherence within a given scope.

---

## 8. Hardware Awareness

Execution location is part of the type system the way memory region is.
A value tagged `@gpu` lives in GPU memory; a function tagged `@simd<8>`
operates on eight-lane vectors; a type tagged `@numa(node=2)` is pinned
to a specific NUMA node. The broker manages transfers between locations
with the same rigor it manages transfers between threads, and the
compiler rejects operations that mix locations incompatibly.

    fn matmul(a: &Matrix @gpu, b: &Matrix @gpu) -> own Matrix @gpu { ... }

This positions Sentinel for heterogeneous compute — accelerators, mixed
big/little cores, distributed memory — as a first-class concern rather
than a library afterthought.

---

## 9. Compilation Model

### 9.1 Generics

Generics compile once against a witness-table dispatch by default,
following the Swift model. Hot paths opt into monomorphization with an
`@specialize` annotation. This dramatically reduces compile times and
binary sizes compared to Rust's all-monomorphization approach, at a
small runtime cost most code does not notice.

### 9.2 Stable ABI

`extern` declarations have a stable, versioned ABI from day one. Dynamic
linking of Sentinel libraries is a supported, intended use case. Plugin
architectures and shared system libraries work without rebuilding the
world on every compiler update.

### 9.3 Compiler as Library

The compiler is built as a salsa-style query engine and exposed as a
public library API. The language server, linters, refactoring tools,
formatters, and code generators all sit on the same query engine, with
incremental recompilation as a foundational property rather than a
retrofit.

---

## 10. Implementation Strategy

### 10.1 Bootstrap in Rust

The first compiler is written in Rust. Rust's ownership model,
exhaustive pattern matching, and ecosystem (`logos`, `chumsky`, `salsa`,
`inkwell`, `cranelift`) align closely with Sentinel's internal needs,
and the bootstrap exercise teaches the implementation patterns that
self-hosted Sentinel will use.

The pipeline is a query-based front-to-back flow: lexer, parser, CST
to AST lowering, name resolution, type and region checking producing a
typed HIR, effect inference, lowering to an SSA-form MIR, Sentinel-
specific optimizations (bounds-check elision, region escape analysis),
and LLVM codegen via `inkwell`. The broker is a separate Rust crate
linked into emitted programs.

### 10.2 Path to Self-Hosting

Stage zero: bootstrap reaches feature completeness for Sentinel 1.0.

Stage one: lexer and parser ported to Sentinel, output fed into the
remaining Rust pipeline.

Stage two: type checker and HIR ported to Sentinel.

Stage three: back end and broker bindings ported.

Stage four: fixed point — the stage-three compiler compiles its own
source to a binary that, fed its own source, produces a byte-identical
binary.

The Rust bootstrap is retained indefinitely as a reproducibility anchor.
Each Sentinel release pins which prior language version its compiler is
written in, avoiding chicken-and-egg upgrade traps.

---

## 11. What Sentinel Gives Up

Sentinel imposes small runtime costs Rust does not: fat slices,
generational handle checks, and broker bookkeeping each cost a few
percent in the worst case. Witness-table generics cost more than
monomorphized generics on cold paths. The ecosystem starts at zero and
will take years to approach Rust's breadth.

These are real costs. Sentinel justifies them by offering structural
capabilities Rust cannot match without breaking its own design
commitments: programmable runtime introspection, region-based safety
with named regions and second-class defaults, algebraic effects
unifying async and capabilities, first-class cross-process and
heterogeneous-compute support, and a stable ABI from day one.

---

## 12. Open Questions

A handful of design questions remain genuinely open and will need
prototyping to resolve. The interaction between effects and traits —
specifically, whether trait methods can declare effects polymorphically
— needs careful work to avoid the pitfalls Haskell encountered with
monad transformers. The exact semantics of cross-process generational
handles under partial segment unmapping need specification. The
performance characteristics of witness-table generics on tight numeric
loops need measurement before committing to the default. And the
ergonomics of named trait implementations need user testing; the design
space between Rust's orphan rule and Scala's implicit search is wide.

---

## 13. Summary

Sentinel is a systems language that treats the runtime as a peer to the
compiler. It keeps C's surface familiarity, adopts Rust-style ownership
with friendlier ergonomics through named regions and second-class
references, and adds a queryable memory broker, algebraic effects,
structured concurrency, first-class cross-process and heterogeneous
compute, and a stable ABI. It is bootstrapped in Rust and self-hosts in
four stages. It does not try to replace Rust everywhere — it competes
specifically where runtime introspection, region control, and process-
level safety matter more than absolute zero-cost abstraction.

*End of document.*
