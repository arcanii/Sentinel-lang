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
broker: phase A5 — stats, list_arenas, where_is

Adds a read-only diagnostic API on Broker. No behavioural changes
to existing types; everything in this milestone is observation.

New module: stats.rs
  - BrokerStats: live_arenas, total_capacity_bytes, total_used_bytes,
    total_allocations, total_frees. All snapshot fields; safe to log,
    serialize, ship across threads.
  - ArenaSummary: id, name, kind (StrategyKind), capacity, used,
    generation, allocations, frees.
  - HandleLocation: arena, arena_name, slot, slot_generation, is_live.

Broker methods (all #[must_use]):
  - stats() -> BrokerStats
       aggregates capacity/used/alloc/free counts across all live arenas
  - list_arenas() -> Vec<ArenaSummary>
       sorted snapshot of every currently-registered arena
  - where_is<T>(&Handle<T>) -> Option<HandleLocation>
       returns None if the arena has been destroyed

Arena instrumentation:
  - Added AtomicU64 alloc_count / free_count fields.
  - alloc<T> bumps alloc_count on success (after strategy write, before
    handle is constructed — counts only successful allocations).
  - free<T> bumps free_count only on successful strategy.free.
  - New accessors: alloc_count(), free_count().

Design notes:
  - Counters use Ordering::Relaxed. Snapshots are best-effort and may
    momentarily disagree across fields under concurrent load; this is
    expected and acceptable for diagnostics.
  - list_arenas() sorts by ArenaId for stable output across calls.
  - stats() and list_arenas() return defaults (empty) if the broker's
    RwLock is poisoned, never panicking.

Tests: 55 green (48 lib + 5 integration + 2 proptest + 1 doc)
  New:
    - stats_reports_live_arenas
    - stats_tracks_allocations_and_frees
    - list_arenas_sorted_by_id_with_correct_kinds
    - list_arenas_excludes_destroyed
    - where_is_returns_some_for_live_handle
    - where_is_returns_none_after_destroy

Scripts included for traceability:
  06-stats-and-queries.sh, 06a-diagnose-alloc-free.sh,
  06b-counters-and-clippy.sh, 06z-commit-phase-a5.sh
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
