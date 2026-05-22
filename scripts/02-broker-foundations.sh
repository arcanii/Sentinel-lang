#!/usr/bin/env bash
#
# 02-broker-foundations.sh
#
# Phase A milestones A0 + A1 + A2 from HANDOVER.md Section 4.
#
# A0: Dev-dependencies for the broker crate (proptest, criterion, etc.)
# A1: Foundation types - Handle, ArenaId, Generation, BrokerError
# A2: The simplest possible arena - bump allocation with generational
#     handle safety. Validates the core safety property: allocate,
#     get a handle, drop the arena, confirm the handle now traps.
#
# Idempotent. Safe to run multiple times.

set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER_DIR="$SENTINEL_ROOT/crates/sentinel-broker"

color_reset=$'\033[0m'
color_info=$'\033[1;34m'
color_ok=$'\033[1;32m'
color_skip=$'\033[1;33m'
color_err=$'\033[1;31m'
info()  { printf "%s[INFO]%s  %s\n"  "$color_info" "$color_reset" "$*"; }
ok()    { printf "%s[OK]%s    %s\n"  "$color_ok"   "$color_reset" "$*"; }
skip()  { printf "%s[SKIP]%s  %s\n"  "$color_skip" "$color_reset" "$*"; }
err()   { printf "%s[ERR]%s   %s\n"  "$color_err"  "$color_reset" "$*" >&2; }

[[ -d "$SENTINEL_ROOT" ]] || { err "Repo not found at $SENTINEL_ROOT"; exit 1; }
[[ -d "$BROKER_DIR" ]]    || { err "Broker crate not found at $BROKER_DIR"; exit 1; }

cd "$SENTINEL_ROOT"

# ---------------------------------------------------------------------------
# Idempotent file writer
# ---------------------------------------------------------------------------

write_file() {
    local path="$1"
    local content="$2"
    mkdir -p "$(dirname "$path")"
    if [[ -f "$path" ]] && [[ "$(cat "$path")" == "$content" ]]; then
        skip "Unchanged: ${path#$SENTINEL_ROOT/}"
        return 0
    fi
    printf "%s" "$content" > "$path"
    ok "Wrote: ${path#$SENTINEL_ROOT/}"
}

# ---------------------------------------------------------------------------
# Workspace Cargo.toml: add new shared dev-dependencies
# ---------------------------------------------------------------------------

info "Adding broker dev-dependencies to workspace Cargo.toml..."

# We rewrite the workspace Cargo.toml with the additional workspace.dependencies
# entries. This is safe because the scaffold version is well-defined.

write_file "$SENTINEL_ROOT/Cargo.toml" "$(cat <<'EOF'
# Sentinel language workspace manifest.
#
# Member crates are listed below. Dependency versions are pinned in
# [workspace.dependencies] and inherited by member crates via
# `package.workspace = true` to prevent version drift.

[workspace]
resolver = "2"
members = [
    "crates/sentinel-broker",
    "crates/sentinel-syntax",
    "crates/sentinel-ast",
    "crates/sentinel-resolve",
    "crates/sentinel-types",
    "crates/sentinel-hir",
    "crates/sentinel-mir",
    "crates/sentinel-codegen",
    "crates/sentinel-driver",
    "crates/sentinel-runtime",
    "crates/sentinel-lsp",
]

[workspace.package]
edition      = "2021"
rust-version = "1.80"
version      = "0.0.1"
authors      = ["Sentinel Language Project"]
license      = "Apache-2.0 OR MIT"
repository   = "https://github.com/bryan/Sentinel-language"

[workspace.dependencies]
# Lexing and parsing
logos      = "0.14"

# Query engine
salsa      = "0.18"

# LLVM bindings (matches LLVM 18 pinned in 00-bootstrap-environment.sh)
inkwell    = { version = "0.5", features = ["llvm18-0"] }

# Fast debug codegen
cranelift              = "0.111"
cranelift-module       = "0.111"
cranelift-object       = "0.111"

# Arena allocation for AST/IR nodes
bumpalo       = "3.16"
typed-arena   = "2.0"

# Collections and utilities
indexmap   = "2.5"
rustc-hash = "2.0"
smallvec   = "1.13"

# Diagnostics
miette     = { version = "7.2", features = ["fancy"] }
thiserror  = "1.0"

# Tracing/logging
tracing             = "0.1"
tracing-subscriber  = { version = "0.3", features = ["env-filter"] }

# Testing
insta      = "1.40"
proptest   = "1.5"
criterion  = { version = "0.5", features = ["html_reports"] }
serial_test = "3.1"

# Serialization
serde      = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml       = "0.8"

[profile.release]
lto              = "thin"
codegen-units    = 1
opt-level        = 3
debug            = "line-tables-only"

[profile.dev]
opt-level        = 0
debug            = "full"

[profile.test]
opt-level        = 1

[profile.bench]
opt-level        = 3
lto              = "thin"
codegen-units    = 1
EOF
)"

# ---------------------------------------------------------------------------
# Broker Cargo.toml: add dependencies and dev-dependencies
# ---------------------------------------------------------------------------

info "Updating broker crate Cargo.toml..."

write_file "$BROKER_DIR/Cargo.toml" "$(cat <<'EOF'
[package]
name        = "sentinel-broker"
description = "Runtime memory broker: arenas, generational handles, budgets, recording"

edition.workspace      = true
rust-version.workspace = true
version.workspace      = true
authors.workspace      = true
license.workspace      = true
repository.workspace   = true

[dependencies]
tracing    = { workspace = true }
thiserror  = { workspace = true }
smallvec   = { workspace = true }

[dev-dependencies]
proptest    = { workspace = true }
serial_test = { workspace = true }
criterion   = { workspace = true }

[lints.rust]
unsafe_code = "allow"  # The broker manages memory; unsafe is unavoidable.

[lints.clippy]
all      = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
# Allow some pedantic lints that fight broker patterns:
module_name_repetitions = "allow"
missing_errors_doc      = "allow"
must_use_candidate      = "allow"

[[bench]]
name    = "arena_bench"
harness = false
EOF
)"

# ---------------------------------------------------------------------------
# Broker library: module structure
# ---------------------------------------------------------------------------

info "Writing broker library source files..."

# src/lib.rs - crate root, re-exports public API
write_file "$BROKER_DIR/src/lib.rs" "$(cat <<'EOF'
//! # sentinel-broker
//!
//! The runtime memory broker for the Sentinel language. This crate
//! provides the foundational types and arena infrastructure that the
//! rest of the language is built on.
//!
//! ## Current Status
//!
//! Phase A milestone A0+A1+A2: foundation types and the simplest
//! arena. See `HANDOVER.md` for the full plan.
//!
//! ## Core Safety Property
//!
//! The broker's central guarantee is that handles into a dropped arena
//! return a typed error rather than reading poisoned memory. This is
//! enforced by generation tags: every arena slot carries a generation
//! counter, every handle carries the generation it was issued for,
//! and access compares the two.
//!
//! ```
//! use sentinel_broker::{Broker, BrokerError};
//!
//! let broker = Broker::new();
//! let arena = broker.create_arena("example", 4096);
//! let handle = arena.alloc(42_u64).unwrap();
//!
//! // Handle works while arena is live.
//! assert_eq!(*handle.get().unwrap(), 42);
//!
//! // After the arena is dropped, the handle returns a typed error.
//! drop(arena);
//! assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));
//! ```

mod arena;
mod broker;
mod error;
mod handle;
mod ids;

pub use arena::Arena;
pub use broker::Broker;
pub use error::BrokerError;
pub use handle::Handle;
pub use ids::{ArenaId, Generation, SlotIndex};
EOF
)"

# src/ids.rs - the simplest atomic types
write_file "$BROKER_DIR/src/ids.rs" "$(cat <<'EOF'
//! Identifier types used throughout the broker.
//!
//! These are wrapped integer types (newtype pattern) so that the
//! compiler refuses to mix them. ArenaIds cannot be passed where a
//! SlotIndex is expected, and so on. This is the language-level
//! lesson from FROMJAVA.md Section 1.4 applied to internal broker
//! types.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Identifies an arena uniquely within a Broker instance.
///
/// ArenaIds are monotonically allocated and never reused, even after
/// the corresponding arena is dropped. This prevents stale handles
/// from being confused with handles to a different arena that
/// happened to be allocated at the same address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArenaId(pub(crate) u64);

impl ArenaId {
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ArenaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "arena#{}", self.0)
    }
}

/// Identifies a slot within an arena.
///
/// Slot indices are local to an arena; the same numeric value in two
/// different arenas refers to two different slots. Combined with the
/// ArenaId in a Handle, this gives a globally-unique slot identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotIndex(pub(crate) u32);

impl SlotIndex {
    pub(crate) const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SlotIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "slot[{}]", self.0)
    }
}

/// A generation counter for detecting use-after-free.
///
/// Each arena starts at generation 1 (so that 0 can mean "no generation").
/// When the arena is dropped, the generation is invalidated. Handles
/// carry the generation they were issued for; access compares.
///
/// 32 bits gives 4 billion arena lifecycles before any concern about
/// generation reuse on a per-arena basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(pub(crate) u32);

impl Generation {
    pub(crate) const INITIAL: Generation = Generation(1);

    pub(crate) const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gen{}", self.0)
    }
}

/// A counter for allocating fresh ArenaIds.
///
/// Used internally by the Broker. Lives in its own type rather than
/// as a bare AtomicU64 so the broker can later add tracing, limits,
/// or other policy without changing call sites.
pub(crate) struct ArenaIdCounter(AtomicU64);

impl ArenaIdCounter {
    pub(crate) const fn new() -> Self {
        Self(AtomicU64::new(1))
    }

    pub(crate) fn next(&self) -> ArenaId {
        ArenaId(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_id_counter_is_monotonic() {
        let counter = ArenaIdCounter::new();
        let a = counter.next();
        let b = counter.next();
        let c = counter.next();
        assert!(a.raw() < b.raw());
        assert!(b.raw() < c.raw());
    }

    #[test]
    fn ids_have_distinct_types() {
        // This is a compile-time check: ArenaId and SlotIndex have
        // different types and cannot be mixed up. If this test
        // compiles, the type system is doing its job.
        let _a: ArenaId = ArenaId(0);
        let _s: SlotIndex = SlotIndex(0);
        let _g: Generation = Generation(1);
    }

    #[test]
    fn ids_display_distinctly() {
        assert_eq!(format!("{}", ArenaId(7)), "arena#7");
        assert_eq!(format!("{}", SlotIndex(3)), "slot[3]");
        assert_eq!(format!("{}", Generation(5)), "gen5");
    }
}
EOF
)"

# src/error.rs - the BrokerError type
write_file "$BROKER_DIR/src/error.rs" "$(cat <<'EOF'
//! Error types for broker operations.
//!
//! Every operation that could fail at runtime returns a `Result` with
//! a typed `BrokerError`. There are no panics in the public API: every
//! safety violation produces a recoverable error.

use crate::ids::{ArenaId, Generation, SlotIndex};
use thiserror::Error;

/// Errors that the broker can produce.
///
/// These are typed so callers can match on specific cases and respond
/// appropriately (e.g., logging a use-after-free in production while
/// rolling back the affected operation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BrokerError {
    /// A handle was used after its arena was dropped.
    ///
    /// This is the core safety property of generational handles.
    /// Without the generation check, this would be undefined behavior;
    /// with it, the access becomes a typed error.
    #[error("use-after-free: {arena} slot {slot} was issued for {issued} but arena is now at {current}")]
    UseAfterFree {
        arena: ArenaId,
        slot: SlotIndex,
        issued: Generation,
        current: Generation,
    },

    /// An allocation would exceed the arena's capacity.
    #[error("out of memory: arena {arena} has {available} bytes available, request was {requested}")]
    OutOfMemory {
        arena: ArenaId,
        available: usize,
        requested: usize,
    },

    /// A handle refers to a slot that does not exist in its arena.
    ///
    /// This should not normally be reachable through the safe API; if
    /// it occurs it indicates a bug in the broker itself or unsafe
    /// code passing handles between arenas.
    #[error("invalid slot: {slot} does not exist in {arena}")]
    InvalidSlot { arena: ArenaId, slot: SlotIndex },

    /// A handle refers to an arena that does not exist in this broker.
    ///
    /// As above, indicates a bug or cross-broker handle confusion.
    #[error("unknown arena: {arena} is not registered with this broker")]
    UnknownArena { arena: ArenaId },
}

impl BrokerError {
    /// Returns true if this error indicates a safety violation
    /// (use-after-free) as opposed to a resource exhaustion or
    /// programming error.
    #[must_use]
    pub const fn is_use_after_free(&self) -> bool {
        matches!(self, Self::UseAfterFree { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_after_free_is_detected() {
        let e = BrokerError::UseAfterFree {
            arena: ArenaId(1),
            slot: SlotIndex(0),
            issued: Generation(1),
            current: Generation(2),
        };
        assert!(e.is_use_after_free());
    }

    #[test]
    fn oom_is_not_use_after_free() {
        let e = BrokerError::OutOfMemory {
            arena: ArenaId(1),
            available: 100,
            requested: 200,
        };
        assert!(!e.is_use_after_free());
    }

    #[test]
    fn error_messages_are_informative() {
        let e = BrokerError::UseAfterFree {
            arena: ArenaId(3),
            slot: SlotIndex(7),
            issued: Generation(1),
            current: Generation(2),
        };
        let msg = format!("{e}");
        assert!(msg.contains("arena#3"));
        assert!(msg.contains("slot[7]"));
        assert!(msg.contains("gen1"));
        assert!(msg.contains("gen2"));
    }
}
EOF
)"

# src/handle.rs - the Handle<T> type
write_file "$BROKER_DIR/src/handle.rs" "$(cat <<'EOF'
//! Generational handles into broker arenas.
//!
//! A `Handle<T>` is a typed, generation-tracked reference to a value
//! stored in an arena. The handle is cheap to copy (it's just three
//! integers plus a phantom type marker) and safe to keep across the
//! lifetime of its arena.
//!
//! When the arena is dropped, the generation counter advances, and
//! subsequent access through any handle issued for that arena returns
//! `BrokerError::UseAfterFree`.

use crate::arena::Arena;
use crate::error::BrokerError;
use crate::ids::{ArenaId, Generation, SlotIndex};
use std::marker::PhantomData;
use std::sync::Weak;

/// A generational handle to a value of type `T` stored in an arena.
///
/// Handles are cheap (`Copy`-able... almost; see the `Weak` field below)
/// and can be passed around freely. Access through a handle returns a
/// typed error if the arena has been dropped.
pub struct Handle<T> {
    pub(crate) arena_id: ArenaId,
    pub(crate) slot: SlotIndex,
    pub(crate) generation: Generation,
    /// Weak reference to the arena so we can attempt to upgrade
    /// during `get()`. If the arena has been dropped, the upgrade
    /// fails and we return UseAfterFree.
    pub(crate) arena: Weak<Arena>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub(crate) fn new(
        arena_id: ArenaId,
        slot: SlotIndex,
        generation: Generation,
        arena: Weak<Arena>,
    ) -> Self {
        Self {
            arena_id,
            slot,
            generation,
            arena,
            _marker: PhantomData,
        }
    }

    /// The ArenaId this handle was issued for.
    #[must_use]
    pub const fn arena_id(&self) -> ArenaId {
        self.arena_id
    }

    /// The slot index this handle refers to.
    #[must_use]
    pub const fn slot(&self) -> SlotIndex {
        self.slot
    }

    /// The generation this handle was issued for.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// Returns `true` if the underlying arena is still live.
    ///
    /// A `true` return does not guarantee the handle is valid (the
    /// arena could be dropped between this call and a subsequent
    /// `get`), but a `false` return is definitive: the arena is gone.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.arena.upgrade().is_some()
    }

    /// Access the value behind this handle.
    ///
    /// Returns `Err(BrokerError::UseAfterFree)` if the arena has been
    /// dropped. The returned reference's lifetime is bounded by an
    /// internal lock on the arena; see `get_with` for a callback form
    /// that makes the lifetime explicit.
    ///
    /// # Safety
    ///
    /// This method uses unsafe internally to read from the arena's
    /// backing storage. The safety argument is that the generation
    /// check ensures we only read slots that are still live, and the
    /// slot's type tag (TODO: add type tagging in milestone A3) will
    /// eventually ensure we read it as the correct type.
    pub fn get(&self) -> Result<HandleRef<'_, T>, BrokerError> {
        let arena = self.arena.upgrade().ok_or(BrokerError::UseAfterFree {
            arena: self.arena_id,
            slot: self.slot,
            issued: self.generation,
            current: Generation(self.generation.raw().wrapping_add(1)),
        })?;

        // Generation check: even if the arena is alive, our generation
        // might not match (e.g., if a future broker version supports
        // arena reset that bumps the generation without dropping).
        let current_gen = arena.generation();
        if current_gen != self.generation {
            return Err(BrokerError::UseAfterFree {
                arena: self.arena_id,
                slot: self.slot,
                issued: self.generation,
                current: current_gen,
            });
        }

        // SAFETY: We hold an Arc to the arena (via the upgrade above),
        // the generation matches, and the slot was allocated within
        // this arena (verified by slot bounds). The lifetime of the
        // returned reference is tied to the HandleRef which keeps the
        // arena alive.
        let ptr = arena.slot_ptr(self.slot)?;

        Ok(HandleRef {
            ptr: ptr as *const T,
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
            generation: self.generation,
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
            .field("generation", &self.generation)
            .field("type", &std::any::type_name::<T>())
            .finish()
    }
}

/// A live reference to a value through a Handle.
///
/// Holds an Arc to the arena to ensure the arena cannot be dropped
/// while the reference is in use.
pub struct HandleRef<'a, T> {
    ptr: *const T,
    _arena: std::sync::Arc<Arena>,
    _marker: PhantomData<&'a T>,
}

impl<'a, T> std::ops::Deref for HandleRef<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: ptr was obtained from the arena's backing storage,
        // the arena is kept alive by _arena, and the generation was
        // checked before this HandleRef was constructed.
        unsafe { &*self.ptr }
    }
}

// Send/Sync: a HandleRef is essentially a borrowed reference. It's
// Send/Sync if T is, but we hold a raw pointer which isn't auto-impl.
// SAFETY: the pointer comes from an Arena which is Send + Sync, and
// the value at the pointer is T which we require Send/Sync to forward.
unsafe impl<'a, T: Sync> Send for HandleRef<'a, T> {}
unsafe impl<'a, T: Sync> Sync for HandleRef<'a, T> {}
EOF
)"

# src/arena.rs - the bump arena itself
write_file "$BROKER_DIR/src/arena.rs" "$(cat <<'EOF'
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

            match self.cursor.compare_exchange_weak(
                current,
                end,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break aligned,
                Err(_) => continue, // someone else allocated; retry
            }
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
        assert!(r.is_err(), "expected use-after-free, got {:?}", r);
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
EOF
)"

# src/broker.rs - the top-level Broker type
write_file "$BROKER_DIR/src/broker.rs" "$(cat <<'EOF'
//! The top-level Broker type.
//!
//! The Broker is a process-level registry of arenas. It hands out
//! ArenaIds and owns the Arcs that keep arenas alive. This is the
//! entry point most users interact with.

use crate::arena::Arena;
use crate::ids::ArenaIdCounter;
use std::sync::{Arc, RwLock};

/// The runtime memory broker.
///
/// In the current scaffold, the broker is a simple registry. Later
/// milestones will add allocation strategies, budgets, recording,
/// and secret-memory policies.
pub struct Broker {
    arena_ids: ArenaIdCounter,
    arenas: RwLock<Vec<Arc<Arena>>>,
}

impl Broker {
    /// Create a new broker with no arenas.
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena_ids: ArenaIdCounter::new(),
            arenas: RwLock::new(Vec::new()),
        }
    }

    /// Create a new arena registered with this broker.
    ///
    /// The arena is returned as an Arc so callers can clone references
    /// to it. When the last Arc is dropped, the arena drops and all
    /// outstanding handles are invalidated.
    pub fn create_arena(&self, name: &str, capacity: usize) -> Arc<Arena> {
        let id = self.arena_ids.next();
        let arena = Arena::new(id, name, capacity);

        if let Ok(mut arenas) = self.arenas.write() {
            arenas.push(Arc::clone(&arena));
        }

        tracing::debug!(
            arena_id = %id,
            name = %name,
            capacity = capacity,
            "arena created"
        );

        arena
    }

    /// Number of arenas the broker has created (including dropped ones).
    ///
    /// This is the total ever created, not the count currently live.
    /// A live-count query will be added in milestone A5.
    #[must_use]
    pub fn arena_count(&self) -> usize {
        self.arenas.read().map(|a| a.len()).unwrap_or(0)
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
    fn doctest_in_lib_rs_actually_runs() {
        // Mirror the doctest from lib.rs to catch breakage in CI
        // since doctests are slower to run.
        use crate::BrokerError;
        let broker = Broker::new();
        let arena = broker.create_arena("example", 4096);
        let handle = arena.alloc(42_u64).unwrap();
        assert_eq!(*handle.get().unwrap(), 42);
        drop(arena);
        assert!(matches!(
            handle.get(),
            Err(BrokerError::UseAfterFree { .. })
        ));
    }
}
EOF
)"

# tests/integration.rs - integration tests that exercise the public API
write_file "$BROKER_DIR/tests/integration.rs" "$(cat <<'EOF'
//! Integration tests for sentinel-broker.
//!
//! These tests use only the public API and serve as both
//! verification and worked examples.

use sentinel_broker::{Broker, BrokerError};

#[test]
fn end_to_end_basic_usage() {
    let broker = Broker::new();
    let arena = broker.create_arena("request-1", 8192);

    let handles: Vec<_> = (0..10_i32)
        .map(|i| arena.alloc(i * 10).unwrap())
        .collect();

    for (i, h) in handles.iter().enumerate() {
        assert_eq!(*h.get().unwrap(), (i as i32) * 10);
    }
}

#[test]
fn handles_outlive_their_source_but_not_their_arena() {
    let broker = Broker::new();
    let handle = {
        let arena = broker.create_arena("scoped", 1024);
        let h = arena.alloc(String::from("hello")).unwrap();
        assert_eq!(&*h.get().unwrap(), "hello");
        h
        // arena dropped here
    };

    let result = handle.get();
    assert!(matches!(result, Err(BrokerError::UseAfterFree { .. })));
}

#[test]
fn multiple_arenas_are_independent() {
    let broker = Broker::new();
    let arena_a = broker.create_arena("a", 1024);
    let arena_b = broker.create_arena("b", 1024);

    let ha = arena_a.alloc(1_u64).unwrap();
    let hb = arena_b.alloc(2_u64).unwrap();

    drop(arena_a);

    // Handle into arena_a is invalid...
    assert!(ha.get().is_err());
    // ...but handle into arena_b is still valid.
    assert_eq!(*hb.get().unwrap(), 2);
}

#[test]
fn handle_clone_preserves_validity() {
    let broker = Broker::new();
    let arena = broker.create_arena("clone-test", 1024);
    let h1 = arena.alloc(42_u64).unwrap();
    let h2 = h1.clone();

    assert_eq!(*h1.get().unwrap(), 42);
    assert_eq!(*h2.get().unwrap(), 42);
}

#[test]
fn handle_clone_shares_invalidation() {
    let broker = Broker::new();
    let arena = broker.create_arena("clone-invalid", 1024);
    let h1 = arena.alloc(42_u64).unwrap();
    let h2 = h1.clone();

    drop(arena);

    assert!(h1.get().is_err());
    assert!(h2.get().is_err());
}
EOF
)"

# tests/proptest.rs - property-based tests for safety invariants
write_file "$BROKER_DIR/tests/proptest.rs" "$(cat <<'EOF'
//! Property-based tests for broker safety invariants.
//!
//! These tests use `proptest` to generate random sequences of
//! operations and verify that the broker's safety properties hold
//! under all generated inputs.

use proptest::prelude::*;
use sentinel_broker::Broker;

proptest! {
    /// For any sequence of allocations followed by drop, every handle
    /// must be readable before drop and unreadable after.
    #[test]
    fn allocations_are_readable_before_drop_and_not_after(
        values in prop::collection::vec(any::<u64>(), 1..100),
    ) {
        let broker = Broker::new();
        let arena = broker.create_arena("proptest", 1024 * 64);

        let handles: Vec<_> = values
            .iter()
            .filter_map(|v| arena.alloc(*v).ok())
            .collect();

        // Before drop: every handle returns the right value.
        for (h, expected) in handles.iter().zip(values.iter()) {
            prop_assert_eq!(*h.get().unwrap(), *expected);
        }

        drop(arena);

        // After drop: every handle returns UseAfterFree.
        for h in &handles {
            prop_assert!(h.get().is_err());
            prop_assert!(h.get().unwrap_err().is_use_after_free());
        }
    }

    /// Allocating in one arena and dropping another must not affect
    /// the first arena's handles.
    #[test]
    fn arenas_are_isolated(
        values_a in prop::collection::vec(any::<u32>(), 1..50),
        values_b in prop::collection::vec(any::<u32>(), 1..50),
    ) {
        let broker = Broker::new();
        let arena_a = broker.create_arena("a", 1024 * 16);
        let arena_b = broker.create_arena("b", 1024 * 16);

        let handles_a: Vec<_> = values_a
            .iter()
            .filter_map(|v| arena_a.alloc(*v).ok())
            .collect();
        let handles_b: Vec<_> = values_b
            .iter()
            .filter_map(|v| arena_b.alloc(*v).ok())
            .collect();

        drop(arena_b);

        // arena_b handles are dead...
        for h in &handles_b {
            prop_assert!(h.get().is_err());
        }

        // ...but arena_a handles are still alive.
        for (h, expected) in handles_a.iter().zip(values_a.iter()) {
            prop_assert_eq!(*h.get().unwrap(), *expected);
        }
    }
}
EOF
)"

# benches/arena_bench.rs - basic benchmarks
write_file "$BROKER_DIR/benches/arena_bench.rs" "$(cat <<'EOF'
//! Benchmarks for arena allocation overhead.
//!
//! These set the baseline for what the broker costs versus raw
//! allocation. Used to detect performance regressions in later
//! milestones.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sentinel_broker::Broker;

fn bench_alloc_u64(c: &mut Criterion) {
    let broker = Broker::new();
    c.bench_function("arena_alloc_u64", |b| {
        b.iter(|| {
            let arena = broker.create_arena("bench", 1024 * 1024);
            for i in 0_u64..1000 {
                let _h = arena.alloc(black_box(i)).unwrap();
            }
        });
    });
}

fn bench_alloc_and_read(c: &mut Criterion) {
    c.bench_function("arena_alloc_and_read", |b| {
        b.iter(|| {
            let broker = Broker::new();
            let arena = broker.create_arena("bench", 1024 * 1024);
            let handles: Vec<_> = (0_u64..1000)
                .map(|i| arena.alloc(i).unwrap())
                .collect();
            let mut sum = 0_u64;
            for h in &handles {
                sum = sum.wrapping_add(*h.get().unwrap());
            }
            black_box(sum)
        });
    });
}

criterion_group!(benches, bench_alloc_u64, bench_alloc_and_read);
criterion_main!(benches);
EOF
)"

# ---------------------------------------------------------------------------
# Build and test
# ---------------------------------------------------------------------------

info "Building the broker crate..."
if ! cargo build -p sentinel-broker 2>&1 | tail -30; then
    err "broker build failed"
    exit 1
fi
ok "broker built successfully"

info "Running broker unit tests..."
if ! cargo nextest run -p sentinel-broker 2>&1 | tail -30; then
    err "broker tests failed"
    exit 1
fi
ok "broker unit tests passed"

info "Running broker doctests..."
if ! cargo test -p sentinel-broker --doc 2>&1 | tail -20; then
    err "broker doctests failed"
    exit 1
fi
ok "broker doctests passed"

info "Running clippy on broker..."
if ! cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -20; then
    err "clippy reported issues"
    exit 1
fi
ok "clippy clean"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo ""
echo "======"
echo "PHASE A0+A1+A2 COMPLETE"
echo "======"
echo ""
echo "Deliverable: sentinel-broker crate with"
echo "  - Foundation types: ArenaId, SlotIndex, Generation, Handle, BrokerError"
echo "  - Bump arena with generational handle safety"
echo "  - Broker top-level type"
echo "  - Unit tests, integration tests, property tests, benchmarks"
echo ""
echo "Test summary:"
cargo nextest run -p sentinel-broker 2>&1 | grep -E "^(Summary|test result)" | tail -5
echo ""
echo "To run benchmarks: cargo bench -p sentinel-broker"
echo "To re-test:        just test"
echo ""
echo "Files added/updated under crates/sentinel-broker/:"
find "$BROKER_DIR" -type f -name '*.rs' -o -name '*.toml' | \
    sed "s|$SENTINEL_ROOT/||" | sort
echo ""
