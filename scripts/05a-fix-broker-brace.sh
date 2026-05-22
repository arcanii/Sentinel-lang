#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "BROKER.RS lines 140-200 (pre-fix)"
echo "======"
sed -n '140,200p' "$BROKER/src/broker.rs"

echo
echo "======"
echo "APPLYING FIX"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()
lines = src.splitlines()

# Find the next_budget_id function and verify the structure around it.
# Strategy: count braces from start of file up to line 167 (0-indexed 166).
# If brace count is balanced at end of line 166 AND there's a lone '}' on
# line 168 (0-indexed 167), that '}' is the stray one.
def brace_balance(text):
    depth = 0
    in_str = False
    in_char = False
    in_line_comment = False
    in_block_comment = False
    i = 0
    s = text
    while i < len(s):
        c = s[i]
        n = s[i+1] if i+1 < len(s) else ''
        if in_line_comment:
            if c == '\n': in_line_comment = False
            i += 1; continue
        if in_block_comment:
            if c == '*' and n == '/':
                in_block_comment = False; i += 2; continue
            i += 1; continue
        if in_str:
            if c == '\\': i += 2; continue
            if c == '"': in_str = False
            i += 1; continue
        if in_char:
            if c == '\\': i += 2; continue
            if c == "'": in_char = False
            i += 1; continue
        if c == '/' and n == '/': in_line_comment = True; i += 2; continue
        if c == '/' and n == '*': in_block_comment = True; i += 2; continue
        if c == '"': in_str = True; i += 1; continue
        if c == "'":
            # naive: only treat as char if next chars look like a char literal
            in_char = True; i += 1; continue
        if c == '{': depth += 1
        elif c == '}': depth -= 1
        i += 1
    return depth

# Find candidate stray '}' by scanning for a line that contains only '}'
# AND that, when removed, balances total braces.
total = brace_balance(src)
print(f"total brace balance (should be 0): {total}")

if total == -1:
    # Look near line 168 for a lone '}' to remove
    target_idx = None
    for idx in range(165, min(len(lines), 180)):
        if lines[idx].strip() == '}':
            # try removing it
            trial = "\n".join(lines[:idx] + lines[idx+1:]) + ("\n" if src.endswith("\n") else "")
            if brace_balance(trial) == 0:
                target_idx = idx
                break
    if target_idx is not None:
        print(f"[OK] removing stray '}}' at line {target_idx+1}")
        new = "\n".join(lines[:target_idx] + lines[target_idx+1:])
        if src.endswith("\n"): new += "\n"
        p.write_text(new)
    else:
        print("[WARN] could not auto-locate stray brace; please inspect manually")
else:
    print("[SKIP] brace balance already 0 — nothing to do")
PY

echo
echo "======"
echo "BROKER.RS lines 140-200 (post-fix)"
echo "======"
sed -n '140,200p' "$BROKER/src/broker.rs"

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
echo "TESTS (nextest)"
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
