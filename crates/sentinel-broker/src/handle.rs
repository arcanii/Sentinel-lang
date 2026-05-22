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
            ptr: ptr.cast::<T>(),
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
            .finish_non_exhaustive()
    }
}

/// A live reference to a value through a Handle.
///
/// Holds an Arc to the arena to ensure the arena cannot be dropped
/// while the reference is in use.
#[derive(Debug)]
pub struct HandleRef<'a, T> {
    ptr: *const T,
    _arena: std::sync::Arc<Arena>,
    _marker: PhantomData<&'a T>,
}

impl<T> std::ops::Deref for HandleRef<'_, T> {
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
unsafe impl<T: Sync> Send for HandleRef<'_, T> {}
unsafe impl<T: Sync> Sync for HandleRef<'_, T> {}