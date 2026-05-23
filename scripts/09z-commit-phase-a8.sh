#!/usr/bin/env bash
set -euo pipefail
cd /Users/bryan/Desktop/github_repos/Sentinel-language

echo "======"
echo "PRE-COMMIT CHECKS"
echo "======"
cargo build -p sentinel-broker --all-targets 2>&1 | tail -3
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -3
cargo test -p sentinel-broker --lib 2>&1 | tail -3
cargo test -p sentinel-broker --doc 2>&1 | tail -3
echo ""
echo "Running all three demos one last time:"
for ex in token_bucket request_pipeline credential_store; do
  echo "  -- $ex --"
  cargo run -q -p sentinel-broker --example "$ex" 2>&1 | tail -3
done

echo ""
echo "======"
echo "UPDATE STATE.md"
echo "======"
python3 <<'PYEOF'
from pathlib import Path
import re
p = Path("docs/STATE.md")
t = p.read_text()
orig = t

# Mark A8 row Done with commit hash placeholder (filled below after commit).
# We use a placeholder string and replace it after `git commit` lands.
pat = re.compile(r"^(\|\s*A8\s*\|[^|]*\|\s*)Next(\s*\|\s*)\|", re.MULTILINE)
t, n = pat.subn(r"\1Done\2PENDING_HASH |", t)
print(f"[OK] A8 row Done (rows={n})")

# Update "Next Milestone (A8 ...)" section to reflect Phase A completion.
start = t.find("## 7. Next Milestone (A8")
if start != -1:
    next_hdr = t.find("\n## ", start + 1)
    if next_hdr == -1:
        next_hdr = len(t)
    new_section = (
        "## 7. Phase A Complete\n\n"
        "All eight Phase A milestones (A0-A8) are landed. The broker is\n"
        "now a feature-complete, production-shape memory subsystem with\n"
        "generational arenas, two allocation strategies, scoped budgets,\n"
        "stats and queries, recording mode, secret-memory policy, and\n"
        "three runnable validation example programs.\n\n"
        "**Test coverage**: 62 lib + 5 integration + 2 proptest + 1 doc = 70 green.\n"
        "**Clippy**: clean with `-D warnings` across crate and examples.\n\n"
        "Next: Phase B (parser / VM / language runtime). See HANDOVER.md.\n\n"
    )
    t = t[:start] + new_section + t[next_hdr+1:]
    print("[OK] Section 7 rewritten: Phase A complete")

# Add A8 details summary near the existing A7 block.
a8_block = (
    "\n\n### Phase A8 - validation examples (PENDING_HASH)\n\n"
    "Three runnable example programs under `crates/sentinel-broker/examples/`:\n\n"
    "- `token_bucket.rs` - high-frequency slab allocation (~100k allocs in ~30ms), generation recycling via 128-slot reuse, `where_is` lookup demo.\n"
    "- `request_pipeline.rs` - scoped per-request bump arenas under `within_budget`, with recorder-based event tracing; demonstrates budget rejection.\n"
    "- `credential_store.rs` - secret slab with STRICT-or-LENIENT fallback; uses unsafe raw-pointer reads via `Arena::__raw_slot_bytes_for_diagnostics` to prove zero-on-free wipes slot bytes (visual hex dump shows `alice:hunter2` -> all zeros).\n\n"
    "API additions:\n"
    "- `AllocStrategy::slot_size_hint() -> Option<usize>`: per-slot byte size for uniform strategies (slab returns Some, bump returns None).\n"
    "- `Arena::__raw_slot_bytes_for_diagnostics(slot)` and `ArenaHandle::__raw_slot_bytes_for_diagnostics(slot)`: `#[doc(hidden)]` diagnostic accessor returning `(*const u8, usize)`. Unstable; for forensic tools and examples only.\n\n"
)
if "Phase A8" not in t:
    if "### Phase A7" in t:
        # Insert after the A7 block (find next top-level heading)
        a7_idx = t.find("### Phase A7")
        next_hdr = t.find("\n## ", a7_idx)
        if next_hdr == -1:
            next_hdr = len(t)
        t = t[:next_hdr] + a8_block + t[next_hdr:]
        print("[OK] inserted A8 block after A7")
    else:
        t = t.rstrip() + "\n" + a8_block
        print("[OK] appended A8 block at end")
else:
    print("[SKIP] A8 block already present")

if t != orig:
    p.write_text(t)
    print("[DONE] STATE.md updated (commit hash placeholder)")
PYEOF

echo ""
echo "======"
echo "STAGING + COMMIT"
echo "======"
git add -A
git --no-pager diff --cached --stat
echo ""
git commit -m "broker: phase A8 -- validation example programs" -m "$(cat <<'MSG'
Adds three runnable example programs that exercise the broker
end-to-end and serve as integration validation for phases A0-A7.

Examples (crates/sentinel-broker/examples/):

- token_bucket.rs (~80 lines)
  High-frequency slab allocation demo: 102,400 alloc/free pairs
  through 128 recycled slots in ~30ms. Exercises slab strategy,
  generation recycling, where_is lookup, and broker stats.

- request_pipeline.rs (~100 lines)
  Scoped per-request bump arenas under within_budget, with a
  recorder attached. Demonstrates budget pre-charging rejection
  (request 99 asks for an arena bigger than the cap and is
  rejected at builder time). Verifies recorded event counts:
  3 ArenaCreated, 3 ArenaDestroyed, 26 Allocated, 4 BudgetOpened,
  4 BudgetClosed (rejected scopes still emit close events).

- credential_store.rs (~140 lines)
  Secret slab with STRICT-or-LENIENT fallback. Uses unsafe raw
  pointer reads to prove zero-on-free actually wipes slot bytes.
  Hex-dump output shows alice:hunter2 in the slot before free,
  all zeros after. Slot recycling demonstrated (dave reuses
  alice's slot index). The STRICT path is attempted first via
  std::panic::catch_unwind probing; falls back to LENIENT if
  mlock is refused (typical on macOS dev machines).

API additions to support the credential_store demo:

- AllocStrategy::slot_size_hint() -> Option<usize>
  Default returns None. SlabStrategy returns Some(slot_size);
  bump returns None (variable-size slots). SecretStrategy
  forwards to inner.

- Arena::__raw_slot_bytes_for_diagnostics(slot) and
  ArenaHandle::__raw_slot_bytes_for_diagnostics(slot)
  doc(hidden) accessor returning (*const u8, usize). Bypasses
  generation check; reads through Strategy::slot_ptr_mut. Marked
  unstable and intended only for forensic/diagnostic tools.
  Double-underscore naming signals "really private."

Phase A status:
  All eight milestones (A0-A8) complete.
  70 tests green (62 lib + 5 integration + 2 proptest + 1 doc).
  Clippy clean with -D warnings across crate AND examples.

Build:
  cargo build -p sentinel-broker --all-targets
  cargo run -p sentinel-broker --example token_bucket
  cargo run -p sentinel-broker --example request_pipeline
  cargo run -p sentinel-broker --example credential_store

Known limitations (deferred):
  - The secret builder API panics on SecretStrategy::wrap failure
    (used .expect()); a future try_bump/try_slab variant returning
    Result would let credential_store skip catch_unwind.
  - slot_size_hint is None for bump strategies, so the diagnostic
    accessor is unavailable there. Fine for the credential demo
    (slab-only) but worth noting.

Scripts: 09z-commit-phase-a8.sh.
MSG
)"

# After commit, replace the PENDING_HASH placeholder with the real commit hash.
HASH=$(git --no-pager log -1 --pretty=%h)
python3 -c "
import pathlib
p = pathlib.Path('docs/STATE.md')
t = p.read_text()
t = t.replace('PENDING_HASH', '$HASH')
p.write_text(t)
print(f'[OK] STATE.md hash placeholder replaced with $HASH')
"
git add docs/STATE.md
git commit --amend --no-edit

echo ""
echo "======"
echo "RESULT"
echo "======"
git --no-pager log -1 --stat
