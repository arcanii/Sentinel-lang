# STATE.md - Sentinel Implementation Status

This document tracks what is actually built, as opposed to
HANDOVER.md which describes the long-term plan. When the two
disagree, STATE.md is the source of truth.

It is intended as a recovery point: a new contributor (or a new chat
session) should be able to read this file and understand the current
state of the crate without re-reading every commit.

Last updated: phase A6 complete.

---

## 1. Phase Tracker

| Phase | Title                                              | Status  | Commit  |
|-------|----------------------------------------------------|---------|---------|
| A0    | Dev dependencies (thiserror, tracing, proptest)    | Done    | 9c7474d |
| A1    | Foundation types (ArenaId, Generation, ...)        | Done    | 9c7474d |
| A2    | Bump arena + generational handles + destroy_arena  | Done    | 9c7474d |
| A3    | Pluggable AllocStrategy (Bump + Slab), builder     | Done    | f606d19 |
| A3.5  | Per-slot generations + slab recycling              | Done    | 37ab02b |
| A4    | Scoped allocation budgets                          | Done    | 493ee7b |
| A5    | Stats, list_arenas, where_is                       | Done    | 15d751c |
| A6    | Recording mode (event log, ring buffer)            | Done    | 2e8fb8b |
| A7    | Secret-memory policy (mlock + zero-on-free)        | Done    | f3170bf |
| A8    | Validation examples / integration demos            | Done    |          683981d |

Test coverage: 70 tests passing (62 lib + 5 integration + 2 proptest
+ 1 doctest). Clippy clean under -D warnings.

---

## 2. Crate Layout

The active crate is crates/sentinel-broker/. Other workspace crates
listed in HANDOVER.md Section 3.2 (sentinel-syntax, sentinel-ast,
etc.) are scaffold-only for now; everything in Phase A lives in the
broker crate.

    crates/sentinel-broker/
      Cargo.toml
      benches/arena_bench.rs
      src/
        lib.rs            crate root + re-exports
        arena.rs          Arena (strategy + recorder + counters)
        broker.rs         Broker, ArenaHandle, within_budget
        budget.rs         Budget, BudgetScope, BudgetArenaBuilder
        builder.rs        ArenaBuilder (non-budget)
        error.rs          BrokerError enum
        handle.rs         Handle<T>, HandleRef<'a, T>
        ids.rs            ArenaId, BudgetId, SlotIndex, Generation,
                          SlotGeneration, monotonic counters
        recording.rs      Recorder, Event (A6)
        stats.rs          BrokerStats, ArenaSummary, HandleLocation (A5)
        strategy/
          mod.rs          AllocStrategy trait + AllocOk/SlotPtr/StrategyKind
          bump.rs         BumpStrategy (Mutex<Vec<SlotInfo>>)
          slab.rs         SlabStrategy (freelist + per-slot generations)
      tests/
        integration.rs    end-to-end API tests
        proptest.rs       property-based isolation/invalidation tests

---

## 3. Public API Surface

All re-exports from sentinel_broker:

- Core: ArenaHandle, Broker, Arena, Handle, ArenaBuilder
- IDs: ArenaId, BudgetId, Generation, SlotGeneration, SlotIndex
- Strategies: AllocStrategy, StrategyKind
- Budgets (A4): Budget, BudgetScope, BudgetArenaBuilder
- Stats (A5): ArenaSummary, BrokerStats, HandleLocation
- Recording (A6): Event, Recorder
- Errors: BrokerError

### 3.1 Broker

- Broker::new()
- Broker::with_recorder(Arc<Recorder>) (A6)
- broker.create_arena(name, capacity) -> ArenaHandle
- broker.arena(name).capacity(n).bump() -> ArenaHandle
- broker.arena(name).slab(slot_size, slot_align, slot_count) -> ArenaHandle
- broker.destroy_arena(id) invalidates all handles
- broker.live_arena_count()
- broker.stats() -> BrokerStats (A5)
- broker.list_arenas() -> Vec<ArenaSummary> (A5, sorted by id)
- broker.where_is(&handle) -> Option<HandleLocation> (A5)
- broker.recorder() -> Option<&Arc<Recorder>> (A6)
- broker.within_budget(cap, |scope| { ... })? (A4, nestable)

### 3.2 Arena / ArenaHandle

- arena.alloc(value) -> Result<Handle<T>, BrokerError>
- arena.free(&handle) (slab only; bump returns NotImplemented)
- handle.get() -> Result<&T, BrokerError> (returns error on invalidation)
- handle.is_live(), arena_id(), slot(), slot_generation()

### 3.3 BrokerError variants

UseAfterFree, UseAfterFreeSlot, OutOfMemory, InvalidSlot,
UnknownArena, BrokerPoisoned, BudgetExceeded, NotImplemented.

---

## 4. Design Invariants

These are properties the test suite enforces; future changes must
preserve them.

1. Arena destruction invalidates every handle. destroy_arena(id)
   removes the broker's Arc<Arena> from its map and calls
   Arena::invalidate(), which advances generation atomically.
   Handle::get() then returns BrokerError::UseAfterFree.

2. Per-slot generations defeat ABA. Each slab slot has its own
   generation counter. Reusing a slot increments it, so a handle to
   the prior occupant returns BrokerError::UseAfterFreeSlot.

3. Bump strategy never recycles. BumpStrategy::free returns
   NotImplemented; only SlabStrategy supports free. This is a
   deliberate design choice - bump's whole point is O(1) bulk free
   via arena destruction.

4. Budgets pre-charge reserved capacity. arena("a").capacity(N).bump()
   inside within_budget(cap, ...) charges N to the budget chain BEFORE
   the arena exists. Reservation, not usage, is what counts. Nested
   budgets charge both inner and every ancestor.

5. Budget refunds are atomic. If try_charge walks the chain and
   exceeds any cap, all prior charges in that walk are refunded
   before returning BudgetExceeded.

6. Recording never affects behaviour. If no recorder is attached,
   the hot path is an Option::None branch. If recording fails
   (mutex poisoned), the event is dropped silently - recording is
   observation, not enforcement.

7. All counters use Ordering::Relaxed. Snapshots from stats() may
   show momentary inconsistency across fields under concurrent load.
   This is expected and acceptable.

8. Broker::with_recorder is construction-time only. No runtime swap,
   no AtomicPtr. The recorder is set once and read on every
   event-emitting path via &Option<Arc<Recorder>>.

---

## 5. Build & Test Commands

    cargo build -p sentinel-broker
    cargo clippy -p sentinel-broker --all-targets -- -D warnings
    cargo nextest run -p sentinel-broker
    cargo test -p sentinel-broker
    cargo test -p sentinel-broker --doc

All four must pass for any commit on main. The expected count is 63
tests as of phase A6.

---

## 6. Script Convention

Every code change in Phase A landed via a script under scripts/:

- NN-<phase>.sh          - primary generator/patch for a milestone
- NNa-..., NNb-...       - follow-up patches (lint fixes, etc.)
- NNz-commit-phase-...sh - runs pre-commit checks and creates the
                           commit with a detailed message

The scripts are committed alongside source changes so that future
contributors can see exactly what was patched, in what order. They
are also a useful debugging aid: if a build breaks, re-run the most
recent NNa- script under verbose set -x to inspect the patch.

Output convention: each script prints ====== delimited sections
(BUILD / CLIPPY / TESTS / DOC TESTS). When asking for help, paste
those sections back.

---

## 7. Phase A Complete

All eight Phase A milestones (A0-A8) are landed. The broker is
now a feature-complete, production-shape memory subsystem with
generational arenas, two allocation strategies, scoped budgets,
stats and queries, recording mode, secret-memory policy, and
three runnable validation example programs.

**Test coverage**: 62 lib + 5 integration + 2 proptest + 1 doc = 70 green.
**Clippy**: clean with `-D warnings` across crate and examples.

Next: Phase B (parser / VM / language runtime). See HANDOVER.md.

## 8. Known Limitations / Tech Debt

- Arena::with_strategy is currently #[allow(dead_code)] - it is
  kept as a convenience wrapper but only the recorder-aware variant
  with_strategy_and_recorder is used. Could be removed after A8.
- Recorder uses Mutex<Vec<Event>>; under very high concurrent
  allocation it serializes through one mutex. Acceptable for now;
  could be replaced with a lock-free MPSC ring later if profiling
  shows it matters.
- Bounded-ring Recorder::record uses Vec::remove(0) (O(n)) on
  overflow. Fine for small caps; a VecDeque would be better for
  larger ones.
- No benchmark gate in CI. benches/arena_bench.rs exists but is not
  exercised on every PR.
- Several doctests are ignored to avoid pulling in test-only types
  into the public API examples. They should be tagged no_run and
  fleshed out before publishing the crate.


### Phase A7 - secret-memory policy (f3170bf)

Opt-in protection for sensitive arenas via the builder.

- SecretPolicy { lock_memory, zero_on_free, zero_on_destroy } with STRICT / LENIENT / NONE constants.
- SecretStrategy decorator wraps any AllocStrategy; forwards alloc/free/slot_ptr and layers mlock + volatile-zero on top.
- mlock via inline extern "C" on Unix, VirtualLock on Windows. Hard-fail on errors.
- secure_zero() uses write_volatile + SeqCst compiler fence.
- Per-slot size tracked via Mutex<HashMap<SlotIndex, usize>> so zero-on-free wipes the exact extent.
- New error variant BrokerError::SecretMemory.
- 7 new tests; total now 70 green.



### Phase A8 - validation examples (683981d)

Three runnable example programs under `crates/sentinel-broker/examples/`:

- `token_bucket.rs` - high-frequency slab allocation (~100k allocs in ~30ms), generation recycling via 128-slot reuse, `where_is` lookup demo.
- `request_pipeline.rs` - scoped per-request bump arenas under `within_budget`, with recorder-based event tracing; demonstrates budget rejection.
- `credential_store.rs` - secret slab with STRICT-or-LENIENT fallback; uses unsafe raw-pointer reads via `Arena::__raw_slot_bytes_for_diagnostics` to prove zero-on-free wipes slot bytes (visual hex dump shows `alice:hunter2` -> all zeros).

API additions:
- `AllocStrategy::slot_size_hint() -> Option<usize>`: per-slot byte size for uniform strategies (slab returns Some, bump returns None).
- `Arena::__raw_slot_bytes_for_diagnostics(slot)` and `ArenaHandle::__raw_slot_bytes_for_diagnostics(slot)`: `#[doc(hidden)]` diagnostic accessor returning `(*const u8, usize)`. Unstable; for forensic tools and examples only.

