#!/usr/bin/env bash
# A9a-secret-memory-multiline.sh - fix multi-line SecretMemory initializers
# missed by the single-line regex in A9. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

echo "====== A9a PATCH START"

cat > /tmp/sentinel_a9a_patch.py <<'PYEOF'
#!/usr/bin/env python3
import re
from pathlib import Path

ROOT = Path.cwd()
SRC = ROOT / "crates" / "sentinel-broker" / "src"

# Match BrokerError::SecretMemory { ... } across lines, where the { ... }
# block contains a `reason: <expr>` field but no `os_errno` field yet.
# Use a non-greedy match bounded by the next `}` at the same nesting.
PATTERN = re.compile(
    r'BrokerError::SecretMemory\s*\{\s*'
    r'reason:\s*(?P<reason>[^,}]+?)\s*'
    r'(?P<trailing>,\s*)?'
    r'\}',
    re.DOTALL,
)

def rewrite(match):
    reason_expr = match.group("reason").strip()
    # If reason is a string literal, wrap with .to_string()
    # If it's already a String/expression, leave alone but still add .to_string()
    # if it's a &str literal. Detection: starts with `"` => literal.
    if reason_expr.startswith('"') and reason_expr.endswith('"'):
        reason_out = f'{reason_expr}.to_string()'
    elif reason_expr.endswith('.to_string()'):
        reason_out = reason_expr
    elif reason_expr.endswith('.into()'):
        reason_out = reason_expr
    else:
        # Could be a variable already typed as String, or a &str variable.
        # Safest: append .to_string() if it doesn't already look String-typed.
        # We can't fully tell, so use .to_string() which is idempotent on &String too via deref.
        reason_out = f'{reason_expr}.to_string()'
    return (
        'BrokerError::SecretMemory {\n'
        f'                    reason: {reason_out},\n'
        '                    os_errno: None,\n'
        '                }'
    )

changed_files = []
for p in SRC.rglob("*.rs"):
    txt = p.read_text()
    if "SecretMemory" not in txt:
        continue
    # Skip blocks that already have os_errno
    # Approach: split into matches and skip ones containing os_errno in body.
    new_parts = []
    last_end = 0
    for m in PATTERN.finditer(txt):
        body = m.group(0)
        if "os_errno" in body:
            # already migrated
            new_parts.append(txt[last_end:m.end()])
        else:
            new_parts.append(txt[last_end:m.start()])
            new_parts.append(rewrite(m))
        last_end = m.end()
    new_parts.append(txt[last_end:])
    new_txt = "".join(new_parts)
    if new_txt != txt:
        print(f"  UPDATE {p.relative_to(ROOT)}")
        p.write_text(new_txt)
        changed_files.append(p)
    else:
        print(f"  UNCHANGED {p.relative_to(ROOT)}")

print(f"\nFiles changed: {len(changed_files)}")
PYEOF

python3 /tmp/sentinel_a9a_patch.py

echo
echo "====== A9a PATCH DONE"
echo
echo "====== BUILD"
cargo build -p sentinel-broker 2>&1 | tail -30
echo
echo "====== CLIPPY"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -40
echo
echo "====== TESTS"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -30
else
  cargo test -p sentinel-broker 2>&1 | tail -30
fi
echo
echo "====== DOC TESTS"
cargo test -p sentinel-broker --doc 2>&1 | tail -15
echo
echo "====== A9a SCRIPT END"
