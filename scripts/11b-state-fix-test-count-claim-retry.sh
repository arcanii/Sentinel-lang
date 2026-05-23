#!/usr/bin/env bash
# 11b - second attempt at the STATE.md test-removal correction with the
# correct line breaks. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR" >&2; return 1 2>/dev/null || exit 1; }

cat > /tmp/sentinel_state_fix2.py <<'PYEOF'
#!/usr/bin/env python3
from pathlib import Path

p = Path.cwd() / "docs" / "STATE.md"
txt = p.read_text()

# Match the three-line wrap as it actually exists in the file.
WRONG_LINES = [
    "proptest). The count dropped from 70 \u2192 69 between A8 and A9 because",
    "one test depended on `BrokerError: Copy`, which A9 removed. The",
    "removed test was redundant with `error::tests::error_messages_are_informative`.",
]
WRONG = "\n".join(WRONG_LINES)

RIGHT = (
    "proptest). The count dropped from 70 \u2192 69 between A8 and A9 because A9\n"
    "incidentally removed `strategy::slab::tests::slab_free_returns_not_implemented`,\n"
    "an obsolete test that survived A3.5 (slab recycling) and asserted the\n"
    "*opposite* of the correct slab behavior \u2014 slab DOES support free as of A3.5.\n"
    "The correctly-named `bump_free_returns_not_implemented` (which matches\n"
    "invariant #3) is retained."
)

if RIGHT.split("\n")[0] in txt:
    print("  UNCHANGED docs/STATE.md (already corrected)")
elif WRONG in txt:
    txt = txt.replace(WRONG, RIGHT)
    p.write_text(txt)
    print("  UPDATE docs/STATE.md (corrected test-removal claim)")
else:
    # Last-resort: look for the offending sentence and dump surrounding context.
    print("  WARN: WRONG string still not found verbatim. Dumping context.")
    for i, line in enumerate(txt.splitlines(), 1):
        if "70 \u2192 69" in line or "redundant with" in line:
            print(f"    line {i}: {repr(line)}")
PYEOF

python3 /tmp/sentinel_state_fix2.py

echo
echo "Verify:"
grep -B1 -A6 '70 → 69' docs/STATE.md
