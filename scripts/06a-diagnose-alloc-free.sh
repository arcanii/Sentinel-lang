#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "ARENA.RS — find alloc<T> body"
echo "======"
awk '/pub fn alloc<T>/,/^    \}$/' "$BROKER/src/arena.rs" | head -n 60

echo
echo "======"
echo "ARENA.RS — find free<T> body (or free method)"
echo "======"
awk '/pub fn free/,/^    \}$/' "$BROKER/src/arena.rs" | head -n 60

echo
echo "======"
echo "ARENA.RS — line numbers of relevant methods"
echo "======"
grep -n "fn alloc\|fn free\|Ok(" "$BROKER/src/arena.rs" | head -n 30
