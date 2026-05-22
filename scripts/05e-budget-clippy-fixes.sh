#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

echo "======"
echo "APPLYING FIXES"
echo "======"
python3 - <<'PY'
from pathlib import Path
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/budget.rs")
src = p.read_text()

# Fix 1: assigning_clones — use clone_from
old1 = "                node = n.parent.clone();"
new1 = "                n.parent.clone_into(&mut node);" if False else None
# Actually simpler: rewrite the loop to not assign clone — use a temporary.
# Cleanest: pull into a separate binding.
old1 = "        let mut node: Option<Arc<Budget>> = Some(Arc::clone(self));\n        while let Some(n) = node {\n            chain.push(Arc::clone(&n));\n            node = n.parent.clone();\n        }"
new1 = "        let mut node: Option<Arc<Budget>> = Some(Arc::clone(self));\n        while let Some(n) = node {\n            let next = n.parent.clone();\n            chain.push(n);\n            node = next;\n        }"
assert old1 in src, "could not find chain-building loop"
src = src.replace(old1, new1)
print("[OK] fixed assigning_clones in chain loop")

# Fix 2: manual_let_else / single_match_else for the checked_add match
old2 = """            loop {
                let new = match current.checked_add(bytes) {
                    Some(v) => v,
                    None => {
                        // Refund prior charges in this chain.
                        for prior in &chain[..idx] {
                            prior.used.fetch_sub(bytes, Ordering::AcqRel);
                        }
                        return Err(BrokerError::BudgetExceeded {
                            budget: b.id,
                            requested: bytes,
                            remaining: b.cap.saturating_sub(current),
                        });
                    }
                };"""
new2 = """            loop {
                let Some(new) = current.checked_add(bytes) else {
                    // Refund prior charges in this chain.
                    for prior in &chain[..idx] {
                        prior.used.fetch_sub(bytes, Ordering::AcqRel);
                    }
                    return Err(BrokerError::BudgetExceeded {
                        budget: b.id,
                        requested: bytes,
                        remaining: b.cap.saturating_sub(current),
                    });
                };"""
assert old2 in src, "could not find checked_add match block"
src = src.replace(old2, new2)
print("[OK] rewrote checked_add match as let-else")

# Fix 3: elidable lifetimes on BudgetArenaBuilder impl
old3 = "impl<'s, 'b> BudgetArenaBuilder<'s, 'b> {"
new3 = "impl BudgetArenaBuilder<'_, '_> {"
assert old3 in src, "could not find BudgetArenaBuilder impl line"
src = src.replace(old3, new3)
print("[OK] elided lifetimes on BudgetArenaBuilder impl")

# Fix 4: allow dead_code on layout_worst_case (used only in tests)
old4 = """#[must_use]
pub fn layout_worst_case(layout: Layout) -> usize {"""
new4 = """#[must_use]
#[allow(dead_code)] // public utility; currently only exercised in tests
pub fn layout_worst_case(layout: Layout) -> usize {"""
assert old4 in src, "could not find layout_worst_case signature"
src = src.replace(old4, new4)
print("[OK] added #[allow(dead_code)] to layout_worst_case")

p.write_text(src)
print("[OK] budget.rs rewritten")
PY

echo
echo "======"
echo "BUDGET.RS lines 60-115 (post-fix)"
echo "======"
sed -n '60,115p' "$BROKER/src/budget.rs"

echo
echo "======"
echo "BUDGET.RS lines 170-200 (post-fix)"
echo "======"
sed -n '170,200p' "$BROKER/src/budget.rs"

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
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 50
else
  cargo test -p sentinel-broker 2>&1 | tail -n 50
fi

echo
echo "======"
echo "CARGO TEST (full)"
echo "======"
cargo test -p sentinel-broker 2>&1 | tail -n 50

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -n 10

echo
echo "======"
echo "DONE"
echo "======"
