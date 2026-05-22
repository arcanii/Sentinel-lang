//! Fixed-size slab allocator strategy with recycling.
//!
//! Slots are uniform: each is `slot_size` bytes, aligned to `slot_align`.
//! Each slot has its own [`SlotGeneration`] counter; freeing a slot
//! advances its generation, so stale handles fail with
//! [`BrokerError::UseAfterFreeSlot`].

use crate::error::BrokerError;
use crate::ids::{ArenaId, SlotGeneration, SlotIndex};
use crate::strategy::{AllocOk, AllocStrategy, SlotPtr, StrategyKind};
use parking_lot::Mutex;
use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct SlabStrategy {
    arena_id: ArenaId,
    slot_size: usize,
    slot_align: usize,
    slot_count: u32,
    buffer: NonNull<u8>,
    buffer_layout: Layout,
    /// High-water mark of slots ever issued. Slots `[0, ever_issued)`
    /// have a known generation; later ones default to INITIAL.
    ever_issued: AtomicU32,
    /// Per-slot generation. Advances on each free(). Lazily grown up
    /// to `slot_count` entries.
    generations: Mutex<Vec<u32>>,
    /// Freelist of slots available for reuse.
    freelist: Mutex<Vec<u32>>,
}

unsafe impl Send for SlabStrategy {}
unsafe impl Sync for SlabStrategy {}

impl SlabStrategy {
    /// Construct a new slab with `slot_count` slots of `slot_size`
    /// bytes, aligned to `slot_align`.
    ///
    /// # Panics
    /// Panics if any of `slot_size`, `slot_align`, `slot_count` is
    /// zero; if `slot_align` is not a power of two; if the total
    /// size overflows `usize`; or if the system allocator refuses
    /// the request.
    #[must_use]
    pub fn new(arena_id: ArenaId, slot_size: usize, slot_align: usize, slot_count: u32) -> Self {
        assert!(slot_size > 0, "slab slot_size must be positive");
        assert!(slot_align > 0, "slab slot_align must be positive");
        assert!(slot_align.is_power_of_two(), "slab slot_align must be a power of two");
        assert!(slot_count > 0, "slab slot_count must be positive");

        let padded = (slot_size + slot_align - 1) & !(slot_align - 1);
        let total = padded.checked_mul(slot_count as usize)
            .expect("slab total size overflows usize");

        let layout = Layout::from_size_align(total, slot_align).expect("slab backing layout");
        let raw = unsafe { alloc(layout) };
        let buffer = NonNull::new(raw).expect("slab backing allocation failed");

        Self {
            arena_id,
            slot_size: padded,
            slot_align,
            slot_count,
            buffer,
            buffer_layout: layout,
            ever_issued: AtomicU32::new(0),
            generations: Mutex::new(Vec::new()),
            freelist: Mutex::new(Vec::new()),
        }
    }

    fn current_generation(&self, slot: u32) -> SlotGeneration {
        let gens = self.generations.lock();
        SlotGeneration(gens.get(slot as usize).copied().unwrap_or(0))
    }
}

impl std::fmt::Debug for SlabStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlabStrategy")
            .field("arena_id", &self.arena_id)
            .field("slot_size", &self.slot_size)
            .field("slot_align", &self.slot_align)
            .field("slot_count", &self.slot_count)
            .field("ever_issued", &self.ever_issued.load(Ordering::Acquire))
            .field("free_count", &self.freelist.lock().len())
            .finish_non_exhaustive()
    }
}

impl Drop for SlabStrategy {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.buffer_layout); }
    }
}

impl AllocStrategy for SlabStrategy {
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError> {
        if layout.size() > self.slot_size || layout.align() > self.slot_align {
            return Err(BrokerError::OutOfMemory {
                arena: self.arena_id,
                available: self.slot_size,
                requested: layout.size(),
            });
        }

        // Prefer reusing a freelist slot before extending the high-water mark.
        let claimed = {
            let mut fl = self.freelist.lock();
            fl.pop()
        };

        let slot_idx = if let Some(s) = claimed {
            s
        } else {
            let next = self.ever_issued.fetch_add(1, Ordering::AcqRel);
            if next >= self.slot_count {
                self.ever_issued.fetch_sub(1, Ordering::AcqRel);
                return Err(BrokerError::OutOfMemory {
                    arena: self.arena_id,
                    available: 0,
                    requested: layout.size(),
                });
            }
            next
        };

        // Make sure the generations vec has space for this slot. New
        // entries start at generation 0 (SlotGeneration::INITIAL).
        let generation = {
            let mut gens = self.generations.lock();
            while gens.len() <= slot_idx as usize {
                gens.push(0);
            }
            SlotGeneration(gens[slot_idx as usize])
        };

        let offset = (slot_idx as usize) * self.slot_size;
        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(AllocOk {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            slot: SlotIndex(slot_idx),
            generation,
        })
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Result<SlotPtr, BrokerError> {
        if slot.raw() >= self.ever_issued.load(Ordering::Acquire) {
            return Err(BrokerError::InvalidSlot {
                arena: self.arena_id,
                slot,
            });
        }
        let generation = self.current_generation(slot.raw());
        let offset = (slot.raw() as usize) * self.slot_size;
        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(SlotPtr {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            generation,
        })
    }

    fn free(&self, slot: SlotIndex, issued: SlotGeneration) -> Result<(), BrokerError> {
        if slot.raw() >= self.ever_issued.load(Ordering::Acquire) {
            return Err(BrokerError::InvalidSlot {
                arena: self.arena_id,
                slot,
            });
        }
        // Bump generation under the lock to guarantee atomicity vs
        // concurrent alloc_raw / slot_ptr reads.
        let mut gens = self.generations.lock();
        while gens.len() <= slot.raw() as usize {
            gens.push(0);
        }
        let current = SlotGeneration(gens[slot.raw() as usize]);
        if current != issued {
            // Stale free attempt — the slot has already been recycled.
            return Err(BrokerError::UseAfterFreeSlot {
                arena: self.arena_id,
                slot,
                issued,
                current,
            });
        }
        gens[slot.raw() as usize] = gens[slot.raw() as usize].wrapping_add(1);
        drop(gens);

        self.freelist.lock().push(slot.raw());
        Ok(())
    }

    fn used(&self) -> usize {
        let issued = self.ever_issued.load(Ordering::Acquire) as usize;
        let free = self.freelist.lock().len();
        (issued.saturating_sub(free)) * self.slot_size
    }

    fn capacity(&self) -> usize {
        (self.slot_count as usize) * self.slot_size
    }

    fn kind(&self) -> StrategyKind { StrategyKind::Slab }
}