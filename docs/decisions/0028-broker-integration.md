# ADR 0028: Broker integration (D4) — route the runtime heap through the Phase A broker

Status: PROPOSED — the C5.4 sub-phase ADR under ADR 0025 (Phase C5
kickoff) D4, mirroring how ADR 0026/0027 detailed earlier C5 sub-phases.
Flips to ACCEPTED(-WITH-AMENDMENTS) as C5.4 lands, recording deviations
as numbered amendments.

**C5.4 (1/N) update (2026-05-29).** Two refinements from building the
substrate: (1) **D4 was not quite "runtime-only"** — the broker is a safe
*handle* allocator that exposes no raw pointer, so a public raw-bytes API
was added (`Arena::alloc_bytes`/`ArenaHandle::alloc_bytes -> NonNull<u8>`,
surfacing the strategy's internal `alloc_raw`); it stays **c51-safe**
because codegen is untouched (objects byte-identical). The substrate
shipped as the bump-arena C-ABI (`sentinel_arena_enter`/`_alloc`/`_exit`),
*additive* — the malloc-replacement framing (slab pool / bump-no-op-free)
was set aside as the wrong shape (the broker is arena-, not malloc-,
shaped). (2) **D3 confirmed feasible without new escape analysis** — a
binding the borrow-check `DropPlan` frees at a scope exit is *provably
non-escaping*, so C5.4 (2/N) routes exactly those allocations into the
scope arena (the narrow-but-safe slice); the full escape analysis stays
post-1.0 (ADR 0026 D2). Next: C5.4 (2/N) — the scope→arena codegen.

Date: 2026-05-29
Related:
  - **0025** (Phase C5 kickoff — PROPOSED): D4 (broker integration) is the
    workstream detailed here. **Numbering note:** ADR 0025 D14 pencilled
    "ADR 0027 (broker)" and the sub-phase table put broker at C5.3 — but
    the bitwise-operator surface (discovered via the C5.0 go/no-go needing
    a constant-time XOR-accumulate MAC verify) took both the C5.3 slot and
    ADR 0027. So broker is **C5.4 / ADR 0028**; ADR 0025's sub-phase table
    shifts by one from C5.3 onward (D14 said the numbers are indicative).
  - **Phase A `sentinel-broker`** (complete): generational arena allocator
    — `Broker` → `Arena` (bump / slab `StrategyKind`) → typed
    `Handle<T>`; `Budget` / `BudgetScope` + `within_budget(cap, f)`;
    `SecretPolicy { lock_memory, zero_on_free, zero_on_destroy }`;
    `Recorder` (deterministic replay); `stats` / `where_is`. **bump is
    monotonic** (its `free` is unimplemented — you bulk-reset the arena);
    **slab is fixed-size slots** with individual free.
  - **0017** (C2 regions + RAII drop — ACCEPTED): the borrow checker's
    `DropPlan` (`moved_sources_for(fn_id)`) classifies which heap bindings
    are *moved out* (escape) vs *dropped at scope exit* (non-escaping).
    This ADR reuses that classification (D3) instead of new escape
    analysis.
  - **0019 / 0008** (secret typing + constant-time): the broker's
    secret-memory policy (zero-on-free) is the memory-side complement to
    C5.2/C5.3's constant-time `secret` — but has no current trigger (D6).
  - **0026** (HIR/MIR): D2 listed escape analysis as a post-1.0 MIR pass;
    D3 here deliberately avoids needing it by reusing the `DropPlan`.

## Context

The runtime heap is raw libc: `sentinel_alloc(size: i64) -> *mut u8`
(malloc) and `sentinel_free(ptr: *mut u8)` (free), called from generated
code (array literals C1.6, kont frames C3, task args C4.4) and from the
runtime itself, with individual free placed at scope exits by codegen
from the C2.4 `DropPlan`. The Phase A broker — a production-shape arena
allocator with budgets, a secret-memory policy, and deterministic
recording — is **built and tested but unused by compiled programs**. D4
is the integration.

**The central problem: an impedance mismatch.** The broker is an *arena*
allocator (bump = bulk-free; slab = fixed-size slots; typed generational
`Handle<T>`). The runtime path is *arbitrary-size, individual-free,
raw-pointer* malloc/free. A naive "drop-in `sentinel_alloc` → broker"
does not type-check against the broker's model: bump can't free an
individual allocation, slab can't serve arbitrary sizes, and neither
returns a raw `*u8` keyed for a later `free(ptr)`. The integration must
reconcile the two — and the *fitting* reconciliation is the one the
broker was designed for: **map Sentinel's lexical scopes/regions onto
arenas, so individual free becomes scope-exit bulk free** (ADR 0025 D4's
"richer" option, not its "drop-in minimum").

Two facts bound the 1.0 scope:
  - **No secret heap data exists yet.** `[secret T]` is unrepresentable
    (the `ArrayElem` flat subset has no `Secret` variant, ADR 0015 D6),
    and secret scalars live in registers/stack. So the secret-memory
    policy has nothing to protect today — it is scaffold (D6).
  - **The go/no-go is single-process, single-file** (ADR 0025 C5.0): no
    cross-process arenas (D6/post-1.0); its one broker hook is a
    **scoped memory budget on the handshake arena** (D5).

## Decision

### D1. Goal.

Route the runtime heap through a process-wide Phase A `Broker`, and map
Sentinel's scope/region model onto broker arenas so that scope-local heap
data is bump-allocated per scope and bulk-freed at scope exit — unlocking
scoped **budgets** (the go/no-go hook), deterministic **recording**, live
**stats**, and the **secret-memory policy** scaffold. Delivered in two
waves (D10): a runtime-only foundation, then the scope→arena codegen.

### D2. The impedance mismatch is the design (recap as a decision).

We do **not** force the broker to behave like malloc. bump's bulk-free
and slab's fixed slots are features to map onto, not limitations to work
around: Sentinel's RAII already frees heap bindings at scope exit, which
is exactly an arena reset. The mapping (D3) turns N individual frees into
one arena reset.

### D3. Scope → arena via the `DropPlan` (no new escape analysis).

The borrow checker's `DropPlan` already separates, per function, the
bindings that are **moved out** (escape their scope — `moved_sources_for`)
from those **dropped at scope exit** (non-escaping; the ones codegen
currently `sentinel_free`s). The non-escaping, scope-exit-freed bindings
are exactly the allocations safe to place in a per-scope **bump arena**
that is reset on scope exit. So:

  - each Sentinel scope with scope-local heap drops gets a broker bump
    arena (created at scope entry, reset/destroyed at scope exit);
  - allocations for its non-escaping heap bindings go in that arena;
  - the per-binding `sentinel_free` calls at that scope exit collapse to a
    single arena reset;
  - **moved / returned (escaping) values stay on the existing path**
    (general allocation), since they outlive the scope — tighter
    placement for them needs the full escape analysis ADR 0026 D2 defers
    post-1.0.

This reuses existing borrow-check information; it adds **no new analysis
pass**, only a codegen change to route allocation + free through arenas.

### D4. The C5.4 minimum — a runtime-only broker foundation (C5.4 (1/N)).

Before the scope→arena codegen (D3), ship the foundation as a
**runtime-only** change: a process-wide `Broker` backing
`sentinel_alloc` / `sentinel_free` via a **growable, size-classed slab
pool** (power-of-two size classes; grow by adding slab arenas) plus a
global pointer → `Handle` registry so individual `sentinel_free(ptr)`
still works. Because the *codegen-emitted calls are unchanged* (same
`sentinel_alloc(size)` / `sentinel_free(ptr)` C-ABI symbols, new
implementation), **the emitted objects stay byte-identical** — the C5.1
`c51` / `repro.rs` bar holds trivially, with zero codegen risk. This
proves the wiring and lights up budgets / recording / stats / the
secret-policy hook for everything above it.

**Fallback (recorded, per ADR 0025 D2's escape-hatch discipline):** if
the growable slab pool over-runs the C5.4 budget, a process-wide **bump**
arena with a no-op `sentinel_free` (bulk-freed at process exit) is the
smaller foundation — acceptable for the short-lived single-process
go/no-go, at the cost of leaking within a run (a semantic regression that
the D3 scope→arena wave then repairs). Invoking it is an amendment.

### D5. Budgets — the go/no-go hook.

The broker's `within_budget(cap, f)` enforces a byte ceiling
(`BudgetClosed` on overrun). C5.4 (1/N) exposes the budget *infrastructure*
(the process-wide broker is budget-capable; a runtime test exercises
`within_budget`). A **`scope budget(N) { … }` surface** that lets a
Sentinel scope declare a budget the broker enforces is **new syntax** —
deferred to its own decision (a follow-on ADR or a C5.4 (2/N) addition),
since it touches lexer→parser→types→codegen.

### D6. Secret-memory policy — scaffold only.

Wire the `SecretPolicy` plumbing (a secret-designated arena with
`zero_on_free`) but leave it **inert**: there is no secret heap data
today (no `[secret T]`; secret scalars are register/stack). It activates
when an arrays-of-secrets surface lands (its own deferred follow-on). The
hook is the memory-side complement to the C5.2/C5.3 constant-time work.

### D7. Reproducibility + the c51 bar.

C5.4 (1/N) is runtime-only → emitted objects unchanged → `repro.rs`
byte-identical + every `tests/pass` exit/stdout unchanged. C5.4 (2/N)
(the scope→arena codegen) *does* change emission; it must hold the same
bar (objects compile-twice-identical; no pass fixture changes its
exit/stdout). The broker's own determinism (source-order arena ids,
recording) supports ADR 0025 D8.

### D8. Out of scope at C5.4 (post-1.0 / later).

Full escape analysis for tighter non-`DropPlan` placement (ADR 0026 D2);
a per-region **arena tree** mirroring the lexical region tree (D3 ships
flat per-scope arenas first); cross-process arenas + capability
enforcement at the process boundary (ADR 0025 D6, post-1.0); the
`scope budget` **surface syntax** (D5, follow-on); deterministic-replay
**recording** wired to a user feature (the infra is present, ADR 0025 D8);
and the arrays-of-secrets surface that would activate D6.

### D9. Phase-go + fixtures.

  - **`c54_broker_arena` (pass):** a program that heap-allocates and frees
    (e.g. an array built, used, dropped at scope exit) runs with the
    **same exit/stdout** broker-backed as on libc — the integration is
    behaviour-preserving.
  - **runtime tests:** the broker-backed `sentinel_alloc`/`sentinel_free`
    round-trips (alloc → write → read → free); `within_budget` enforces a
    cap (`BudgetClosed` past it); the size-classed pool grows.
  - **C5.4 (2/N), when it lands:** a fixture whose scope-exit drops are
    served by a per-scope arena reset (asserted via broker `stats` /
    recording in a runtime-level test), still exit/stdout-identical.

### D10. Sub-phase split.

| Sub        | Title                                                           | Risk   | Est.        |
|------------|-----------------------------------------------------------------|--------|-------------|
| C5.4 (1/N) | Runtime-only broker foundation (D4): process-wide `Broker`      | medium | 1 session   |
|            | backing `sentinel_alloc`/`free` via a size-classed slab pool +  |        |             |
|            | ptr→handle registry; budget/recording/stats infra. c51-safe     |        |             |
|            | (codegen untouched). `c54_broker_arena` + runtime tests.        |        |             |
| C5.4 (2/N) | Scope → arena via the `DropPlan` (D3): codegen places           | high   | 1-2 sessions|
|            | non-escaping scope-local allocs in per-scope bump arenas, reset |        |             |
|            | at scope exit; optional `scope budget` surface (D5). Holds the  |        |             |
|            | c51 bar. **May defer post-1.0** if it threatens the budget.     |        |             |

Total ~2-3 sessions, consistent with ADR 0025's C5.3/now-C5.4 band
(1-2). C5.4 (1/N) ships the integration's value (broker wired, budgets
live) at low risk; C5.4 (2/N) is the region-faithful payoff and carries
the codegen risk.

## Reasoning

**Why reuse the `DropPlan` instead of escape analysis.** The borrow
checker already computed exactly the escape classification the arena
mapping needs (moved-out = escapes; dropped-at-scope-exit = safe to
arena-place). Reusing it makes the region mapping a *codegen* change, not
a new *analysis*, and keeps the full escape-analysis pass (ADR 0026 D2)
genuinely post-1.0.

**Why a runtime-only foundation first.** Keeping the codegen-emitted
`sentinel_alloc`/`free` calls fixed makes C5.4 (1/N) byte-identical at the
object level — the integration's risk is then entirely inside the runtime
crate, behind a stable C-ABI, and the strict `c51` bar is met for free.
It also lights up budgets/recording immediately, so C5.4 (2/N) builds on a
proven broker rather than introducing the broker and the codegen change at
once.

**Why budgets are the only go/no-go-forced piece.** The C5.0 resolution's
TLS handshake is single-process/single-file, so cross-process and the
arena tree are out; its one concrete broker need is a **scoped budget on
the handshake arena**. C5.4 (1/N) makes the broker budget-capable; the
`scope budget` surface (D5) is the smallest addition that exposes it.

## Consequences

### Positive
- The Phase A broker — the project's first and largest runtime component
  — finally backs compiled programs; budgets / recording / stats / the
  secret-memory policy become reachable. RAII scope-exit frees become
  arena resets (fewer, cheaper) once D3 lands.
- The foundation (1/N) is object-byte-identical, so it carries zero
  regression risk against the whole existing suite.

### Negative
- C5.4 (2/N) is a real codegen change to allocation + drop lowering, with
  the escaping-value path as the subtle case; gated behind the proven 1/N
  foundation, and may defer post-1.0.
- The size-classed slab pool + global ptr→handle registry must be
  thread-safe (the C4.4 runtime spawns threads) — concurrency the raw
  malloc path got from libc.

### Neutral
- No surface-syntax change at C5.4 (1/N); programs behave identically
  (same exit/stdout), now broker-backed. The `scope budget` surface (D5)
  is the only user-visible addition, and it is deferred.

## Alternatives considered

- **Drop-in `sentinel_alloc` → broker (ADR 0025 D4 "minimum").** Rejected
  as literally specified: the broker's arena model has no arbitrary-size
  individual-free path, so "drop-in" forces either a slab pool (which is
  D4 here, kept as the *foundation* not the end) or a no-op free (the
  recorded fallback). The honest minimum is the runtime foundation, not a
  one-line swap.
- **Go straight to scope→arena codegen (skip the runtime foundation).**
  Rejected: introduces the broker and a high-risk codegen change at once,
  forfeiting the byte-identical safety net of a runtime-only first step.
- **Full escape analysis now.** Rejected: ADR 0026 D2 defers it post-1.0,
  and the `DropPlan` already supplies the needed classification (D3).

## Revisit

PROPOSED until C5.4 closes. Per-D triggers:
- **D4**: revisit at C5.4 (1/N) — if the growable slab pool over-runs,
  invoke the bump + no-op-free fallback via amendment.
- **D3/D7**: revisit at C5.4 (2/N) — if the scope→arena codegen threatens
  the c51 bar or the budget, defer it post-1.0 (the 1/N foundation still
  delivers the broker + budgets).
- **D5**: revisit when the go/no-go is assembled — confirm the
  `scope budget` surface shape against its actual need.

## Appendix: estimated implementation footprint

| Workstream                                              | LOC estimate |
|--------------------------------------------------------|--------------|
| runtime: broker-backed alloc/free + slab pool + registry | ~250-400   |
| runtime tests (round-trip, budget, pool growth)        | ~120         |
| `c54_broker_arena` fixture + driver test               | ~30          |
| **C5.4 (1/N) total**                                   | **~400-550** |
| C5.4 (2/N): codegen scope→arena + `DropPlan` reuse     | ~300-600     |
| optional `scope budget` surface (lexer→codegen)        | ~200-350     |
