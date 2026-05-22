#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import sys, re, pathlib
broker = pathlib.Path(sys.argv[1])

# ---- 1. Relax pedantic doc lint at the crate root --------------------------
lib_path = broker / "src/lib.rs"
lib = lib_path.read_text()

# Collect inner attributes we want present. Use #![allow(...)] at crate root.
wanted = [
    "#![allow(clippy::doc_markdown)]",
    "#![allow(clippy::module_name_repetitions)]",
    "#![allow(clippy::missing_errors_doc)]",
    "#![allow(clippy::must_use_candidate)]",
]

changed = False
for attr in wanted:
    if attr in lib:
        continue
    # Insert near the top, after any leading //! doc-comment block.
    lines = lib.splitlines(keepends=True)
    insert_at = 0
    for i, ln in enumerate(lines):
        if ln.startswith("//!") or ln.strip() == "":
            insert_at = i + 1
            continue
        break
    lines.insert(insert_at, attr + "\n")
    lib = "".join(lines)
    changed = True
    print(f"[OK]   lib.rs: added {attr}")

if changed:
    lib_path.write_text(lib)
else:
    print("[SKIP] lib.rs: crate-level allows already present")

# ---- 2. Properly silence the two dead_code warnings -----------------------
# 2a. SlotInfo.size  (struct is private, field is not pub — earlier regex missed it).
arena_path = broker / "src/arena.rs"
arena = arena_path.read_text()

# Look for the SlotInfo struct and ensure the size field has an allow.
pat = re.compile(
    r'(struct\s+SlotInfo\s*\{[^}]*?)'
    r'(\n\s*)(size:\s*usize,)',
    re.DOTALL,
)
m = pat.search(arena)
if m and "#[allow(dead_code)]\n" + m.group(2).lstrip("\n") + "size:" not in arena:
    # Inject the attribute on its own line, matching indentation.
    indent = m.group(2).rstrip("\n").lstrip("\r")
    # m.group(2) starts with newline + indent — preserve that newline, then add attr+indent.
    replacement = f"{m.group(1)}\n{indent}#[allow(dead_code)]\n{indent}{m.group(3)}"
    arena = arena[:m.start()] + replacement + arena[m.end():]
    arena_path.write_text(arena)
    print("[OK]   arena.rs: allow(dead_code) on SlotInfo::size (private struct path)")
else:
    print("[SKIP] arena.rs: SlotInfo::size already allowed or pattern not found")

# 2b. ArenaId::raw  — earlier patch targeted `pub fn raw`, but it's `pub(crate) const fn raw`.
ids_path = broker / "src/ids.rs"
ids = ids_path.read_text()

pat2 = re.compile(
    r'(?m)^(\s*)(pub\(crate\)\s+const\s+fn\s+raw\(self\))'
)
m2 = pat2.search(ids)
if m2:
    # Check if an #[allow(dead_code)] already sits immediately above.
    above = ids[:m2.start()].rstrip().splitlines()
    if above and "allow(dead_code)" in above[-1]:
        print("[SKIP] ids.rs: ArenaId::raw already allowed")
    else:
        indent = m2.group(1)
        new = f"{indent}#[allow(dead_code)]\n{m2.group(0)}"
        ids = ids[:m2.start()] + new + ids[m2.end():]
        ids_path.write_text(ids)
        print("[OK]   ids.rs: allow(dead_code) on ArenaId::raw (pub(crate) const fn)")
else:
    print("[WARN] ids.rs: could not find `pub(crate) const fn raw(self)`")
PYEOF

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
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -30

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -25 || \
  cargo test -p sentinel-broker 2>&1 | tail -25

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "02c COMPLETE"
echo "======"
echo "If all green, commit via GitHub Desktop with message:"
echo "  broker: phase A0+A1+A2 — foundations, bump arena, lint hygiene"
