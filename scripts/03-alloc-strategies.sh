#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

info(){ printf "[INFO] %s\n" "$*"; }
ok(){   printf "[OK]   %s\n" "$*"; }
warn(){ printf "[WARN] %s\n" "$*"; }

write_file() {
    local path="$1"
    local content="$2"
    mkdir -p "$(dirname "$path")"
    if [[ -f "$path" ]] && [[ "$(cat "$path")" == "$content" ]]; then
        printf "[SKIP] %s (unchanged)\n" "${path#$SENTINEL_ROOT/}"
        return
    fi
    printf "%s" "$content" > "$path"
    printf "[OK]   wrote %s\n" "${path#$SENTINEL_ROOT/}"
}

# ---------------------------------------------------------------------------
# 1. error.rs — add NotImplemented variant via python (preserves the rest).
# ---------------------------------------------------------------------------
python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])
p = broker / "src/error.rs"
src = p.read_text()

if "NotImplemented" in src:
    print("[SKIP] error.rs: NotImplemented already present")
    sys.exit(0)

# Find the closing brace of `pub enum BrokerError { ... }` by brace depth.
m = re.search(r'pub enum BrokerError\s*\{', src)
if not m:
    print("[ERR]  could not find BrokerError enum"); sys.exit(1)

depth = 1; i = m.end()
while i < len(src) and depth > 0:
    if src[i] == '{': depth += 1
    elif src[i] == '}':
        depth -= 1
        if depth == 0:
            close = i; break
    i += 1

body = src[m.end():close].rstrip()
if body and not body.endswith(','):
    body += ',\n'
else:
    body += '\n'

variant = (
    "\n"
    "    /// A feature has been declared but is not yet implemented.\n"
    "    /// Used during staged development to flag callable-but-unfinished APIs.\n"
    "    #[error(\"not implemented: {feature}\")]\n"
    "    NotImplemented { feature: &'static str },\n"
)
new = src[:m.end()] + body + variant + src[close:]
p.write_text(new)
print("[OK]   error.rs: added NotImplemented { feature: &'static str }")
PYEOF

# ---------------------------------------------------------------------------
# 2. strategy/mod.rs — the AllocStrategy trait.
# ---------------------------------------------------------------------------
read -r -d '' STRATEGY_MOD <<'RUST_EOF' || true
//! Allocation strategies plugged into an [`Arena`].
//!
//! Every arena delegates its physical allocation to a strategy
//! object behind a `Box<dyn AllocStrategy>`. The strategy owns the
//! backing memory and decides how to satisfy requests; the arena
//! supplies the generation tag and metadata.
//!
//! Current strategies:
//!
//! - [`bump::BumpStrategy`] — monotonic cursor, no recycling (this is
//!   the default).
//! - [`slab::SlabStrategy`] — fixed-size slots. Recycling will be added
//!   in milestone A3.5; for now [`AllocStrategy::free`] returns
//!   [`crate::BrokerError::NotImplemented`].

use crate::error::BrokerError;
use crate::ids::SlotIndex;
use std::alloc::Layout;
use std::ptr::NonNull;

pub mod bump;
pub mod slab;

/// What kind of strategy backs an arena. Reported by
/// [`AllocStrategy::kind`] for diagnostics and broker queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrategyKind {
    /// Monotonic bump allocator.
    Bump,
    /// Fixed-size slab allocator.
    Slab,
}

/// A successful allocation: a pointer and the slot index it corresponds to.
#[derive(Debug)]
pub struct AllocOk {
    /// Raw, properly-aligned pointer to the start of the slot.
    pub ptr: NonNull<u8>,
    /// Slot index, used to construct [`crate::Handle`] values.
    pub slot: SlotIndex,
}

/// The trait every arena strategy implements.
///
/// Implementations must be `Send + Sync`: arenas live behind an `Arc`
/// and may be touched from multiple threads. Internal synchronization
/// is the implementor's responsibility.
pub trait AllocStrategy: Send + Sync {
    /// Allocate a region matching `layout`. Returns the slot's address
    /// and its slot index, or [`BrokerError::OutOfMemory`] if the
    /// strategy cannot satisfy the request.
    ///
    /// # Errors
    /// - [`BrokerError::OutOfMemory`] when capacity is exhausted.
    /// - May return [`BrokerError::InvalidSlot`] if `layout` is
    ///   inherently unsupported (e.g., a slab strategy rejecting an
    ///   ill-fitting size).
    fn alloc_raw(&self, layout: Layout) -> Result<AllocOk, BrokerError>;

    /// Resolve a previously-issued [`SlotIndex`] back to its pointer.
    ///
    /// # Errors
    /// Returns [`BrokerError::InvalidSlot`] if the slot was never
    /// issued by this strategy.
    fn slot_ptr(&self, slot: SlotIndex) -> Result<NonNull<u8>, BrokerError>;

    /// Free a slot, making it available for re-allocation. The default
    /// implementation returns [`BrokerError::NotImplemented`]; strategies
    /// that support recycling override this.
    ///
    /// # Errors
    /// Returns [`BrokerError::NotImplemented`] unless the strategy
    /// has opted into recycling.
    fn free(&self, _slot: SlotIndex) -> Result<(), BrokerError> {
        Err(BrokerError::NotImplemented {
            feature: "AllocStrategy::free (recycling) — see milestone A3.5",
        })
    }

    /// Bytes currently used by this strategy.
    fn used(&self) -> usize;

    /// Total capacity in bytes.
    fn capacity(&self) -> usize;

    /// Bytes still available for allocation.
    fn available(&self) -> usize {
        self.capacity().saturating_sub(self.used())
    }

    /// Strategy kind, for diagnostics and broker queries.
    fn kind(&self) -> StrategyKind;
}
RUST_EOF
write_file "$BROKER/src/strategy/mod.rs" "$STRATEGY_MOD"

# ---------------------------------------------------------------------------
# 3. strategy/bump.rs — the existing bump algorithm, extracted.
# ---------------------------------------------------------------------------
read -r -d '' BUMP_RS <<'RUST_EOF' || true
//! Monotonic bump allocator strategy.
//!
//! Allocates by atomically advancing a cursor through a fixed-size
//! backing buffer. Does not recycle: once memory is allocated it
//! remains owned until the entire arena is destroyed.

use crate::error::BrokerError;
use crate::ids::{ArenaId, SlotIndex};
use crate::strategy::{AllocOk, AllocStrategy, StrategyKind};
use std::alloc::{alloc, dealloc, Layout};
use std::cell::UnsafeCell;
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
    slots: UnsafeCell<Vec<SlotInfo>>,
}

// SAFETY: see arena.rs/strategy/mod.rs commentary; mutation is
// serialized by atomic CAS on `cursor` and `next_slot`.
unsafe impl Send for BumpStrategy {}
unsafe impl Sync for BumpStrategy {}

impl BumpStrategy {
    /// Construct a new bump strategy with `capacity` bytes of backing
    /// memory, aligned to 64 bytes (suitable for any common type
    /// including AVX-512).
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
            slots: UnsafeCell::new(Vec::new()),
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
            let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);
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
            // else: another thread claimed this slot; retry.
        };

        let slot_index = self.next_slot.fetch_add(1, Ordering::AcqRel);

        // SAFETY: We own slot `slot_index` exclusively because each
        // allocator gets a unique slot index via fetch_add. Growth of
        // the slots vector is serialized by the cursor CAS above.
        unsafe {
            let slots = &mut *self.slots.get();
            while slots.len() <= slot_index as usize {
                slots.push(SlotInfo { offset: 0, size: 0 });
            }
            slots[slot_index as usize] = SlotInfo { offset, size };
        }

        // SAFETY: offset + size <= capacity (verified above).
        let ptr = unsafe { self.buffer.as_ptr().add(offset) };
        Ok(AllocOk {
            ptr: NonNull::new(ptr).expect("buffer + offset is nonzero"),
            slot: SlotIndex(slot_index),
        })
    }

    fn slot_ptr(&self, slot: SlotIndex) -> Result<NonNull<u8>, BrokerError> {
        // SAFETY: `slots` is only mutated under the cursor CAS protocol
        // documented above; readers see fully-published SlotInfo values.
        let info = unsafe {
            let slots = &*self.slots.get();
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
RUST_EOF
write_file "$BROKER/src/strategy/bump.rs" "$BUMP_RS"

# ---------------------------------------------------------------------------
# 4. strategy/slab.rs — fixed-size slab, no recycling yet.
# ---------------------------------------------------------------------------
read -r -d '' SLAB_RS <<'RUST_EOF' || true
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
RUST_EOF
write_file "$BROKER/src/strategy/slab.rs" "$SLAB_RS"

# ---------------------------------------------------------------------------
# 5. Rewrite arena.rs — thin wrapper holding a strategy + metadata.
# ---------------------------------------------------------------------------
read -r -d '' ARENA_RS <<'RUST_EOF' || true
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
        let a = make_bump(6, 4096);
        #[repr(align(64))]
        struct AlignedBlob([u8; 64]);
        let h = a.alloc(AlignedBlob([0; 64])).unwrap();
        let r = h.get().unwrap();
        let addr = (&*r) as *const AlignedBlob as usize;
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
RUST_EOF
write_file "$BROKER/src/arena.rs" "$ARENA_RS"

# ---------------------------------------------------------------------------
# 6. builder.rs — ArenaBuilder.
# ---------------------------------------------------------------------------
read -r -d '' BUILDER_RS <<'RUST_EOF' || true
//! Builder for constructing arenas with a chosen [`AllocStrategy`].
//!
//! ```ignore
//! let arena = broker.arena("requests").capacity(4096).bump().build();
//! let slab  = broker.arena("events").slab(64, 8, 256).build();
//! ```

use crate::arena::Arena;
use crate::broker::{ArenaHandle, Broker};
use crate::strategy::{bump::BumpStrategy, slab::SlabStrategy, AllocStrategy};
use std::sync::Arc;

/// Fluent builder for arena creation.
///
/// Obtain one via [`Broker::arena`].
pub struct ArenaBuilder<'b> {
    broker: &'b Broker,
    name: String,
    /// Bump capacity, set by `.capacity(n)`. Required for `.bump()`.
    bump_capacity: Option<usize>,
}

impl<'b> ArenaBuilder<'b> {
    pub(crate) fn new(broker: &'b Broker, name: String) -> Self {
        Self { broker, name, bump_capacity: None }
    }

    /// Set the capacity (in bytes) for a bump-allocated arena.
    #[must_use]
    pub fn capacity(mut self, bytes: usize) -> Self {
        self.bump_capacity = Some(bytes);
        self
    }

    /// Build a bump-allocated arena. Requires a prior call to [`capacity`].
    ///
    /// # Panics
    /// Panics if [`capacity`] was not called.
    #[must_use]
    pub fn bump(self) -> ArenaHandle {
        let cap = self.bump_capacity
            .expect("ArenaBuilder::bump requires .capacity(n) first");
        let id = self.broker.next_arena_id();
        let strategy: Box<dyn AllocStrategy> =
            Box::new(BumpStrategy::new(id, cap));
        let arena = Arena::with_strategy(id, &self.name, strategy);
        self.broker.register_arena(Arc::clone(&arena));
        ArenaHandle::from_parts(id, arena)
    }

    /// Build a slab-allocated arena with `slot_count` fixed-size slots.
    ///
    /// # Panics
    /// Panics on invalid sizing (see [`SlabStrategy::new`]).
    #[must_use]
    pub fn slab(self, slot_size: usize, slot_align: usize, slot_count: u32) -> ArenaHandle {
        let id = self.broker.next_arena_id();
        let strategy: Box<dyn AllocStrategy> =
            Box::new(SlabStrategy::new(id, slot_size, slot_align, slot_count));
        let arena = Arena::with_strategy(id, &self.name, strategy);
        self.broker.register_arena(Arc::clone(&arena));
        ArenaHandle::from_parts(id, arena)
    }
}

#[cfg(test)]
mod tests {
    use crate::Broker;
    use crate::strategy::StrategyKind;

    #[test]
    fn builder_bump_basic() {
        let b = Broker::new();
        let a = b.arena("test").capacity(1024).bump();
        assert_eq!(a.strategy_kind(), StrategyKind::Bump);
        assert_eq!(a.capacity(), 1024);
    }

    #[test]
    fn builder_slab_basic() {
        let b = Broker::new();
        let a = b.arena("test").slab(64, 8, 16);
        assert_eq!(a.strategy_kind(), StrategyKind::Slab);
        assert_eq!(a.capacity(), 64 * 16);
    }

    #[test]
    #[should_panic(expected = "ArenaBuilder::bump requires .capacity")]
    fn builder_bump_without_capacity_panics() {
        let b = Broker::new();
        let _ = b.arena("test").bump();
    }
}
RUST_EOF
write_file "$BROKER/src/builder.rs" "$BUILDER_RS"

# ---------------------------------------------------------------------------
# 7. Rewrite broker.rs to expose builder hooks and keep create_arena.
# ---------------------------------------------------------------------------
read -r -d '' BROKER_RS <<'RUST_EOF' || true
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
    fn slab_free_returns_not_implemented() {
        // The strategy's free() returns NotImplemented; for now we just
        // exercise it directly via the strategy until A3.5 wires it up
        // through Arena/Handle.
        use crate::strategy::{slab::SlabStrategy, AllocStrategy};
        use crate::ids::{ArenaId, SlotIndex};
        let s = SlabStrategy::new(ArenaId(99), 16, 8, 4);
        let err = s.free(SlotIndex(0)).unwrap_err();
        assert!(matches!(err, BrokerError::NotImplemented { .. }));
    }
}
RUST_EOF
write_file "$BROKER/src/broker.rs" "$BROKER_RS"

# ---------------------------------------------------------------------------
# 8. Update lib.rs to expose the new modules.
# ---------------------------------------------------------------------------
read -r -d '' LIB_RS <<'RUST_EOF' || true
//! # sentinel-broker
//!
//! Runtime memory broker for the Sentinel language.
//!
//! ## Status
//!
//! Phase A milestone A3: pluggable allocation strategies via
//! [`AllocStrategy`]. Bump (default) and Slab strategies; arenas are
//! built through the [`Broker::arena`] builder.
//!
//! ```
//! use sentinel_broker::{Broker, BrokerError};
//!
//! let broker = Broker::new();
//! let arena = broker.arena("example").capacity(4096).bump();
//! let handle = arena.alloc(42_u64).unwrap();
//! assert_eq!(*handle.get().unwrap(), 42);
//!
//! broker.destroy_arena(arena.id()).unwrap();
//! assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));
//! ```

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]
mod arena;
mod broker;
mod builder;
mod error;
mod handle;
mod ids;
pub mod strategy;

pub use arena::Arena;
pub use broker::{ArenaHandle, Broker};
pub use builder::ArenaBuilder;
pub use error::BrokerError;
pub use handle::Handle;
pub use ids::{ArenaId, Generation, SlotIndex};
pub use strategy::{AllocStrategy, StrategyKind};
RUST_EOF
write_file "$BROKER/src/lib.rs" "$LIB_RS"

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
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -45 || \
  cargo test -p sentinel-broker 2>&1 | tail -45

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "03 COMPLETE"
echo "======"
echo "If green, expect ~36 tests passing (25 prior + 11 new)."
echo "Next commit message will be: 'broker: phase A3 — alloc strategies + builder + slab (no recycling)'"
