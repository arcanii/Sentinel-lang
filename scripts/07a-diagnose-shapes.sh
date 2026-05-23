#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "ARENA.RS — with_strategy + Self struct literal"
echo "======"
awk '/fn with_strategy\(/,/^    \}$/' "$BROKER/src/arena.rs"

echo
echo "======"
echo "ARENA.RS — first 50 lines"
echo "======"
sed -n '1,50p' "$BROKER/src/arena.rs"

echo
echo "======"
echo "BROKER.RS — register_arena (full)"
echo "======"
awk '/fn register_arena/,/^    \}$/' "$BROKER/src/broker.rs"

echo
echo "======"
echo "FULL BUILD ERRORS"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 80
