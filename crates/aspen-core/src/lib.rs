//! aspen-core — the domain vocabulary shared by every part of the node.
//!
//! Nothing in this crate knows about any particular agent runtime's wire
//! protocol. Adapters (aspen-claude, later others) translate their native
//! protocols into these types; everything above the adapter seam — session
//! manager, bus, API, UI — speaks only this vocabulary.

pub mod adapter;
pub mod bus;
pub mod event;
pub mod harness;
pub mod ids;
pub mod permission;
pub mod process;
pub mod store;

pub use adapter::{AdapterCapabilities, AgentAdapter, SessionHandle, SpawnSpec, ToolDef, ToolProvider, Unsupported};
pub use harness::{DecisionOption, DecisionScope, Harness, HarnessCapabilities, McpAuth, McpServerState, McpStatus, PermissionMode, Posture, PromptKind, RuntimeInfo, ToolKind};
pub use permission::{BrokerDecision, DecidedBy, PermissionBroker, PermissionPolicy, PermissionRequest, PolicyBroker};
pub use store::{ProjectDirs, SessionInfo, SessionOrigin, SessionStore};
pub use bus::{BusMessage, Urgency};
pub use event::SessionEvent;
pub use ids::{AgentName, NodeId, RepoId, SessionId};
pub use process::quiet_command;
