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