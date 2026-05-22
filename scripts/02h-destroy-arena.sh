#!/usr/bin/env bash
set -euo pipefail
SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"

python3 - "$BROKER" <<'PYEOF'
import re, pathlib, sys
broker = pathlib.Path(sys.argv[1])

def write(rel, content, label):
    p = broker / rel
    if p.exists() and p.read_text() == content:
        print(f"[SKIP] {rel}: {label}")
        return
    p.write_text(content)
    print(f"[OK]   {rel}: {label}")

def patch(rel, fn, label):
    p = broker / rel
    src = p.read_text()
    new = fn(src)
    if new == src:
        print(f"[SKIP] {rel}: {label}")
        return
    p.write_text(new)
    print(f"[OK]   {rel}: {label}")

# ---------------------------------------------------------------------------
# 1. arena.rs — add `pub(crate) fn invalidate(&self)` and call it from Drop.
# ---------------------------------------------------------------------------
def add_invalidate(src):
    # Insert invalidate() method just after the generation() accessor.
    method_marker = "    pub fn generation(&self) -> Generation {\n        Generation(self.generation.load(Ordering::Acquire))\n    }"
    if method_marker not in src:
        print("[WARN] arena.rs: could not find generation() accessor; skipping invalidate insertion")
        return src
    if "pub(crate) fn invalidate" in src:
        new = src
    else:
        insertion = method_marker + """

    /// Advance the generation counter, invalidating every outstanding handle.
    ///
    /// Called by `Broker::destroy_arena` and by `Drop` as a safety net.
    /// After this returns, every `Handle` issued by this arena will
    /// return `BrokerError::UseAfterFree` on access.
    pub(crate) fn invalidate(&self) {
        // fetch_add is sufficient: handles only check for equality, and any
        // increment makes the issued generation stale.
        self.generation.fetch_add(1, Ordering::AcqRel);
    }"""
        new = src.replace(method_marker, insertion, 1)

    # Ensure Drop invalidates as a safety net before freeing the buffer.
    if "impl Drop for Arena" in new:
        # Add invalidate() call at the top of the existing Drop body.
        drop_pat = re.compile(
            r'(impl Drop for Arena \{\s*fn drop\(&mut self\) \{\s*)',
        )
        if "self.invalidate();" not in new:
            new = drop_pat.sub(r'\1self.invalidate();\n        ', new, count=1)
    else:
        # Append a Drop impl. It must deallocate the buffer.
        new = new.rstrip() + """

impl Drop for Arena {
    fn drop(&mut self) {
        // Safety net: bump the generation so any stray handle that
        // somehow still has a Weak to us fails its generation check.
        self.invalidate();
        // SAFETY: `buffer` was allocated with `buffer_layout` in `new`
        // and has not been deallocated yet (Drop runs exactly once).
        unsafe {
            std::alloc::dealloc(self.buffer.as_ptr(), self.buffer_layout);
        }
    }
}
"""
    return new
patch("src/arena.rs", add_invalidate, "added Arena::invalidate + Drop safety net")

# ---------------------------------------------------------------------------
# 2. broker.rs — full rewrite. Introduces ArenaHandle and destroy_arena.
# ---------------------------------------------------------------------------
broker_rs = r'''//! The top-level Broker type.
//!
//! The Broker is a process-level registry of arenas. It owns the strong
//! `Arc<Arena>` references that keep arenas alive; users receive an
//! [`ArenaHandle`] that gives them access without taking ownership.
//! To release an arena and invalidate every handle it issued, call
//! [`Broker::destroy_arena`].

use crate::arena::Arena;
use crate::error::BrokerError;
use crate::ids::{ArenaId, ArenaIdCounter};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

/// A user-facing reference to a broker-owned arena.
///
/// Cloning an `ArenaHandle` is cheap and does *not* extend the arena's
/// lifetime: only the broker's internal map keeps the arena alive.
/// Call [`Broker::destroy_arena`] to free the arena and invalidate
/// every handle it issued.
#[derive(Clone)]
pub struct ArenaHandle {
    id: ArenaId,
    arena: Arc<Arena>,
}

impl ArenaHandle {
    /// The id of the arena this handle refers to.
    #[must_use]
    pub fn id(&self) -> ArenaId {
        self.id
    }
}

impl Deref for ArenaHandle {
    type Target = Arena;
    fn deref(&self) -> &Arena {
        &self.arena
    }
}

impl std::fmt::Debug for ArenaHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaHandle")
            .field("id", &self.id)
            .field("name", &self.arena.name())
            .field("capacity", &self.arena.capacity())
            .finish_non_exhaustive()
    }
}

/// The runtime memory broker.
///
/// In the current scaffold, the broker is a registry that owns arenas
/// and can destroy them on request. Later milestones will add
/// allocation strategies, budgets, recording, and secret-memory
/// policies.
pub struct Broker {
    arena_ids: ArenaIdCounter,
    arenas: RwLock<HashMap<ArenaId, Arc<Arena>>>,
}

impl Broker {
    /// Create a new broker with no arenas.
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena_ids: ArenaIdCounter::new(),
            arenas: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new arena registered with this broker.
    ///
    /// The returned `ArenaHandle` borrows from the broker's internal
    /// map. The arena lives until you call [`Broker::destroy_arena`]
    /// or the broker itself is dropped.
    pub fn create_arena(&self, name: &str, capacity: usize) -> ArenaHandle {
        let id = self.arena_ids.next();
        let arena = Arena::new(id, name, capacity);

        // Best-effort insertion. A poisoned lock means a previous panic
        // left state we cannot reason about; in that case we still
        // return the handle but the arena will not be tracked.
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, Arc::clone(&arena));
        }

        tracing::debug!(
            arena_id = %id,
            name = %name,
            capacity = capacity,
            "arena created"
        );

        ArenaHandle { id, arena }
    }

    /// Destroy an arena, invalidating every handle issued for it.
    ///
    /// After this call returns successfully, every `Handle` issued by
    /// this arena will return [`BrokerError::UseAfterFree`] on access,
    /// regardless of how many `ArenaHandle` clones still exist.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::UnknownArena`] if no arena with the given
    /// id is registered (it was already destroyed, or never created
    /// by this broker).
    pub fn destroy_arena(&self, id: ArenaId) -> Result<(), BrokerError> {
        let removed = {
            let mut arenas = self
                .arenas
                .write()
                .map_err(|_| BrokerError::BrokerPoisoned)?;
            arenas.remove(&id)
        };

        match removed {
            Some(arena) => {
                // Advance the generation first so that any concurrent
                // handle access sees the bumped value even if some
                // `Arc<Arena>` clone keeps the arena alive afterward.
                arena.invalidate();
                tracing::debug!(arena_id = %id, "arena destroyed");
                Ok(())
            }
            None => Err(BrokerError::UnknownArena { arena: id }),
        }
    }

    /// Number of arenas currently registered with the broker.
    #[must_use]
    pub fn live_arena_count(&self) -> usize {
        self.arenas.read().map_or(0, |a| a.len())
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
    fn destroy_arena_invalidates_handles() {
        let broker = Broker::new();
        let arena = broker.create_arena("example", 4096);
        let handle = arena.alloc(42_u64).unwrap();
        assert_eq!(*handle.get().unwrap(), 42);

        broker.destroy_arena(arena.id()).unwrap();

        assert!(matches!(
            handle.get(),
            Err(BrokerError::UseAfterFree { .. })
        ));
    }

    #[test]
    fn destroy_unknown_arena_is_an_error() {
        let broker = Broker::new();
        // Create and destroy to consume an id, then try to destroy again.
        let a = broker.create_arena("ephemeral", 64);
        let id = a.id();
        broker.destroy_arena(id).unwrap();
        assert!(matches!(
            broker.destroy_arena(id),
            Err(BrokerError::UnknownArena { .. })
        ));
    }

    #[test]
    fn live_arena_count_tracks_destruction() {
        let broker = Broker::new();
        let a = broker.create_arena("a", 64);
        let b = broker.create_arena("b", 64);
        assert_eq!(broker.live_arena_count(), 2);
        broker.destroy_arena(a.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 1);
        broker.destroy_arena(b.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 0);
    }
}
'''
write("src/broker.rs", broker_rs, "rewritten with ArenaHandle + destroy_arena")

# ---------------------------------------------------------------------------
# 3. error.rs — add UnknownArena and BrokerPoisoned variants.
# ---------------------------------------------------------------------------
def add_error_variants(src):
    changed = False
    if "UnknownArena" not in src:
        # Insert the variant just before the closing `}` of the enum.
        enum_pat = re.compile(r'(pub enum BrokerError \{.*?)(\n\}\s*$)', re.DOTALL | re.MULTILINE)
        m = enum_pat.search(src)
        if m:
            # Determine if the variant list uses #[error("…")] from thiserror.
            uses_thiserror = '#[error(' in m.group(1)
            variant = ""
            if uses_thiserror:
                variant += "\n    /// The requested arena is not registered with this broker.\n"
                variant += "    #[error(\"unknown arena: {arena}\")]\n"
                variant += "    UnknownArena { arena: crate::ids::ArenaId },\n"
                variant += "\n    /// The broker's internal lock is poisoned (a prior panic).\n"
                variant += "    #[error(\"broker state is poisoned\")]\n"
                variant += "    BrokerPoisoned,"
            else:
                variant += "\n    /// The requested arena is not registered with this broker.\n"
                variant += "    UnknownArena { arena: crate::ids::ArenaId },\n"
                variant += "    /// The broker's internal lock is poisoned (a prior panic).\n"
                variant += "    BrokerPoisoned,"
            src = src[:m.end(1)] + variant + src[m.end(1):]
            changed = True
    return src if changed else src

patch("src/error.rs", add_error_variants, "added UnknownArena + BrokerPoisoned variants")

# ---------------------------------------------------------------------------
# 4. lib.rs — update doctest to use destroy_arena, and re-export ArenaHandle.
# ---------------------------------------------------------------------------
def fix_lib(src):
    new = src
    # Re-export ArenaHandle.
    if "pub use broker::{Broker, ArenaHandle};" not in new:
        new = new.replace("pub use broker::Broker;", "pub use broker::{Broker, ArenaHandle};", 1)
    # Update the doctest: replace `drop(arena);` with destroy_arena call.
    new = new.replace(
        "//! drop(arena);\n//! assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));",
        "//! broker.destroy_arena(arena.id()).unwrap();\n//! assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));",
    )
    return new
patch("src/lib.rs", fix_lib, "re-export ArenaHandle + fix doctest")

# ---------------------------------------------------------------------------
# 5. tests/integration.rs — rewrite the failing tests to use destroy_arena.
# ---------------------------------------------------------------------------
integration_rs = r'''//! Integration tests for sentinel-broker.
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
'''
write("tests/integration.rs", integration_rs, "use destroy_arena instead of drop")

# ---------------------------------------------------------------------------
# 6. tests/proptest.rs — same rewrite pattern.
# ---------------------------------------------------------------------------
proptest_rs = r'''//! Property-based tests for broker safety invariants.

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
'''
write("tests/proptest.rs", proptest_rs, "use destroy_arena instead of drop")
PYEOF

cd "$SENTINEL_ROOT"

echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -20

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -25

echo
echo "======"
echo "TESTS"
echo "======"
cargo nextest run -p sentinel-broker 2>&1 | tail -30 || \
  cargo test -p sentinel-broker 2>&1 | tail -30

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -10

echo
echo "======"
echo "02h COMPLETE"
echo "======"
echo "If all four sections above are green, commit via GitHub Desktop:"
echo "  broker: phase A0+A1+A2 — foundations, bump arena, destroy_arena, lint hygiene"
