//! aspen-claude — the Claude Code adapter.
//!
//! Speaks the headless NDJSON protocol (see `docs/reference/` in this repo:
//! `CLAUDE_RUNTIME_REFERENCE.md` is the field-verified contract, and every
//! non-obvious choice in this crate cites it). The architecture follows the
//! reference's proven split: a thin, schema-ignorant process layer
//! ([`process`]) and a protocol client that owns all semantics
//! ([`session`]).
//!
//! Invariants honored here, each one a scar in the reference's bug museum:
//! - stdin line discipline: one JSON value per line; a malformed line kills
//!   the child, so the last writer before the pipe validates.
//! - every CLI-initiated `control_request` is answered, including JSON-RPC
//!   *notifications* tunneled over `mcp_message`.
//! - our own control requests carry timeouts; a dead child leaks no promises.
//! - `result` is the only end-of-turn signal.
//! - `--permission-prompt-tool stdio` or the permission surface is dead code.

pub mod broker;
pub mod mcp;
pub mod normalize;
pub mod process;
pub mod session;
pub mod transcript;

pub use broker::{BrokerDecision, DecidedBy, PermissionBroker, PermissionRequest};
pub use session::{ClaudeConfig, ClaudeSession, PermissionPolicy, PolicyBroker};
