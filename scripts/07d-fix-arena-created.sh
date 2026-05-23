#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "CURRENT register_arena"
echo "======"
awk '/fn register_arena/,/^    \}$/' "$BROKER/src/broker.rs"

echo
echo "======"
echo "REWRITING register_arena (idempotent, hand-shaped)"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()

# Replace whatever register_arena body currently is with a clean canonical version.
# Anchor: from `pub(crate) fn register_arena(&self, arena: Arc<Arena>) {`
# through to the matching closing brace.
m = re.search(r"pub\(crate\) fn register_arena\(&self, arena: Arc<Arena>\) \{", src)
if not m:
    print("[WARN] could not find register_arena signature")
    raise SystemExit(0)

start = m.start()
# Find the opening `{` after the signature.
brace_open = src.index("{", m.end() - 1)
depth = 0
i = brace_open
end = None
while i < len(src):
    if src[i] == '{':
        depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            end = i + 1
            break
    i += 1

if end is None:
    print("[WARN] could not find end of register_arena")
    raise SystemExit(0)

old_method = src[start:end]
new_method = """pub(crate) fn register_arena(&self, arena: Arc<Arena>) {
        let id = arena.id();
        // Capture summary BEFORE moving `arena` into the map.
        let name = arena.name().to_string();
        let kind = arena.strategy_kind();
        let capacity = arena.capacity();
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, arena);
        }
        tracing::debug!(arena_id = %id, "arena registered");
        if let Some(r) = &self.recorder {
            r.record(Event::ArenaCreated {
                id,
                name,
                kind,
                capacity,
                at_ns: r.now_ns(),
            });
        }
    }"""

src = src[:start] + new_method + src[end:]
p.write_text(src)
print("[OK] register_arena rewritten with ArenaCreated event emission")
PY

echo
echo "======"
echo "NEW register_arena"
echo "======"
awk '/fn register_arena/,/^    \}$/' "$BROKER/src/broker.rs"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 15

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
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 70
else
  cargo test -p sentinel-broker --lib 2>&1 | tail -n 70
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
