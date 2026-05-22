//! Fixed-size slab allocator strategy.
//!
//! Allocates `slot_count` slots of `slot_size` bytes each, aligned to
//! `slot_align`. Linear allocation only — recycling via `free` returns
//! [`BrokerError::NotImplemented`] until milestone A3.5 introduces
//! per-slot generations.

use crate::error::BrokerError;
use crate::ids::{ArenaId, SlotIndex};
use crate::strategy::{AllocOk, AllocStrategy, StrategyKind};
use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

/// Fixed-size slab strategy.
///
/// Slots are uniform: each is `slot_size` bytes, aligned to `slot_align`.
/// Callers requesting a different size or stricter alignment receive
/// [`BrokerError::OutOfMemory`] with the per-slot capacity as the
/// available figure.
pub struct SlabStrategy {
    arena_id: ArenaId,
    slot_size: usize,
    slot_align: usize,
    slot_count: u32,
    buffer: NonNull<u8>,
    buffer_layout: Layout,
    /// Index of the next slot to allocate linearly.
    next_slot: AtomicU32,
}

// SAFETY: per-slot indices are atomic; once a slot is claimed it
// belongs exclusively to one caller.
unsafe impl Send for SlabStrategy {}
unsafe impl Sync for SlabStrategy {}

impl SlabStrategy {
    /// Construct a slab with `slot_count` slots of `slot_size` bytes each.
    ///
    /// # Panics
    /// Panics if `slot_size == 0`, `slot_align == 0`, `slot_align` is
    /// not a power of two, or `slot_count == 0`.
    #[must_use]
    pub fn new(
        arena_id: ArenaId,
        slot_size: usize,
        slot_align: usize,
        slot_count: u32,
    ) -> Self {
        assert!(slot_size > 0, "slab slot_size must be positive");
        assert!(slot_align > 0, "slab slot_align must be positive");
        assert!(slot_align.is_power_of_two(), "slab slot_align must be a power of two");
        assert!(slot_count > 0, "slab slot_count must be positive");

        // Pad slot_size up to slot_align so consecutive slots remain
        // aligned without per-allocation arithmetic.
        let padded = (slot_size + slot_align - 1) & !(slot_align - 1);
        let total = padded
            .checked_mul(slot_count as usize)
            .expect("slab total size overflows usize");

        let layout = Layout::from_size_align(total, slot_align)
            .expect("slab backing layout");
        // SAFETY: layout has nonzero size.
        let raw = unsafe { alloc(layout) };
        let buffer = NonNull::new(raw).expect("slab backing allocation failed");

        Self {
            arena_id,
            slot_size: padded,
            slot_align,
            slot_count,
            buffer,
            buffer_layout: layout,
            next_slot: AtomicU32::new(0),
        }
    }

    /// Bytes per slot (the padded size, not the original `slot_size`).
    #[must_use]
    pub fn slot_stride(&self) -> usize {
        self.slot_size
    }

    /// Configured slot alignment.
    #[must_use]
    pub fn slot_align(&self) -> usize {
        self.slot_align
    }

    /// Total number of slots configured.
    #[must_use]
    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }
}

impl std::fmt::Debug for SlabStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlabStrategy")
            .field("arena_id", &self.arena_id)
            .field("slot_size", &self.slot_size)
            .field("slot_align", &self.slot_align)
            .field("slot_count", &self.slot_count)
            .field("used_slots", &self.next_slot.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Drop for SlabStrategy {
    fn drop(&mut self) {
        // SAFETY: buffer/layout pair from `Self::new`, not deallocated.
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

        // Linear claim. fetch_add returns the prior value.
        let claimed = self.next_slot.fetch_add(1, Ordering::AcqRel);
        if claimed >= self.slot_count {
            // Roll back our increment so reported counters stay sane.
            // Note: this isn't strictly thread-safe under heavy
            // contention (another thread could observe an inflated
            // count momentarily), but used()/available() are advisory.
            self.next_slot.fetch_sub(1, Ordering::AcqRel);
            return Err(BrokerError::OutOfMemory {
                arena: self.arena_id,
                available: 0,
                requested: layout.size(),
            });
        }

        let offset = (claimed as usize) * self.slot_size;
        // SAFETY: offset + slot_size <= slot_count * slot_size = total
        // capacity (verified at construction).
        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(AllocOk {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            slot: SlotIndex(claimed),
        })
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Result<NonNull<u8>, BrokerError> {
        if slot.raw() >= self.next_slot.load(Ordering::Acquire) {
            return Err(BrokerError::InvalidSlot {
                arena: self.arena_id,
                slot,
            });
        }
        let offset = (slot.raw() as usize) * self.slot_size;
        // SAFETY: offset within total capacity (bounds checked above).
        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(NonNull::new(ptr).expect("buffer + offset is nonzero"))
    }

    fn used(&self) -> usize {
        (self.next_slot.load(Ordering::Acquire) as usize) * self.slot_size
    }

    fn capacity(&self) -> usize {
        (self.slot_count as usize) * self.slot_size
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::Slab
    }

    // free() inherits the default impl: returns NotImplemented.
}