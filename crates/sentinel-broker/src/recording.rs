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
        self.inner.lock().map_or_else(|_| Vec::new(), |g| g.clone())
    }

    /// Number of events currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |g| g.len())
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
