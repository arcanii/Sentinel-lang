#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import sys, re, pathlib
broker = pathlib.Path(sys.argv[1])

def edit(rel, fn, label):
    p = broker / rel
    src = p.read_text()
    new = fn(src)
    if new == src:
        print(f"[SKIP] {rel}: {label}")
        return
    p.write_text(new)
    print(f"[OK]   {rel}: {label}")

# 1. arena.rs:182 — drop the redundant `continue`.
def fix_continue(src):
    return src.replace(
        "Err(_) => continue, // someone else allocated; retry",
        "Err(_) => {} // someone else allocated; retry",
    )
edit("src/arena.rs", fix_continue, "drop redundant continue")

# 2. handle.rs — Debug impl for Handle<T> ignores the `arena: Weak<Arena>` field.
#    Add .finish_non_exhaustive() so clippy knows it's intentional.
def fix_handle_debug(src):
    # Locate the Debug impl for Handle and replace its terminator.
    pattern = re.compile(
        r'(impl<T> std::fmt::Debug for Handle<T> \{[^}]*?f\.debug_struct\("Handle"\)[^}]*?)\.finish\(\)',
        re.DOTALL,
    )
    return pattern.sub(r'\1.finish_non_exhaustive()', src, count=1)
edit("src/handle.rs", fix_handle_debug, "Handle Debug uses finish_non_exhaustive")

# 3. ids.rs:115-117 — the test bindings `_a`, `_s`, `_g` exist only to prove the
#    types construct. Rename to `a`, `s`, `g` and add a trivial use (assert_eq! on
#    a Debug-formatted string) so they're actually exercised.
def fix_id_test_bindings(src):
    pattern = re.compile(
        r'(fn ids_have_distinct_types[^{]*\{\s*)'
        r'let _a: ArenaId = ArenaId\(0\);\s*'
        r'let _s: SlotIndex = SlotIndex\(0\);\s*'
        r'let _g: Generation = Generation\(1\);',
        re.DOTALL,
    )
    replacement = (
        r'\1'
        'let a: ArenaId = ArenaId(0);\n'
        '        let s: SlotIndex = SlotIndex(0);\n'
        '        let g: Generation = Generation(1);\n'
        '        // Exercise each binding so the test is meaningful.\n'
        '        assert!(format!("{a:?}").contains("ArenaId"));\n'
        '        assert!(format!("{s:?}").contains("SlotIndex"));\n'
        '        assert!(format!("{g:?}").contains("Generation"));'
    )
    return pattern.sub(replacement, src, count=1)
edit("src/ids.rs", fix_id_test_bindings, "make _a/_s/_g bindings effective")

# 4. integration.rs — the cast warning. Use i32::try_from to be explicit.
def fix_cast(src):
    return src.replace(
        "assert_eq!(*h.get().unwrap(), (i as i32) * 10);",
        "assert_eq!(*h.get().unwrap(), i32::try_from(i).unwrap() * 10);",
    )
edit("tests/integration.rs", fix_cast, "i32::try_from instead of `as`")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "CLIPPY (after style fixes)"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -20

echo
echo "======"
echo "DIAGNOSTIC: dump key source files for analysis"
echo "======"
echo "--- handle.rs (struct definitions and get/get_mut) ---"
sed -n '1,180p' "$SENTINEL_ROOT/crates/sentinel-broker/src/handle.rs"
echo
echo "--- arena.rs (struct, alloc, Drop) ---"
sed -n '1,220p' "$SENTINEL_ROOT/crates/sentinel-broker/src/arena.rs"
echo
echo "--- broker.rs:90-130 (the failing doctest body) ---"
sed -n '80,140p' "$SENTINEL_ROOT/crates/sentinel-broker/src/broker.rs"

echo
echo "======"
echo "DIAGNOSTIC: re-run one failing test with RUST_BACKTRACE=1"
echo "======"
RUST_BACKTRACE=1 cargo test -p sentinel-broker --lib \
    broker::tests::doctest_in_lib_rs_actually_runs 2>&1 | tail -40 || true

echo
echo "======"
echo "02e COMPLETE — clippy should be green; tests still failing (expected)"
echo "======"
echo "Send back the CLIPPY block, the three source dumps, and the backtrace."
echo "I will then send 02f with the correct invalidation fix."
