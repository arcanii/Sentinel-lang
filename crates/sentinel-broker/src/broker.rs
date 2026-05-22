//! The top-level Broker type.
//!
//! Owns the strong `Arc<Arena>` references that keep arenas alive.
//! Users receive an [`ArenaHandle`] for access without ownership.

use crate::arena::Arena;
use crate::builder::ArenaBuilder;
use crate::error::BrokerError;
use crate::ids::{ArenaId, ArenaIdCounter};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

/// A user-facing reference to a broker-owned arena.
#[derive(Clone)]
pub struct ArenaHandle {
    id: ArenaId,
    arena: Arc<Arena>,
}

impl ArenaHandle {
    pub(crate) fn from_parts(id: ArenaId, arena: Arc<Arena>) -> Self {
        Self { id, arena }
    }

    #[must_use]
    pub fn id(&self) -> ArenaId { self.id }

    /// Allocate a value of type `T` into this arena.
    ///
    /// # Errors
    /// Returns [`BrokerError::OutOfMemory`] if capacity is exhausted.
    pub fn alloc<T>(&self, value: T) -> Result<crate::Handle<T>, BrokerError>
    where
        T: 'static,
    {
        self.arena.alloc(value)
    }

    /// Free a handle's slot, advancing the slot's generation.
    ///
    /// After this returns Ok, the handle becomes invalid; further
    /// access returns [`BrokerError::UseAfterFreeSlot`].
    ///
    /// # Errors
    /// - [`BrokerError::NotImplemented`] for strategies that don't recycle.
    /// - [`BrokerError::UseAfterFreeSlot`] for double-free.
    pub fn free<T>(&self, handle: &crate::Handle<T>) -> Result<(), BrokerError> {
        self.arena.free(handle)
    }
}

impl Deref for ArenaHandle {
    type Target = Arena;
    fn deref(&self) -> &Arena { &self.arena }
}

impl std::fmt::Debug for ArenaHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaHandle")
            .field("id", &self.id)
            .field("name", &self.arena.name())
            .field("kind", &self.arena.strategy_kind())
            .field("capacity", &self.arena.capacity())
            .finish_non_exhaustive()
    }
}

pub struct Broker {
    arena_ids: ArenaIdCounter,
    arenas: RwLock<HashMap<ArenaId, Arc<Arena>>>,
}

impl Broker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena_ids: ArenaIdCounter::new(),
            arenas: RwLock::new(HashMap::new()),
        }
    }

    /// Start building an arena with a fluent API.
    ///
    /// ```ignore
    /// let arena = broker.arena("requests").capacity(4096).bump();
    /// let slab  = broker.arena("events").slab(64, 8, 256);
    /// ```
    #[must_use]
    pub fn arena(&self, name: &str) -> ArenaBuilder<'_> {
        ArenaBuilder::new(self, name.to_string())
    }

    /// Convenience: create a bump arena directly (kept for compatibility
    /// with the A0–A2 API). Equivalent to
    /// `broker.arena(name).capacity(capacity).bump()`.
    pub fn create_arena(&self, name: &str, capacity: usize) -> ArenaHandle {
        self.arena(name).capacity(capacity).bump()
    }

    /// Destroy an arena, invalidating every handle issued for it.
    ///
    /// # Errors
    /// Returns [`BrokerError::UnknownArena`] if no such arena exists.
    pub fn destroy_arena(&self, id: ArenaId) -> Result<(), BrokerError> {
        let removed = {
            let mut arenas = self.arenas.write().map_err(|_| BrokerError::BrokerPoisoned)?;
            arenas.remove(&id)
        };
        match removed {
            Some(arena) => {
                arena.invalidate();
                tracing::debug!(arena_id = %id, "arena destroyed");
                Ok(())
            }
            None => Err(BrokerError::UnknownArena { arena: id }),
        }
    }

    /// Number of arenas currently registered.
    #[must_use]
    pub fn live_arena_count(&self) -> usize {
        self.arenas.read().map_or(0, |a| a.len())
    }

    // --- pub(crate) hooks used by the builder ---

    pub(crate) fn next_arena_id(&self) -> ArenaId {
        self.arena_ids.next()
    }

    pub(crate) fn register_arena(&self, arena: Arc<Arena>) {
        let id = arena.id();
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, arena);
        }
        tracing::debug!(arena_id = %id, "arena registered");
    }
}

impl Default for Broker {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::StrategyKind;

    #[test]
    fn broker_creates_arenas() {
        let b = Broker::new();
        let a1 = b.create_arena("first", 1024);
        let a2 = b.create_arena("second", 2048);
        assert_ne!(a1.id(), a2.id());
        assert_eq!(a1.capacity(), 1024);
        assert_eq!(a2.capacity(), 2048);
    }

    #[test]
    fn broker_arena_ids_are_distinct() {
        let b = Broker::new();
        let mut ids = std::collections::HashSet::new();
        for i in 0..100 {
            let a = b.create_arena(&format!("arena{i}"), 64);
            ids.insert(a.id());
        }
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn destroy_arena_invalidates_handles() {
        let broker = Broker::new();
        let arena = broker.create_arena("example", 4096);
        let handle = arena.alloc(42_u64).unwrap();
        assert_eq!(*handle.get().unwrap(), 42);
        broker.destroy_arena(arena.id()).unwrap();
        assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));
    }

    #[test]
    fn destroy_unknown_arena_is_an_error() {
        let broker = Broker::new();
        let a = broker.create_arena("ephemeral", 64);
        let id = a.id();
        broker.destroy_arena(id).unwrap();
        assert!(matches!(broker.destroy_arena(id), Err(BrokerError::UnknownArena { .. })));
    }

    #[test]
    fn live_arena_count_tracks_destruction() {
        let broker = Broker::new();
        let a = broker.create_arena("a", 64);
        let b2 = broker.create_arena("b", 64);
        assert_eq!(broker.live_arena_count(), 2);
        broker.destroy_arena(a.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 1);
        broker.destroy_arena(b2.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 0);
    }

    #[test]
    fn mixed_bump_and_slab_arenas() {
        let b = Broker::new();
        let bump = b.arena("bump").capacity(1024).bump();
        let slab = b.arena("slab").slab(64, 8, 16);
        assert_eq!(bump.strategy_kind(), StrategyKind::Bump);
        assert_eq!(slab.strategy_kind(), StrategyKind::Slab);

        let hb = bump.alloc(1_u64).unwrap();
        let hs = slab.alloc(2_u64).unwrap();
        assert_eq!(*hb.get().unwrap(), 1);
        assert_eq!(*hs.get().unwrap(), 2);

        b.destroy_arena(slab.id()).unwrap();
        assert!(hs.get().is_err());
        assert_eq!(*hb.get().unwrap(), 1);
    }

    #[test]
    fn slab_oom_when_slots_exhausted() {
        let b = Broker::new();
        let slab = b.arena("tiny").slab(8, 8, 2);
        slab.alloc(1_u64).unwrap();
        slab.alloc(2_u64).unwrap();
        let err = slab.alloc(3_u64).unwrap_err();
        assert!(matches!(err, BrokerError::OutOfMemory { .. }));
    }

    #[test]
    fn slab_rejects_oversized_allocation() {
        let b = Broker::new();
        let slab = b.arena("small-slots").slab(4, 4, 16);
        let err = slab.alloc(0_u64).unwrap_err(); // u64 is 8 bytes, slot is 4
        assert!(matches!(err, BrokerError::OutOfMemory { .. }));
    }

    #[test]
    fn bump_strategy_free_returns_not_implemented() {
        // Bump never recycles: free() returns NotImplemented.
        use crate::ids::{ArenaId, SlotGeneration, SlotIndex};
        use crate::strategy::{bump::BumpStrategy, AllocStrategy};
        let s = BumpStrategy::new(ArenaId(99), 64);
        let err = s.free(SlotIndex(0), SlotGeneration::INITIAL).unwrap_err();
        assert!(matches!(err, BrokerError::NotImplemented { .. }));
    }

    #[test]
    fn slab_strategy_free_recycles() {
        // Slab supports recycling. Allocate, free with the issued
        // generation, then allocate again into the same slot.
        use crate::ids::ArenaId;
        use crate::strategy::{slab::SlabStrategy, AllocStrategy};
        use std::alloc::Layout;
        let s = SlabStrategy::new(ArenaId(100), 16, 8, 2);
        let a = s.alloc_raw(Layout::new::<u64>()).unwrap();
        s.free(a.slot, a.generation).unwrap();
        // Double free: generation has advanced, so the second free fails.
        let err = s.free(a.slot, a.generation).unwrap_err();
        assert!(matches!(err, BrokerError::UseAfterFreeSlot { .. }));
    }
}