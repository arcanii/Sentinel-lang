#!/usr/bin/env bash
# 12z-commit-readme.sh - commit README.md, LICENSE.md, and Cargo.toml
# metadata fixes. Workspace builds clean as sanity check. Does NOT push.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -f README.md || ! -f LICENSE.md ]]; then
  echo "ERROR: README.md or LICENSE.md missing" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== README COMMIT START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

echo "====== SANITY: workspace builds"
cargo build --workspace 2>&1 | tail -6
B_RC=${PIPESTATUS[0]}
echo "build rc=$B_RC"

if [[ $B_RC -ne 0 ]]; then
  echo "ERROR: build failed; aborting commit."
  return 1 2>/dev/null || exit 1
fi

echo
echo "====== GIT STATUS"
git status --short

echo
echo "====== STAGING"
git add \
  README.md \
  LICENSE.md \
  Cargo.toml \
  scripts/12-readme-and-license.sh \
  scripts/12a-fix-cargo-license.sh \
  scripts/12z-commit-readme.sh
git status --short

cat > /tmp/sentinel_readme_commit_msg.txt <<'MSGEOF'
docs: add README.md and LICENSE.md; fix Cargo.toml repository + license

First public-facing front-door documentation for the repository.

README.md structure:

  - One-sentence pitch (matching SENTINEL_SUMMARY.md's tagline).
  - Brief framing paragraph naming Anie Ltd. as the entity building
    Sentinel and identifying the security-bug-class gap it targets.
  - Pointers to SENTINEL_SUMMARY.md (short-form pitch) and the two
    SENTINEL_DESIGN documents (full specification).
  - Status section: bracketed checklist of phases A through D with
    sub-checkboxes for A0-A9 (done) and B0 (done), B1-B4 (planned).
  - "What works today" featuring the three broker example programs,
    with credential_store called out as the most concrete demo of
    the security thesis.
  - Build instructions.
  - Repository layout summary.
  - "Who's building this" paragraph with single link to
    aniesolutions.ai; commercial context lives on the company site.
  - "What this is not" disclaimers (not production-ready, not stable,
    not accepting general contributions yet, not making security
    claims for end-user code today).
  - License pointer.

LICENSE.md: standard MIT license, copyright Anie Ltd. 2026. Permissive
for both research and commercial use, including the products Anie is
building on top of Sentinel.

Cargo.toml fixes:

  - repository: changed from the incorrect "https://github.com/bryan/
    Sentinel-language" placeholder to the actual repo URL
    "https://github.com/arcanii/Sentinel-lang".
  - license: changed from "Apache-2.0 OR MIT" (Rust ecosystem default)
    to "MIT" only, matching LICENSE.md.

Honest note on the patch sequence: the initial 12-readme-and-license.sh
correctly fixed the repository URL but incorrectly left the license
field as "Apache-2.0 OR MIT" because the detection logic was wrong.
12a-fix-cargo-license.sh corrected the license field. Both scripts are
included in this commit per the NN-/NNa-/NNz- traceability convention.

The 12- script uses base64-encoded payloads for README.md and
LICENSE.md to avoid heredoc-and-backtick-fence parsing issues that
broke an earlier inline-content attempt. This is the pattern HANDOVER
section 0.1 specifically recommends for terminal-pasted scripts.

Workspace builds clean. No test changes (docs-and-metadata only).
MSGEOF

echo
echo "====== COMMIT"
git commit -F /tmp/sentinel_readme_commit_msg.txt
COMMIT_RC=$?
echo "commit rc=$COMMIT_RC"

echo
echo "====== POST-COMMIT GIT LOG"
git log --oneline -8

echo
echo "====== README COMMIT END"
echo "NOTE: commit is local only. Push via Github Desktop when ready."
