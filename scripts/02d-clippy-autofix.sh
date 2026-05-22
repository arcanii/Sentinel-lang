#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

echo "======"
echo "CLIPPY --fix (auto-applies mechanical suggestions)"
echo "======"
# --allow-dirty/--allow-staged: don't require a clean git tree.
# --lib + --tests + --benches: cover all targets we lint.
cargo clippy --fix --allow-dirty --allow-staged \
    -p sentinel-broker --lib --tests --benches 2>&1 | tail -30 || true

echo
echo "======"
echo "REMAINING CLIPPY ERRORS (if any)"
echo "======"
# Capture the full clippy output and show only the first 10 errors with context.
TMPFILE=$(mktemp)
if cargo clippy -p sentinel-broker --all-targets -- -D warnings >"$TMPFILE" 2>&1; then
    echo "[OK] clippy is clean"
else
    # Show error blocks (each error: line plus ~6 lines of context).
    grep -E "^error" "$TMPFILE" | head -20
    echo
    echo "--- full first-10-errors context ---"
    awk '/^error/{c++} c<=10{print}' "$TMPFILE" | head -120
fi
rm -f "$TMPFILE"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -10

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -25 || \
  cargo test -p sentinel-broker 2>&1 | tail -25

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "02d COMPLETE"
echo "======"
echo "If all green, commit via GitHub Desktop with message:"
echo "  broker: phase A0+A1+A2 — foundations, bump arena, lint hygiene"
