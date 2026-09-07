//! Aspen's Codex adapter (docs/HARNESSES.md, docs/CODEX_RUNTIME_REFERENCE.md):
//! `codex app-server --listen stdio://` per session, JSON-RPC over lines,
//! approvals through the neutral broker, rollouts as the session store,
//! and the bus tools reached through the `aspen mcp` stdio bridge.

pub mod adapter;
pub mod normalize;
pub mod rpc;
pub mod session;
pub mod store;

pub use adapter::{capabilities, CodexAdapter, MODES};
pub use session::{CodexConfig, CodexSession};
pub use store::{codex_home, CodexStore};
