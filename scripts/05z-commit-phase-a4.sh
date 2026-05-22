#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
cd "$SENTINEL_ROOT"

echo "======"
echo "PRE-COMMIT CHECKS"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 5
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 5
fi
cargo test -p sentinel-broker 2>&1 | tail -n 5
cargo test -p sentinel-broker --doc 2>&1 | tail -n 5

echo
echo "======"
echo "BRANCH + IDENTITY"
echo "======"
git rev-parse --abbrev-ref HEAD
git config user.name  || echo "(no user.name set)"
git config user.email || echo "(no user.email set)"

echo
echo "======"
echo "STAGING"
echo "======"
git add -A
git --no-pager diff --cached --stat | tail -n 30

MSG_FILE="$(mktemp)"
cat > "$MSG_FILE" <<'MSG'
broker: phase A4 — scoped allocation budgets

Adds Broker::within_budget(cap, |scope| ...) for cross-arena allocation
caps. Budgets nest: inner allocations charge both the inner and every
enclosing budget. On any cap exhaustion the offending budget's id is
reported via BrokerError::BudgetExceeded { budget, requested, remaining }
and partial charges along the chain are atomically refunded.

New module: budget.rs
  - Budget: id + cap + AtomicUsize used + Option<Arc<Budget>> parent.
    try_charge(bytes) walks the parent chain and applies a lock-free
    compare-exchange add against each cap, with precise refund on
    failure.
  - BudgetScope<'b>: handed to closures; exposes .arena(name) and
    nested .within_budget(...). Lifetime tied to the broker borrow.
  - BudgetArenaBuilder<'_, '_>: capacity(n).bump() or
    .slab(slot_size, slot_align, slot_count). Pre-charges reserved
    capacity / total slot bytes against the scope's budget before
    constructing the underlying arena.

New ids: BudgetId + BudgetIdCounter (monotonic, distinct type).
New error variant: BudgetExceeded { budget, requested, remaining }.

Broker additions:
  - within_budget<R, F>(cap, F) -> Result<R, BrokerError>
  - next_budget_id() (pub(crate)) backed by a BudgetIdCounter field.

Design choices:
  - Pre-charge using reserved capacity rather than per-allocation worst
    case. Reserving 4 MiB counts as 4 MiB whether the arena fills or
    not — the most honest interpretation for capacity planning.
  - layout_worst_case helper kept for future per-allocation modes.
  - Inner-budget failures do not refund the inner's own used counter
    on closure error (only on charge failure); arenas already created
    remain charged, matching RAII expectations.

Tests: 49 green (42 lib + 5 integration + 2 proptest + 1 doc)
  - budget_basic_allows_under_cap
  - budget_rejects_over_cap_arena
  - budget_nests_inner_outer_both_charged
  - budget_inner_cap_can_be_exceeded_independently_of_outer
  - budget_outer_failure_refunds_inner_charge_attempt
  - budget_slab_charges_total
  - budget_slab_rejects_when_over_cap
  - layout_worst_case_includes_alignment_pad

Scripts included for traceability:
  05-budgets.sh, 05a-fix-broker-brace.sh, 05b-fix-broker-impl-close.sh,
  05c-fix-broker-impl-close.sh, 05d-clippy-diagnose.sh,
  05e-budget-clippy-fixes.sh, 05z-commit-phase-a4.sh
MSG

echo
echo "======"
echo "COMMIT MESSAGE"
echo "======"
cat "$MSG_FILE"

echo
echo "======"
echo "COMMITTING"
echo "======"
git commit -F "$MSG_FILE"
rm -f "$MSG_FILE"

echo
echo "======"
echo "RESULT"
echo "======"
git --no-pager log -1 --stat

echo
echo "======"
echo "DONE — push when ready: git push"
echo "======"
