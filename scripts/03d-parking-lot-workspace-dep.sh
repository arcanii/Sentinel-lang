#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"

python3 - "$SENTINEL_ROOT" <<'PYEOF'
import re, pathlib, sys
root = pathlib.Path(sys.argv[1])
ws = root / "Cargo.toml"
src = ws.read_text()

# Locate [workspace.dependencies] section.
m = re.search(r'(?m)^\[workspace\.dependencies\]\s*$', src)
if not m:
    print("[ERR]  no [workspace.dependencies] section in workspace Cargo.toml")
    sys.exit(1)

# Find where the section ends (next [section] header or EOF).
section_start = m.end()
next_section = re.search(r'(?m)^\[', src[section_start:])
section_end = section_start + next_section.start() if next_section else len(src)
section_body = src[section_start:section_end]

if re.search(r'(?m)^parking_lot\s*=', section_body):
    print("[SKIP] parking_lot already in [workspace.dependencies]")
else:
    # Insert a parking_lot line at end of the section (before the next [...]).
    insertion = '\nparking_lot = "0.12"\n'
    new = src[:section_end].rstrip() + insertion + src[section_end:]
    ws.write_text(new)
    print('[OK]   added parking_lot = "0.12" to [workspace.dependencies]')
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "TESTS (nextest)"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -50

echo
echo "======"
echo "TESTS (cargo test)"
echo "======"
cargo test -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "03d COMPLETE"
echo "======"
