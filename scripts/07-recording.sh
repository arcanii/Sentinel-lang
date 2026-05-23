#!/usr/bin/env bash
set -euo pipefail

SENTINEL_ROOT="/Users/bryan/Desktop/github_repos/Sentinel-language"
BROKER="$SENTINEL_ROOT/crates/sentinel-broker"
cd "$SENTINEL_ROOT"

# -----------------------------------------------------------------------------
# 1) Write src/recording.rs
# -----------------------------------------------------------------------------
echo "======"
echo "WRITING src/recording.rs"
echo "======"
cat > "$BROKER/src/recording.rs" <<'RS'
//! Recording mode — capture an ordered log of broker events for
//! replay, audit, and debugging.
//!
//! Recording is **opt-in** and disabled by default. Construct a
//! [`Recorder`] and pass it to [`Broker::with_recorder`]; every
//! event-emitting site in the broker will then push an [`Event`] to
//! the recorder.
//!
//! Two modes are supported:
//! - **Unbounded** (`Recorder::unbounded`): grows freely; good for
//!   tests and short-running programs.
//! - **Bounded ring buffer** (`Recorder::with_capacity(n)`): keeps the
//!   last `n` events, dropping the oldest. Suitable for long-running
//!   processes where you want bounded memory.
//!
//! ```ignore
//! let rec = Recorder::unbounded();
//! let broker = Broker::with_recorder(Arc::clone(&rec));
//! let arena = broker.create_arena("a", 1024);
//! let _h = arena.alloc(42_u64)?;
//! let events = rec.snapshot();
//! assert_eq!(events.len(), 2); // ArenaCreated + Allocated
//! ```
//!
//! [`Broker::with_recorder`]: crate::Broker::with_recorder

use crate::ids::{ArenaId, BudgetId, SlotGeneration, SlotIndex};
use crate::strategy::StrategyKind;
use std::sync::Mutex;
use std::time::Instant;

/// A recorded broker event.
///
/// Timestamps (`at_ns`) are monotonic nanoseconds since the recorder
/// was constructed. They are intended for ordering and rough timing,
/// not wall-clock correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ArenaCreated {
        id: ArenaId,
        name: String,
        kind: StrategyKind,
        capacity: usize,
        at_ns: u64,
    },
    ArenaDestroyed {
        id: ArenaId,
        at_ns: u64,
    },
    Allocated {
        arena: ArenaId,
        slot: SlotIndex,
        slot_generation: SlotGeneration,
        size: usize,
        align: usize,
        at_ns: u64,
    },
    Freed {
        arena: ArenaId,
        slot: SlotIndex,
        slot_generation: SlotGeneration,
        at_ns: u64,
    },
    BudgetOpened {
        id: BudgetId,
        cap: usize,
        parent: Option<BudgetId>,
        at_ns: u64,
    },
    BudgetClosed {
        id: BudgetId,
        used_at_close: usize,
        at_ns: u64,
    },
}

impl Event {
    /// Monotonic nanoseconds since recorder start.
    #[must_use]
    pub const fn at_ns(&self) -> u64 {
        match self {
            Event::ArenaCreated   { at_ns, .. }
            | Event::ArenaDestroyed { at_ns, .. }
            | Event::Allocated    { at_ns, .. }
            | Event::Freed        { at_ns, .. }
            | Event::BudgetOpened { at_ns, .. }
            | Event::BudgetClosed { at_ns, .. } => *at_ns,
        }
    }
}

/// Storage mode for a [`Recorder`].
#[derive(Debug, Clone, Copy)]
enum Mode {
    Unbounded,
    Bounded(usize),
}

/// Captures broker events in order.
///
/// `Recorder` is itself thread-safe: it holds an internal mutex
/// over the event buffer. Wrap it in an `Arc` and share freely.
pub struct Recorder {
    started: Instant,
    mode: Mode,
    inner: Mutex<Vec<Event>>,
}

impl Recorder {
    /// Construct an unbounded recorder. Events accumulate until the
    /// `Arc<Recorder>` is dropped.
    #[must_use]
    pub fn unbounded() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            started: Instant::now(),
            mode: Mode::Unbounded,
            inner: Mutex::new(Vec::new()),
        })
    }

    /// Construct a bounded ring-buffer recorder. After `capacity`
    /// events, each new event evicts the oldest.
    ///
    /// # Panics
    /// Panics if `capacity == 0`.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> std::sync::Arc<Self> {
        assert!(capacity > 0, "Recorder::with_capacity requires capacity > 0");
        std::sync::Arc::new(Self {
            started: Instant::now(),
            mode: Mode::Bounded(capacity),
            inner: Mutex::new(Vec::with_capacity(capacity)),
        })
    }

    /// Monotonic nanoseconds since recorder start. Used by event
    /// emission sites; you can also call it directly.
    #[must_use]
    pub fn now_ns(&self) -> u64 {
        let elapsed = self.started.elapsed();
        // saturate at u64::MAX for ~584 years of runtime; fine.
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    }

    /// Append an event. Best-effort: if the internal mutex is
    /// poisoned, the event is silently dropped (recording must not
    /// take down the broker).
    pub fn record(&self, event: Event) {
        let Ok(mut buf) = self.inner.lock() else { return };
        match self.mode {
            Mode::Unbounded => buf.push(event),
            Mode::Bounded(cap) => {
                if buf.len() >= cap {
                    // Evict oldest. For small caps this is O(n) but
                    // simple; a VecDeque would be faster for large caps.
                    buf.remove(0);
                }
                buf.push(event);
            }
        }
    }

    /// Snapshot the current event buffer (clone).
    #[must_use]
    pub fn snapshot(&self) -> Vec<Event> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Number of events currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// `true` if no events have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear the buffer. Useful between test phases.
    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("mode", &self.mode)
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}
RS
echo "[OK] wrote src/recording.rs"

# -----------------------------------------------------------------------------
# 2) Patch lib.rs: declare recording module + re-exports
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/lib.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/lib.rs")
src = p.read_text()
changed = False

if "pub mod recording;" not in src:
    # Insert near other module declarations.
    if "pub mod stats;" in src:
        src = src.replace("pub mod stats;", "pub mod stats;\npub mod recording;", 1)
    elif "pub mod budget;" in src:
        src = src.replace("pub mod budget;", "pub mod budget;\npub mod recording;", 1)
    else:
        src += "\npub mod recording;\n"
    changed = True
    print("[OK] declared pub mod recording")

if "pub use recording::" not in src:
    if "pub use stats::" in src:
        src = re.sub(
            r"(pub use stats::[^\n]+\n)",
            r"\1pub use recording::{Event, Recorder};\n",
            src,
            count=1,
        )
    else:
        if not src.endswith("\n"):
            src += "\n"
        src += "pub use recording::{Event, Recorder};\n"
    changed = True
    print("[OK] added pub use recording::{Event, Recorder}")

if changed:
    p.write_text(src)
PY

# -----------------------------------------------------------------------------
# 3) Patch broker.rs: add recorder field, with_recorder constructor,
#    emit ArenaCreated / ArenaDestroyed, pass recorder into Arena.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/broker.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()
changed = False

# Imports
if "use crate::recording::" not in src:
    m = re.search(r"(use crate::stats::[^\n]+\n)", src)
    line = "use crate::recording::{Event, Recorder};\n"
    if m:
        src = src.replace(m.group(1), m.group(1) + line, 1)
    else:
        # Insert after first `use crate::`
        m2 = re.search(r"(use crate::[^\n]+\n)", src)
        if m2:
            src = src.replace(m2.group(1), m2.group(1) + line, 1)
        else:
            src = line + src
    changed = True
    print("[OK] imported Event + Recorder")

# Add recorder field to Broker struct
m = re.search(r"pub struct Broker \{([^}]*)\}", src, re.DOTALL)
if m and "recorder:" not in m.group(1):
    body = m.group(1).rstrip()
    if not body.endswith(","):
        body += ","
    new_body = body + "\n    recorder: Option<Arc<Recorder>>,\n"
    src = src.replace(m.group(0), f"pub struct Broker {{{new_body}}}", 1)
    changed = True
    print("[OK] added recorder field to Broker")

# Initialize recorder: None in Self { ... } inside Broker::new()
# Find the Self { ... } block containing budget_ids (the Broker constructor).
def inject_recorder_none(src: str) -> tuple[str, bool]:
    pattern = re.compile(r"Self\s*\{([^{}]*?)\}", re.DOTALL)
    for m in pattern.finditer(src):
        body = m.group(1)
        if "budget_ids" in body and "recorder" not in body:
            new_body = body.rstrip()
            if not new_body.endswith(","):
                new_body += ","
            new_body += "\n            recorder: None,\n        "
            return src.replace(m.group(0), f"Self {{{new_body}}}", 1), True
    return src, False

if "recorder: None" not in src and "recorder: Some" not in src:
    src, ok = inject_recorder_none(src)
    if ok:
        changed = True
        print("[OK] initialized recorder: None in Broker::new()")
    else:
        print("[WARN] could not locate Broker::new() Self {} block")

# Add `with_recorder` constructor + accessor.
if "pub fn with_recorder" not in src:
    # Find end of `impl Default for Broker { ... }` to anchor — actually easier:
    # insert right before `impl Default for Broker {`.
    anchor = "impl Default for Broker"
    if anchor in src:
        addition = """
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

"""
        src = src.replace(anchor, addition + anchor, 1)
        changed = True
        print("[OK] added Broker::with_recorder + recorder() accessor")
    else:
        print("[WARN] could not find `impl Default for Broker` anchor")

# Emit ArenaCreated in register_arena (best site — all paths funnel through it).
# Find `pub(crate) fn register_arena` and inject an event push after the insert.
if "Event::ArenaCreated" not in src:
    m = re.search(r"(pub\(crate\) fn register_arena[^{]*\{[^}]*\})", src, re.DOTALL)
    if m:
        block = m.group(1)
        if "tracing::debug!(arena_id = %id, \"arena registered\");" in block:
            new_block = block.replace(
                "tracing::debug!(arena_id = %id, \"arena registered\");",
                """tracing::debug!(arena_id = %id, "arena registered");
        if let Some(r) = &self.recorder {
            r.record(Event::ArenaCreated {
                id: arena.id(),
                name: arena.name().to_string(),
                kind: arena.strategy_kind(),
                capacity: arena.capacity(),
                at_ns: r.now_ns(),
            });
        }""",
                1,
            )
            src = src.replace(block, new_block, 1)
            changed = True
            print("[OK] emit Event::ArenaCreated from register_arena")
        else:
            print("[WARN] register_arena body shape unexpected; ArenaCreated not emitted")
    else:
        print("[WARN] could not find register_arena")

# Emit ArenaDestroyed in destroy_arena, just before/after arena.invalidate().
if "Event::ArenaDestroyed" not in src:
    needle = """arena.invalidate();
                tracing::debug!(arena_id = %id, \"arena destroyed\");
                Ok(())"""
    if needle in src:
        replacement = """arena.invalidate();
                if let Some(r) = &self.recorder {
                    r.record(Event::ArenaDestroyed { id, at_ns: r.now_ns() });
                }
                tracing::debug!(arena_id = %id, "arena destroyed");
                Ok(())"""
        src = src.replace(needle, replacement, 1)
        changed = True
        print("[OK] emit Event::ArenaDestroyed in destroy_arena")
    else:
        print("[WARN] destroy_arena body shape unexpected; ArenaDestroyed not emitted")

# Provide a pub(crate) helper to fetch the recorder for arenas / builder.
if "pub(crate) fn recorder_arc(&self)" not in src:
    # Insert near next_arena_id
    if "pub(crate) fn next_arena_id" in src:
        src = src.replace(
            "pub(crate) fn next_arena_id(&self) -> ArenaId {",
            "pub(crate) fn recorder_arc(&self) -> Option<Arc<Recorder>> {\n        self.recorder.clone()\n    }\n\n    pub(crate) fn next_arena_id(&self) -> ArenaId {",
            1,
        )
        changed = True
        print("[OK] added pub(crate) recorder_arc accessor")

if changed:
    p.write_text(src)
    print("[OK] broker.rs updated")
else:
    print("[SKIP] broker.rs unchanged")
PY

# -----------------------------------------------------------------------------
# 4) Patch arena.rs: optional recorder field, plumb through with_strategy,
#    emit Allocated / Freed.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/arena.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/arena.rs")
src = p.read_text()
changed = False

# Add import for recording types.
if "use crate::recording::" not in src:
    # Insert after the first `use crate::` import.
    m = re.search(r"(use crate::[^\n]+\n)", src)
    line = "use crate::recording::{Event, Recorder};\n"
    if m:
        src = src.replace(m.group(1), m.group(1) + line, 1)
        changed = True
        print("[OK] imported Event + Recorder in arena.rs")
    else:
        print("[WARN] no `use crate::` import in arena.rs; adding at top")
        src = line + src
        changed = True

# Add recorder field to Arena struct.
m = re.search(r"pub struct Arena \{([^}]*)\}", src, re.DOTALL)
if m and "recorder:" not in m.group(1):
    body = m.group(1).rstrip()
    if not body.endswith(","):
        body += ","
    new_body = body + "\n    recorder: Option<Arc<Recorder>>,\n"
    src = src.replace(m.group(0), f"pub struct Arena {{{new_body}}}", 1)
    changed = True
    print("[OK] added recorder field to Arena")

# Locate Arena::with_strategy and add a second constructor that accepts a
# recorder; rewrite existing one to forward.
# Strategy: find `pub fn with_strategy(...)` signature and body.
m = re.search(
    r"pub fn with_strategy\([^)]*\) -> Arc<Self> \{([^}]*)\}",
    src,
    re.DOTALL,
)
if m and "recorder: None" not in m.group(0) and "recorder: Some" not in m.group(0):
    body = m.group(0)
    # Inject `recorder: None,` into the Self { ... } literal inside with_strategy.
    new_body = re.sub(
        r"(Self\s*\{[^{}]*?)\}",
        lambda mm: mm.group(1).rstrip().rstrip(",") + ",\n            recorder: None,\n        }",
        body,
        count=1,
        flags=re.DOTALL,
    )
    if new_body != body:
        src = src.replace(body, new_body, 1)
        changed = True
        print("[OK] initialized recorder: None in with_strategy()")

# Add a new constructor `with_strategy_and_recorder`.
if "pub fn with_strategy_and_recorder" not in src:
    # Append after the `with_strategy` method ends.
    m = re.search(
        r"(pub fn with_strategy\([^)]*\) -> Arc<Self> \{[^}]*\})",
        src,
        re.DOTALL,
    )
    if m:
        addition = m.group(1) + """

    /// Same as `with_strategy`, but attaches a recorder.
    #[must_use]
    pub fn with_strategy_and_recorder(
        id: ArenaId,
        name: &str,
        strategy: Box<dyn crate::strategy::AllocStrategy>,
        recorder: Option<Arc<Recorder>>,
    ) -> Arc<Self> {
        let a = Self::with_strategy(id, name, strategy);
        // SAFETY: we're the only owner of the Arc right now, and Arena
        // is not Sync until we hand it out. Pry into the Arc to set
        // the recorder. Easier: build the Arc with recorder set.
        // We instead construct a fresh Arc via Arc::get_mut.
        let mut a = a;
        if let Some(arena) = Arc::get_mut(&mut a) {
            arena.recorder = recorder;
        }
        a
    }"""
        src = src.replace(m.group(1), addition, 1)
        changed = True
        print("[OK] added Arena::with_strategy_and_recorder")

# Emit Allocated event in alloc<T>: after alloc_count bump, before Ok(Handle::new(...)).
if "Event::Allocated" not in src:
    needle = """self.alloc_count.fetch_add(1, Ordering::Relaxed);
        Ok(Handle::new("""
    if needle in src:
        replacement = """self.alloc_count.fetch_add(1, Ordering::Relaxed);
        if let Some(r) = &self.recorder {
            r.record(Event::Allocated {
                arena: self.id,
                slot: allocated.slot,
                slot_generation: allocated.generation,
                size: layout.size(),
                align: layout.align(),
                at_ns: r.now_ns(),
            });
        }
        Ok(Handle::new("""
        src = src.replace(needle, replacement, 1)
        changed = True
        print("[OK] emit Event::Allocated in alloc<T>")
    else:
        print("[WARN] alloc<T> body shape unexpected; Allocated not emitted")

# Emit Freed event in free<T>: after free_count bump.
if "Event::Freed" not in src:
    needle = """let r = self.strategy.free(handle.slot, handle.slot_generation);
        if r.is_ok() {
            self.free_count.fetch_add(1, Ordering::Relaxed);
        }
        r"""
    if needle in src:
        replacement = """let res = self.strategy.free(handle.slot, handle.slot_generation);
        if res.is_ok() {
            self.free_count.fetch_add(1, Ordering::Relaxed);
            if let Some(rec) = &self.recorder {
                rec.record(Event::Freed {
                    arena: self.id,
                    slot: handle.slot,
                    slot_generation: handle.slot_generation,
                    at_ns: rec.now_ns(),
                });
            }
        }
        res"""
        src = src.replace(needle, replacement, 1)
        changed = True
        print("[OK] emit Event::Freed in free<T>")
    else:
        print("[WARN] free<T> body shape unexpected; Freed not emitted")

if changed:
    p.write_text(src)
    print("[OK] arena.rs updated")
else:
    print("[SKIP] arena.rs unchanged")
PY

# -----------------------------------------------------------------------------
# 5) Patch builder.rs: when constructing an Arena, pass recorder along.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/builder.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/builder.rs")
src = p.read_text()
changed = False

# Replace any `Arena::with_strategy(id, &self.name, strategy)` calls with
# `Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc())`.
old = "Arena::with_strategy(id, &self.name, strategy)"
new = "Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc())"
if old in src:
    src = src.replace(old, new)
    changed = True
    print("[OK] threaded recorder into builder Arena construction")
else:
    # Try qualified path variant
    old2 = "crate::arena::Arena::with_strategy(id, &self.name, strategy)"
    new2 = "crate::arena::Arena::with_strategy_and_recorder(id, &self.name, strategy, self.broker.recorder_arc())"
    if old2 in src:
        src = src.replace(old2, new2)
        changed = True
        print("[OK] threaded recorder into builder Arena construction (qualified)")
    else:
        print("[SKIP] no with_strategy call site found in builder.rs (already patched or different shape)")

if changed:
    p.write_text(src)
PY

# -----------------------------------------------------------------------------
# 6) Patch budget.rs: thread recorder into the budget arena builder,
#    and emit BudgetOpened / BudgetClosed.
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/budget.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/budget.rs")
src = p.read_text()
changed = False

# Add recording import.
if "use crate::recording::" not in src:
    m = re.search(r"(use crate::[^\n]+\n)", src)
    line = "use crate::recording::Event;\n"
    if m:
        src = src.replace(m.group(1), m.group(1) + line, 1)
        changed = True
        print("[OK] imported Event in budget.rs")

# Replace `Arena::with_strategy(...)` calls (qualified or not) in budget.rs.
patterns = [
    ("crate::arena::Arena::with_strategy(id, &self.name, strategy)",
     "crate::arena::Arena::with_strategy_and_recorder(id, &self.name, strategy, self.scope.broker.recorder_arc())"),
    ("Arena::with_strategy(id, &self.name, strategy)",
     "Arena::with_strategy_and_recorder(id, &self.name, strategy, self.scope.broker.recorder_arc())"),
]
for old, new in patterns:
    if old in src:
        src = src.replace(old, new)
        changed = True
        print(f"[OK] threaded recorder into budget builder ({old[:40]}...)")

# Emit BudgetOpened/Closed around `within_budget` closures on both
# Broker::within_budget AND BudgetScope::within_budget.
# We implement this via a small wrapper:
#   1) build the inner Budget
#   2) record BudgetOpened
#   3) run f
#   4) record BudgetClosed with budget.used()
#   5) return result
# We must touch the inner-scope method here in budget.rs. Broker's top-level
# version is patched in broker.rs below.
if "Event::BudgetOpened" not in src:
    needle = """pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
    where
        F: FnOnce(&BudgetScope<'_>) -> Result<R, BrokerError>,
    {
        let id = self.broker.next_budget_id();
        let inner = Budget::new(id, cap, Some(Arc::clone(&self.budget)));
        let scope = BudgetScope::new(self.broker, inner);
        f(&scope)
    }"""
    replacement = """pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
    where
        F: FnOnce(&BudgetScope<'_>) -> Result<R, BrokerError>,
    {
        let id = self.broker.next_budget_id();
        let inner = Budget::new(id, cap, Some(Arc::clone(&self.budget)));
        if let Some(r) = self.broker.recorder_arc() {
            r.record(Event::BudgetOpened {
                id,
                cap,
                parent: Some(self.budget.id()),
                at_ns: r.now_ns(),
            });
        }
        let scope = BudgetScope::new(self.broker, inner);
        let result = f(&scope);
        if let Some(r) = self.broker.recorder_arc() {
            r.record(Event::BudgetClosed {
                id,
                used_at_close: scope.budget().used(),
                at_ns: r.now_ns(),
            });
        }
        result
    }"""
    if needle in src:
        src = src.replace(needle, replacement, 1)
        changed = True
        print("[OK] emit BudgetOpened/Closed in BudgetScope::within_budget")
    else:
        print("[WARN] inner within_budget shape unexpected in budget.rs")

if changed:
    p.write_text(src)
    print("[OK] budget.rs updated")
else:
    print("[SKIP] budget.rs unchanged")
PY

# -----------------------------------------------------------------------------
# 7) Patch broker.rs (again): emit BudgetOpened/Closed in top-level within_budget
# -----------------------------------------------------------------------------
echo
echo "======"
echo "PATCHING src/broker.rs (budget events)"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()

if "Event::BudgetOpened" in src:
    print("[SKIP] broker.rs already emits BudgetOpened")
else:
    needle = """pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
    where
        F: FnOnce(&BudgetScope<'_>) -> Result<R, BrokerError>,
    {
        let id = self.next_budget_id();
        let budget = Budget::new(id, cap, None);
        let scope = BudgetScope::new(self, budget);
        f(&scope)
    }"""
    replacement = """pub fn within_budget<R, F>(&self, cap: usize, f: F) -> Result<R, BrokerError>
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
    }"""
    if needle in src:
        src = src.replace(needle, replacement, 1)
        p.write_text(src)
        print("[OK] emit BudgetOpened/Closed in Broker::within_budget")
    else:
        print("[WARN] broker.rs within_budget shape unexpected")
PY

# -----------------------------------------------------------------------------
# 8) Append recording tests to broker.rs
# -----------------------------------------------------------------------------
echo
echo "======"
echo "APPENDING TESTS to src/broker.rs"
echo "======"
python3 - <<'PY'
from pathlib import Path
import re
p = Path("/Users/bryan/Desktop/github_repos/Sentinel-language/crates/sentinel-broker/src/broker.rs")
src = p.read_text()

if "fn recording_disabled_by_default" in src:
    print("[SKIP] recording tests already present")
else:
    new_tests = '''
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
        let h = a.alloc(0xCAFEBABEu32).unwrap();
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
'''
    m = re.search(r"#\[cfg\(test\)\]\s*mod tests \{", src)
    if not m:
        print("[WARN] no #[cfg(test)] mod tests block found")
    else:
        start = m.end()
        depth = 1
        i = start
        close = None
        while i < len(src):
            if src[i] == '{': depth += 1
            elif src[i] == '}':
                depth -= 1
                if depth == 0:
                    close = i
                    break
            i += 1
        if close is None:
            print("[WARN] could not find end of mod tests")
        else:
            src = src[:close] + new_tests + src[close:]
            p.write_text(src)
            print("[OK] appended 7 recording tests")
PY

# -----------------------------------------------------------------------------
# 9) Build / clippy / tests / doc tests
# -----------------------------------------------------------------------------
echo
echo "======"
echo "BUILD"
echo "======"
cargo build -p sentinel-broker 2>&1 | tail -n 25

echo
echo "======"
echo "CLIPPY"
echo "======"
cargo clippy -p sentinel-broker --all-targets -- -D warnings 2>&1 | tail -n 40

echo
echo "======"
echo "TESTS (nextest)"
echo "======"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-broker 2>&1 | tail -n 60
else
  cargo test -p sentinel-broker 2>&1 | tail -n 60
fi

echo
echo "======"
echo "DOC TESTS"
echo "======"
cargo test -p sentinel-broker --doc 2>&1 | tail -n 10

echo
echo "======"
echo "DONE"
echo "======"
