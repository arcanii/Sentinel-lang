#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import sys, re, pathlib
broker = pathlib.Path(sys.argv[1])
ids_path = broker / "src/ids.rs"
src = ids_path.read_text()

# Replace each `let _NAME` line individually (tolerant of whitespace/types/values).
patterns = [
    (re.compile(r'(?m)^(\s*)let\s+_a\s*(:\s*ArenaId)?\s*=\s*ArenaId\(0\);\s*$'),
     r'\1let a\2 = ArenaId(0);'),
    (re.compile(r'(?m)^(\s*)let\s+_s\s*(:\s*SlotIndex)?\s*=\s*SlotIndex\(0\);\s*$'),
     r'\1let s\2 = SlotIndex(0);'),
    (re.compile(r'(?m)^(\s*)let\s+_g\s*(:\s*Generation)?\s*=\s*Generation\(1\);\s*$'),
     r'\1let g\2 = Generation(1);'),
]
new = src
hits = 0
for pat, repl in patterns:
    new2, n = pat.subn(repl, new, count=1)
    hits += n
    new = new2

if hits == 0:
    print("[WARN] no _a/_s/_g rebindings made — dumping ids.rs tests block below")
else:
    # Inject the use lines just after the last rebinding, before the closing brace
    # of `fn ids_have_distinct_types`.
    fn_pat = re.compile(
        r'(fn\s+ids_have_distinct_types[^{]*\{[^}]*?Generation\(1\);)([^}]*?)(\n\s*\})',
        re.DOTALL,
    )
    m = fn_pat.search(new)
    if m and "format!(\"{a:?}\")" not in new:
        uses = (
            "\n        assert!(format!(\"{a:?}\").contains(\"ArenaId\"));"
            "\n        assert!(format!(\"{s:?}\").contains(\"SlotIndex\"));"
            "\n        assert!(format!(\"{g:?}\").contains(\"Generation\"));"
        )
        new = new[:m.end(1)] + uses + new[m.end(1):]
        print(f"[OK]   ids.rs: rebinded {hits} let(s) and added use-assertions")
    else:
        print(f"[OK]   ids.rs: rebinded {hits} let(s); use-assertions already present or fn not found")

if new != src:
    ids_path.write_text(new)
PYEOF

echo
echo "======"
echo "DUMP: ids.rs tail (tests module)"
echo "======"
awk '/#\[cfg\(test\)\]/{p=1} p{print}' "$BROKER/src/ids.rs" | head -60

echo
echo "======"
echo "DUMP: handle.rs (full)"
echo "======"
cat "$BROKER/src/handle.rs"

echo
echo "======"
echo "DUMP: arena.rs (struct, alloc, Drop) — lines 1-220"
echo "======"
sed -n '1,220p' "$BROKER/src/arena.rs"

echo
echo "======"
echo "DUMP: broker.rs (lines 80-150 — the failing doctest_in_lib_rs test)"
echo "======"
sed -n '80,150p' "$BROKER/src/broker.rs"

echo
echo "======"
echo "CLIPPY"
echo "======"
cd "$SENTINEL_ROOT"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25 || true

echo
echo "======"
echo "02f COMPLETE"
echo "======"
echo "Paste all five DUMP/CLIPPY blocks back; I will use the dumps to write a precise correctness fix in 02g."
