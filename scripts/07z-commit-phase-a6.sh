#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

echo "======"
echo "PRE-COMMIT CHECKS"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 5
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 5
fi
cargo test -p sentinel-broker 2>&1 | tail -n 5
cargo test -p sentinel-broker --doc 2>&1 | tail -n 5

echo
echo "======"
echo "BRANCH + IDENTITY"
echo "======"
git rev-parse --abbrev-ref HEAD
git config user.name  || echo "(no user.name set)"
git config user.email || echo "(no user.email set)"

echo
echo "======"
echo "STAGING"
echo "======"
git add -A
git --no-pager diff --cached --stat | tail -n 30

MSG_FILE="$(mktemp)"
cat > "$MSG_FILE" <<'MSG'
broker: phase A6 — recording mode

Adds opt-in recording of broker events for replay, audit, and
debugging. Recording is disabled by default; the hot path is an
Option::None branch when no recorder is attached.

New module: recording.rs
  - Event: ArenaCreated | ArenaDestroyed | Allocated | Freed
           | BudgetOpened | BudgetClosed
    All variants carry a monotonic at_ns timestamp (u64 ns since the
    recorder was constructed). #[derive(Debug, Clone, PartialEq, Eq)].
  - Event::at_ns() accessor for ordering checks.
  - Recorder: Mutex<Vec<Event>> with two modes:
      * Recorder::unbounded() — grows freely
      * Recorder::with_capacity(n) — ring buffer, evicts oldest at cap
  - Recorder is Send+Sync; share via Arc.
  - record(), snapshot(), len(), is_empty(), clear() all safe under
    mutex poisoning (drop-on-failure semantics so recording cannot
    take down the broker).

Broker integration:
  - Broker carries Option<Arc<Recorder>>; default None.
  - Broker::with_recorder(Arc<Recorder>) constructor (construction-time
    only — no runtime swap, simpler invariants on the hot path).
  - Broker::recorder() accessor.
  - register_arena emits ArenaCreated after the map insert (summary
    captured before move).
  - destroy_arena emits ArenaDestroyed right after invalidate().
  - within_budget (both top-level and nested) emits BudgetOpened
    before the closure and BudgetClosed with used_at_close after.

Arena integration:
  - Arena gained an Option<Arc<Recorder>> field, threaded in via the
    new pub(crate) with_strategy_and_recorder constructor.
  - alloc<T> emits Allocated (size, align from Layout; arena/slot/
    slot_generation from the strategy result).
  - free<T> emits Freed only on successful strategy.free.
  - Builder (ArenaBuilder) and BudgetArenaBuilder both thread the
    broker's recorder into the arena at construction.

Design choices:
  - Construction-time attachment only. Runtime swap would require
    AtomicPtr or RwLock on every alloc; not worth the complexity.
  - Bounded ring buffer included now (~30 LOC) because audit logs
    in long-running processes need it.
  - Ordering::Relaxed-style "best effort": if the recorder mutex is
    contended or poisoned, events may be dropped silently. Recording
    is observation, not enforcement.
  - Event ordering is the recorder mutex's lock order; within a
    single thread at_ns is monotonic non-decreasing.

Tests: 63 green (55 lib + 5 integration + 2 proptest + 1 doc)
  New (7):
    - recording_disabled_by_default
    - recording_captures_basic_lifecycle
        (ArenaCreated -> Allocated -> Freed -> ArenaDestroyed)
    - recording_carries_correct_payload (arena id, name, capacity,
        size, align, slot match)
    - recording_timestamps_monotonic_per_thread
    - recording_bounded_ring_buffer_evicts_oldest
    - recording_emits_budget_open_close
    - recording_concurrent_allocations_consistent
        (16 threads x 100 allocs = 1600 events)

Scripts included for traceability:
  07-recording.sh, 07a-diagnose-shapes.sh,
  07b-arena-constructor-fix.sh, 07c-clippy-cleanups.sh,
  07d-fix-arena-created.sh, 07z-commit-phase-a6.sh
MSG

echo
echo "======"
echo "COMMIT MESSAGE"
echo "======"
cat "$MSG_FILE"

echo
echo "======"
echo "COMMITTING"
echo "======"
git commit -F "$MSG_FILE"
rm -f "$MSG_FILE"

echo
echo "======"
echo "RESULT"
echo "======"
git --no-pager log -1 --stat

echo
echo "======"
echo "DONE — push when ready: git push"
echo "======"
