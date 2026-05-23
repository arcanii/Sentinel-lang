#!/usr/bin/env bash
# 11z-commit-state-update.sh - commit the STATE.md restructure after A9+B0.
# Docs-only; no build/test checks needed beyond a sanity build to confirm
# the workspace still resolves. Does NOT push.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -f docs/STATE.md ]]; then
  echo "ERROR: docs/STATE.md missing" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== STATE COMMIT START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

echo "====== SANITY: workspace builds"
cargo build --workspace 2>&1 | tail -6
SB_RC=${PIPESTATUS[0]}
echo "workspace build rc=$SB_RC"

if [[ $SB_RC -ne 0 ]]; then
  echo "ERROR: workspace build failed; aborting commit."
  return 1 2>/dev/null || exit 1
fi

echo
echo "====== REMOVE BACKUP"
if [[ -f docs/STATE.md.bak ]]; then
  rm docs/STATE.md.bak
  echo "  removed docs/STATE.md.bak"
else
  echo "  no backup present (already removed)"
fi

echo
echo "====== GIT STATUS"
git status --short

echo
echo "====== STAGING"
git add \
  docs/STATE.md \
  scripts/11-state-update-after-a9-b0.sh \
  scripts/11a-state-fix-test-count-claim.sh \
  scripts/11b-state-fix-test-count-claim-retry.sh \
  scripts/11z-commit-state-update.sh
git status --short

cat > /tmp/sentinel_state_commit_msg.txt <<'MSGEOF'
docs: restructure STATE.md for two crates (broker A9 + effects-proto B0)

STATE.md previously tracked only the broker. With sentinel-effects-proto
landed at B0 the file now needs to cover two crates with very different
purposes: a production-shape memory subsystem (broker, intended for
external adoption) and a research-grade interpreter (effects-proto,
expected to be thrown away or rewritten once its lessons land).

Per the "Option B" structure agreed during the B0 follow-up:

  - Section A: sentinel-broker (Phase A0 through A9). Phase tracker,
    crate layout, public API, design invariants (now nine, with the
    A9 panicking-vs-fallible-builders invariant added), and known
    limitations including the still-open BACKLOG section 0.1 items.
  - Section B: sentinel-effects-proto (Phase B0). Phase tracker with
    B0 done and B1-B4 planned, crate layout, B0 language grammar,
    public API surface, design decisions (hand-written RD parser
    rationale, Box-allocated AST, persistent Arc-env), and the
    intentional B0 limitations.
  - Conventions: build/test commands, the NN-/NNa-/NNz- script
    convention, and the working norms carried from HANDOVER section
    0.1 (plus one addition: avoid `set -e` and bare `exit` in pasted
    scripts because they can close the user's terminal — learned the
    hard way during A9 setup).

Honest correction landed inline: the original A9 commit message
asserted that one test was removed and was "redundant with
error_messages_are_informative". That was wrong. The actual removed
test was `strategy::slab::tests::slab_free_returns_not_implemented`,
which had survived A3.5 by accident and asserted the *opposite* of
the correct slab behavior. STATE.md section A.1 now records this
correctly. Lesson for future commits: do not assert causes for
test-count deltas without checking the diff.

Mechanical: the restructure was a full template rewrite rather than
in-place editing, so docs/STATE.md.bak was created during the patch
and removed before commit. Two follow-up scripts (11a, 11b) appear
in this commit because the first verbatim-match correction had the
wrong line breaks; 11b succeeded. Both are kept per the
NN-/NNa-/NNz- traceability convention.
MSGEOF

echo
echo "====== COMMIT"
git commit -F /tmp/sentinel_state_commit_msg.txt
COMMIT_RC=$?
echo "commit rc=$COMMIT_RC"

echo
echo "====== POST-COMMIT GIT LOG"
git log --oneline -7

echo
echo "====== STATE COMMIT END"
echo "NOTE: commit is local only. Push via Github Desktop when ready."
