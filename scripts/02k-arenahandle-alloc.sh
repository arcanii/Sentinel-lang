#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/broker.rs"
src = p.read_text()

if "pub fn alloc<T>" in src and "impl ArenaHandle" in src:
    print("[SKIP] ArenaHandle::alloc already present")
    sys.exit(0)

# Locate the existing `impl ArenaHandle { ... }` block and inject the
# alloc method just before its closing brace.
m = re.search(r'impl ArenaHandle\s*\{', src)
if not m:
    print("[ERR]  could not find `impl ArenaHandle {`")
    sys.exit(1)

depth = 1
i = m.end()
while i < len(src) and depth > 0:
    if src[i] == '{': depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            close = i
            break
    i += 1
else:
    print("[ERR]  unmatched brace in impl ArenaHandle")
    sys.exit(1)

method = '''
    /// Allocate a value of type `T` into this arena.
    ///
    /// Returns a [`crate::Handle`] that can be used to access the
    /// value while the arena is alive. The handle is invalidated when
    /// the broker destroys the arena.
    ///
    /// # Errors
    ///
    /// Returns [`crate::BrokerError::OutOfMemory`] if the arena's
    /// capacity is exhausted.
    pub fn alloc<T>(&self, value: T) -> Result<crate::Handle<T>, crate::BrokerError>
    where
        T: 'static,
    {
        self.arena.alloc(value)
    }
'''

new = src[:close] + method + src[close:]
p.write_text(new)
print("[OK]   added ArenaHandle::alloc forwarding to inner Arc<Arena>")
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
echo "02k COMPLETE"
echo "======"
echo "If green, commit via GitHub Desktop with message:"
echo "  broker: phase A0+A1+A2 — foundations, bump arena, destroy_arena, lint hygiene"
