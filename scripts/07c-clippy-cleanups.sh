#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "PATCHING arena.rs (allow dead_code on with_strategy wrapper)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/arena.rs")
src = p.read_text()
old = "    pub(crate) fn with_strategy(id: ArenaId, name: &str, strategy: Box<dyn AllocStrategy>) -> Arc<Self> {"
new = "    #[allow(dead_code)] // kept as a convenience wrapper; primary constructor is with_strategy_and_recorder\n    pub(crate) fn with_strategy(id: ArenaId, name: &str, strategy: Box<dyn AllocStrategy>) -> Arc<Self> {"
if "#[allow(dead_code)] // kept as a convenience wrapper" in src:
    print("[SKIP] already annotated")
elif old in src:
    src = src.replace(old, new, 1)
    p.write_text(src)
    print("[OK] added #[allow(dead_code)] to with_strategy")
else:
    print("[WARN] with_strategy signature not found")
PY

echo
echo "======"
echo "PATCHING broker.rs (literal separator)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()
if "0xCAFEBABEu32" in src:
    src = src.replace("0xCAFEBABEu32", "0xCAFE_BABE_u32")
    p.write_text(src)
    print("[OK] added underscores to hex literal")
else:
    print("[SKIP] literal already split or absent")
PY

echo
echo "======"
echo "PATCHING recording.rs (map_unwrap_or)"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/recording.rs")
src = p.read_text()
changed = False

# len()
old1 = "self.inner.lock().map(|g| g.len()).unwrap_or(0)"
new1 = "self.inner.lock().map_or(0, |g| g.len())"
if old1 in src:
    src = src.replace(old1, new1)
    changed = True
    print("[OK] rewrote len() with map_or")

# snapshot()
old2 = "self.inner.lock().map(|g| g.clone()).unwrap_or_default()"
new2 = "self.inner.lock().map_or_else(|_| Vec::new(), |g| g.clone())"
if old2 in src:
    src = src.replace(old2, new2)
    changed = True
    print("[OK] rewrote snapshot() with map_or_else")

if changed:
    p.write_text(src)
PY

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 15

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 30

echo
echo "======"
echo "TESTS"
echo "======"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 80
else
  cargo test -p sentinel-broker 2>&1 | tail -n 80
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
