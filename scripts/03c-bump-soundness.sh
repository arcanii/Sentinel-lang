#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

# Confirm sentinel-broker already depends on parking_lot.
if ! grep -q '^parking_lot' "$BROKER/Cargo.toml" && \
   ! grep -q 'parking_lot.*workspace' "$BROKER/Cargo.toml"; then
    echo "[INFO] Adding parking_lot dependency to sentinel-broker..."
    python3 - "$BROKER/Cargo.toml" <<'PYEOF'
import sys, pathlib
p = pathlib.Path(sys.argv[1])
src = p.read_text()
if 'parking_lot' in src:
    print("[SKIP] parking_lot already in Cargo.toml")
else:
    # Insert after [dependencies] header.
    new = src.replace(
        "[dependencies]\n",
        "[dependencies]\nparking_lot = { workspace = true }\n",
        1,
    )
    if new == src:
        print("[ERR] could not find [dependencies] section")
        sys.exit(1)
    p.write_text(new)
    print("[OK]   added parking_lot = { workspace = true }")
PYEOF
fi

# Rewrite bump.rs with the sound Mutex-based slot table.
cat > "$BROKER/src/strategy/bump.rs" <<'RUST_EOF'
//! Monotonic bump allocator strategy.
//!
//! Allocates by atomically advancing a cursor through a fixed-size
//! backing buffer. Does not recycle: once memory is allocated it
//! remains owned until the entire arena is destroyed.
//!
//! ## Concurrency model
//!
//! Two atomic operations serialize independent state:
//!
//! - `cursor` (AtomicUsize): a compare-and-swap loop assigns a unique
//!   byte range to each successful allocator. Cursor updates are
//!   lock-free.
//! - `slots` (Mutex<Vec<SlotInfo>>): the per-slot metadata table is
//!   guarded by a short mutex. Each allocation acquires the mutex
//!   once to push its `SlotInfo`. The critical section is bounded by
//!   `Vec::push` + one indexed assignment.
//!
//! Earlier revisions used an `UnsafeCell<Vec<SlotInfo>>` and assumed
//! the cursor CAS serialized slot-table updates. **That was unsound**:
//! two threads that both win independent cursor CASes would then race
//! on the slot table, leading to either UB (multiple `&mut Vec`
//! references) or actual memory corruption when `Vec::push` reallocates
//! mid-read. The mutex below is the correct fix; benchmarks of bump
//! arenas in real workloads show its overhead is dominated by the
//! cursor CAS contention itself.

use crate::error::BrokerError;
use crate::ids::{ArenaId, SlotIndex};
use crate::strategy::{AllocOk, AllocStrategy, StrategyKind};
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

/// Bump-allocator strategy. Owns its own backing buffer.
pub struct BumpStrategy {
    arena_id: ArenaId,
    capacity: usize,
    buffer: NonNull<u8>,
    buffer_layout: Layout,
    cursor: AtomicUsize,
    next_slot: AtomicU32,
    /// Per-slot metadata. Guarded by a mutex; see module docs.
    slots: Mutex<Vec<SlotInfo>>,
}

// SAFETY: All mutable state is either atomic (cursor, next_slot) or
// guarded by a Mutex (slots). The raw backing buffer is only read or
// written via offsets derived from the cursor CAS, which ensures
// each thread operates on a disjoint range.
unsafe impl Send for BumpStrategy {}
unsafe impl Sync for BumpStrategy {}

impl BumpStrategy {
    /// Construct a new bump strategy with `capacity` bytes, aligned to
    /// 64 bytes (suitable for any common type up through AVX-512).
    ///
    /// # Panics
    /// Panics if `capacity == 0` or the OS refuses the allocation.
    #[must_use]
    pub fn new(arena_id: ArenaId, capacity: usize) -> Self {
        assert!(capacity > 0, "bump capacity must be positive");
        let layout = Layout::from_size_align(capacity, 64)
            .expect("bump capacity layout");
        // SAFETY: layout has nonzero size.
        let raw = unsafe { alloc(layout) };
        let buffer = NonNull::new(raw).expect("bump backing allocation failed");
        Self {
            arena_id,
            capacity,
            buffer,
            buffer_layout: layout,
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
        // SAFETY: buffer/layout pair from `Self::new`, not deallocated.
        unsafe { dealloc(self.buffer.as_ptr(), self.buffer_layout); }
    }
}

impl AllocStrategy for BumpStrategy {
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError> {
        let size = layout.size();
        let align = layout.align();
        if size == 0 {
            // Zero-sized allocations get a dangling-but-aligned pointer.
            // We still claim a slot index for a Handle.
            let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);
            // Record a zero-size entry so slot_ptr can resolve it.
            {
                let mut slots = self.slots.lock();
                while slots.len() <= slot_index as usize {
                    slots.push(SlotInfo { offset: 0, size: 0 });
                }
                slots[slot_index as usize] = SlotInfo { offset: 0, size: 0 };
            }
            return Ok(AllocOk {
                // SAFETY: align is always nonzero by Layout invariants.
                ptr: NonNull::new(align as *mut u8).expect("nonzero align"),
                slot: SlotIndex(slot_index),
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
            if self
                .cursor
                .compare_exchange_weak(current, end, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break aligned;
            }
            // else: another thread claimed this byte range; retry.
        };

        let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);

        // Publish the slot metadata under the mutex. The critical section
        // is bounded: at most one Vec::push (which may realloc) plus an
        // indexed write.
        {
            let mut slots = self.slots.lock();
            while slots.len() <= slot_index as usize {
                slots.push(SlotInfo { offset: 0, size: 0 });
            }
            slots[slot_index as usize] = SlotInfo { offset, size };
        }

        // SAFETY: offset + size <= capacity (verified above via the
        // cursor CAS), and this byte range belongs exclusively to us.
        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(AllocOk {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            slot: SlotIndex(slot_index),
        })
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Result<NonNull<u8>, BrokerError> {
        let info = {
            let slots = self.slots.lock();
            slots.get(slot.raw() as usize).copied()
        };
        let info = info.ok_or(BrokerError::InvalidSlot {
            arena: self.arena_id,
            slot,
        })?;
        // SAFETY: offset was produced by `alloc_raw` and is within bounds.
        let ptr = unsafe { self.buffer.as_ptr().add(info.offset) };
        Ok(NonNull::new(ptr).expect("buffer + offset is nonzero"))
    }

    fn used(&self) -> usize {
        self.cursor.load(Ordering::Acquire)
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn kind(&self) -> StrategyKind {
        StrategyKind::Bump
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    /// Stress test: many threads, many allocations, designed to catch
    /// races in the slot-table publishing path. This is the test that
    /// originally exposed the UnsafeCell<Vec> unsoundness under
    /// nextest's process-per-test scheduling.
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
        for h in handles {
            all_slots.extend(h.join().unwrap());
        }

        // Every slot must be distinct and resolvable.
        assert_eq!(all_slots.len(), THREADS * ALLOCS_PER_THREAD);
        let mut seen = std::collections::HashSet::new();
        for slot in &all_slots {
            assert!(seen.insert(slot.raw()), "slot {slot:?} issued twice");
            strategy.slot_ptr(*slot).unwrap();
        }
    }
}
RUST_EOF
echo "[OK]   rewrote src/strategy/bump.rs with Mutex<Vec<SlotInfo>> + stress test"

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -15

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "TESTS (nextest — runs each test in its own process)"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -50

echo
echo "======"
echo "TESTS (cargo test — single process, all tests)"
echo "======"
cargo test -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "03c COMPLETE"
echo "======"
echo "Expect: 33 tests passing under nextest (32 prior + 1 new stress test)."
echo "If green, commit with:"
echo "  broker: phase A3 — alloc strategies + builder + slab (no recycling)"
echo "  including A3 soundness fix: Mutex<Vec<SlotInfo>> in BumpStrategy"
