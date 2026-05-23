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
    pub size: usize,
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

    /// The strategy's backing buffer, if any. Used by SecretStrategy
    /// to call mlock / zero-on-destroy on the underlying memory.
    /// Default: None (strategy is opaque or does not own a buffer).
    fn backing_buffer(&self) -> Option<(*mut u8, usize)> { None }

    /// Mutable pointer + size to a slot's bytes. Used by SecretStrategy
    /// to wipe slot contents before forwarding to free. Default: None.
    fn slot_ptr_mut(&self, _slot: crate::ids::SlotIndex) -> Option<SlotPtr> { None }

    /// Per-slot byte size for strategies with uniform slots.
    /// Returns None for variable-size strategies (e.g. bump).
    /// Used by diagnostic tools to read raw slot bytes.
    fn slot_size_hint(&self) -> Option<usize> { None }
}