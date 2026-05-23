//! Builder for constructing arenas with a chosen [`AllocStrategy`].
//!
//! ```ignore
//! let arena = broker.arena("requests").capacity(4096).bump().build();
//! let slab  = broker.arena("events").slab(64, 8, 256).build();
//! ```

use crate::arena::Arena;
use crate::broker::{ArenaHandle, Broker};
use crate::strategy::{bump::BumpStrategy, slab::SlabStrategy, AllocStrategy};
use crate::secret::{SecretPolicy, SecretStrategy};
use std::sync::Arc;

/// Fluent builder for arena creation.
///
/// Obtain one via [`Broker::arena`].
pub struct ArenaBuilder<'b> {
    broker: &'b Broker,
    name: String,
    /// Bump capacity, set by `.capacity(n)`. Required for `.bump()`.
    bump_capacity: Option<usize>,
    /// Optional secret-memory policy. If set, the chosen strategy
    /// will be wrapped in a [`SecretStrategy`].
    secret_policy: Option<SecretPolicy>,
}

impl<'b> ArenaBuilder<'b> {
    pub(crate) fn new(broker: &'b Broker, name: String) -> Self {
        Self { broker, name, bump_capacity: None, secret_policy: None }
    }

    /// Set the capacity (in bytes) for a bump-allocated arena.
    #[must_use]
    pub fn capacity(mut self, bytes: usize) -> Self {
        self.bump_capacity = Some(bytes);
        self
    }


    /// Apply a secret-memory policy to this arena. The strategy
    /// produced by [`bump`] or [`slab`] will be wrapped in a
    /// [`SecretStrategy`] that enforces the policy.
    #[must_use]
    pub fn secret(mut self, policy: SecretPolicy) -> Self {
        self.secret_policy = Some(policy);
        self
    }

    /// Build a bump-allocated arena. Requires a prior call to [`capacity`].
    ///
    /// # Panics
    /// Panics if [`capacity`] was not called.
    #[must_use]
    pub fn bump(self) -> ArenaHandle {
        let cap = self.bump_capacity
            .expect("ArenaBuilder::bump requires .capacity(n) first");
        let id = self.broker.next_arena_id();
        let inner: Box<dyn AllocStrategy> = Box::new(BumpStrategy::new(id, cap));
        let strategy: Box<dyn AllocStrategy> = match self.secret_policy {
            Some(p) if p != SecretPolicy::NONE => Box::new(
                SecretStrategy::wrap(inner, p)
                    .expect("SecretStrategy::wrap failed (check mlock permissions)"),
            ),
            _ => inner,
        };
        let arena = Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc());
        self.broker.register_arena(Arc::clone(&arena));
        ArenaHandle::from_parts(id, arena)
    }

    /// Build a slab-allocated arena with `slot_count` fixed-size slots.
    ///
    /// # Panics
    /// Panics on invalid sizing (see [`SlabStrategy::new`]).
    #[must_use]
    pub fn slab(self, slot_size: usize, slot_align: usize, slot_count: u32) -> ArenaHandle {
        let id = self.broker.next_arena_id();
        let inner: Box<dyn AllocStrategy> = Box::new(SlabStrategy::new(id, slot_size, slot_align, slot_count));
        let strategy: Box<dyn AllocStrategy> = match self.secret_policy {
            Some(p) if p != SecretPolicy::NONE => Box::new(
                SecretStrategy::wrap(inner, p)
                    .expect("SecretStrategy::wrap failed (check mlock permissions)"),
            ),
            _ => inner,
        };
        let arena = Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc());
        self.broker.register_arena(Arc::clone(&arena));
        ArenaHandle::from_parts(id, arena)
    }
}

#[cfg(test)]
mod tests {
    use crate::Broker;
    use crate::strategy::StrategyKind;

    #[test]
    fn builder_bump_basic() {
        let b = Broker::new();
        let a = b.arena("test").capacity(1024).bump();
        assert_eq!(a.strategy_kind(), StrategyKind::Bump);
        assert_eq!(a.capacity(), 1024);
    }

    #[test]
    fn builder_slab_basic() {
        let b = Broker::new();
        let a = b.arena("test").slab(64, 8, 16);
        assert_eq!(a.strategy_kind(), StrategyKind::Slab);
        assert_eq!(a.capacity(), 64 * 16);
    }

    #[test]
    #[should_panic(expected = "ArenaBuilder::bump requires .capacity")]
    fn builder_bump_without_capacity_panics() {
        let b = Broker::new();
        let _ = b.arena("test").bump();
    }
}