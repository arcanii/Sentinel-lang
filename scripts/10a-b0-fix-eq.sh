#!/usr/bin/env bash
# 10a-b0-fix-eq.sh - drop Eq derive from ParseError and EvalError since
# Token (and potentially Value) only derive PartialEq. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

echo "====== B0a FIX START"

cat > /tmp/sentinel_b0a_fix.py <<'PYEOF'
#!/usr/bin/env python3
import re
from pathlib import Path

ROOT = Path.cwd()
SRC = ROOT / "crates" / "sentinel-effects-proto" / "src"

def drop_eq_from_derive(path: Path, type_name: str):
    txt = path.read_text()
    # Find the derive immediately preceding `pub enum {type_name}`.
    pattern = re.compile(
        r"#\[derive\(([^)]*)\)\]\s*\npub enum " + re.escape(type_name),
        re.MULTILINE,
    )
    m = pattern.search(txt)
    if not m:
        print(f"  WARN {path.name}: no derive before {type_name}")
        return
    traits = [t.strip() for t in m.group(1).split(",")]
    if "Eq" not in traits:
        print(f"  UNCHANGED {path.name} ({type_name} already has no Eq)")
        return
    new_traits = [t for t in traits if t != "Eq"]
    replacement = f"#[derive({', '.join(new_traits)})]\npub enum {type_name}"
    new_txt = pattern.sub(replacement, txt, count=1)
    path.write_text(new_txt)
    print(f"  UPDATE {path.name} (dropped Eq from {type_name})")

drop_eq_from_derive(SRC / "parser.rs", "ParseError")
drop_eq_from_derive(SRC / "eval.rs", "EvalError")
PYEOF

python3 /tmp/sentinel_b0a_fix.py

echo
echo "====== B0a FIX DONE"
echo
echo "====== BUILD"
cargo build -p sentinel-effects-proto 2>&1 | tail -20
echo
echo "====== CLIPPY"
cargo clippy -p sentinel-effects-proto --all-targets -- -D warnings 2>&1 | tail -40
echo
echo "====== TESTS"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-effects-proto 2>&1 | tail -40
else
  cargo test -p sentinel-effects-proto 2>&1 | tail -40
fi
echo
echo "====== DOC TESTS"
cargo test -p sentinel-effects-proto --doc 2>&1 | tail -15
echo
echo "====== B0a SCRIPT END"
