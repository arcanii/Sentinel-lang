//! # sentinel-broker
//!
//! Runtime memory broker for the Sentinel language.
//!
//! ## Status
//!
//! Phase A milestone A3: pluggable allocation strategies via
//! [`AllocStrategy`]. Bump (default) and Slab strategies; arenas are
//! built through the [`Broker::arena`] builder.
//!
//! ```
//! use sentinel_broker::{Broker, BrokerError};
//!
//! let broker = Broker::new();
//! let arena = broker.arena("example").capacity(4096).bump();
//! let handle = arena.alloc(42_u64).unwrap();
//! assert_eq!(*handle.get().unwrap(), 42);
//!
//! broker.destroy_arena(arena.id()).unwrap();
//! assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));
//! ```

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]
mod arena;
mod broker;
mod budget;
pub mod stats;
mod builder;
mod error;
mod handle;
mod ids;
pub mod strategy;

pub use arena::Arena;
pub use broker::{ArenaHandle, Broker};
pub use budget::{Budget, BudgetScope, BudgetArenaBuilder};
pub use stats::{ArenaSummary, BrokerStats, HandleLocation};
pub use builder::ArenaBuilder;
pub use error::BrokerError;
pub use handle::Handle;
pub use ids::{ArenaId, BudgetId, Generation, SlotGeneration, SlotIndex};
pub use strategy::{AllocStrategy, StrategyKind};