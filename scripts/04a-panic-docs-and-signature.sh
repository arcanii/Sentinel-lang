#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

echo "======"
echo "FULL E0061 CONTEXT (to find the broken call site)"
echo "======"
cd "$SENTINEL_ROOT"
cargo build -p sentinel-broker --tests 2>&1 | grep -B 2 -A 20 "E0061" | head -80 || true

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])

def patch(rel, fn, label):
    p = broker / rel
    src = p.read_text()
    new = fn(src)
    if new == src:
        print(f"[SKIP] {rel}: {label}")
        return
    p.write_text(new)
    print(f"[OK]   {rel}: {label}")

# Add # Panics sections to BumpStrategy::new and SlabStrategy::new.
def add_panics_bump(src):
    needle = "    #[must_use]\n    pub fn new(arena_id: ArenaId, capacity: usize) -> Self {"
    replacement = (
        "    /// Construct a new bump strategy with `capacity` bytes.\n"
        "    ///\n"
        "    /// # Panics\n"
        "    /// Panics if `capacity == 0`, the layout is unrepresentable,\n"
        "    /// or the system allocator refuses the request.\n"
        "    #[must_use]\n"
        "    pub fn new(arena_id: ArenaId, capacity: usize) -> Self {"
    )
    return src.replace(needle, replacement, 1)
patch("src/strategy/bump.rs", add_panics_bump, "added # Panics doc to BumpStrategy::new")

def add_panics_slab(src):
    needle = "    #[must_use]\n    pub fn new(arena_id: ArenaId, slot_size: usize, slot_align: usize, slot_count: u32) -> Self {"
    replacement = (
        "    /// Construct a new slab with `slot_count` slots of `slot_size`\n"
        "    /// bytes, aligned to `slot_align`.\n"
        "    ///\n"
        "    /// # Panics\n"
        "    /// Panics if any of `slot_size`, `slot_align`, `slot_count` is\n"
        "    /// zero; if `slot_align` is not a power of two; if the total\n"
        "    /// size overflows `usize`; or if the system allocator refuses\n"
        "    /// the request.\n"
        "    #[must_use]\n"
        "    pub fn new(arena_id: ArenaId, slot_size: usize, slot_align: usize, slot_count: u32) -> Self {"
    )
    return src.replace(needle, replacement, 1)
patch("src/strategy/slab.rs", add_panics_slab, "added # Panics doc to SlabStrategy::new")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD (lib + tests)"
echo "======"
cargo build -p sentinel-broker --tests 2>&1 | tail -30

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "TESTS (nextest)"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -55

echo
echo "======"
echo "TESTS (cargo test)"
echo "======"
cargo test -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "04a COMPLETE"
echo "======"
