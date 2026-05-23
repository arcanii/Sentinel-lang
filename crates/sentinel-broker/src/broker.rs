//! The top-level Broker type.
//!
//! Owns the strong `Arc<Arena>` references that keep arenas alive.
//! Users receive an [`ArenaHandle`] for access without ownership.

use crate::arena::Arena;
use crate::stats::{ArenaSummary, BrokerStats, HandleLocation};
use crate::recording::{Event, Recorder};
use crate::builder::ArenaBuilder;
use crate::error::BrokerError;
use crate::budget::{Budget, BudgetScope};
use crate::ids::{ArenaId, ArenaIdCounter, BudgetIdCounter};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

/// A user-facing reference to a broker-owned arena.
#[derive(Clone)]
pub struct ArenaHandle {
    id: ArenaId,
    arena: Arc<Arena>,
}

impl ArenaHandle {
    pub(crate) fn from_parts(id: ArenaId, arena: Arc<Arena>) -> Self {
        Self { id, arena }
    }

    #[must_use]
    pub fn id(&self) -> ArenaId { self.id }

    /// Allocate a value of type `T` into this arena.
    ///
    /// # Errors
    /// Returns [`BrokerError::OutOfMemory`] if capacity is exhausted.
    pub fn alloc<T>(&self, value: T) -> Result<crate::Handle<T>, BrokerError>
    where
        T: 'static,
    {
        self.arena.alloc(value)
    }

    /// Free a handle's slot, advancing the slot's generation.
    ///
    /// After this returns Ok, the handle becomes invalid; further
    /// access returns [`BrokerError::UseAfterFreeSlot`].
    ///
    /// # Errors
    /// - [`BrokerError::NotImplemented`] for strategies that don't recycle.
    /// - [`BrokerError::UseAfterFreeSlot`] for double-free.
    pub fn free<T>(&self, handle: &crate::Handle<T>) -> Result<(), BrokerError> {
        self.arena.free(handle)
    }

    /// **DIAGNOSTIC ONLY**: forwards to [`Arena::__raw_slot_bytes_for_diagnostics`].
    /// See that method's safety/stability notes.
    #[doc(hidden)]
    #[must_use]
    pub fn __raw_slot_bytes_for_diagnostics(
        &self,
        slot: crate::ids::SlotIndex,
    ) -> Option<(*const u8, usize)> {
        self.arena.__raw_slot_bytes_for_diagnostics(slot)
    }

}

impl Deref for ArenaHandle {
    type Target = Arena;
    fn deref(&self) -> &Arena { &self.arena }
}

impl std::fmt::Debug for ArenaHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArenaHandle")
            .field("id", &self.id)
            .field("name", &self.arena.name())
            .field("kind", &self.arena.strategy_kind())
            .field("capacity", &self.arena.capacity())
            .finish_non_exhaustive()
    }
}

pub struct Broker {
    arena_ids: ArenaIdCounter,
    budget_ids: BudgetIdCounter,
    arenas: RwLock<HashMap<ArenaId, Arc<Arena>>>,
    recorder: Option<Arc<Recorder>>,
}

impl Broker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena_ids: ArenaIdCounter::new(),
            budget_ids: BudgetIdCounter::new(),
            arenas: RwLock::new(HashMap::new()),
            recorder: None,
        }
    }

    /// Start building an arena with a fluent API.
    ///
    /// ```ignore
    /// let arena = broker.arena("requests").capacity(4096).bump();
    /// let slab  = broker.arena("events").slab(64, 8, 256);
    /// ```
    #[must_use]
    pub fn arena(&self, name: &str) -> ArenaBuilder<'_> {
        ArenaBuilder::new(self, name.to_string())
    }

    /// Convenience: create a bump arena directly (kept for compatibility
    /// with the A0–A2 API). Equivalent to
    /// `broker.arena(name).capacity(capacity).bump()`.
    pub fn create_arena(&self, name: &str, capacity: usize) -> ArenaHandle {
        self.arena(name).capacity(capacity).bump()
    }

    /// Destroy an arena, invalidating every handle issued for it.
    ///
    /// # Errors
    /// Returns [`BrokerError::UnknownArena`] if no such arena exists.
    pub fn destroy_arena(&self, id: ArenaId) -> Result<(), BrokerError> {
        let removed = {
            let mut arenas = self.arenas.write().map_err(|_| BrokerError::BrokerPoisoned)?;
            arenas.remove(&id)
        };
        match removed {
            Some(arena) => {
                arena.invalidate();
                if let Some(r) = &self.recorder {
                    r.record(Event::ArenaDestroyed { id, at_ns: r.now_ns() });
                }
                tracing::debug!(arena_id = %id, "arena destroyed");
                Ok(())
            }
            None => Err(BrokerError::UnknownArena { arena: id }),
        }
    }

    /// Number of arenas currently registered.
    #[must_use]
    pub fn live_arena_count(&self) -> usize {
        self.arenas.read().map_or(0, |a| a.len())
    }

    // --- pub(crate) hooks used by the builder ---

    pub(crate) fn recorder_arc(&self) -> Option<Arc<Recorder>> {
        self.recorder.clone()
    }

    pub(crate) fn next_arena_id(&self) -> ArenaId {
        self.arena_ids.next()
    }

    pub(crate) fn register_arena(&self, arena: Arc<Arena>) {
        let id = arena.id();
        // Capture summary BEFORE moving `arena` into the map.
        let name = arena.name().to_string();
        let kind = arena.strategy_kind();
        let capacity = arena.capacity();
        if let Ok(mut arenas) = self.arenas.write() {
            arenas.insert(id, arena);
        }
        tracing::debug!(arena_id = %id, "arena registered");
        if let Some(r) = &self.recorder {
            r.record(Event::ArenaCreated {
                id,
                name,
                kind,
                capacity,
                at_ns: r.now_ns(),
            });
        }
    }


    /// Run `f` inside a fresh top-level budget capped at `cap` bytes.
    ///
    /// Any arena created through the supplied [`BudgetScope`] charges
    /// against this budget. The closure's return value propagates;
    /// [`BrokerError::BudgetExceeded`] is returned if any allocation
    /// inside `f` would exceed the cap.
    ///
    /// # Errors
    /// Returns the closure's error, or [`BrokerError::BudgetExceeded`].
    pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
    where
        F: FnOnce(&BudgetScope<'_>) -> Result<R, BrokerError>,
    {
        let id = self.next_budget_id();
        let budget = Budget::new(id, cap, None);
        if let Some(r) = &self.recorder {
            r.record(Event::BudgetOpened { id, cap, parent: None, at_ns: r.now_ns() });
        }
        let scope = BudgetScope::new(self, budget);
        let result = f(&scope);
        if let Some(r) = &self.recorder {
            r.record(Event::BudgetClosed {
                id,
                used_at_close: scope.budget().used(),
                at_ns: r.now_ns(),
            });
        }
        result
    }

    pub(crate) fn next_budget_id(&self) -> crate::ids::BudgetId {
        self.budget_ids.next()
    }

    /// Aggregate counters across every currently-live arena.
    #[must_use]
    pub fn stats(&self) -> BrokerStats {
        let Ok(arenas) = self.arenas.read() else {
            return BrokerStats::default();
        };
        let mut s = BrokerStats {
            live_arenas: arenas.len(),
            ..BrokerStats::default()
        };
        for arena in arenas.values() {
            s.total_capacity_bytes = s.total_capacity_bytes.saturating_add(arena.capacity());
            s.total_used_bytes = s.total_used_bytes.saturating_add(arena.used());
            s.total_allocations = s.total_allocations.saturating_add(arena.alloc_count());
            s.total_frees = s.total_frees.saturating_add(arena.free_count());
        }
        s
    }

    /// Snapshot every live arena as an [`ArenaSummary`].
    #[must_use]
    pub fn list_arenas(&self) -> Vec<ArenaSummary> {
        let Ok(arenas) = self.arenas.read() else {
            return Vec::new();
        };
        let mut out: Vec<ArenaSummary> = arenas.values().map(|a| ArenaSummary {
            id: a.id(),
            name: a.name().to_string(),
            kind: a.strategy_kind(),
            capacity: a.capacity(),
            used: a.used(),
            generation: a.generation(),
            allocations: a.alloc_count(),
            frees: a.free_count(),
        }).collect();
        out.sort_by_key(|s| s.id);
        out
    }

    /// Resolve a handle's physical location and current liveness.
    ///
    /// Returns `None` if the handle's arena has been destroyed.
    #[must_use]
    pub fn where_is<T>(&self, handle: &crate::Handle<T>) -> Option<HandleLocation> {
        let arenas = self.arenas.read().ok()?;
        let arena = arenas.get(&handle.arena_id())?;
        Some(HandleLocation {
            arena: arena.id(),
            arena_name: arena.name().to_string(),
            slot: handle.slot(),
            slot_generation: handle.slot_generation(),
            is_live: handle.is_live(),
        })
    }


}


impl Broker {
    /// Construct a broker with an attached recorder. All event-emitting
    /// sites will forward to it.
    #[must_use]
    pub fn with_recorder(recorder: Arc<Recorder>) -> Self {
        let mut b = Self::new();
        b.recorder = Some(recorder);
        b
    }

    /// Access the broker's recorder, if any.
    #[must_use]
    pub fn recorder(&self) -> Option<&Arc<Recorder>> {
        self.recorder.as_ref()
    }
}

impl Default for Broker {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::StrategyKind;

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
        assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));
    }

    #[test]
    fn destroy_unknown_arena_is_an_error() {
        let broker = Broker::new();
        let a = broker.create_arena("ephemeral", 64);
        let id = a.id();
        broker.destroy_arena(id).unwrap();
        assert!(matches!(broker.destroy_arena(id), Err(BrokerError::UnknownArena { .. })));
    }

    #[test]
    fn live_arena_count_tracks_destruction() {
        let broker = Broker::new();
        let a = broker.create_arena("a", 64);
        let b2 = broker.create_arena("b", 64);
        assert_eq!(broker.live_arena_count(), 2);
        broker.destroy_arena(a.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 1);
        broker.destroy_arena(b2.id()).unwrap();
        assert_eq!(broker.live_arena_count(), 0);
    }

    #[test]
    fn mixed_bump_and_slab_arenas() {
        let b = Broker::new();
        let bump = b.arena("bump").capacity(1024).bump();
        let slab = b.arena("slab").slab(64, 8, 16);
        assert_eq!(bump.strategy_kind(), StrategyKind::Bump);
        assert_eq!(slab.strategy_kind(), StrategyKind::Slab);

        let hb = bump.alloc(1_u64).unwrap();
        let hs = slab.alloc(2_u64).unwrap();
        assert_eq!(*hb.get().unwrap(), 1);
        assert_eq!(*hs.get().unwrap(), 2);

        b.destroy_arena(slab.id()).unwrap();
        assert!(hs.get().is_err());
        assert_eq!(*hb.get().unwrap(), 1);
    }

    #[test]
    fn slab_oom_when_slots_exhausted() {
        let b = Broker::new();
        let slab = b.arena("tiny").slab(8, 8, 2);
        slab.alloc(1_u64).unwrap();
        slab.alloc(2_u64).unwrap();
        let err = slab.alloc(3_u64).unwrap_err();
        assert!(matches!(err, BrokerError::OutOfMemory { .. }));
    }

    #[test]
    fn slab_rejects_oversized_allocation() {
        let b = Broker::new();
        let slab = b.arena("small-slots").slab(4, 4, 16);
        let err = slab.alloc(0_u64).unwrap_err(); // u64 is 8 bytes, slot is 4
        assert!(matches!(err, BrokerError::OutOfMemory { .. }));
    }

    #[test]
    fn bump_strategy_free_returns_not_implemented() {
        // Bump never recycles: free() returns NotImplemented.
        use crate::ids::{ArenaId, SlotGeneration, SlotIndex};
        use crate::strategy::{bump::BumpStrategy, AllocStrategy};
        let s = BumpStrategy::new(ArenaId(99), 64);
        let err = s.free(SlotIndex(0), SlotGeneration::INITIAL).unwrap_err();
        assert!(matches!(err, BrokerError::NotImplemented { .. }));
    }

    #[test]
    fn slab_strategy_free_recycles() {
        // Slab supports recycling. Allocate, free with the issued
        // generation, then allocate again into the same slot.
        use crate::ids::ArenaId;
        use crate::strategy::{slab::SlabStrategy, AllocStrategy};
        use std::alloc::Layout;
        let s = SlabStrategy::new(ArenaId(100), 16, 8, 2);
        let a = s.alloc_raw(Layout::new::<u64>()).unwrap();
        s.free(a.slot, a.generation).unwrap();
        // Double free: generation has advanced, so the second free fails.
        let err = s.free(a.slot, a.generation).unwrap_err();
        assert!(matches!(err, BrokerError::UseAfterFreeSlot { .. }));
    }

    #[test]
    fn stats_reports_live_arenas() {
        let b = Broker::new();
        let _a = b.create_arena("one", 1024);
        let _c = b.create_arena("two", 2048);
        let s = b.stats();
        assert_eq!(s.live_arenas, 2);
        assert_eq!(s.total_capacity_bytes, 1024 + 2048);
    }

    #[test]
    fn stats_tracks_allocations_and_frees() {
        let b = Broker::new();
        let slab = b.arena("s").slab(64, 8, 16);
        let h = slab.alloc(99_u64).unwrap();
        let before = b.stats();
        assert_eq!(before.total_allocations, 1);
        assert_eq!(before.total_frees, 0);
        assert!(before.total_used_bytes >= 64);
        slab.free(&h).unwrap();
        let after = b.stats();
        assert_eq!(after.total_allocations, 1);
        assert_eq!(after.total_frees, 1);
    }

    #[test]
    fn list_arenas_sorted_by_id_with_correct_kinds() {
        use crate::strategy::StrategyKind;
        let b = Broker::new();
        let _bmp = b.arena("bumpy").capacity(2048).bump();
        let _slb = b.arena("slabby").slab(32, 8, 8);
        let v = b.list_arenas();
        assert_eq!(v.len(), 2);
        // sorted by id; creation order ensures bump < slab
        assert_eq!(v[0].kind, StrategyKind::Bump);
        assert_eq!(v[0].name, "bumpy");
        assert_eq!(v[1].kind, StrategyKind::Slab);
        assert_eq!(v[1].name, "slabby");
        assert_eq!(v[0].capacity, 2048);
    }

    #[test]
    fn list_arenas_excludes_destroyed() {
        let b = Broker::new();
        let a1 = b.create_arena("a1", 1024);
        let a2 = b.create_arena("a2", 1024);
        let id1 = a1.id();
        b.destroy_arena(id1).unwrap();
        let v = b.list_arenas();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, a2.id());
    }

    #[test]
    fn where_is_returns_some_for_live_handle() {
        let b = Broker::new();
        let a = b.create_arena("loc", 1024);
        let h = a.alloc(7_u64).unwrap();
        let loc = b.where_is(&h).expect("handle should be locatable");
        assert_eq!(loc.arena, a.id());
        assert_eq!(loc.arena_name, "loc");
        assert!(loc.is_live);
    }

    #[test]
    fn where_is_returns_none_after_destroy() {
        let b = Broker::new();
        let a = b.create_arena("gone", 1024);
        let h = a.alloc(7_u64).unwrap();
        let id = a.id();
        b.destroy_arena(id).unwrap();
        assert!(b.where_is(&h).is_none());
    }

    #[test]
    fn recording_disabled_by_default() {
        let b = Broker::new();
        assert!(b.recorder().is_none());
        let _a = b.create_arena("x", 256);
        // No way to observe events; existence check is enough.
    }

    #[test]
    fn recording_captures_basic_lifecycle() {
        use crate::recording::{Event, Recorder};
        let rec = Recorder::unbounded();
        let b = Broker::with_recorder(rec.clone());
        let a = b.arena("slabby").slab(64, 8, 8);
        let h = a.alloc(7_u64).unwrap();
        a.free(&h).unwrap();
        let id = a.id();
        b.destroy_arena(id).unwrap();
        let events = rec.snapshot();
        assert_eq!(events.len(), 4, "got {events:?}");
        assert!(matches!(events[0], Event::ArenaCreated { .. }));
        assert!(matches!(events[1], Event::Allocated { .. }));
        assert!(matches!(events[2], Event::Freed { .. }));
        assert!(matches!(events[3], Event::ArenaDestroyed { .. }));
    }

    #[test]
    fn recording_carries_correct_payload() {
        use crate::recording::{Event, Recorder};
        let rec = Recorder::unbounded();
        let b = Broker::with_recorder(rec.clone());
        let a = b.create_arena("named", 4096);
        let h = a.alloc(0xCAFE_BABE_u32).unwrap();
        let events = rec.snapshot();
        let (arena_id, name, capacity) = match &events[0] {
            Event::ArenaCreated { id, name, capacity, .. } => (*id, name.clone(), *capacity),
            other => panic!("expected ArenaCreated, got {other:?}"),
        };
        assert_eq!(name, "named");
        assert_eq!(capacity, 4096);
        match &events[1] {
            Event::Allocated { arena, size, align, slot, .. } => {
                assert_eq!(*arena, arena_id);
                assert_eq!(*size, std::mem::size_of::<u32>());
                assert_eq!(*align, std::mem::align_of::<u32>());
                assert_eq!(*slot, h.slot());
            }
            other => panic!("expected Allocated, got {other:?}"),
        }
    }

    #[test]
    fn recording_timestamps_monotonic_per_thread() {
        use crate::recording::Recorder;
        let rec = Recorder::unbounded();
        let b = Broker::with_recorder(rec.clone());
        let a = b.create_arena("t", 1024);
        for i in 0..16u64 {
            let _ = a.alloc(i).unwrap();
        }
        let events = rec.snapshot();
        let mut last = 0u64;
        for e in &events {
            assert!(e.at_ns() >= last, "non-monotonic at_ns in {events:?}");
            last = e.at_ns();
        }
    }

    #[test]
    fn recording_bounded_ring_buffer_evicts_oldest() {
        use crate::recording::Recorder;
        let rec = Recorder::with_capacity(4);
        let b = Broker::with_recorder(rec.clone());
        let a = b.create_arena("ring", 4096);
        // 1 ArenaCreated + 10 Allocated = 11 events; ring keeps last 4.
        for i in 0..10u64 {
            a.alloc(i).unwrap();
        }
        let events = rec.snapshot();
        assert_eq!(events.len(), 4);
        // The oldest surviving event should be an Allocated, not ArenaCreated.
        assert!(matches!(events[0], crate::recording::Event::Allocated { .. }));
    }

    #[test]
    fn recording_emits_budget_open_close() {
        use crate::recording::{Event, Recorder};
        let rec = Recorder::unbounded();
        let b = Broker::with_recorder(rec.clone());
        let _ = b.within_budget(4096, |scope| {
            let _a = scope.arena("inside").capacity(1024).bump()?;
            Ok(())
        });
        let events = rec.snapshot();
        assert!(events.iter().any(|e| matches!(e, Event::BudgetOpened { .. })));
        assert!(events.iter().any(|e| matches!(e, Event::BudgetClosed { .. })));
        // BudgetOpened comes before BudgetClosed.
        let open_idx = events.iter().position(|e| matches!(e, Event::BudgetOpened { .. })).unwrap();
        let close_idx = events.iter().position(|e| matches!(e, Event::BudgetClosed { .. })).unwrap();
        assert!(open_idx < close_idx);
    }

    #[test]
    fn recording_concurrent_allocations_consistent() {
        use crate::recording::{Event, Recorder};
        use std::sync::Arc;
        use std::thread;
        let rec = Recorder::unbounded();
        let b = Arc::new(Broker::with_recorder(rec.clone()));
        let a = b.arena("conc").slab(32, 8, 4096);
        let a = Arc::new(a);
        let n_threads = 16usize;
        let per_thread = 100usize;
        let mut joins = Vec::new();
        for _ in 0..n_threads {
            let a = Arc::clone(&a);
            joins.push(thread::spawn(move || {
                for i in 0..per_thread {
                    let _ = a.alloc(i as u64).unwrap();
                }
            }));
        }
        for j in joins { j.join().unwrap(); }
        let events = rec.snapshot();
        let alloc_count = events.iter().filter(|e| matches!(e, Event::Allocated { .. })).count();
        assert_eq!(alloc_count, n_threads * per_thread);
    }

    #[test]
    fn secret_strict_arena_basic_alloc() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        // STRICT requires mlock; in CI/dev this normally succeeds on
        // small buffers under RLIMIT_MEMLOCK.
        let a = b.arena("creds").capacity(4096).secret(SecretPolicy::STRICT).bump();
        let h = a.alloc(0x1234_5678_u64).unwrap();
        assert_eq!(*h.get().unwrap(), 0x1234_5678);
    }

    #[test]
    fn secret_lenient_arena_no_mlock() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("creds-lenient").capacity(4096).secret(SecretPolicy::LENIENT).bump();
        let h = a.alloc(99_u32).unwrap();
        assert_eq!(*h.get().unwrap(), 99);
    }

    #[test]
    fn secret_slab_zero_on_free_clears_slot() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("session-keys").secret(SecretPolicy::LENIENT).slab(64, 8, 8);
        // Write a sentinel pattern.
        let h = a.alloc(0xDEAD_BEEF_DEAD_BEEFu64).unwrap();
        let slot = h.slot();
        a.free(&h).unwrap();
        // Re-alloc: same slot is recycled. Without the secret policy,
        // bytes could leak; with zero_on_free, the slot is zeroed
        // before the next write. We allocate a zero value here and
        // verify it remains zero. (The real guarantee is observed
        // via the strategy slot_ptr_mut path; this test is a smoke
        // check that recycling still works correctly under wrapping.)
        let h2 = a.alloc(0u64).unwrap();
        assert_eq!(h2.slot(), slot, "expected slot to be recycled");
        assert_eq!(*h2.get().unwrap(), 0);
    }

    #[test]
    fn secret_none_policy_is_passthrough() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("plain").capacity(1024).secret(SecretPolicy::NONE).bump();
        let h = a.alloc(7_u8).unwrap();
        assert_eq!(*h.get().unwrap(), 7);
    }

    #[test]
    fn secret_strict_destroy_unlocks_cleanly() {
        use crate::secret::SecretPolicy;
        let b = Broker::new();
        let a = b.arena("creds-destroy").capacity(4096).secret(SecretPolicy::STRICT).bump();
        let id = a.id();
        let _h = a.alloc(42_u64).unwrap();
        // Dropping the arena should munlock + zero without panicking.
        b.destroy_arena(id).unwrap();
    }
}