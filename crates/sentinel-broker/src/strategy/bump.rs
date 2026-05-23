//! Monotonic bump allocator strategy.

use crate::error::BrokerError;
use crate::ids::{ArenaId, SlotGeneration, SlotIndex};
use crate::strategy::{AllocOk, AllocStrategy, SlotPtr, StrategyKind};
use parking_lot::Mutex;
use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy)]
struct SlotInfo {
    offset: usize,
    #[allow(dead_code)]
    size: usize,
}

pub struct BumpStrategy {
    arena_id: ArenaId,
    capacity: usize,
    buffer: NonNull<u8>,
    buffer_layout: Layout,
    cursor: AtomicUsize,
    next_slot: AtomicU32,
    slots: Mutex<Vec<SlotInfo>>,
}

unsafe impl Send for BumpStrategy {}
unsafe impl Sync for BumpStrategy {}

impl BumpStrategy {
    /// Construct a new bump strategy with `capacity` bytes.
    ///
    /// # Panics
    /// Panics if `capacity == 0`, the layout is unrepresentable,
    /// or the system allocator refuses the request.
    #[must_use]
    pub fn new(arena_id: ArenaId, capacity: usize) -> Self {
        assert!(capacity > 0, "bump capacity must be positive");
        let layout = Layout::from_size_align(capacity, 64).expect("bump capacity layout");
        let raw = unsafe { alloc(layout) };
        let buffer = NonNull::new(raw).expect("bump backing allocation failed");
        Self {
            arena_id, capacity, buffer, buffer_layout: layout,
            cursor: AtomicUsize::new(0),
            next_slot: AtomicU32::new(0),
            slots: Mutex::new(Vec::new()),
        }
    }
}

impl std::fmt::Debug for BumpStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BumpStrategy")
            .field("arena_id", &self.arena_id)
            .field("capacity", &self.capacity)
            .field("used", &self.used())
            .finish_non_exhaustive()
    }
}

impl Drop for BumpStrategy {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.buffer_layout); }
    }
}

impl AllocStrategy for BumpStrategy {
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError> {
        let size = layout.size();
        let align = layout.align();

        if size == 0 {
            let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);
            {
                let mut slots = self.slots.lock();
                while slots.len() <= slot_index as usize {
                    slots.push(SlotInfo { offset: 0, size: 0 });
                }
                slots[slot_index as usize] = SlotInfo { offset: 0, size: 0 };
            }
            return Ok(AllocOk {
                ptr: NonNull::new(align as *mut u8).expect("nonzero align"),
                size: 0,
                slot: SlotIndex(slot_index),
                generation: SlotGeneration::INITIAL,
            });
        }

        let offset = loop {
            let current = self.cursor.load(Ordering::Acquire);
            let aligned = (current + align - 1) & !(align - 1);
            let end = aligned.checked_add(size).ok_or(BrokerError::OutOfMemory {
                arena: self.arena_id,
                available: self.capacity.saturating_sub(current),
                requested: size,
            })?;
            if end > self.capacity {
                return Err(BrokerError::OutOfMemory {
                    arena: self.arena_id,
                    available: self.capacity.saturating_sub(current),
                    requested: size,
                });
            }
            if self.cursor.compare_exchange_weak(current, end, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                break aligned;
            }
        };

        let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);
        {
            let mut slots = self.slots.lock();
            while slots.len() <= slot_index as usize {
                slots.push(SlotInfo { offset: 0, size: 0 });
            }
            slots[slot_index as usize] = SlotInfo { offset, size };
        }

        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(AllocOk {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            size,
            slot: SlotIndex(slot_index),
            generation: SlotGeneration::INITIAL,
        })
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Result<SlotPtr, BrokerError> {
        let info = {
            let slots = self.slots.lock();
            slots.get(slot.raw() as usize).copied()
        };
        let info = info.ok_or(BrokerError::InvalidSlot { arena: self.arena_id, slot })?;
        let ptr = unsafe { self.buffer.as_ptr().add(info.offset) };
        Ok(SlotPtr {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            generation: SlotGeneration::INITIAL })
    }

    // Bump strategy intentionally inherits the default free() which
    // returns NotImplemented. Once a byte is allocated, it's owned
    // until the arena is destroyed.

    fn used(&self) -> usize { self.cursor.load(Ordering::Acquire) }
    fn capacity(&self) -> usize { self.capacity }
    fn kind(&self) -> StrategyKind { StrategyKind::Bump }

    fn backing_buffer(&self) -> Option<(*mut u8, usize)> {
        Some((self.buffer.as_ptr(), self.capacity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn bump_free_returns_not_implemented() {
        let s = BumpStrategy::new(ArenaId(1), 64);
        let ok = s.alloc_raw(Layout::new::<u64>()).unwrap();
        let err = s.free(ok.slot, ok.generation).unwrap_err();
        assert!(matches!(err, BrokerError::NotImplemented { .. }));
    }

    #[test]
    fn concurrent_allocation_stress() {
        const THREADS: usize = 16;
        const ALLOCS_PER_THREAD: usize = 500;
        const SLOT_BYTES: usize = 16;

        let strategy = Arc::new(BumpStrategy::new(
            ArenaId(1),
            THREADS * ALLOCS_PER_THREAD * SLOT_BYTES * 2,
        ));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let s = Arc::clone(&strategy);
            handles.push(thread::spawn(move || {
                let mut slots = Vec::with_capacity(ALLOCS_PER_THREAD);
                for _ in 0..ALLOCS_PER_THREAD {
                    let layout = Layout::from_size_align(SLOT_BYTES, 8).unwrap();
                    let ok = s.alloc_raw(layout).unwrap();
                    slots.push(ok.slot);
                }
                slots
            }));
        }
        let mut all_slots = Vec::new();
        for h in handles { all_slots.extend(h.join().unwrap()); }
        assert_eq!(all_slots.len(), THREADS * ALLOCS_PER_THREAD);
        let mut seen = std::collections::HashSet::new();
        for slot in &all_slots {
            assert!(seen.insert(slot.raw()), "slot {slot:?} issued twice");
            strategy.slot_ptr(*slot).unwrap();
        }
    }
}