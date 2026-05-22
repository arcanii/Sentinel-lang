#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/error.rs"
src = p.read_text()

# Remove the duplicate (shorter) UnknownArena variant added by 02i.
# Pattern: blank line, doc comment, #[error("unknown arena: {arena}")],
# UnknownArena { arena: crate::ids::ArenaId },
dup_pat = re.compile(
    r'\n\s*/// The requested arena is not registered with this broker\.\s*\n'
    r'\s*#\[error\("unknown arena: \{arena\}"\)\]\s*\n'
    r'\s*UnknownArena \{ arena: crate::ids::ArenaId \},\s*\n',
)
new, n = dup_pat.subn('\n', src, count=1)
if n == 0:
    print("[SKIP] duplicate UnknownArena not found (already removed?)")
else:
    print(f"[OK]   removed duplicate UnknownArena variant ({n} occurrence)")

# Sanity: ensure BrokerPoisoned is still present.
if "BrokerPoisoned" not in new:
    print("[ERR]  BrokerPoisoned missing — aborting")
    sys.exit(1)

# Count UnknownArena occurrences in the enum body to confirm exactly one.
enum_m = re.search(r'pub enum BrokerError\s*\{', new)
if enum_m:
    depth = 1
    i = enum_m.end()
    while i < len(new) and depth > 0:
        if new[i] == '{': depth += 1
        elif new[i] == '}': depth -= 1
        i += 1
    body = new[enum_m.end():i-1]
    occ = len(re.findall(r'\bUnknownArena\b', body))
    print(f"[INFO] UnknownArena now appears {occ} time(s) in enum body (expected 1)")
    if occ != 1:
        print("[ERR]  unexpected duplicate count")
        sys.exit(1)

p.write_text(new)
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
echo "02j COMPLETE"
echo "======"
echo "If all green, commit via GitHub Desktop:"
echo "  broker: phase A0+A1+A2 — foundations, bump arena, destroy_arena, lint hygiene"
