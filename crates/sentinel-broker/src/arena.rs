//! Arena: a generation-tagged region of memory backed by an [`AllocStrategy`].
//!
//! An arena is a thin wrapper that pairs:
//! - identity (id, name) and a generation counter, with
//! - a strategy object that owns the backing memory.
//!
//! `Arena::alloc<T>` asks the strategy for raw bytes, writes `T` into
//! them, and returns a typed [`crate::Handle<T>`]. When the arena is
//! destroyed (via the broker), `invalidate()` bumps the generation,
//! turning every outstanding handle into a typed `UseAfterFree` error.

use crate::error::BrokerError;
use crate::handle::Handle;
use crate::ids::{ArenaId, Generation};
use crate::strategy::{AllocStrategy, StrategyKind};
use std::alloc::Layout;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct Arena {
    id: ArenaId,
    name: Box<str>,
    strategy: Box<dyn AllocStrategy>,
    /// Current generation. Initialized to 1; advanced when
    /// [`Arena::invalidate`] runs (from broker destroy_arena or Drop).
    generation: AtomicU32,
}

impl Arena {
    /// Create a new arena from a pre-built strategy.
    pub(crate) fn with_strategy(
        id: ArenaId,
        name: &str,
        strategy: Box<dyn AllocStrategy>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            name: name.into(),
            strategy,
            generation: AtomicU32::new(Generation::INITIAL.raw()),
        })
    }

    #[must_use]
    pub const fn id(&self) -> ArenaId { self.id }

    #[must_use]
    pub fn name(&self) -> &str { &self.name }

    #[must_use]
    pub fn capacity(&self) -> usize { self.strategy.capacity() }

    #[must_use]
    pub fn used(&self) -> usize { self.strategy.used() }

    #[must_use]
    pub fn available(&self) -> usize { self.strategy.available() }

    #[must_use]
    pub fn strategy_kind(&self) -> StrategyKind { self.strategy.kind() }

    #[must_use]
    pub fn generation(&self) -> Generation {
        Generation(self.generation.load(Ordering::Acquire))
    }

    /// Advance the generation counter, invalidating every outstanding
    /// handle. Called by `Broker::destroy_arena` and as a safety net
    /// from `Drop`.
    pub(crate) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Allocate a value of type `T` into this arena.
    ///
    /// # Errors
    /// Returns [`BrokerError::OutOfMemory`] if the strategy can't
    /// satisfy the request.
    pub fn alloc<T>(self: &Arc<Self>, value: T) -> Result<Handle<T>, BrokerError>
    where
        T: 'static,
    {
        let layout = Layout::new::<T>();
        let allocated = self.strategy.alloc_raw(layout)?;

        // SAFETY: strategy returned a properly-aligned, properly-sized
        // pointer; we own it exclusively until this function returns.
        unsafe {
            allocated.ptr.as_ptr().cast::<T>().write(value);
        }

        Ok(Handle::new(
            self.id,
            allocated.slot,
            self.generation(),
            Arc::downgrade(self),
        ))
    }

    /// Resolve a slot index back to a pointer. Used by [`Handle::get`].
    ///
    /// # Errors
    /// Returns [`BrokerError::InvalidSlot`] for unknown slots.
    pub(crate) fn slot_ptr(&self, slot: crate::ids::SlotIndex) -> Result<*const u8, BrokerError> {
        Ok(self.strategy.slot_ptr(slot)?.as_ptr())
    }
}

impl std::fmt::Debug for Arena {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arena")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.strategy.kind())
            .field("capacity", &self.capacity())
            .field("used", &self.used())
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // Safety net: bump generation so any handle that somehow
        // still has a Weak to us fails its generation check.
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::bump::BumpStrategy;

    fn make_bump(id_raw: u64, cap: usize) -> Arc<Arena> {
        let id = ArenaId(id_raw);
        Arena::with_strategy(id, "test", Box::new(BumpStrategy::new(id, cap)))
    }

    #[test]
    fn arena_basic_alloc() {
        let a = make_bump(1, 1024);
        let h = a.alloc(42_u64).unwrap();
        assert_eq!(*h.get().unwrap(), 42);
    }

    #[test]
    fn arena_oom_returns_error() {
        let a = make_bump(2, 8);
        a.alloc(0_u64).unwrap(); // fills it
        let err = a.alloc(0_u64).unwrap_err();
        assert!(matches!(err, BrokerError::OutOfMemory { .. }));
    }

    #[test]
    fn arena_drop_invalidates_handles() {
        let a = make_bump(3, 64);
        let h = a.alloc(7_u64).unwrap();
        assert_eq!(*h.get().unwrap(), 7);
        drop(a);
        assert!(h.get().is_err());
    }

    #[test]
    fn arena_tracks_usage() {
        let a = make_bump(4, 1024);
        assert_eq!(a.used(), 0);
        a.alloc(0_u64).unwrap();
        assert!(a.used() >= 8);
        assert_eq!(a.strategy_kind(), StrategyKind::Bump);
    }

    #[test]
    fn arena_many_allocations() {
        let a = make_bump(5, 1024);
        for i in 0..100_u64 {
            a.alloc(i).unwrap();
        }
        assert!(a.used() >= 800);
    }

    #[test]
    fn arena_alignment_respected() {
        #[repr(align(64))]
        #[allow(dead_code)]
        struct AlignedBlob([u8; 64]);

        let a = make_bump(6, 4096);
        let h = a.alloc(AlignedBlob([0; 64])).unwrap();
        let r = h.get().unwrap();
        let addr = std::ptr::addr_of!(*r) as usize;
        assert_eq!(addr % 64, 0);
    }

    #[test]
    fn arena_concurrent_allocation() {
        use std::thread;
        let a = make_bump(7, 1024 * 64);
        let mut handles = vec![];
        for t in 0..8_u64 {
            let a2 = Arc::clone(&a);
            handles.push(thread::spawn(move || {
                for i in 0..100_u64 {
                    a2.alloc(t * 1000 + i).unwrap();
                }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert!(a.used() >= 8 * 100 * 8);
    }
}