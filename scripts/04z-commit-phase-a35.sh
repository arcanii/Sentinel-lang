#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

info(){ printf "[INFO] %s\n" "$*"; }
ok(){   printf "[OK]   %s\n" "$*"; }
warn(){ printf "[WARN] %s\n" "$*"; }
err(){  printf "[ERR]  %s\n" "$*" >&2; }

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    err "not a git repo: $SENTINEL_ROOT"; exit 1
fi
BRANCH=$(git rev-parse --abbrev-ref HEAD)
info "current branch: $BRANCH"
if ! git config user.name >/dev/null || ! git config user.email >/dev/null; then
    err "git identity not configured"; exit 1
fi
info "committing as: $(git config user.name) <$(git config user.email)>"

echo
echo "======"
echo "PRE-COMMIT VERIFICATION"
echo "======"
info "clippy..."
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -3
ok "clippy clean"
info "nextest..."
cargo nextest run -p sentinel-broker 2>&1 | tail -3
ok "nextest passing"
info "cargo test..."
cargo test -p sentinel-broker 2>&1 | tail -3
ok "cargo test passing"
info "doc tests..."
cargo test -p sentinel-broker --doc 2>&1 | tail -3
ok "doc tests passing"

echo
echo "======"
echo "STAGING"
echo "======"
git add -A
STAGED=$(git diff --cached --name-only | wc -l | tr -d ' ')
if [[ "$STAGED" == "0" ]]; then
    warn "nothing staged"; git status --short; exit 0
fi
info "$STAGED file(s) staged:"
git diff --cached --name-only | sed 's/^/  /'

echo
echo "diffstat:"
git diff --cached --shortstat

MSG_FILE=$(mktemp)
trap 'rm -f "$MSG_FILE"' EXIT

cat > "$MSG_FILE" <<'COMMIT_MSG_EOF'
broker: phase A3.5 — per-slot generations + slab recycling

Promotes generational invalidation from per-arena to per-slot,
making slab recycling sound: a freed slot can be reused without
risk that stale handles into the old occupant resolve to the new
one. Bump retains the "no recycling" contract and continues to
return NotImplemented from free().

Types (src/ids.rs):
- New SlotGeneration(u32), distinct from the existing per-arena
  Generation. INITIAL = 0; Display formats as "sgenN".

Strategy trait (src/strategy/mod.rs):
- alloc_raw returns AllocOk { ptr, slot, generation }: callers
  receive the slot's generation at the moment of allocation.
- slot_ptr returns SlotPtr { ptr, generation }: callers compare
  the returned generation to the one their handle stored.
- free(slot, issued_generation): checks the issued generation
  against the slot's current; mismatch is BrokerError::UseAfterFreeSlot
  (sound double-free protection); match advances the slot's gen
  and pushes the slot onto the freelist.
- Default free() inheritance still returns NotImplemented for
  strategies that don't recycle (i.e., bump).

BumpStrategy (src/strategy/bump.rs):
- Always reports SlotGeneration::INITIAL (no recycling).
- Inherits the default free() => NotImplemented.

SlabStrategy (src/strategy/slab.rs):
- Adds Mutex<Vec<u32>> generations (parallel to slot index) and
  Mutex<Vec<u32>> freelist of recyclable slots.
- alloc_raw prefers the freelist before extending the high-water
  mark, so recycled slots are reused before fresh capacity.
- free advances the slot's generation under the same lock used
  to publish allocations, guaranteeing atomicity vs. concurrent
  reads.
- used() now reflects live slots (issued - freed) rather than
  the high-water mark, so it tracks recycling correctly.

Errors (src/error.rs):
- New variant BrokerError::UseAfterFreeSlot { arena, slot,
  issued: SlotGeneration, current: SlotGeneration }: distinguishes
  slot-level recycling UAF from arena-level destruction UAF.
- is_use_after_free() matches both variants.

Handle (src/handle.rs):
- Handle<T> now carries both arena_generation and slot_generation.
- get() checks the arena generation first (cheap, definitive for
  destruction), then asks the strategy for the slot's current
  generation and compares it. Either mismatch produces a typed
  error: UseAfterFree (arena destroyed) or UseAfterFreeSlot (slot
  recycled).

Arena (src/arena.rs):
- Arena::alloc<T> propagates the strategy's reported generation
  into the Handle.
- New Arena::free<T>(&Handle<T>): verifies arena generation,
  delegates to strategy.free.

Broker (src/broker.rs):
- ArenaHandle::free<T> forwards to Arena::free, exposing
  recycling through the public API.

Tests: 41 under nextest (33 prior + 8 new). New coverage:
- slab_free_invalidates_only_that_handle: freed handle dies,
  siblings live.
- slab_recycles_freed_slots: free + alloc reuses the same slot;
  the original handle returns UseAfterFreeSlot.
- slab_double_free_is_caught: second free of the same handle
  returns UseAfterFreeSlot.
- bump_free_returns_not_implemented_via_arena: bump's no-recycling
  contract holds through the arena wrapper.
- slab_recycling_stress: 8 threads x 200 alloc/free cycles on
  a 64-slot slab; no panics, no UB.
- bump_strategy_free_returns_not_implemented + slab_strategy_free_recycles:
  direct strategy-level tests for both strategies.
- slot_generation_display: SlotGeneration formatting.

All 41 tests pass under both nextest (process-per-test) and
cargo test. Clippy clean with -D warnings. Doctest passes.
COMMIT_MSG_EOF

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

echo
echo "======"
echo "RESULT"
echo "======"
git --no-pager log -1 --stat

echo
ok "commit created on branch '$BRANCH'"
info "push from GitHub Desktop when ready"
