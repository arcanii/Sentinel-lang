//! Arena: a generation-tagged region of memory backed by an [`AllocStrategy`].

use crate::error::BrokerError;
use crate::handle::Handle;
use crate::ids::{ArenaId, Generation, SlotIndex};
use crate::strategy::{AllocStrategy, SlotPtr, StrategyKind};
use std::alloc::Layout;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct Arena {
    id: ArenaId,
    name: Box<str>,
    strategy: Box<dyn AllocStrategy>,
    generation: AtomicU32,
}

impl Arena {
    pub(crate) fn with_strategy(id: ArenaId, name: &str, strategy: Box<dyn AllocStrategy>) -> Arc<Self> {
        Arc::new(Self {
            id,
            name: name.into(),
            strategy,
            generation: AtomicU32::new(Generation::INITIAL.raw()),
        })
    }

    #[must_use] pub const fn id(&self) -> ArenaId { self.id }
    #[must_use] pub fn name(&self) -> &str { &self.name }
    #[must_use] pub fn capacity(&self) -> usize { self.strategy.capacity() }
    #[must_use] pub fn used(&self) -> usize { self.strategy.used() }
    #[must_use] pub fn available(&self) -> usize { self.strategy.available() }
    #[must_use] pub fn strategy_kind(&self) -> StrategyKind { self.strategy.kind() }

    #[must_use]
    pub fn generation(&self) -> Generation {
        Generation(self.generation.load(Ordering::Acquire))
    }

    pub(crate) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Allocate a value of type `T` into this arena.
    ///
    /// # Errors
    /// Returns [`BrokerError::OutOfMemory`] if the strategy is full.
    pub fn alloc<T>(self: &Arc<Self>, value: T) -> Result<Handle<T>, BrokerError>
    where T: 'static,
    {
        let layout = Layout::new::<T>();
        let allocated = self.strategy.alloc_raw(layout)?;

        // SAFETY: strategy returned a properly aligned, properly sized
        // pointer; this byte range is ours.
        unsafe {
            allocated.ptr.as_ptr().cast::<T>().write(value);
        }

        Ok(Handle::new(
            self.id,
            allocated.slot,
            self.generation(),
            allocated.generation,
            Arc::downgrade(self),
        ))
    }

    /// Free a slot, returning whether the strategy supports it.
    ///
    /// After this returns Ok, the supplied handle becomes invalid;
    /// further use returns [`BrokerError::UseAfterFreeSlot`].
    ///
    /// # Errors
    /// - [`BrokerError::NotImplemented`] if the strategy doesn't recycle.
    /// - [`BrokerError::UseAfterFreeSlot`] for double-free.
    /// - [`BrokerError::UseAfterFree`] if the arena was destroyed.
    pub fn free<T>(&self, handle: &Handle<T>) -> Result<(), BrokerError> {
        // First verify the arena generation matches.
        let cur = self.generation();
        if cur != handle.arena_generation {
            return Err(BrokerError::UseAfterFree {
                arena: self.id,
                slot: handle.slot,
                issued: handle.arena_generation,
                current: cur,
            });
        }
        self.strategy.free(handle.slot, handle.slot_generation)
    }

    pub(crate) fn strategy_slot_ptr(&self, slot: SlotIndex) -> Result<SlotPtr, BrokerError> {
        self.strategy.slot_ptr(slot)
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
    fn drop(&mut self) { self.invalidate(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::bump::BumpStrategy;
    use crate::strategy::slab::SlabStrategy;

    fn make_bump(id_raw: u64, cap: usize) -> Arc<Arena> {
        let id = ArenaId(id_raw);
        Arena::with_strategy(id, "test", Box::new(BumpStrategy::new(id, cap)))
    }

    fn make_slab(id_raw: u64, slot_size: usize, slot_align: usize, slot_count: u32) -> Arc<Arena> {
        let id = ArenaId(id_raw);
        Arena::with_strategy(id, "test-slab",
            Box::new(SlabStrategy::new(id, slot_size, slot_align, slot_count)))
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
        a.alloc(0_u64).unwrap();
        assert!(matches!(a.alloc(0_u64).unwrap_err(), BrokerError::OutOfMemory { .. }));
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
        for i in 0..100_u64 { a.alloc(i).unwrap(); }
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
                for i in 0..100_u64 { a2.alloc(t * 1000 + i).unwrap(); }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert!(a.used() >= 8 * 100 * 8);
    }

    #[test]
    fn slab_free_invalidates_only_that_handle() {
        let a = make_slab(10, 16, 8, 4);
        let h1 = a.alloc(11_u64).unwrap();
        let h2 = a.alloc(22_u64).unwrap();
        assert_eq!(*h1.get().unwrap(), 11);
        assert_eq!(*h2.get().unwrap(), 22);
        a.free(&h1).unwrap();
        assert!(matches!(h1.get().unwrap_err(), BrokerError::UseAfterFreeSlot { .. }));
        // h2 is unaffected.
        assert_eq!(*h2.get().unwrap(), 22);
    }

    #[test]
    fn slab_recycles_freed_slots() {
        let a = make_slab(11, 16, 8, 2);
        let h1 = a.alloc(1_u64).unwrap();
        let _h2 = a.alloc(2_u64).unwrap();
        // Slab full now.
        assert!(matches!(a.alloc(3_u64).unwrap_err(), BrokerError::OutOfMemory { .. }));
        a.free(&h1).unwrap();
        // Recycled allocation succeeds.
        let h3 = a.alloc(33_u64).unwrap();
        assert_eq!(*h3.get().unwrap(), 33);
        // Original handle for the now-recycled slot is dead.
        assert!(matches!(h1.get().unwrap_err(), BrokerError::UseAfterFreeSlot { .. }));
    }

    #[test]
    fn slab_double_free_is_caught() {
        let a = make_slab(12, 16, 8, 4);
        let h = a.alloc(1_u64).unwrap();
        a.free(&h).unwrap();
        let err = a.free(&h).unwrap_err();
        assert!(matches!(err, BrokerError::UseAfterFreeSlot { .. }));
    }

    #[test]
    fn bump_free_returns_not_implemented_via_arena() {
        let a = make_bump(13, 64);
        let h = a.alloc(5_u64).unwrap();
        assert!(matches!(a.free(&h).unwrap_err(), BrokerError::NotImplemented { .. }));
    }

    #[test]
    fn slab_recycling_stress() {
        use std::thread;
        let a = make_slab(14, 32, 8, 64);
        let mut threads = vec![];
        for _ in 0..8 {
            let a2 = Arc::clone(&a);
            threads.push(thread::spawn(move || {
                for i in 0..200_u64 {
                    if let Ok(h) = a2.alloc(i) {
                        // Some randomness via i parity.
                        let _ = h.get().unwrap();
                        if i % 2 == 0 { a2.free(&h).unwrap(); }
                    }
                }
            }));
        }
        for t in threads { t.join().unwrap(); }
    }
}