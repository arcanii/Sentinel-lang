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
    #[allow(dead_code)]
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
        let a: ArenaId = ArenaId(0);
        let s: SlotIndex = SlotIndex(0);
        let g: Generation = Generation(1);
        assert!(format!("{a:?}").contains("ArenaId"));
        assert!(format!("{s:?}").contains("SlotIndex"));
        assert!(format!("{g:?}").contains("Generation"));
    }

    #[test]
    fn ids_display_distinctly() {
        assert_eq!(format!("{}", ArenaId(7)), "arena#7");
        assert_eq!(format!("{}", SlotIndex(3)), "slot[3]");
        assert_eq!(format!("{}", Generation(5)), "gen5");
    }
}