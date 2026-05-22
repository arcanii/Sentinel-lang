//! Property-based tests for broker safety invariants.

use proptest::prelude::*;
use sentinel_broker::Broker;

proptest! {
    /// For any sequence of allocations followed by destroy_arena,
    /// every handle must be readable before destruction and
    /// unreadable after.
    #[test]
    fn allocations_are_readable_before_destroy_and_not_after(
        values in prop::collection::vec(any::<u64>(), 1..100),
    ) {
        let broker = Broker::new();
        let arena = broker.create_arena("proptest", 1024 * 64);
        let arena_id = arena.id();

        let handles: Vec<_> = values
            .iter()
            .filter_map(|v| arena.alloc(*v).ok())
            .collect();

        // Before destroy: every handle returns the right value.
        for (h, expected) in handles.iter().zip(values.iter()) {
            prop_assert_eq!(*h.get().unwrap(), *expected);
        }

        broker.destroy_arena(arena_id).unwrap();

        // After destroy: every handle returns UseAfterFree.
        for h in &handles {
            prop_assert!(h.get().is_err());
            prop_assert!(h.get().unwrap_err().is_use_after_free());
        }
    }

    /// Destroying one arena must not affect handles in another arena.
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

        broker.destroy_arena(arena_b.id()).unwrap();

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
