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