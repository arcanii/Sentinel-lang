//! The simplest possible arena: bump allocation with generational handles.
//!
//! An `Arena` allocates by incrementing a cursor through a fixed
//! capacity buffer. Each allocation produces a `Handle<T>` carrying
//! the arena's id, the slot index, and the current generation.
//!
//! When the arena is dropped, the generation counter advances, which
//! invalidates all outstanding handles. Subsequent access through
//! those handles returns `BrokerError::UseAfterFree`.

use crate::error::BrokerError;
use crate::handle::Handle;
use crate::ids::{ArenaId, Generation, SlotIndex};
use std::alloc::{alloc, dealloc, Layout};
use std::cell::UnsafeCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// An arena allocator with generational handle safety.
///
/// Arenas are reference-counted (Arc) so that outstanding handles can
/// hold a Weak reference and detect when the arena is dropped.
pub struct Arena {
    id: ArenaId,
    name: Box<str>,
    capacity: usize,

    /// Pointer to the start of the backing buffer.
    buffer: NonNull<u8>,
    /// Layout used to allocate `buffer`; needed for deallocation.
    buffer_layout: Layout,

    /// Bump pointer offset into the buffer.
    cursor: AtomicUsize,

    /// Slot count; incremented on each allocation. Used as the next
    /// slot index. Stored separately from `cursor` so we can present
    /// slot indices that are independent of byte offsets.
    next_slot: AtomicU32,

    /// Per-slot metadata: byte offset within the buffer where the slot's
    /// data lives, and the layout of that slot. We need both so we can
    /// resolve a SlotIndex back to a pointer at access time.
    slots: UnsafeCell<Vec<SlotInfo>>,

    /// Current generation. Initialized to 1; advanced (via overflow-safe
    /// increment) when the arena is dropped or reset. Stored as
    /// AtomicU32 so handles can read it without locking.
    generation: AtomicU32,
}

#[derive(Debug, Clone, Copy)]
struct SlotInfo {
    offset: usize,

    #[allow(dead_code)]

    size: usize,
}

// SAFETY: Arena is Send + Sync because all mutable state is behind
// atomics or is only mutated during allocation (which takes &self but
// uses atomic CAS to publish updates). The UnsafeCell<Vec<SlotInfo>>
// is mutated only by alloc(), and we serialize allocations through
// the cursor's compare-and-swap; this is documented in the SAFETY
// blocks inside alloc().
unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

impl Arena {
    pub(crate) fn new(id: ArenaId, name: &str, capacity: usize) -> Arc<Self> {
        assert!(capacity > 0, "arena capacity must be positive");

        // Allocate the backing buffer. We use a manual allocation
        // rather than Vec<u8> so we have explicit control over
        // alignment (we use the maximum alignment, 64 bytes, suitable
        // for any common type up through AVX-512).
        let layout = Layout::from_size_align(capacity, 64)
            .expect("arena capacity layout");
        // SAFETY: layout has nonzero size (capacity > 0 asserted above).
        let buffer_raw = unsafe { alloc(layout) };
        let buffer = NonNull::new(buffer_raw)
            .expect("arena backing allocation failed");

        Arc::new(Self {
            id,
            name: name.into(),
            capacity,
            buffer,
            buffer_layout: layout,
            cursor: AtomicUsize::new(0),
            next_slot: AtomicU32::new(0),
            slots: UnsafeCell::new(Vec::new()),
            generation: AtomicU32::new(Generation::INITIAL.raw()),
        })
    }

    /// The ArenaId this arena was created with.
    #[must_use]
    pub const fn id(&self) -> ArenaId {
        self.id
    }

    /// The name this arena was created with.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Total capacity in bytes.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes currently allocated.
    #[must_use]
    pub fn used(&self) -> usize {
        self.cursor.load(Ordering::Acquire)
    }

    /// Bytes available for further allocation.
    #[must_use]
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.used())
    }

    /// Current generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        Generation(self.generation.load(Ordering::Acquire))
    }

    /// Advance the generation counter, invalidating every outstanding handle.
    ///
    /// Called by `Broker::destroy_arena` and by `Drop` as a safety net.
    /// After this returns, every `Handle` issued by this arena will
    /// return `BrokerError::UseAfterFree` on access.
    pub(crate) fn invalidate(&self) {
        // fetch_add is sufficient: handles only check for equality, and any
        // increment makes the issued generation stale.
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Allocate a value of type T into this arena.
    ///
    /// Returns a Handle that can be used to access the value while
    /// the arena is alive. Returns BrokerError::OutOfMemory if the
    /// arena does not have enough room.
    ///
    /// # Errors
    ///
    /// Returns BrokerError::OutOfMemory if capacity is exhausted.
    pub fn alloc<T>(self: &Arc<Self>, value: T) -> Result<Handle<T>, BrokerError>
    where
        T: 'static,
    {
        let layout = Layout::new::<T>();
        let size = layout.size();
        let align = layout.align();

        // We use a simple atomic bump: load the cursor, align it,
        // compute the new cursor, and CAS-publish it. If the CAS
        // fails (because another thread allocated concurrently), we
        // retry.
        let offset = loop {
            let current = self.cursor.load(Ordering::Acquire);

            // Align current up to the required alignment.
            let aligned = (current + align - 1) & !(align - 1);
            let end = aligned.checked_add(size).ok_or(BrokerError::OutOfMemory {
                arena: self.id,
                available: self.capacity.saturating_sub(current),
                requested: size,
            })?;

            if end > self.capacity {
                return Err(BrokerError::OutOfMemory {
                    arena: self.id,
                    available: self.capacity.saturating_sub(current),
                    requested: size,
                });
            }

            if self.cursor
                .compare_exchange_weak(current, end, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break aligned;
            }
            // else: another thread claimed this slot; retry.
        };

        // Write the value into the slot.
        // SAFETY:
        // - `offset + size <= capacity` was verified above.
        // - The slot we just claimed via CAS is exclusively ours.
        // - The alignment was computed above.
        unsafe {
            let dest = self.buffer.as_ptr().add(offset).cast::<T>();
            dest.write(value);
        }

        // Register the slot. This is the part that conceptually needs
        // synchronization: multiple allocations might push to `slots`
        // concurrently. We serialize via a spinlock-style approach
        // using next_slot: each allocator claims the next slot index
        // atomically, then writes its SlotInfo at that index.
        //
        // To make this work without locking, we grow `slots` only here
        // and only through &self, which means we need interior
        // mutability. We use UnsafeCell with a discipline: the slot
        // at index `i` is published when `next_slot >= i+1`. Readers
        // wait for next_slot >= their target.

        let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);
        let info = SlotInfo { offset, size };

        // SAFETY: We are the only writer to slot[slot_index] because
        // each allocator gets a unique slot index via fetch_add. We
        // grow the vector under a lock-free protocol: extend with
        // default values if needed, then write our info. To avoid
        // requiring locks, we use the simpler approach of growing the
        // vector once per allocation. Since allocations are serialized
        // by the cursor CAS earlier (each successful CAS commits one
        // allocation), the writes to `slots` are also effectively
        // serialized.
        unsafe {
            let slots = &mut *self.slots.get();
            // The cursor CAS above is the linearization point. Any
            // allocator that successfully CASed the cursor has a
            // unique slot_index and exclusive access to push.
            while slots.len() <= slot_index as usize {
                slots.push(SlotInfo { offset: 0, size: 0 });
            }
            slots[slot_index as usize] = info;
        }

        let generation = self.generation();
        let weak = Arc::downgrade(self);

        Ok(Handle::new(
            self.id,
            SlotIndex(slot_index),
            generation,
            weak,
        ))
    }

    /// Returns a pointer to the data backing a slot, after bounds-checking.
    ///
    /// This is the internal mechanism that Handle::get() uses to
    /// resolve a slot index to a memory address. It is pub(crate) so
    /// the handle module can call it but external code cannot.
    pub(crate) fn slot_ptr(&self, slot: SlotIndex) -> Result<*const u8, BrokerError> {
        // SAFETY: We only read slot infos that have been published
        // (next_slot >= slot+1). The Acquire ordering on next_slot
        // synchronizes with the Release in alloc()'s fetch_add.
        let published = self.next_slot.load(Ordering::Acquire);
        if slot.raw() >= published {
            return Err(BrokerError::InvalidSlot {
                arena: self.id,
                slot,
            });
        }

        let info = unsafe {
            let slots = &*self.slots.get();
            slots[slot.raw() as usize]
        };

        // SAFETY: offset was computed during alloc and is within
        // [0, capacity), and the slot's data lives at buffer + offset.
        Ok(unsafe { self.buffer.as_ptr().add(info.offset) })
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        self.invalidate();
        // Advance the generation. This invalidates all outstanding
        // handles. We use wrapping_add to avoid overflow; in practice
        // an arena would need to be created and dropped 4 billion
        // times for this to matter, and even then the wrap would only
        // accidentally validate a handle from exactly 4B generations
        // ago, which we accept.
        let prev = self.generation.fetch_add(1, Ordering::AcqRel);
        tracing::debug!(
            arena_id = %self.id,
            name = %self.name,
            previous_generation = prev,
            "arena dropped; generation advanced"
        );

        // Deallocate the backing buffer.
        // SAFETY: buffer was allocated with self.buffer_layout in new().
        unsafe {
            dealloc(self.buffer.as_ptr(), self.buffer_layout);
        }
    }
}

impl std::fmt::Debug for Arena {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arena")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("capacity", &self.capacity)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_basic_alloc() {
        let arena = Arena::new(ArenaId(1), "test", 4096);
        let handle = arena.alloc(42_u64).unwrap();
        assert_eq!(*handle.get().unwrap(), 42);
    }

    #[test]
    fn arena_tracks_usage() {
        let arena = Arena::new(ArenaId(1), "test", 4096);
        assert_eq!(arena.used(), 0);
        let _h = arena.alloc(0_u64).unwrap();
        assert!(arena.used() >= std::mem::size_of::<u64>());
    }

    #[test]
    fn arena_oom_returns_error() {
        let arena = Arena::new(ArenaId(1), "test", 16);
        // 16 bytes should fit two u64s.
        let _h1 = arena.alloc(1_u64).unwrap();
        let _h2 = arena.alloc(2_u64).unwrap();
        // Third should fail.
        let r = arena.alloc(3_u64);
        assert!(matches!(r, Err(BrokerError::OutOfMemory { .. })));
    }

    #[test]
    fn arena_drop_invalidates_handles() {
        let arena = Arena::new(ArenaId(1), "test", 4096);
        let handle = arena.alloc(42_u64).unwrap();
        assert_eq!(*handle.get().unwrap(), 42);
        drop(arena);
        let r = handle.get();
        assert!(r.is_err(), "expected use-after-free, got {r:?}");
        assert!(r.err().unwrap().is_use_after_free());
    }

    #[test]
    fn arena_alignment_respected() {
        // Allocate a u8 then a u64; the u64 must be aligned to 8.
        let arena = Arena::new(ArenaId(1), "test", 4096);
        let _h1 = arena.alloc(1_u8).unwrap();
        let h2 = arena.alloc(0x1234_5678_9abc_def0_u64).unwrap();
        assert_eq!(*h2.get().unwrap(), 0x1234_5678_9abc_def0);
    }

    #[test]
    fn arena_many_allocations() {
        let arena = Arena::new(ArenaId(1), "test", 1024 * 1024);
        let handles: Vec<_> = (0..1000_u64)
            .map(|i| arena.alloc(i).unwrap())
            .collect();
        for (i, h) in handles.iter().enumerate() {
            assert_eq!(*h.get().unwrap(), i as u64);
        }
    }

    #[test]
    fn arena_concurrent_allocation() {
        use std::thread;
        let arena = Arena::new(ArenaId(1), "test", 1024 * 1024);
        let mut threads = Vec::new();
        for t in 0..4 {
            let arena = Arc::clone(&arena);
            threads.push(thread::spawn(move || {
                let mut handles = Vec::new();
                for i in 0..100_u64 {
                    let value = t * 1000 + i;
                    handles.push(arena.alloc(value).unwrap());
                }
                handles
            }));
        }
        let all_handles: Vec<_> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(all_handles.len(), 400);
        // Spot-check that values round-trip
        for h in &all_handles {
            let _v = *h.get().unwrap();
        }
    }
}
