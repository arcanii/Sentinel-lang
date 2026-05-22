#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])

# Fix clippy::single_match in arena.rs by collapsing the match into an if.
arena_path = broker / "src/arena.rs"
src = arena_path.read_text()

pat = re.compile(
    r'match self\.cursor\.compare_exchange_weak\(\s*'
    r'current,\s*end,\s*Ordering::AcqRel,\s*Ordering::Acquire,\s*\)\s*\{\s*'
    r'Ok\(_\) => break aligned,\s*'
    r'Err\(_\) => \{\}[^\n]*\n\s*\}',
    re.DOTALL,
)
replacement = (
    'if self.cursor\n'
    '                .compare_exchange_weak(current, end, Ordering::AcqRel, Ordering::Acquire)\n'
    '                .is_ok()\n'
    '            {\n'
    '                break aligned;\n'
    '            }\n'
    '            // else: another thread claimed this slot; retry.'
)
new = pat.sub(replacement, src, count=1)
if new != src:
    arena_path.write_text(new)
    print("[OK]   arena.rs: match -> if .is_ok() (clippy::single_match)")
else:
    print("[SKIP] arena.rs: single_match pattern not found (already fixed?)")
PYEOF

echo
echo "======"
echo "DUMP: broker.rs (full)"
echo "======"
cat "$BROKER/src/broker.rs"

echo
echo "======"
echo "DUMP: lib.rs (full)"
echo "======"
cat "$BROKER/src/lib.rs"

echo
echo "======"
echo "DUMP: tests/integration.rs (full)"
echo "======"
cat "$BROKER/tests/integration.rs"

echo
echo "======"
echo "DUMP: tests/proptest.rs (full)"
echo "======"
cat "$BROKER/tests/proptest.rs"

echo
echo "======"
echo "CLIPPY (after single_match fix)"
echo "======"
cd "$SENTINEL_ROOT"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -20 || true

echo
echo "======"
echo "02g COMPLETE"
echo "======"
echo "Send back the four DUMP blocks and the CLIPPY block."
echo "Next (02h) will implement Broker::destroy_arena and update the failing tests."
