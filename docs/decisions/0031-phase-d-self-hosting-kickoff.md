# ADR 0031: Phase D kickoff — self-hosting (the Sentinel compiler in Sentinel)

Status: PROPOSED — the Phase D kickoff ADR, opened immediately after the
Sentinel 1.0 declaration (ADR 0025 / 0030). Mirrors how ADR 0009 opened
Phase C and 0011/0021/0025 opened its sub-stages. Phase D is the project's
**largest and longest** phase; this ADR maps the terrain and fixes the
strategy — it does **not** start writing compiler code, because (D2) the
1.0 language cannot yet express a compiler.

Date: 2026-05-30
Related:
  - **HANDOVER §7** (Phase D — Self-Hosting) + **§7.2** ("Keep the Rust
    Bootstrap Alive"): the staged self-host + the rule that the Rust
    bootstrap remains the reference oracle.
  - **0001** (staged validation): self-hosting is the final validation —
    the language proving it can build itself.
  - **0029** (`abi-v1`) + **0025 D8** (reproducible builds): built in C5
    *precisely so* Phase D has a frozen, byte-identical target to emit to
    and a fixed-point to converge on. This is why D7/D8 preceded Phase D.
  - **0026** (HIR/MIR): the thick-HIR desugar + codegen-consumes-MIR
    migration were deferred from C5 as "post-1.0, still Phase-D-valuable";
    they re-enter scope here.

## Context — why now, and an honest readiness assessment

**Why now.** 1.0 is declared: the Rust bootstrap compiles the full
language, the headline constant-time `secret` guarantee is
machine-verified, and `abi-v1` + reproducible builds give a frozen,
byte-identical emission target. Self-hosting is the capstone validation —
the language eating its own dogfood — and the project's long-term shape
(a Sentinel compiler maintained in Sentinel).

**Honest readiness assessment (the load-bearing part of this ADR).** A
compiler is one of the most demanding programs to express: it reads text,
builds and walks recursive sum-typed trees, threads growable symbol
tables, and emits output. Measured against that, **the 1.0 language is
far from self-hosting-capable.** Verified gaps (none present at 1.0):

  - **No sum types / enums / pattern matching.** An AST, a token stream,
    a `Result`/error type — all are sum types. 1.0 has structs, `?T`
    (a 2-case nullable), arrays, generics, classes, traits — but **no
    general tagged union and no `match`.** This is the single biggest
    blocker; everything in a compiler is a tree of variants.
  - **No strings / `char` / byte type.** Literals are `i64`/`i32`/`bool`/
    `null` only; there is no string or byte literal and no `u8`. A
    compiler is fundamentally text processing.
  - **No growable collections.** Only fixed-size `[T]` arrays (`len`, no
    `push`); no `Vec`, no `HashMap`. Token lists and symbol tables need
    them (the broker can back them, but the surface + stdlib do not exist).
  - **No file I/O.** The only runtime I/O symbol is `sentinel_print`; there
    is no read/open/write. A compiler reads source files and writes
    objects. Needs a real stdlib (modelled via effects/handlers).
  - **No modules / multi-file.** 1.0 is single-file (ADR 0025 D9). A
    compiler is many units.
  - **No loops.** Recursion substitutes (verified at 1.0), but iterating a
    token vector or a symbol table recursively-only is a real constraint
    (deep recursion, no early break); loops are likely wanted.
  - Minor: no `<< >> ~` (ADR 0027 A1), no `%`.

**Conclusion: Phase D does not begin with "write the lexer in Sentinel."**
It begins with a **language + standard-library build-out** that makes the
language expressive enough to host a compiler — a multi-stage effort on
the order of C1–C4 again. Pretending otherwise would be the kind of
confident-but-wrong move this project's norms reject.

## Decision

### D1. Goal.

A Sentinel compiler **written in Sentinel** that compiles Sentinel to the
same `abi-v1` native code, validated by a **bootstrap fixed-point**: the
Sentinel-written compiler, compiled by the Rust bootstrap, then used to
compile *itself*, produces a byte-identical compiler (the `repro.rs`
discipline extended to the whole compiler). 1.0's reproducible-build + ABI
freeze exist to make this fixed-point well-defined.

### D2. Strategy — language build-out first, then incremental self-host.

Not a from-scratch rewrite, and not a stalled "wait for the whole stdlib."
Three movements, in order:

  1. **Foundational language + stdlib (D4)** — add the features a compiler
     needs (sum types + `match`, strings + a byte type, growable
     collections, file I/O via a stdlib, modules, probably loops), each as
     its own ADR-first sub-phase exactly like C1–C4. This is the bulk of
     Phase D's early calendar.
  2. **Incremental self-host (D5)** — once the language can express them,
     port the compiler stages front-to-back **in Sentinel** — lexer →
     parser → resolve → types/borrow/effect checks → HIR/MIR → codegen —
     each **differentially validated against the Rust `snc`** on the whole
     fixture corpus before it replaces the Rust stage.
  3. **Fixed-point + cutover (D1)** — when the Sentinel compiler compiles
     itself byte-identically, it becomes the reference; the Rust bootstrap
     is retired to a maintenance oracle (D6).

### D3. The Rust bootstrap stays alive (HANDOVER §7.2).

Throughout movements 1–2 the Rust `snc` remains the **reference oracle**:
every Sentinel-written stage is checked to produce identical output to its
Rust counterpart (the existing `tests/pass` + `tests/ui` + `repro.rs`
corpus is the differential harness). The Rust bootstrap is not deleted
until the fixed-point (D1) holds and bakes; it remains the fallback if a
self-hosted stage regresses.

### D4. The prerequisite roadmap (ordered; indicative, per-feature ADRs).

Each is a normal language sub-phase (ADR-first, feat+docs, four-check),
sequenced by how much it unblocks:

  1. **Sum types + `match` (pattern matching)** — the AST/token/`Result`
     enabler; the biggest blocker and the most foundational. **Recommended
     first (D7).**
  2. **Strings + a `u8`/byte type + string/byte literals** — text in, the
     compiler's input; a heap string type + core ops (stdlib).
  3. **Growable collections** — `Vec<T>` + a `Map` keyed on the above,
     broker-backed; generics already exist to parameterise them.
  4. **File I/O via a minimal stdlib** — read source / write artifacts,
     modelled as effects + handlers over real OS syscalls in the runtime.
  5. **Modules / multi-file** (ADR 0025 D9, deferred from 1.0) — `mod`/
     `use` + a resolve-layer module graph + separate-compilation units
     keyed to `abi-v1`. A compiler is many files.
  6. **Loops** — `while`/`for` (the surface has been recursion-only by
     design); likely wanted for the compiler's iteration-heavy passes.

These also retire long-standing deferrals: the thick-HIR desugar +
codegen-consumes-MIR migration (ADR 0026), the full escape analysis
(ADR 0026 D2), bitwise shifts (ADR 0027 A1).

### D5. The self-host sequence (movement 2).

Port stage-by-stage, smallest/most-self-contained first, each landing only
once it matches the Rust stage on the full corpus:
lexer → parser → AST → resolve → types (+ borrow / effect checks) →
HIR/MIR → codegen/runtime glue. Each stage is a Sentinel program compiled
by the *current* compiler (Rust at first, then partially self-hosted).

### D6. Out of scope here / honest sizing.

This ADR does not design any individual feature (each gets its own ADR)
and makes **no timeline promise** — Phase D is plausibly the longest phase
(the design docs budget it in quarters, after a multi-month C-phase).
Actors (ADR 0030 D3), cross-process (ADR 0025 D6), and LSP (ADR 0025 D10)
are independent post-1.0 tracks that may interleave but do not gate
self-hosting.

### D7. First sub-phase.

**D.1 = sum types + pattern matching** (its own PROPOSED ADR next): the
foundational, AST-enabling language feature, and a clean type-system +
codegen addition in the C1–C4 mould (interner variant, exhaustiveness
check, `match` lowering). Strings (D.2) and collections (D.3) follow. The
ordering in D4 is the proposal; D.1's ADR confirms it.

## Reasoning

**Why build-out-first rather than "just start the lexer."** You cannot
write a lexer in a language with no strings, a parser with no sum types,
or a symbol table with no map. Starting compiler code now would stall
immediately against the D2 gaps. The honest critical path is: make the
language expressive enough, *then* port — and the readiness assessment
(D2) is what turns that from a vague "someday" into an ordered roadmap.

**Why the Rust bootstrap is the oracle, not a thing to race.** Self-host
correctness is *defined* as "produces what the trusted Rust compiler
produces"; the differential corpus already exists. This makes each
self-hosted stage independently verifiable instead of a big-bang leap of
faith — the same staged-validation discipline (ADR 0001) the whole project
runs on.

**Why 1.0 shipped `abi-v1` + reproducible builds first.** They are Phase
D's substrate: the fixed-point (D1) is only well-defined against a frozen
ABI + deterministic emission. C5 D7/D8 were, in effect, Phase D
prerequisites paid down early.

## Consequences

### Positive
- A clear, honest, ordered path from 1.0 to a self-hosted compiler, with
  each step independently validated against the Rust oracle.
- The roadmap doubles as Sentinel's general maturation (sum types,
  strings, collections, I/O, modules, loops are wanted regardless of
  self-hosting — they make Sentinel a real general-purpose language).

### Negative
- Phase D is long; self-hosting is many sub-phases away, gated behind a
  substantial language/stdlib build-out. No quick win here.

### Neutral
- The 1.0 Rust bootstrap is unaffected and remains the production compiler
  throughout Phase D.

## Revisit

PROPOSED until D.1 (sum types) opens. Triggers:
- **D4 ordering**: if a self-host stage turns out to need a later
  prerequisite earlier (e.g. the parser wants modules sooner), reorder.
- **D2**: if a feature proves unnecessary for a *reduced* self-hostable
  subset, drop it from the critical path (a "Sentinel-0" metacircular
  subset is an alternative worth weighing at D.1).
