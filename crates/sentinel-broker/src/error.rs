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
    /// The broker's internal lock is poisoned by a prior panic.
    #[error("broker state is poisoned")]
    BrokerPoisoned,

    /// A feature has been declared but is not yet implemented.
    /// Used during staged development to flag callable-but-unfinished APIs.
    #[error("not implemented: {feature}")]
    NotImplemented { feature: &'static str },

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

    /// An allocation was rejected because it would exceed a Budget cap.
    ///
    /// Returned by allocations inside [`crate::Broker::within_budget`]
    /// scopes when the cumulative byte total would exceed the budget.
    #[error("budget exceeded: {budget} would exceed cap (requested {requested}, remaining {remaining})")]
    BudgetExceeded {
        budget: crate::ids::BudgetId,
        requested: usize,
        remaining: usize,
    },

    #[error("secret memory policy failed: {reason}")]
    SecretMemory { reason: &'static str },
}

impl BrokerError {
    /// Returns true if this error indicates a safety violation
    /// (use-after-free) as opposed to a resource exhaustion or
    /// programming error.
    #[must_use]
    pub const fn is_use_after_free(&self) -> bool {
        matches!(self, Self::UseAfterFree { .. } | Self::UseAfterFreeSlot { .. })
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