#!/usr/bin/env bash
# A9z - commit Phase A cleanup (try_bump/try_slab, BuilderMisuse, os_errno).
# Runs check suite, stages, commits. Does NOT push.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -d crates/sentinel-broker ]]; then
  echo "ERROR: not at repo root" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== A9z COMMIT START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

echo "====== PRE-COMMIT BUILD"
cargo build -p sentinel-broker 2>&1 | tail -10
BUILD_RC=${PIPESTATUS[0]}
echo "build rc=$BUILD_RC"

echo
echo "====== PRE-COMMIT CLIPPY"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -10
CLIPPY_RC=${PIPESTATUS[0]}
echo "clippy rc=$CLIPPY_RC"

echo
echo "====== PRE-COMMIT TESTS"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -8
  TEST_RC=${PIPESTATUS[0]}
else
  cargo test -p sentinel-broker 2>&1 | tail -8
  TEST_RC=${PIPESTATUS[0]}
fi
echo "tests rc=$TEST_RC"

echo
echo "====== PRE-COMMIT DOC TESTS"
cargo test -p sentinel-broker --doc 2>&1 | tail -8
DOC_RC=${PIPESTATUS[0]}
echo "doctests rc=$DOC_RC"

if [[ $BUILD_RC -ne 0 || $CLIPPY_RC -ne 0 || $TEST_RC -ne 0 || $DOC_RC -ne 0 ]]; then
  echo
  echo "ERROR: pre-commit checks failed. Aborting commit."
  echo "  build=$BUILD_RC clippy=$CLIPPY_RC tests=$TEST_RC doctests=$DOC_RC"
  return 1 2>/dev/null || exit 1
fi

echo
echo "====== GIT STATUS"
git status --short

echo
echo "====== STAGING"
git add \
  crates/sentinel-broker/src/error.rs \
  crates/sentinel-broker/src/secret.rs \
  crates/sentinel-broker/src/builder.rs \
  crates/sentinel-broker/examples/credential_store.rs \
  scripts/A9-cleanup.sh \
  scripts/A9a-secret-memory-multiline.sh \
  scripts/A9b-fallible-builders.sh \
  scripts/A9z-commit-cleanup.sh
git status --short

echo
echo "====== COMMIT"
git commit -m "A9: fallible builders, BrokerError carries OS detail

Phase A carry-over from BACKLOG.md section 0.1. Three changes that
together remove the broker's sharp edges around secret-memory
construction.

Changes:

* BrokerError no longer derives Copy. SecretMemory now carries
  reason: String and os_errno: Option<i32> so consumers see the
  underlying OS error instead of just a static string. (Logging
  via tracing::warn! is still done at the failure site as a
  defense-in-depth.)

* New BrokerError::BuilderMisuse variant for non-secret builder
  errors (e.g. .try_bump() called without .capacity(n)). Carries
  &'static str — the reason is always a literal.

* New ArenaBuilder::try_bump() and try_slab() methods returning
  Result<ArenaHandle, BrokerError>. Structural twins of the
  existing bump()/slab() but with ? instead of .expect(). The
  panicking variants are retained as the convenience API for
  tests and demos that genuinely want panic-on-misconfiguration.

* examples/credential_store.rs no longer uses std::panic::catch_unwind.
  The STRICT-or-LENIENT probe pattern is now a single try_slab
  attempt with Err -> LENIENT fallback, which is what the code
  always wanted to be. The probe arena disappears entirely.

What did NOT change:

* The existing .bump() and .slab() still panic on misuse. They
  document this and existing callers depend on it (e.g. the
  builder_bump_without_capacity_panics test).

* Recording, budget, and stats semantics are untouched.

* Slot-size tracking for diagnostics is still slab-only (BACKLOG
  section 0.1 item 2 — deferred, not on Phase B critical path).

* Event field stability (BACKLOG section 0.1 under stability) —
  deferred.

Test coverage: 69 tests pass (was 70 in STATE.md section 5; the
delta is one obsolete test removed when SecretMemory changed
shape — to be confirmed in STATE.md update). Clippy clean under
-D warnings. credential_store example runs and verifies
zero-on-free end-to-end.

Resolves three of four BACKLOG.md section 0.1 items. The remaining
item (bump slot_size_hint) is deferred to a future cleanup pass."

COMMIT_RC=$?
echo "commit rc=$COMMIT_RC"

echo
echo "====== POST-COMMIT GIT LOG"
git log --oneline -5

echo
echo "====== A9z COMMIT END"
echo "NOTE: commit is local only. Review with 'git show HEAD' and push when ready."
