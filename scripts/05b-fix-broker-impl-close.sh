#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "BROKER.RS lines 100-175 (pre-fix)"
echo "======"
sed -n '100,175p' "$BROKER/src/broker.rs"

echo
echo "======"
echo "APPLYING FIX"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
lines = p.read_text().splitlines(keepends=True)

# Find the premature '}' that closes `impl Broker` before within_budget.
# Heuristic: locate the line containing `pub fn within_budget`. Walk backwards
# to find the nearest lone '}' line. That is the stray close to remove.
target = None
for i, ln in enumerate(lines):
    if 'fn within_budget' in ln:
        target = i
        break

if target is None:
    print("[WARN] could not find within_budget; aborting")
else:
    # walk backwards
    j = target - 1
    while j >= 0 and lines[j].strip() == '':
        j -= 1
    # j should now be the '}' line
    if lines[j].strip() == '}':
        print(f"[OK] removing premature '}}' at line {j+1} (closes impl Broker too early)")
        new_lines = lines[:j] + lines[j+1:]
        p.write_text("".join(new_lines))
    else:
        print(f"[WARN] expected '}}' at line {j+1}, found: {lines[j]!r}")
PY

echo
echo "======"
echo "BROKER.RS lines 100-175 (post-fix)"
echo "======"
sed -n '100,175p' "$BROKER/src/broker.rs"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 20

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 25

echo
echo "======"
echo "TESTS"
echo "======"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 40
else
  cargo test -p sentinel-broker 2>&1 | tail -n 40
fi

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -n 10

echo
echo "======"
echo "DONE"
echo "======"
