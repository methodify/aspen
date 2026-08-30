use serde::Serialize;
use serde_json::Value;

/// The normalized event vocabulary every adapter translates into.
///
/// Deliberately conservative: strong fields only where the hub layer *acts*
/// on a value; everything else rides in `raw` so no adapter detail is lost
/// and forward compatibility costs nothing (unknown upstream frames become
/// `Raw`, never errors).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The runtime's startup inventory arrived (Claude: `system/init` — note
    /// it arrives with the first turn, not the handshake).
    RuntimeInit {
        session_id: String,
        model: Option<String>,
        raw: Value,
    },
    /// Token-level streaming text.
    TextDelta { text: String, thinking: bool },
    /// A block-level assistant snapshot (UI performs the snapshot-reconciling
    /// merge; adapters must not).
    AssistantMessage { message_id: Option<String>, raw: Value },
    /// The model invoked a tool.
    ToolUse {
        tool_use_id: String,
        tool_name: String,
        input: Value,
        parent_tool_use_id: Option<String>,
    },
    /// A tool's result came back.
    ToolResult { tool_use_id: Option<String>, raw: Value },
    /// A user message we sent was accepted by the runtime (delivery ack).
    UserReplay { uuid: String },
    /// The runtime asked permission for a tool call and hub's policy did not
    /// decide it silently — an operator decision is wanted.
    PermissionAsked {
        request_id: String,
        tool_name: String,
        input: Value,
        suggestions: Value,
    },
    /// How a permission request settled (by policy or operator).
    PermissionSettled {
        request_id: String,
        tool_name: String,
        allowed: bool,
        by_policy: bool,
    },
    /// The turn ended. The only authoritative "idle again" signal.
    TurnEnded {
        subtype: String,
        duration_ms: Option<u64>,
        total_cost_usd: Option<f64>,
        result_text: Option<String>,
        raw: Value,
    },
    /// Runtime status traffic worth mirroring (mode changes, compaction…).
    Status { raw: Value },
    /// A line from the runtime's stderr — the debug channel.
    Stderr { line: String },
    /// Anything we do not (yet) interpret. Never dropped, never an error.
    Raw { raw: Value },
    /// The runtime process exited.
    Exited { code: Option<i32> },
}
