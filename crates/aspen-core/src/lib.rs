//! aspen-core — the domain vocabulary shared by every part of the node.
//!
//! Nothing in this crate knows about any particular agent runtime's wire
//! protocol. Adapters (aspen-claude, later others) translate their native
//! protocols into these types; everything above the adapter seam — session
//! manager, bus, API, UI — speaks only this vocabulary.

pub mod adapter;
pub mod bus;
pub mod event;
pub mod ids;

pub use adapter::{AdapterCapabilities, SessionHandle};
pub use bus::{BusMessage, Urgency};
pub use event::SessionEvent;
pub use ids::{AgentName, NodeId, RepoId, SessionId};
