#!/usr/bin/env bash
# 12a - replace `Apache-2.0 OR MIT` with `MIT` in workspace Cargo.toml.
# Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR" >&2; return 1 2>/dev/null || exit 1; }

python3 - <<'PYEOF'
from pathlib import Path
p = Path.cwd() / "Cargo.toml"
txt = p.read_text()
orig = txt
txt = txt.replace('license      = "Apache-2.0 OR MIT"', 'license      = "MIT"')
if txt == orig:
    if 'license      = "MIT"' in txt:
        print("  UNCHANGED Cargo.toml (already MIT)")
    else:
        print("  WARN: neither old nor new license line found verbatim")
        for i, line in enumerate(txt.splitlines(), 1):
            if "license" in line:
                print(f"    line {i}: {line!r}")
else:
    p.write_text(txt)
    print("  UPDATE Cargo.toml (license = MIT)")
PYEOF

echo
echo "====== Cargo.toml workspace.package after fix"
sed -n '/\[workspace.package\]/,/^\[/p' Cargo.toml | sed '$d'

echo
echo "====== SANITY"
cargo build --workspace 2>&1 | tail -4
