#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

info(){ printf "[INFO] %s\n" "$*"; }
ok(){   printf "[OK]   %s\n" "$*"; }
warn(){ printf "[WARN] %s\n" "$*"; }
err(){  printf "[ERR]  %s\n" "$*" >&2; }

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    err "not a git repo: $SENTINEL_ROOT"
    exit 1
fi

BRANCH=$(git rev-parse --abbrev-ref HEAD)
info "current branch: $BRANCH"

if ! git config user.name >/dev/null || ! git config user.email >/dev/null; then
    err "git identity not configured"
    exit 1
fi
info "committing as: $(git config user.name) <$(git config user.email)>"

echo
echo "======"
echo "PRE-COMMIT VERIFICATION"
echo "======"
info "clippy..."
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -3
ok "clippy clean"

info "nextest (process-per-test, catches concurrency bugs)..."
cargo nextest run -p sentinel-broker 2>&1 | tail -3
ok "nextest passing"

info "cargo test (single-process)..."
cargo test -p sentinel-broker 2>&1 | tail -3
ok "cargo test passing"

info "doc tests..."
cargo test -p sentinel-broker --doc 2>&1 | tail -3
ok "doc tests passing"

echo
echo "======"
echo "STAGING CHANGES"
echo "======"
git add -A
STAGED=$(git diff --cached --name-only | wc -l | tr -d ' ')
if [[ "$STAGED" == "0" ]]; then
    warn "nothing staged"
    git status --short
    exit 0
fi
info "$STAGED file(s) staged:"
git diff --cached --name-only | sed 's/^/  /'

echo
echo "diffstat:"
git diff --cached --shortstat

MSG_FILE=$(mktemp)
trap 'rm -f "$MSG_FILE"' EXIT

cat > "$MSG_FILE" <<'COMMIT_MSG_EOF'
broker: phase A3 — alloc strategies, builder, slab (no recycling)

Introduces pluggable allocation strategies behind a trait object,
extracts the existing bump algorithm into a strategy implementation,
adds a fixed-size slab strategy, and exposes both through a fluent
ArenaBuilder API.

Strategy trait (src/strategy/mod.rs):
- AllocStrategy: Send + Sync trait with alloc_raw, slot_ptr, free,
  used, capacity, available, kind.
- StrategyKind enum (Bump, Slab) for diagnostics.
- AllocOk { ptr, slot } as the success return.
- Default free() returns BrokerError::NotImplemented; strategies
  opt into recycling. Per-slot generations land in A3.5.

BumpStrategy (src/strategy/bump.rs):
- Atomic CAS cursor + AtomicU32 slot counter; lock-free fast path.
- Slot metadata table guarded by parking_lot::Mutex<Vec<SlotInfo>>.

SOUNDNESS FIX: earlier revisions of the bump arena used
UnsafeCell<Vec<SlotInfo>> and asserted that the cursor CAS
serialized slot-table updates. It does not — two threads that
both win cursor CASes would race on the Vec, with potential UB
from concurrent &mut references and actual heap corruption when
Vec::push reallocates. Exposed by nextest's process-per-test
scheduling (SIGABRT in arena_concurrent_allocation under nextest,
silently passed under cargo test). The fix is a short mutex
critical section bounded by one Vec::push + one indexed write.
A new strategy::bump::concurrent_allocation_stress test (16
threads x 500 allocs) explicitly exercises this path.

SlabStrategy (src/strategy/slab.rs):
- Fixed-size, fixed-align slots; slot_size padded up to slot_align.
- Linear allocation via AtomicU32 next_slot; lock-free.
- Oversized or over-aligned requests return OutOfMemory.
- free() returns NotImplemented (A3.5 will wire recycling with
  per-slot generations to keep handle invalidation sound).

Arena refactor (src/arena.rs):
- Arena now holds Box<dyn AllocStrategy> + id, name, generation.
- All allocator-specific state moved into the strategies; arena
  is a thin policy/identity wrapper.
- alloc<T> delegates to strategy.alloc_raw and writes T in place.

ArenaBuilder (src/builder.rs):
- broker.arena("name").capacity(N).bump()
- broker.arena("name").slab(slot_size, slot_align, slot_count)
- create_arena(name, capacity) kept as a bump-default shortcut.

Test count: 33 tests under nextest (25 prior + 8 new for strategies,
builder, mixed bump/slab arenas, and the concurrency stress). All
pass under both nextest (process-per-test) and cargo test
(single-process). Clippy clean with -D warnings.

Workspace deps: parking_lot 0.12 added at workspace root.
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
