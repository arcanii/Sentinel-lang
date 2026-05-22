#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import sys, re, pathlib
broker = pathlib.Path(sys.argv[1])
arena_path = broker / "src/arena.rs"
src = arena_path.read_text()

# Bail out if a Debug impl for Arena already exists.
if re.search(r'impl\s+(std::fmt::|fmt::)?Debug\s+for\s+Arena\b', src):
    print("[SKIP] manual Debug impl for Arena already present")
    sys.exit(0)

# Discover which fields Arena actually has so the impl only references real ones.
m = re.search(r'pub struct Arena\s*\{([^}]*)\}', src, re.DOTALL)
if not m:
    print("[ERR]  could not locate `pub struct Arena { ... }`", file=sys.stderr)
    sys.exit(1)

body = m.group(1)
field_names = re.findall(r'(?m)^\s*(?:pub(?:\(crate\))?\s+)?([A-Za-z_]\w*)\s*:', body)
print(f"[INFO] Arena fields discovered: {field_names}")

# Pick a safe subset to display (skip raw buffers, locks, pointers).
SAFE_HINTS = {"id", "generation", "capacity", "used", "len", "size", "name", "kind", "strategy"}
safe = [f for f in field_names if f.lower() in SAFE_HINTS or any(h in f.lower() for h in SAFE_HINTS)]
if not safe:
    # Fallback: just print the type name with no fields.
    safe = []
print(f"[INFO] Safe fields to include in Debug output: {safe}")

lines = ["", "impl std::fmt::Debug for Arena {",
         "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {",
         '        f.debug_struct("Arena")']
for fname in safe:
    lines.append(f'            .field("{fname}", &self.{fname})')
lines.append("            .finish_non_exhaustive()")
lines.append("    }")
lines.append("}")
impl_block = "\n".join(lines) + "\n"

arena_path.write_text(src.rstrip() + "\n" + impl_block)
print(f"[OK]   appended manual Debug impl for Arena ({len(safe)} fields shown)")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -25

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -40 || \
  cargo test -p sentinel-broker 2>&1 | tail -40

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "FIXUP 02b COMPLETE"
echo "======"
