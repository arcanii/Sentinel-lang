//! Read-only diagnostic types for broker introspection.
//!
//! These structures are *snapshots* produced by [`Broker::stats`],
//! [`Broker::list_arenas`], and [`Broker::where_is`]. They never hold
//! locks or arena references — they are safe to log, serialize, or
//! ship across threads.
//!
//! ```ignore
//! let stats = broker.stats();
//! println!("live arenas: {}", stats.live_arenas);
//! for summary in broker.list_arenas() {
//!     println!("{}: {}/{} bytes", summary.name, summary.used, summary.capacity);
//! }
//! ```
//!
//! [`Broker::stats`]: crate::Broker::stats
//! [`Broker::list_arenas`]: crate::Broker::list_arenas
//! [`Broker::where_is`]: crate::Broker::where_is

use crate::ids::{ArenaId, Generation, SlotGeneration, SlotIndex};
use crate::strategy::StrategyKind;

/// Aggregate counters across every live arena registered with the broker.
///
/// Cumulative counters (`total_allocations`, `total_frees`) include
/// allocations made into arenas that have since been destroyed —
/// they reflect lifetime activity, not just current state. Capacity
/// and usage figures sum *only* over currently-live arenas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrokerStats {
    /// Number of arenas currently registered with the broker.
    pub live_arenas: usize,
    /// Sum of `capacity()` over all live arenas, in bytes.
    pub total_capacity_bytes: usize,
    /// Sum of `used()` over all live arenas, in bytes.
    pub total_used_bytes: usize,
    /// Lifetime count of successful allocations across all live arenas.
    pub total_allocations: u64,
    /// Lifetime count of successful frees across all live arenas.
    pub total_frees: u64,
}

/// A snapshot of one arena's state at the moment of the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaSummary {
    pub id: ArenaId,
    pub name: String,
    pub kind: StrategyKind,
    pub capacity: usize,
    pub used: usize,
    /// The arena's own generation. Increments on `destroy_arena`.
    pub generation: Generation,
    pub allocations: u64,
    pub frees: u64,
}

/// The physical location and liveness of a handle, as resolved through
/// the broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleLocation {
    pub arena: ArenaId,
    pub arena_name: String,
    pub slot: SlotIndex,
    /// The slot generation embedded in the handle.
    pub slot_generation: SlotGeneration,
    /// `true` if the arena is still live *and* the handle's slot
    /// generation matches the arena's current slot generation for
    /// that slot. `false` if the slot has been freed/reused.
    pub is_live: bool,
}
