#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

info(){ printf "[INFO] %s\n" "$*"; }
ok(){   printf "[OK]   %s\n" "$*"; }
warn(){ printf "[WARN] %s\n" "$*"; }
err(){  printf "[ERR]  %s\n" "$*" >&2; }

# Pre-flight: confirm we're in a git repo and on the expected branch.
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    err "not a git repo: $SENTINEL_ROOT"
    exit 1
fi

BRANCH=$(git rev-parse --abbrev-ref HEAD)
info "current branch: $BRANCH"
if [[ "$BRANCH" != "main" ]]; then
    warn "not on main (on '$BRANCH'); proceeding anyway"
fi

# Identity check — git requires user.name and user.email to commit.
if ! git config user.name >/dev/null || ! git config user.email >/dev/null; then
    err "git user.name / user.email not configured"
    err "set them with:"
    err "  git config --global user.name  'Your Name'"
    err "  git config --global user.email 'you@example.com'"
    exit 1
fi
info "committing as: $(git config user.name) <$(git config user.email)>"

# Re-verify the broker is green before committing so we don't ship a broken tree.
echo
echo "======"
echo "PRE-COMMIT VERIFICATION"
echo "======"
info "running clippy..."
if ! cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -5; then
    err "clippy failed; aborting commit"
    exit 1
fi
ok "clippy clean"

info "running tests..."
if ! cargo nextest run -p sentinel-broker 2>&1 | tail -5; then
    err "tests failed; aborting commit"
    exit 1
fi
ok "tests passing"

info "running doc tests..."
if ! cargo test -p sentinel-broker --doc 2>&1 | tail -5; then
    err "doc tests failed; aborting commit"
    exit 1
fi
ok "doc tests passing"

# Stage everything.
echo
echo "======"
echo "STAGING CHANGES"
echo "======"
git add -A

# Show what's about to be committed.
STAGED=$(git diff --cached --name-only | wc -l | tr -d ' ')
if [[ "$STAGED" == "0" ]]; then
    warn "nothing staged; nothing to commit"
    git status --short
    exit 0
fi
info "$STAGED file(s) staged:"
git diff --cached --name-only | sed 's/^/  /'

echo
echo "diffstat:"
git diff --cached --shortstat

# Write the commit message to a temp file so multi-line bodies survive intact.
MSG_FILE=$(mktemp)
trap 'rm -f "$MSG_FILE"' EXIT

cat > "$MSG_FILE" <<'COMMIT_MSG_EOF'
broker: phase A0+A1+A2 — foundations, bump arena, destroy_arena

Implements the first three milestones of the runtime memory broker:

A0 — Dev dependencies: thiserror, tracing, proptest, criterion,
     serial_test, parking_lot, tempfile.

A1 — Foundation types: ArenaId, SlotIndex, Generation, with a
     monotonic ArenaIdCounter and Display impls.

A2 — Bump arena with generational handle safety:
     - Atomic CAS allocation with alignment up to 64 bytes
     - Manual buffer management (alloc/dealloc) for explicit control
     - SlotInfo metadata and unsafe-but-documented pointer access
     - Handle<T> carrying ArenaId + SlotIndex + Generation + Weak<Arena>
     - HandleRef<'_, T> with Deref<Target = T>, Send/Sync forwarding
     - Generation check on every get(); UseAfterFree returned cleanly

Broker API design (Option 1, "broker owns arenas"):
     - Broker stores RwLock<HashMap<ArenaId, Arc<Arena>>>
     - create_arena returns ArenaHandle (id + Arc<Arena> + alloc forwarder)
     - destroy_arena(id) removes the entry and calls arena.invalidate(),
       advancing the generation regardless of remaining Arc clones
     - Drop on Arena also invalidates as a safety net

Error model: BrokerError is a typed enum (thiserror) with UseAfterFree,
OutOfMemory, InvalidSlot, UnknownArena, BrokerPoisoned. is_use_after_free()
helper for the most-common match case.

Test coverage: 25 unit/integration tests + 2 proptest properties + 1
doctest. All green under -D warnings with pedantic clippy enabled
(crate-level allows for module_name_repetitions, doc_markdown,
missing_errors_doc, must_use_candidate).

Build scripts under scripts/02*-*.sh document each iterative fix and
are part of the commit for traceability.
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
git log -1 --stat
echo
ok "commit created on branch '$BRANCH'"
info "push with GitHub Desktop, or:  git push origin $BRANCH"
