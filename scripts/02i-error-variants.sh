#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

echo "======"
echo "DUMP: error.rs (full)"
echo "======"
cat "$BROKER/src/error.rs"

echo
echo "======"
echo "APPLYING PATCH"
echo "======"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
err_path = broker / "src/error.rs"
src = err_path.read_text()

if "UnknownArena" in src and "BrokerPoisoned" in src:
    print("[SKIP] variants already present")
    sys.exit(0)

# Find the BrokerError enum body and append the two variants before
# its closing brace. We locate the matching brace by counting depth.
m = re.search(r'pub enum BrokerError\s*\{', src)
if not m:
    print("[ERR]  could not locate `pub enum BrokerError {`")
    sys.exit(1)

start = m.end()
depth = 1
i = start
while i < len(src) and depth > 0:
    if src[i] == '{':
        depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            close = i
            break
    i += 1
else:
    print("[ERR]  could not find matching closing brace for BrokerError")
    sys.exit(1)

body = src[start:close]
uses_thiserror = '#[error(' in body
indent = "    "

# Find indentation actually used in the enum (variant lines).
indent_match = re.search(r'\n(\s+)#\[error', body)
if indent_match:
    indent = indent_match.group(1)
else:
    indent_match = re.search(r'\n(\s+)[A-Z][A-Za-z0-9_]+\s*[\{\(,]', body)
    if indent_match:
        indent = indent_match.group(1)

extra_lines = []
if uses_thiserror:
    extra_lines += [
        "",
        f"{indent}/// The requested arena is not registered with this broker.",
        f"{indent}#[error(\"unknown arena: {{arena}}\")]",
        f"{indent}UnknownArena {{ arena: crate::ids::ArenaId }},",
        "",
        f"{indent}/// The broker's internal lock is poisoned by a prior panic.",
        f"{indent}#[error(\"broker state is poisoned\")]",
        f"{indent}BrokerPoisoned,",
    ]
else:
    extra_lines += [
        "",
        f"{indent}/// The requested arena is not registered with this broker.",
        f"{indent}UnknownArena {{ arena: crate::ids::ArenaId }},",
        "",
        f"{indent}/// The broker's internal lock is poisoned by a prior panic.",
        f"{indent}BrokerPoisoned,",
    ]

# Ensure the existing body ends with a comma on the last variant; if it
# doesn't, add one. Look at the last non-blank, non-comment line.
body_stripped = body.rstrip()
if body_stripped and not body_stripped.endswith(','):
    # Append a trailing comma to the last variant.
    body = body_stripped + ",\n"
else:
    body = body_stripped + "\n"

new_body = body + "\n".join(extra_lines) + "\n"
new_src = src[:start] + new_body + src[close:]
err_path.write_text(new_src)
print(f"[OK]   added UnknownArena and BrokerPoisoned variants (thiserror={uses_thiserror})")

# If BrokerError has a helper impl block (e.g., is_use_after_free), make
# sure we don't break it by surveying its methods.
helper_pat = re.search(r'impl BrokerError\s*\{([^}]*)\}', new_src, re.DOTALL)
if helper_pat:
    print(f"[INFO] impl BrokerError has {helper_pat.group(1).count('pub fn')} pub fn(s) (unchanged)")
PYEOF

echo
echo "======"
echo "DUMP: error.rs (after patch)"
echo "======"
cat "$BROKER/src/error.rs"

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -15

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -30 || \
  cargo test -p sentinel-broker 2>&1 | tail -30

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "02i COMPLETE"
echo "======"
echo "If green, commit via GitHub Desktop:"
echo "  broker: phase A0+A1+A2 — foundations, bump arena, destroy_arena, lint hygiene"
