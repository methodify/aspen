use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

use crate::ids::SessionId;

/// What a runtime can actually do. Capabilities degrade *honestly*: a
/// runtime without `interrupt` gets `gating` delivered as `normal` and the
/// sender is told so at send time. No silent downgrades, ever.
#[derive(Debug, Clone, Serialize)]
pub struct AdapterCapabilities {
    pub streaming: bool,
    pub interrupt: bool,
    pub mid_turn_inject: bool,
    pub permission_callback: bool,
    pub in_process_mcp: bool,
    pub resume: bool,
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
    fn capabilities(&self) -> AdapterCapabilities;

    /// Send content into the session as user-role input. Returns the wire
    /// uuid of the sent message so callers can correlate the runtime's
    /// delivery ack (`UserReplay`) — the bus trail's proof of ingestion.
    async fn send_user(&self, text: String) -> Result<String>;

    /// Abort the in-flight turn (if the runtime supports it).
    async fn interrupt(&self) -> Result<()>;

    /// Clean shutdown ladder. Must be safe to call on an already-dead
    /// session.
    async fn shutdown(&self) -> Result<()>;
}
