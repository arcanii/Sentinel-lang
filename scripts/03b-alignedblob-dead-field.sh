#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/arena.rs"
src = p.read_text()

if "#[allow(dead_code)]\n        struct AlignedBlob" in src:
    print("[SKIP] AlignedBlob already has allow(dead_code)")
    sys.exit(0)

new = src.replace(
    "        #[repr(align(64))]\n        struct AlignedBlob([u8; 64]);",
    "        #[repr(align(64))]\n        #[allow(dead_code)]\n        struct AlignedBlob([u8; 64]);",
    1,
)
if new == src:
    print("[ERR] could not locate AlignedBlob declaration")
    sys.exit(1)
p.write_text(new)
print("[OK]   added allow(dead_code) to AlignedBlob (only its address is read)")

# Also tighten back the BumpStrategy struct: now that we know the dead
# field was elsewhere, restore strict hygiene on BumpStrategy.
b = broker / "src/strategy/bump.rs"
bs = b.read_text()
if "#[allow(dead_code)]\npub struct BumpStrategy" in bs:
    bs_new = bs.replace(
        "#[allow(dead_code)]\npub struct BumpStrategy {",
        "pub struct BumpStrategy {",
        1,
    )
    b.write_text(bs_new)
    print("[OK]   removed blanket allow(dead_code) from BumpStrategy (no longer needed)")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -10

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -50 || \
  cargo test -p sentinel-broker 2>&1 | tail -50

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "03b COMPLETE"
echo "======"
echo "If green, ~36 tests should pass. Commit message:"
echo "  broker: phase A3 — alloc strategies + builder + slab (no recycling)"
