//! # sentinel-broker
//!
//! The runtime memory broker for the Sentinel language. This crate
//! provides the foundational types and arena infrastructure that the
//! rest of the language is built on.
//!
//! ## Current Status
//!
//! Phase A milestone A0+A1+A2: foundation types and the simplest
//! arena. See `HANDOVER.md` for the full plan.
//!
//! ## Core Safety Property
//!
//! The broker's central guarantee is that handles into a dropped arena
//! return a typed error rather than reading poisoned memory. This is
//! enforced by generation tags: every arena slot carries a generation
//! counter, every handle carries the generation it was issued for,
//! and access compares the two.
//!
//! ```
//! use sentinel_broker::{Broker, BrokerError};
//!
//! let broker = Broker::new();
//! let arena = broker.create_arena("example", 4096);
//! let handle = arena.alloc(42_u64).unwrap();
//!
//! // Handle works while arena is live.
//! assert_eq!(*handle.get().unwrap(), 42);
//!
//! // After the arena is dropped, the handle returns a typed error.
//! broker.destroy_arena(arena.id()).unwrap();
//! assert!(matches!(handle.get(), Err(BrokerError::UseAfterFree { .. })));
//! ```

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]
mod arena;
mod broker;
mod error;
mod handle;
mod ids;

pub use arena::Arena;
pub use broker::{Broker, ArenaHandle};
pub use error::BrokerError;
pub use handle::Handle;
pub use ids::{ArenaId, Generation, SlotIndex};