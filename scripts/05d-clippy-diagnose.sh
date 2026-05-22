#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "FULL CLIPPY OUTPUT"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 || true

echo
echo "======"
echo "BUDGET.RS (full dump)"
echo "======"
cat -n "$BROKER/src/budget.rs"
