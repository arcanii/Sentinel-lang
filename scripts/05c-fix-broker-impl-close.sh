#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "APPLYING FIX"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
text = p.read_text()
lines = text.splitlines()

# Find the line index containing the 'arena registered' tracing call.
anchor = None
for i, ln in enumerate(lines):
    if 'arena registered' in ln:
        anchor = i
        break

if anchor is None:
    print("[WARN] anchor 'arena registered' not found; aborting")
    raise SystemExit(0)

# From the anchor, find the first lone '}' (closes register_arena),
# then find the SECOND lone '}' (the premature impl Broker close) and remove it.
first_brace = None
second_brace = None
for i in range(anchor + 1, min(len(lines), anchor + 20)):
    if lines[i].strip() == '}':
        if first_brace is None:
            first_brace = i
        else:
            second_brace = i
            break

if second_brace is None:
    print(f"[WARN] could not find premature '}}' after anchor; first_brace={first_brace}")
    raise SystemExit(0)

print(f"[OK] anchor at line {anchor+1}")
print(f"[OK] first '}}' (closes register_arena) at line {first_brace+1} — keeping")
print(f"[OK] second '}}' (premature impl Broker close) at line {second_brace+1} — REMOVING")

new_lines = lines[:second_brace] + lines[second_brace+1:]
new_text = "\n".join(new_lines)
if text.endswith("\n"):
    new_text += "\n"
p.write_text(new_text)
print("[OK] file rewritten")
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
