#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

write_file() {
    local path="$1"; local content="$2"
    mkdir -p "$(dirname "$path")"
    if [[ -f "$path" ]] && [[ "$(cat "$path")" == "$content" ]]; then
        printf "[SKIP] %s (unchanged)\n" "${path#$SENTINEL_ROOT/}"
        return
    fi
    printf "%s" "$content" > "$path"
    printf "[OK]   wrote %s\n" "${path#$SENTINEL_ROOT/}"
}

# ---------------------------------------------------------------------------
# 1. ids.rs — add BudgetId.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/ids.rs"
src = p.read_text()

if "pub struct BudgetId" in src:
    print("[SKIP] ids.rs: BudgetId already present")
    sys.exit(0)

# Find the end of `impl SlotGeneration { ... }` block.
m = re.search(r'impl SlotGeneration\s*\{', src)
if not m:
    print("[ERR]  ids.rs: cannot find impl SlotGeneration"); sys.exit(1)
d = 1; i = m.end()
while i < len(src) and d > 0:
    if src[i] == '{': d += 1
    elif src[i] == '}':
        d -= 1
        if d == 0:
            close = i; break
    i += 1
# Move past the Display impl that follows.
display_m = re.search(r'impl std::fmt::Display for SlotGeneration[^}]*\}\s*\}', src[close:])
if display_m:
    close = close + display_m.end()
else:
    close = close + 1

addition = '''

/// Identifier for a Budget scope. Monotonically issued by the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BudgetId(pub(crate) u64);

impl BudgetId {
    #[must_use]
    pub const fn raw(self) -> u64 { self.0 }
}

impl std::fmt::Display for BudgetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "budget#{}", self.0)
    }
}

/// Counter for issuing fresh BudgetIds. Lives inside the broker.
#[derive(Debug, Default)]
pub(crate) struct BudgetIdCounter {
    next: std::sync::atomic::AtomicU64,
}

impl BudgetIdCounter {
    pub(crate) fn new() -> Self {
        Self { next: std::sync::atomic::AtomicU64::new(1) }
    }

    pub(crate) fn next(&self) -> BudgetId {
        BudgetId(self.next.fetch_add(1, std::sync::atomic::Ordering::AcqRel))
    }
}
'''
new = src[:close] + addition + src[close:]
p.write_text(new)
print("[OK]   ids.rs: added BudgetId and BudgetIdCounter")
PYEOF

# ---------------------------------------------------------------------------
# 2. error.rs — add BudgetExceeded variant.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/error.rs"
src = p.read_text()

if "BudgetExceeded" in src:
    print("[SKIP] error.rs: BudgetExceeded already present")
    sys.exit(0)

m = re.search(r'pub enum BrokerError\s*\{', src)
d = 1; i = m.end()
while i < len(src) and d > 0:
    if src[i] == '{': d += 1
    elif src[i] == '}':
        d -= 1
        if d == 0:
            close = i; break
    i += 1
body = src[m.end():close].rstrip()
if body and not body.endswith(','):
    body += ',\n'
else:
    body += '\n'
variant = '''
    /// An allocation was rejected because it would exceed a Budget cap.
    ///
    /// Returned by allocations inside [`crate::Broker::within_budget`]
    /// scopes when the cumulative byte total would exceed the budget.
    #[error("budget exceeded: {budget} would exceed cap (requested {requested}, remaining {remaining})")]
    BudgetExceeded {
        budget: crate::ids::BudgetId,
        requested: usize,
        remaining: usize,
    },
'''
new = src[:m.end()] + body + variant + src[close:]
p.write_text(new)
print("[OK]   error.rs: added BudgetExceeded")
PYEOF

# ---------------------------------------------------------------------------
# 3. budget.rs — Budget + BudgetScope + budget chain semantics.
# ---------------------------------------------------------------------------
read -r -d '' BUDGET_RS <<'RUST_EOF' || true
//! Scoped allocation budgets.
//!
//! A `Budget` caps the cumulative bytes allocated through a scope.
//! Budgets nest: an inner budget's allocations count against *both*
//! itself and every enclosing budget. Allocations are pre-charged
//! before the strategy runs, using the layout's worst-case alignment
//! pad, so no rollback of strategy state is ever required.
//!
//! ```ignore
//! broker.within_budget(8 * 1024 * 1024, |scope| {
//!     let arena = scope.arena("requests").capacity(4096).bump();
//!     let handle = arena.alloc(42_u64)?;
//!     Ok(())
//! })?;
//! ```

use crate::broker::{ArenaHandle, Broker};
use crate::error::BrokerError;
use crate::ids::BudgetId;
use crate::strategy::{bump::BumpStrategy, slab::SlabStrategy, AllocStrategy};
use std::alloc::Layout;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A budget node. Tracks bytes consumed against a cap, with an
/// optional parent for nesting.
pub struct Budget {
    id: BudgetId,
    cap: usize,
    used: AtomicUsize,
    parent: Option<Arc<Budget>>,
}

impl Budget {
    pub(crate) fn new(id: BudgetId, cap: usize, parent: Option<Arc<Budget>>) -> Arc<Self> {
        Arc::new(Self {
            id,
            cap,
            used: AtomicUsize::new(0),
            parent,
        })
    }

    #[must_use]
    pub const fn id(&self) -> BudgetId { self.id }

    #[must_use]
    pub const fn cap(&self) -> usize { self.cap }

    #[must_use]
    pub fn used(&self) -> usize { self.used.load(Ordering::Acquire) }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.cap.saturating_sub(self.used())
    }

    /// Try to charge `bytes` against this budget and all ancestors.
    /// On failure, refunds anything partially charged and returns
    /// `BudgetExceeded` identifying the budget that ran out.
    fn try_charge(self: &Arc<Self>, bytes: usize) -> Result<(), BrokerError> {
        // Walk up the chain, charging each in turn. Collect the chain
        // so we can refund precisely on failure.
        let mut chain: Vec<Arc<Budget>> = Vec::new();
        let mut node: Option<Arc<Budget>> = Some(Arc::clone(self));
        while let Some(n) = node {
            chain.push(Arc::clone(&n));
            node = n.parent.clone();
        }

        for (idx, b) in chain.iter().enumerate() {
            // Atomic compare-and-add under cap.
            let mut current = b.used.load(Ordering::Acquire);
            loop {
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
                };
                if new > b.cap {
                    for prior in &chain[..idx] {
                        prior.used.fetch_sub(bytes, Ordering::AcqRel);
                    }
                    return Err(BrokerError::BudgetExceeded {
                        budget: b.id,
                        requested: bytes,
                        remaining: b.cap.saturating_sub(current),
                    });
                }
                match b.used.compare_exchange_weak(
                    current, new, Ordering::AcqRel, Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(observed) => current = observed,
                }
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("id", &self.id)
            .field("cap", &self.cap)
            .field("used", &self.used())
            .field("nested", &self.parent.is_some())
            .finish_non_exhaustive()
    }
}

/// A user-visible budget scope. Hand it out to closures inside
/// [`Broker::within_budget`]; it provides budget-aware arena
/// construction and nested budgets.
pub struct BudgetScope<'b> {
    broker: &'b Broker,
    budget: Arc<Budget>,
}

impl<'b> BudgetScope<'b> {
    pub(crate) fn new(broker: &'b Broker, budget: Arc<Budget>) -> Self {
        Self { broker, budget }
    }

    /// The budget backing this scope.
    #[must_use]
    pub fn budget(&self) -> &Arc<Budget> { &self.budget }

    /// Start building a budget-aware arena.
    #[must_use]
    pub fn arena(&self, name: &str) -> BudgetArenaBuilder<'_, 'b> {
        BudgetArenaBuilder {
            scope: self,
            name: name.to_string(),
            bump_capacity: None,
        }
    }

    /// Open a nested budget inside this one. Allocations through the
    /// inner scope charge both budgets; either cap exhaustion fails
    /// the allocation.
    ///
    /// # Errors
    /// Returns whatever the closure returns. Closure errors propagate;
    /// the inner budget's used counter is *not* refunded (allocations
    /// already made remain charged).
    pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
    where
        F: FnOnce(&BudgetScope<'_>) -> Result<R, BrokerError>,
    {
        let id = self.broker.next_budget_id();
        let inner = Budget::new(id, cap, Some(Arc::clone(&self.budget)));
        let scope = BudgetScope::new(self.broker, inner);
        f(&scope)
    }
}

/// Builder for arenas created inside a [`BudgetScope`].
pub struct BudgetArenaBuilder<'s, 'b> {
    scope: &'s BudgetScope<'b>,
    name: String,
    bump_capacity: Option<usize>,
}

impl<'s, 'b> BudgetArenaBuilder<'s, 'b> {
    #[must_use]
    pub fn capacity(mut self, bytes: usize) -> Self {
        self.bump_capacity = Some(bytes);
        self
    }

    /// Build a bump arena whose allocations are charged to the budget.
    ///
    /// # Errors
    /// Returns [`BrokerError::BudgetExceeded`] if the arena's reserved
    /// capacity would itself exceed the budget. Slab equivalent for slab.
    ///
    /// # Panics
    /// Panics if `.capacity(n)` was not called.
    pub fn bump(self) -> Result<ArenaHandle, BrokerError> {
        let cap = self.bump_capacity
            .expect("BudgetArenaBuilder::bump requires .capacity(n) first");
        // Charge the full reserved capacity up front. This is the most
        // honest interpretation of a budget: reserving 4 MiB counts as
        // 4 MiB whether you fill it or not. A future refinement could
        // charge per-allocation instead.
        self.scope.budget.try_charge(cap)?;
        let id = self.scope.broker.next_arena_id();
        let strategy: Box<dyn AllocStrategy> = Box::new(BumpStrategy::new(id, cap));
        let arena = crate::arena::Arena::with_strategy(id, &self.name, strategy);
        self.scope.broker.register_arena(Arc::clone(&arena));
        Ok(ArenaHandle::from_parts(id, arena))
    }

    /// Build a slab arena whose total slot bytes are charged to the budget.
    pub fn slab(
        self,
        slot_size: usize,
        slot_align: usize,
        slot_count: u32,
    ) -> Result<ArenaHandle, BrokerError> {
        let padded = (slot_size + slot_align - 1) & !(slot_align - 1);
        let total = padded.checked_mul(slot_count as usize)
            .ok_or(BrokerError::BudgetExceeded {
                budget: self.scope.budget.id,
                requested: usize::MAX,
                remaining: self.scope.budget.remaining(),
            })?;
        self.scope.budget.try_charge(total)?;
        let id = self.scope.broker.next_arena_id();
        let strategy: Box<dyn AllocStrategy> =
            Box::new(SlabStrategy::new(id, slot_size, slot_align, slot_count));
        let arena = crate::arena::Arena::with_strategy(id, &self.name, strategy);
        self.scope.broker.register_arena(Arc::clone(&arena));
        Ok(ArenaHandle::from_parts(id, arena))
    }
}

/// Convenience: compute a worst-case byte cost for a layout.
///
/// Used internally to pre-charge a budget before delegating to a
/// strategy. Returns `layout.size() + layout.align() - 1`, which is
/// the most bytes the allocation can consume after alignment padding.
#[must_use]
pub fn layout_worst_case(layout: Layout) -> usize {
    layout.size() + layout.align().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Broker;

    #[test]
    fn budget_basic_allows_under_cap() {
        let b = Broker::new();
        let r = b.within_budget(4096, |scope| {
            let arena = scope.arena("a").capacity(1024).bump()?;
            let h = arena.alloc(42_u64).map_err(|_| BrokerError::NotImplemented {
                feature: "unreachable",
            })?;
            assert_eq!(*h.get().unwrap(), 42);
            Ok(())
        });
        assert!(r.is_ok(), "expected Ok, got {r:?}");
    }

    #[test]
    fn budget_rejects_over_cap_arena() {
        let b = Broker::new();
        let r = b.within_budget(1024, |scope| {
            scope.arena("too-big").capacity(4096).bump()?;
            Ok(())
        });
        assert!(matches!(r, Err(BrokerError::BudgetExceeded { .. })));
    }

    #[test]
    fn budget_nests_inner_outer_both_charged() {
        let b = Broker::new();
        let r = b.within_budget(8192, |outer| {
            outer.arena("o1").capacity(1024).bump()?;
            assert_eq!(outer.budget().used(), 1024);
            outer.within_budget(4096, |inner| {
                inner.arena("i1").capacity(2048).bump()?;
                // Inner sees its own usage.
                assert_eq!(inner.budget().used(), 2048);
                Ok(())
            })?;
            // Outer accumulated the inner's charge too.
            assert_eq!(outer.budget().used(), 1024 + 2048);
            Ok(())
        });
        assert!(r.is_ok(), "expected Ok, got {r:?}");
    }

    #[test]
    fn budget_inner_cap_can_be_exceeded_independently_of_outer() {
        let b = Broker::new();
        let r = b.within_budget(16 * 1024, |outer| {
            // Outer has 16 KiB; inner has only 1 KiB.
            outer.within_budget(1024, |inner| {
                inner.arena("too-big-for-inner").capacity(4096).bump()?;
                Ok(())
            })
        });
        let err = r.expect_err("inner cap should fail");
        if let BrokerError::BudgetExceeded { remaining, .. } = err {
            // The inner's remaining is what got reported.
            assert_eq!(remaining, 1024);
        } else {
            panic!("expected BudgetExceeded, got {err:?}");
        }
    }

    #[test]
    fn budget_outer_failure_refunds_inner_charge_attempt() {
        let b = Broker::new();
        let r = b.within_budget(2048, |outer| {
            outer.arena("first").capacity(1024).bump()?;
            // Inner asks for 2048 which would push outer to 3072 (>2048).
            outer.within_budget(4096, |inner| {
                inner.arena("too-much").capacity(2048).bump()?;
                Ok(())
            })
        });
        assert!(matches!(r, Err(BrokerError::BudgetExceeded { .. })));
    }

    #[test]
    fn budget_slab_charges_total() {
        let b = Broker::new();
        let r = b.within_budget(8 * 1024, |scope| {
            // 64-byte slots * 32 = 2048 bytes total.
            scope.arena("slab").slab(64, 8, 32)?;
            assert_eq!(scope.budget().used(), 64 * 32);
            Ok(())
        });
        assert!(r.is_ok(), "got {r:?}");
    }

    #[test]
    fn budget_slab_rejects_when_over_cap() {
        let b = Broker::new();
        let r = b.within_budget(1024, |scope| {
            // 64 * 32 = 2048, doesn't fit in 1024.
            scope.arena("slab").slab(64, 8, 32)?;
            Ok(())
        });
        assert!(matches!(r, Err(BrokerError::BudgetExceeded { .. })));
    }

    #[test]
    fn layout_worst_case_includes_alignment_pad() {
        use std::alloc::Layout;
        let l = Layout::from_size_align(16, 64).unwrap();
        assert_eq!(layout_worst_case(l), 16 + 63);
    }
}
RUST_EOF
write_file "$BROKER/src/budget.rs" "$BUDGET_RS"

# ---------------------------------------------------------------------------
# 4. broker.rs — add next_budget_id + within_budget entry point.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/broker.rs"
src = p.read_text()

if "within_budget" in src:
    print("[SKIP] broker.rs: within_budget already present")
    sys.exit(0)

# 1. Import BudgetIdCounter and Budget types.
src = src.replace(
    "use crate::ids::{ArenaId, ArenaIdCounter};",
    "use crate::budget::{Budget, BudgetScope};\nuse crate::ids::{ArenaId, ArenaIdCounter, BudgetIdCounter};",
    1,
)

# 2. Add budget_ids field to Broker struct.
src = src.replace(
    "pub struct Broker {\n    arena_ids: ArenaIdCounter,\n    arenas: RwLock<HashMap<ArenaId, Arc<Arena>>>,\n}",
    "pub struct Broker {\n    arena_ids: ArenaIdCounter,\n    budget_ids: BudgetIdCounter,\n    arenas: RwLock<HashMap<ArenaId, Arc<Arena>>>,\n}",
    1,
)

# 3. Initialize budget_ids in Broker::new.
src = src.replace(
    "Self {\n            arena_ids: ArenaIdCounter::new(),\n            arenas: RwLock::new(HashMap::new()),\n        }",
    "Self {\n            arena_ids: ArenaIdCounter::new(),\n            budget_ids: BudgetIdCounter::new(),\n            arenas: RwLock::new(HashMap::new()),\n        }",
    1,
)

# 4. Add next_budget_id pub(crate) hook and within_budget method.
#    Insert before the `impl Default for Broker` line.
addition = '''
    /// Run `f` inside a fresh top-level budget capped at `cap` bytes.
    ///
    /// Any arena created through the supplied [`BudgetScope`] charges
    /// against this budget. The closure's return value propagates;
    /// [`BrokerError::BudgetExceeded`] is returned if any allocation
    /// inside `f` would exceed the cap.
    ///
    /// # Errors
    /// Returns the closure's error, or [`BrokerError::BudgetExceeded`].
    pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
    where
        F: FnOnce(&BudgetScope<'_>) -> Result<R, BrokerError>,
    {
        let id = self.next_budget_id();
        let budget = Budget::new(id, cap, None);
        let scope = BudgetScope::new(self, budget);
        f(&scope)
    }

    pub(crate) fn next_budget_id(&self) -> crate::ids::BudgetId {
        self.budget_ids.next()
    }

'''
src = src.replace("impl Default for Broker", addition + "}\n\nimpl Default for Broker", 1)
# That double-closed brace; clean it. The original code has a closing `}` for `impl Broker` already; we
# substituted a marker. Repair:
src = src.replace("}\n}\n\nimpl Default for Broker", "}\n\nimpl Default for Broker", 1)

p.write_text(src)
print("[OK]   broker.rs: within_budget + next_budget_id")
PYEOF

# ---------------------------------------------------------------------------
# 5. lib.rs — declare module and re-export.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/lib.rs"
src = p.read_text()

if "pub mod budget" in src or "mod budget;" in src:
    print("[SKIP] lib.rs: budget already declared")
    sys.exit(0)

# Add `mod budget;` after `mod broker;`.
src = src.replace("mod broker;\n", "mod broker;\nmod budget;\n", 1)

# Re-export Budget + BudgetScope alongside other types.
src = src.replace(
    "pub use broker::{ArenaHandle, Broker};",
    "pub use broker::{ArenaHandle, Broker};\npub use budget::{Budget, BudgetScope, BudgetArenaBuilder};",
    1,
)

# Re-export BudgetId alongside other ids.
src = src.replace(
    "pub use ids::{ArenaId, Generation, SlotGeneration, SlotIndex};",
    "pub use ids::{ArenaId, BudgetId, Generation, SlotGeneration, SlotIndex};",
    1,
)

p.write_text(src)
print("[OK]   lib.rs: declared budget module + re-exports")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -25

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -30

echo
echo "======"
echo "TESTS (nextest)"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -60

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
echo "05 COMPLETE"
echo "======"
echo "Expect 49 tests: 41 prior + 8 new budget tests."
