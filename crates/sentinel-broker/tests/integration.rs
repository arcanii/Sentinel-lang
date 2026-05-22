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
        assert_eq!(*h.get().unwrap(), i32::try_from(i).unwrap() * 10);
    }
}

#[test]
fn handles_outlive_their_source_but_not_their_arena() {
    let broker = Broker::new();
    let arena = broker.create_arena("scoped", 1024);
    let arena_id = arena.id();
    let handle = arena.alloc(String::from("hello")).unwrap();
    assert_eq!(&*handle.get().unwrap(), "hello");

    broker.destroy_arena(arena_id).unwrap();

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

    broker.destroy_arena(arena_a.id()).unwrap();

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

    broker.destroy_arena(arena.id()).unwrap();

    assert!(h1.get().is_err());
    assert!(h2.get().is_err());
}
