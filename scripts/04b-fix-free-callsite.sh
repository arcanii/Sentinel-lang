#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/broker.rs"
src = p.read_text()

# Update the broker::tests::slab_free_returns_not_implemented test so it
# passes the new SlotGeneration argument. The test calls `s.free(SlotIndex(0))`
# on a SlabStrategy — but SlabStrategy now implements real recycling, so
# calling free on a fresh slot would return UseAfterFreeSlot (generations
# don't match because the slot was never allocated). The test was named for
# the A3 "NotImplemented" behavior which no longer applies to slab.
#
# Rewrite the test to verify the *bump* strategy returns NotImplemented,
# which is the actual current invariant.
old = '''    #[test]
    fn slab_free_returns_not_implemented() {
        // The strategy's free() returns NotImplemented; for now we just
        // exercise it directly via the strategy until A3.5 wires it up
        // through Arena/Handle.
        use crate::strategy::{slab::SlabStrategy, AllocStrategy};
        use crate::ids::{ArenaId, SlotIndex};
        let s = SlabStrategy::new(ArenaId(99), 16, 8, 4);
        let err = s.free(SlotIndex(0)).unwrap_err();
        assert!(matches!(err, BrokerError::NotImplemented { .. }));
    }'''

new_test = '''    #[test]
    fn bump_strategy_free_returns_not_implemented() {
        // Bump never recycles: free() returns NotImplemented.
        use crate::ids::{ArenaId, SlotGeneration, SlotIndex};
        use crate::strategy::{bump::BumpStrategy, AllocStrategy};
        let s = BumpStrategy::new(ArenaId(99), 64);
        let err = s.free(SlotIndex(0), SlotGeneration::INITIAL).unwrap_err();
        assert!(matches!(err, BrokerError::NotImplemented { .. }));
    }

    #[test]
    fn slab_strategy_free_recycles() {
        // Slab supports recycling. Allocate, free with the issued
        // generation, then allocate again into the same slot.
        use crate::ids::ArenaId;
        use crate::strategy::{slab::SlabStrategy, AllocStrategy};
        use std::alloc::Layout;
        let s = SlabStrategy::new(ArenaId(100), 16, 8, 2);
        let a = s.alloc_raw(Layout::new::<u64>()).unwrap();
        s.free(a.slot, a.generation).unwrap();
        // Double free: generation has advanced, so the second free fails.
        let err = s.free(a.slot, a.generation).unwrap_err();
        assert!(matches!(err, BrokerError::UseAfterFreeSlot { .. }));
    }'''

if old not in src:
    print("[WARN] could not find old slab_free_returns_not_implemented test verbatim")
    # Fallback: regex match.
    pat = re.compile(
        r'    #\[test\]\s*\n\s*fn slab_free_returns_not_implemented\(\)[^}]*?\}\s*\n',
        re.DOTALL,
    )
    src2, n = pat.subn(new_test + "\n", src, count=1)
    if n == 0:
        print("[ERR]  could not locate test to replace"); sys.exit(1)
    src = src2
    print("[OK]   replaced via regex fallback")
else:
    src = src.replace(old, new_test, 1)
    print("[OK]   replaced slab_free_returns_not_implemented with two new tests")

p.write_text(src)
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker --tests 2>&1 | tail -15

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
echo "04b COMPLETE"
echo "======"
echo "Expect ~40 tests passing. If green, ready to commit A3.5."
