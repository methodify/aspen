use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::harness::{Harness, HarnessCapabilities, PermissionMode, Posture, RuntimeInfo};
use crate::ids::SessionId;
use crate::permission::{PermissionBroker, PermissionPolicy};
use crate::store::SessionStore;
use crate::SessionEvent;

/// What a runtime can actually do. Capabilities degrade *honestly*: a
/// runtime without `interrupt` gets `gating` delivered as `normal` and the
/// sender is told so at send time. No silent downgrades, ever.
pub type AdapterCapabilities = HarnessCapabilities;

/// A tool the node offers every session (the bus tools), served in
/// whatever way the harness reaches tools — in-process for Claude, a
/// stdio bridge for Codex.
pub struct ToolDef {
    pub name: &'static str,
    pub description: String,
    pub input_schema: Value,
}

pub trait ToolProvider: Send + Sync {
    fn list(&self) -> Vec<ToolDef>;
    /// Sync and fast by contract; returns the text result or an error text.
    fn call(&self, name: &str, args: Value) -> std::result::Result<String, String>;
}

/// How to start a session, harness-neutrally. Adapters map it to their argv
/// and handshake (PROPOSALS-HARNESSES.md §3.1).
#[derive(Clone)]
pub struct SpawnSpec {
    pub repo: PathBuf,
    /// Aspen's own id for a fresh session (Claude uses it as the session id).
    pub session_id: SessionId,
    pub resume: Option<String>,
    pub fork: bool,
    pub resume_at: Option<String>,
    pub model: Option<String>,
    /// The harness's own mode id, when the operator chose one explicitly.
    pub mode: Option<String>,
    /// Aspen's posture; the adapter maps it when `mode` is absent.
    pub posture: Option<Posture>,
    pub policy: PermissionPolicy,
    pub charter: Option<String>,
    pub extra_args: Vec<String>,
    /// Plugin directories (Claude `--plugin-dir`); ignored by harnesses
    /// without the capability.
    pub plugin_dirs: Vec<String>,
    pub tools: Option<Arc<dyn ToolProvider>>,
    pub broker: Option<Arc<dyn PermissionBroker>>,
    /// The agent's address, for the harness's originator/entrypoint stamp
    /// and the tool bridge.
    pub agent: String,
    /// A token the tool bridge presents to the node (Codex).
    pub bridge_token: Option<String>,
    /// The node's local API address, for the tool bridge.
    pub node_api: Option<String>,
}

/// One harness: how to spawn its sessions and read its store.
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn harness(&self) -> Harness;
    fn capabilities(&self) -> HarnessCapabilities;
    fn permission_modes(&self) -> Vec<PermissionMode>;
    /// The harness's mode id for a posture.
    fn mode_for_posture(&self, posture: Posture) -> Option<String>;
    async fn spawn(&self, spec: SpawnSpec) -> Result<(Arc<dyn SessionHandle>, tokio::sync::mpsc::Receiver<SessionEvent>)>;
    fn store(&self) -> Arc<dyn SessionStore>;
    /// The installed binary's version, if it can be found.
    fn version(&self) -> Option<String>;
    /// The binary name, for inventory.
    fn binary(&self) -> &'static str;
}

/// A capability the handle does not have.
#[derive(Debug, thiserror::Error)]
#[error("not available for {harness} sessions: {what}")]
pub struct Unsupported {
    pub harness: Harness,
    pub what: &'static str,
}

/// The live handle to one running agent session.
///
/// The adapter behind this trait owns every protocol quirk of its runtime;
/// nothing protocol-shaped leaks upward. Events flow out separately (an
/// `mpsc::Receiver<SessionEvent>` returned at spawn) so consumers can be
/// moved freely.
#[async_trait]
pub trait SessionHandle: Send + Sync {
    fn id(&self) -> SessionId;
    fn harness(&self) -> Harness;
    fn capabilities(&self) -> AdapterCapabilities;
    /// What the runtime said about itself (models, commands, mode…), once known.
    fn runtime_info(&self) -> RuntimeInfo;

    /// Send content into the session as user-role input. Returns the wire
    /// uuid of the sent message so callers can correlate the runtime's
    /// delivery ack (`UserReplay`) — the bus trail's proof of ingestion.
    async fn send_user(&self, text: String) -> Result<String>;

    /// Send structured user content — an array of runtime content blocks
    /// (text and images interleaved) — for runtimes that accept it.
    /// Default: not supported.
    async fn send_user_content(&self, content: serde_json::Value) -> Result<String> {
        let _ = content;
        anyhow::bail!("this runtime does not accept structured user content")
    }

    /// Abort the in-flight turn (if the runtime supports it).
    async fn interrupt(&self) -> Result<()>;

    /// Clean shutdown ladder. Must be safe to call on an already-dead
    /// session.
    async fn shutdown(&self) -> Result<()>;

    /// Switch model; takes effect next turn.
    async fn set_model(&self, model: Option<&str>) -> Result<()> {
        let _ = model;
        Err(Unsupported { harness: self.harness(), what: "changing the model" }.into())
    }
    /// Live-switch the permission mode (a harness mode id).
    async fn set_mode(&self, mode: &str) -> Result<()> {
        let _ = mode;
        Err(Unsupported { harness: self.harness(), what: "changing the mode" }.into())
    }
    /// Context usage breakdown, in the harness's own shape.
    async fn context_usage(&self) -> Result<Value> {
        Err(Unsupported { harness: self.harness(), what: "context usage" }.into())
    }
    /// Reload plugins/skills/commands from disk.
    async fn reload(&self) -> Result<Value> {
        Err(Unsupported { harness: self.harness(), what: "reloading plugins" }.into())
    }
}
