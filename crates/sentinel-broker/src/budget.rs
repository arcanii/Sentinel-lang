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
use crate::recording::Event;
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
            let next = n.parent.clone();
            chain.push(n);
            node = next;
        }

        for (idx, b) in chain.iter().enumerate() {
            // Atomic compare-and-add under cap.
            let mut current = b.used.load(Ordering::Acquire);
            loop {
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
        if let Some(r) = self.broker.recorder_arc() {
            r.record(Event::BudgetOpened {
                id,
                cap,
                parent: Some(self.budget.id()),
                at_ns: r.now_ns(),
            });
        }
        let scope = BudgetScope::new(self.broker, inner);
        let result = f(&scope);
        if let Some(r) = self.broker.recorder_arc() {
            r.record(Event::BudgetClosed {
                id,
                used_at_close: scope.budget().used(),
                at_ns: r.now_ns(),
            });
        }
        result
    }
}

/// Builder for arenas created inside a [`BudgetScope`].
pub struct BudgetArenaBuilder<'s, 'b> {
    scope: &'s BudgetScope<'b>,
    name: String,
    bump_capacity: Option<usize>,
}

impl BudgetArenaBuilder<'_, '_> {
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
        let arena = crate::arena::Arena::with_strategy_and_recorder(id, &self.name, strategy, self.scope.broker.recorder_arc());
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
        let arena = crate::arena::Arena::with_strategy_and_recorder(id, &self.name, strategy, self.scope.broker.recorder_arc());
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
#[allow(dead_code)] // public utility; currently only exercised in tests
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