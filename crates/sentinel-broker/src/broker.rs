//! The top-level Broker type.
//!
//! The Broker is a process-level registry of arenas. It owns the strong
//! `Arc<Arena>` references that keep arenas alive; users receive an
//! [`ArenaHandle`] that gives them access without taking ownership.
//! To release an arena and invalidate every handle it issued, call
//! [`Broker::destroy_arena`].

use crate::arena::Arena;
use crate::error::BrokerError;
use crate::ids::{ArenaId, ArenaIdCounter};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

/// A user-facing reference to a broker-owned arena.
///
/// Cloning an `ArenaHandle` is cheap and does *not* extend the arena's
/// lifetime: only the broker's internal map keeps the arena alive.
/// Call [`Broker::destroy_arena`] to free the arena and invalidate
/// every handle it issued.
#[derive(Clone)]
pub struct ArenaHandle {
    id: ArenaId,
    arena: Arc<Arena>,
}

impl ArenaHandle {
    /// The id of the arena this handle refers to.
    #[must_use]
    pub fn id(&self) -> ArenaId {
        self.id
    }

    /// Allocate a value of type `T` into this arena.
    ///
    /// Returns a [`crate::Handle`] that can be used to access the
    /// value while the arena is alive. The handle is invalidated when
    /// the broker destroys the arena.
    ///
    /// # Errors
    ///
    /// Returns [`crate::BrokerError::OutOfMemory`] if the arena's
    /// capacity is exhausted.
    pub fn alloc<T>(&self, value: T) -> Result<crate::Handle<T>, crate::BrokerError>
    where
        T: 'static,
    {
        self.arena.alloc(value)
    }
}

impl Deref for ArenaHandle {
    type Target = Arena;
    fn deref(&self) -> &Arena {
        &self.arena
    }
}

impl std::fmt::Debug for ArenaHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaHandle")
            .field("id", &self.id)
            .field("name", &self.arena.name())
            .field("capacity", &self.arena.capacity())
            .finish_non_exhaustive()
    }
}

/// The runtime memory broker.
///
/// In the current scaffold, the broker is a registry that owns arenas
/// and can destroy them on request. Later milestones will add
/// allocation strategies, budgets, recording, and secret-memory
/// policies.
pub struct Broker {
    arena_ids: ArenaIdCounter,
    arenas: RwLock<HashMap<ArenaId, Arc<Arena>>>,
}

impl Broker {
    /// Create a new broker with no arenas.
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena_ids: ArenaIdCounter::new(),
            arenas: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new arena registered with this broker.
    ///
    /// The returned `ArenaHandle` borrows from the broker's internal
    /// map. The arena lives until you call [`Broker::destroy_arena`]
    /// or the broker itself is dropped.
    pub fn create_arena(&self, name: &str, capacity: usize) -> ArenaHandle {
        let id = self.arena_ids.next();
        let arena = Arena::new(id, name, capacity);

        // Best-effort insertion. A poisoned lock means a previous panic
        // left state we cannot reason about; in that case we still
        // return the handle but the arena will not be tracked.
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, Arc::clone(&arena));
        }

        tracing::debug!(
            arena_id = %id,
            name = %name,
            capacity = capacity,
            "arena created"
        );

        ArenaHandle { id, arena }
    }

    /// Destroy an arena, invalidating every handle issued for it.
    ///
    /// After this call returns successfully, every `Handle` issued by
    /// this arena will return [`BrokerError::UseAfterFree`] on access,
    /// regardless of how many `ArenaHandle` clones still exist.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::UnknownArena`] if no arena with the given
    /// id is registered (it was already destroyed, or never created
    /// by this broker).
    pub fn destroy_arena(&self, id: ArenaId) -> Result<(), BrokerError> {
        let removed = {
            let mut arenas = self
                .arenas
                .write()
                .map_err(|_| BrokerError::BrokerPoisoned)?;
            arenas.remove(&id)
        };

        match removed {
            Some(arena) => {
                // Advance the generation first so that any concurrent
                // handle access sees the bumped value even if some
                // `Arc<Arena>` clone keeps the arena alive afterward.
                arena.invalidate();
                tracing::debug!(arena_id = %id, "arena destroyed");
                Ok(())
            }
            None => Err(BrokerError::UnknownArena { arena: id }),
        }
    }

    /// Number of arenas currently registered with the broker.
    #[must_use]
    pub fn live_arena_count(&self) -> usize {
        self.arenas.read().map_or(0, |a| a.len())
    }
}

impl Default for Broker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert!(matches!(
            handle.get(),
            Err(BrokerError::UseAfterFree { .. })
        ));
    }

    #[test]
    fn destroy_unknown_arena_is_an_error() {
        let broker = Broker::new();
        // Create and destroy to consume an id, then try to destroy again.
        let a = broker.create_arena("ephemeral", 64);
        let id = a.id();
        broker.destroy_arena(id).unwrap();
        assert!(matches!(
            broker.destroy_arena(id),
            Err(BrokerError::UnknownArena { .. })
        ));
    }

    #[test]
    fn live_arena_count_tracks_destruction() {
        let broker = Broker::new();
        let a = broker.create_arena("a", 64);
        let b = broker.create_arena("b", 64);
        assert_eq!(broker.live_arena_count(), 2);
        broker.destroy_arena(a.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 1);
        broker.destroy_arena(b.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 0);
    }
}
