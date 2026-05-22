#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

write_file() {
    local path="$1"; local content="$2"
    mkdir -p "$(dirname "$path")"
    if [[ -f "$path" ]] && [[ "$(cat "$path")" == "$content" ]]; then
        printf "[SKIP] %s (unchanged)\n" "${path#$SENTINEL_ROOT/}"
        return
    fi
    printf "%s" "$content" > "$path"
    printf "[OK]   wrote %s\n" "${path#$SENTINEL_ROOT/}"
}

# ---------------------------------------------------------------------------
# 1. ids.rs — add SlotGeneration alongside Generation.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/ids.rs"
src = p.read_text()

if "pub struct SlotGeneration" in src:
    print("[SKIP] ids.rs: SlotGeneration already present")
    sys.exit(0)

# Insert SlotGeneration right after Generation's impl block. We find the
# end of `impl Generation { ... }` by brace depth.
m = re.search(r'impl Generation\s*\{', src)
if not m:
    print("[ERR]  ids.rs: cannot find `impl Generation {`")
    sys.exit(1)
depth = 1; i = m.end()
while i < len(src) and depth > 0:
    if src[i] == '{': depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            close = i + 1; break
    i += 1

addition = '''

/// Per-slot generation. Each strategy slot has its own counter that
/// advances on free, so a recycled slot rejects stale handles.
///
/// Distinct from [`Generation`], which is per-arena and advances
/// only when the arena itself is destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotGeneration(pub(crate) u32);

impl SlotGeneration {
    /// The starting generation of a fresh slot.
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub const fn raw(self) -> u32 { self.0 }
}

impl std::fmt::Display for SlotGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sgen{}", self.0)
    }
}
'''
new = src[:close] + addition + src[close:]

# Add a tiny test for Display.
test_addition = '''
    #[test]
    fn slot_generation_display() {
        assert_eq!(format!("{}", SlotGeneration(3)), "sgen3");
        assert_eq!(SlotGeneration::INITIAL.raw(), 0);
    }
'''
# Insert before the closing `}` of `mod tests`.
test_m = re.search(r'(#\[cfg\(test\)\]\s*mod tests\s*\{)', new)
if test_m:
    # Find matching closing brace by depth.
    d = 1; j = test_m.end()
    while j < len(new) and d > 0:
        if new[j] == '{': d += 1
        elif new[j] == '}':
            d -= 1
            if d == 0:
                tclose = j; break
        j += 1
    new = new[:tclose] + test_addition + new[tclose:]

p.write_text(new)
print("[OK]   ids.rs: added SlotGeneration + Display + test")
PYEOF

# ---------------------------------------------------------------------------
# 2. error.rs — split UseAfterFree into two variants for clarity.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/error.rs"
src = p.read_text()

if "UseAfterFreeSlot" in src:
    print("[SKIP] error.rs: UseAfterFreeSlot already present")
    sys.exit(0)

# Add a new variant UseAfterFreeSlot for slot-level (recycled) UAF.
# We keep the existing UseAfterFree variant for arena-level destruction.
m = re.search(r'pub enum BrokerError\s*\{', src)
if not m:
    print("[ERR] cannot find BrokerError enum"); sys.exit(1)

# Find brace-balanced end of the enum.
d = 1; i = m.end()
while i < len(src) and d > 0:
    if src[i] == '{': d += 1
    elif src[i] == '}':
        d -= 1
        if d == 0:
            close = i; break
    i += 1

body = src[m.end():close].rstrip()
if body and not body.endswith(','):
    body += ',\n'
else:
    body += '\n'

variant = '''
    /// A handle was used after its slot was freed and possibly recycled.
    ///
    /// Distinct from [`Self::UseAfterFree`] (which fires when the entire
    /// arena is destroyed). This variant covers slot-level recycling:
    /// the arena is still alive, but the specific slot now belongs to
    /// a newer allocation.
    #[error("slot use-after-free: {arena} slot {slot} was issued at {issued} but slot is now at {current}")]
    UseAfterFreeSlot {
        arena: crate::ids::ArenaId,
        slot: crate::ids::SlotIndex,
        issued: crate::ids::SlotGeneration,
        current: crate::ids::SlotGeneration,
    },
'''
new = src[:m.end()] + body + variant + src[close:]

# Extend is_use_after_free() to cover the new variant.
new = re.sub(
    r'matches!\(self, Self::UseAfterFree \{ \.\. \}\)',
    'matches!(self, Self::UseAfterFree { .. } | Self::UseAfterFreeSlot { .. })',
    new,
    count=1,
)

p.write_text(new)
print("[OK]   error.rs: added UseAfterFreeSlot, extended is_use_after_free")
PYEOF

# ---------------------------------------------------------------------------
# 3. strategy/mod.rs — extend trait with slot generations.
# ---------------------------------------------------------------------------
read -r -d '' STRATEGY_MOD <<'RUST_EOF' || true
//! Allocation strategies plugged into an [`Arena`].
//!
//! Each strategy owns its backing memory and decides how to satisfy
//! requests. Arenas pair a strategy with identity (id, name, arena-level
//! generation); the strategy supplies per-slot generations for sound
//! recycling.
//!
//! Current strategies:
//! - [`bump::BumpStrategy`] — monotonic, no recycling.
//! - [`slab::SlabStrategy`] — fixed-size, supports recycling with
//!   per-slot generations.

use crate::error::BrokerError;
use crate::ids::{SlotGeneration, SlotIndex};
use std::alloc::Layout;
use std::ptr::NonNull;

pub mod bump;
pub mod slab;

/// What kind of strategy backs an arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrategyKind {
    Bump,
    Slab,
}

/// A successful allocation: pointer, slot index, and the slot's
/// current generation.
#[derive(Debug)]
pub struct AllocOk {
    pub ptr: NonNull<u8>,
    pub slot: SlotIndex,
    pub generation: SlotGeneration,
}

/// What [`AllocStrategy::slot_ptr`] returns: the pointer plus the
/// slot's current generation so the caller can verify it matches the
/// generation a handle was issued with.
#[derive(Debug)]
pub struct SlotPtr {
    pub ptr: NonNull<u8>,
    pub generation: SlotGeneration,
}

/// The trait every arena strategy implements.
pub trait AllocStrategy: Send + Sync {
    /// Allocate a region matching `layout`.
    ///
    /// # Errors
    /// - [`BrokerError::OutOfMemory`] when capacity is exhausted.
    /// - [`BrokerError::InvalidSlot`] if `layout` is unsupported.
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError>;

    /// Resolve a slot index to its pointer and current generation.
    ///
    /// Callers compare the returned generation against the one their
    /// handle stores and reject mismatches with [`BrokerError::UseAfterFreeSlot`].
    ///
    /// # Errors
    /// Returns [`BrokerError::InvalidSlot`] for unknown slots.
    fn slot_ptr(&self, slot: SlotIndex) -> Result<SlotPtr, BrokerError>;

    /// Free a slot. Advances the slot's generation so future
    /// access through stale handles fails cleanly. Strategies that
    /// don't support recycling return [`BrokerError::NotImplemented`].
    ///
    /// # Errors
    /// - [`BrokerError::NotImplemented`] if the strategy doesn't recycle.
    /// - [`BrokerError::InvalidSlot`] for unknown slots.
    /// - [`BrokerError::UseAfterFreeSlot`] if the slot was already freed
    ///   (double-free protection).
    fn free(&self, slot: SlotIndex, generation: SlotGeneration) -> Result<(), BrokerError> {
        let _ = (slot, generation);
        Err(BrokerError::NotImplemented {
            feature: "AllocStrategy::free (this strategy does not recycle)",
        })
    }

    fn used(&self) -> usize;
    fn capacity(&self) -> usize;
    fn available(&self) -> usize {
        self.capacity().saturating_sub(self.used())
    }
    fn kind(&self) -> StrategyKind;
}
RUST_EOF
write_file "$BROKER/src/strategy/mod.rs" "$STRATEGY_MOD"

# ---------------------------------------------------------------------------
# 4. strategy/bump.rs — return INITIAL slot generation, never recycle.
# ---------------------------------------------------------------------------
read -r -d '' BUMP_RS <<'RUST_EOF' || true
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
            generation: SlotGeneration::INITIAL,
        })
    }

    // Bump strategy intentionally inherits the default free() which
    // returns NotImplemented. Once a byte is allocated, it's owned
    // until the arena is destroyed.

    fn used(&self) -> usize { self.cursor.load(Ordering::Acquire) }
    fn capacity(&self) -> usize { self.capacity }
    fn kind(&self) -> StrategyKind { StrategyKind::Bump }
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
RUST_EOF
write_file "$BROKER/src/strategy/bump.rs" "$BUMP_RS"

# ---------------------------------------------------------------------------
# 5. strategy/slab.rs — real recycling with per-slot generations.
# ---------------------------------------------------------------------------
read -r -d '' SLAB_RS <<'RUST_EOF' || true
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
RUST_EOF
write_file "$BROKER/src/strategy/slab.rs" "$SLAB_RS"

# ---------------------------------------------------------------------------
# 6. handle.rs — add slot_generation; check both arena and slot gens.
# ---------------------------------------------------------------------------
read -r -d '' HANDLE_RS <<'RUST_EOF' || true
//! Generational handles into broker arenas.
//!
//! A `Handle<T>` carries two generations:
//!
//! - The **arena generation**: detects destruction of the entire arena.
//! - The **slot generation**: detects recycling of an individual slot.
//!
//! Access through a handle compares both. The arena generation is
//! checked first (it's cheap and definitive for destruction); the slot
//! generation is checked via [`crate::strategy::AllocStrategy::slot_ptr`].

use crate::arena::Arena;
use crate::error::BrokerError;
use crate::ids::{ArenaId, Generation, SlotGeneration, SlotIndex};
use std::marker::PhantomData;
use std::sync::Weak;

pub struct Handle<T> {
    pub(crate) arena_id: ArenaId,
    pub(crate) slot: SlotIndex,
    pub(crate) arena_generation: Generation,
    pub(crate) slot_generation: SlotGeneration,
    pub(crate) arena: Weak<Arena>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub(crate) fn new(
        arena_id: ArenaId,
        slot: SlotIndex,
        arena_generation: Generation,
        slot_generation: SlotGeneration,
        arena: Weak<Arena>,
    ) -> Self {
        Self {
            arena_id, slot, arena_generation, slot_generation, arena,
            _marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn arena_id(&self) -> ArenaId { self.arena_id }

    #[must_use]
    pub const fn slot(&self) -> SlotIndex { self.slot }

    #[must_use]
    pub const fn arena_generation(&self) -> Generation { self.arena_generation }

    #[must_use]
    pub const fn slot_generation(&self) -> SlotGeneration { self.slot_generation }

    #[must_use]
    pub fn is_live(&self) -> bool { self.arena.upgrade().is_some() }

    pub fn get(&self) -> Result<HandleRef<'_, T>, BrokerError> {
        let arena = self.arena.upgrade().ok_or(BrokerError::UseAfterFree {
            arena: self.arena_id,
            slot: self.slot,
            issued: self.arena_generation,
            current: Generation(self.arena_generation.raw().wrapping_add(1)),
        })?;

        let current_arena_gen = arena.generation();
        if current_arena_gen != self.arena_generation {
            return Err(BrokerError::UseAfterFree {
                arena: self.arena_id,
                slot: self.slot,
                issued: self.arena_generation,
                current: current_arena_gen,
            });
        }

        let slot_ptr = arena.strategy_slot_ptr(self.slot)?;
        if slot_ptr.generation != self.slot_generation {
            return Err(BrokerError::UseAfterFreeSlot {
                arena: self.arena_id,
                slot: self.slot,
                issued: self.slot_generation,
                current: slot_ptr.generation,
            });
        }

        Ok(HandleRef {
            ptr: slot_ptr.ptr.as_ptr().cast::<T>(),
            _arena: arena,
            _marker: PhantomData,
        })
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            arena_id: self.arena_id,
            slot: self.slot,
            arena_generation: self.arena_generation,
            slot_generation: self.slot_generation,
            arena: self.arena.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> std::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("arena_id", &self.arena_id)
            .field("slot", &self.slot)
            .field("arena_generation", &self.arena_generation)
            .field("slot_generation", &self.slot_generation)
            .field("type", &std::any::type_name::<T>())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct HandleRef<'a, T> {
    ptr: *const T,
    _arena: std::sync::Arc<Arena>,
    _marker: PhantomData<&'a T>,
}

impl<T> std::ops::Deref for HandleRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T { unsafe { &*self.ptr } }
}

unsafe impl<T: Sync> Send for HandleRef<'_, T> {}
unsafe impl<T: Sync> Sync for HandleRef<'_, T> {}
RUST_EOF
write_file "$BROKER/src/handle.rs" "$HANDLE_RS"

# ---------------------------------------------------------------------------
# 7. arena.rs — propagate slot generation through alloc; add free + tests.
# ---------------------------------------------------------------------------
read -r -d '' ARENA_RS <<'RUST_EOF' || true
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
RUST_EOF
write_file "$BROKER/src/arena.rs" "$ARENA_RS"

# ---------------------------------------------------------------------------
# 8. broker.rs — expose ArenaHandle::free alongside alloc.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/broker.rs"
src = p.read_text()

if "pub fn free<T>" in src:
    print("[SKIP] broker.rs: ArenaHandle::free already present")
    sys.exit(0)

# Insert free() right after the alloc<T> method on ArenaHandle.
pat = re.compile(
    r'(pub fn alloc<T>\(&self, value: T\) -> Result<crate::Handle<T>, BrokerError>\s*'
    r'where\s*T: \'static,\s*\{\s*self\.arena\.alloc\(value\)\s*\})',
)
m = pat.search(src)
if not m:
    print("[ERR]  could not locate ArenaHandle::alloc"); sys.exit(1)

addition = '''

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
    }'''
new = src[:m.end()] + addition + src[m.end():]
p.write_text(new)
print("[OK]   broker.rs: added ArenaHandle::free forwarder")
PYEOF

# ---------------------------------------------------------------------------
# 9. lib.rs — re-export SlotGeneration.
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/lib.rs"
src = p.read_text()

if "SlotGeneration" in src:
    print("[SKIP] lib.rs: SlotGeneration already exported")
    sys.exit(0)

new = src.replace(
    "pub use ids::{ArenaId, Generation, SlotIndex};",
    "pub use ids::{ArenaId, Generation, SlotGeneration, SlotIndex};",
    1,
)
if new == src:
    print("[ERR] could not find ids re-export"); sys.exit(1)
p.write_text(new)
print("[OK]   lib.rs: re-exported SlotGeneration")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -25

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -30

echo
echo "======"
echo "TESTS (nextest)"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -60

echo
echo "======"
echo "TESTS (cargo test)"
echo "======"
cargo test -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "04 COMPLETE"
echo "======"
echo "Expect: 38+ tests passing under nextest."
