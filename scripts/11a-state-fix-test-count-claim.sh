#!/usr/bin/env bash
# 11a-state-fix-test-count-claim.sh - correct the false claim in STATE.md
# about which test was removed in A9. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -f docs/STATE.md ]]; then
  echo "ERROR: docs/STATE.md missing" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== B0a-STATE FIX START"

cat > /tmp/sentinel_state_fix.py <<'PYEOF'
#!/usr/bin/env python3
from pathlib import Path

p = Path.cwd() / "docs" / "STATE.md"
txt = p.read_text()

WRONG = (
    "The count dropped from 70 → 69 between A8 and A9 because one test depended on `BrokerError: Copy`, which A9 removed. The\n"
    "removed test was redundant with `error::tests::error_messages_are_informative`."
)

RIGHT = (
    "The count dropped from 70 → 69 between A8 and A9 because A9 incidentally removed\n"
    "`strategy::slab::tests::slab_free_returns_not_implemented`, an obsolete test\n"
    "that survived A3.5 (slab recycling) and asserted the *opposite* of the correct\n"
    "slab behavior — slab DOES support free as of A3.5. The correctly-named\n"
    "`bump_free_returns_not_implemented` (which matches invariant #3) is retained."
)

if RIGHT in txt:
    print("  UNCHANGED docs/STATE.md (already corrected)")
elif WRONG in txt:
    txt = txt.replace(WRONG, RIGHT)
    p.write_text(txt)
    print("  UPDATE docs/STATE.md (corrected test-removal claim)")
else:
    print("  WARN: could not find the incorrect paragraph verbatim.")
    print("  Looking for any line containing 'redundant with `error::tests::'...")
    for i, line in enumerate(txt.splitlines(), 1):
        if "redundant with `error::tests" in line:
            print(f"    line {i}: {line}")
    print("  No automatic fix applied. Edit STATE.md by hand if the above lines exist.")
PYEOF

python3 /tmp/sentinel_state_fix.py

echo
echo "====== B0a-STATE FIX DONE"
echo
echo "Verify with:"
echo "  grep -A2 '70 → 69' docs/STATE.md"
echo
echo "====== B0a-STATE FIX END"
