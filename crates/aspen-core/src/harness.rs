//! Harness-neutral vocabulary (docs/HARNESSES.md, PROPOSALS-HARNESSES.md
//! §3): which harness a session runs on, what its tools *are* (kinds, not
//! names), what a prompt asks, what answers it takes, what the runtime can
//! do, and what a session's posture is. Every adapter maps into these;
//! nothing above the seam switches on a harness's own names.

use serde::{Deserialize, Serialize};

/// The agent runtime behind a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    #[default]
    Claude,
    Codex,
}

impl Harness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }
    pub fn parse(s: &str) -> Option<Harness> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Harness::Claude),
            "codex" => Some(Harness::Codex),
            _ => None,
        }
    }
    /// Two-letter glyph for badges.
    pub fn glyph(&self) -> &'static str {
        match self {
            Harness::Claude => "cl",
            Harness::Codex => "cx",
        }
    }
}

impl std::fmt::Display for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a tool call *does*, independent of what the harness calls it.
/// Policies, renderers, the artifact index and the activity ledger key on
/// this first and on the harness's name second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Runs a command.
    Shell,
    /// Creates or overwrites a file.
    FileWrite,
    /// Edits a file in place (a patch or a replacement).
    FileEdit,
    /// Reads a file.
    FileRead,
    /// Searches files or content.
    Search,
    /// Reaches the web.
    Web,
    /// Calls an MCP tool.
    Mcp,
    /// Starts or drives a subagent / background work.
    Agent,
    /// Asks the operator a question.
    Question,
    /// Anything else.
    Other,
}

impl ToolKind {
    /// Reads and searches: the "silent tier" a read-only policy allows.
    pub fn is_read_only(&self) -> bool {
        matches!(self, ToolKind::FileRead | ToolKind::Search | ToolKind::Web)
    }
    pub fn writes_files(&self) -> bool {
        matches!(self, ToolKind::FileWrite | ToolKind::FileEdit)
    }
}

/// What a prompt is asking of the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// May the agent do this?
    #[default]
    Permission,
    /// The agent wants an answer (a question, a choice).
    Question,
    /// An MCP server's elicitation.
    Elicitation,
}

/// How far an answer reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionScope {
    #[default]
    Once,
    Session,
    Always,
}

/// One answer a prompt accepts, as the harness offers it. The console
/// renders these as buttons; `id` goes back to the adapter verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    pub allow: bool,
    #[serde(default)]
    pub scope: DecisionScope,
    /// Harness payload the adapter needs to apply this answer (Claude's
    /// `PermissionUpdate[]`, Codex's execpolicy amendment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Aspen's operator-facing posture, one vocabulary across harnesses
/// (PROPOSALS-HARNESSES.md §3.3); each adapter maps it to its own modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Posture {
    /// Prompt for anything that writes, runs or leaves the machine.
    #[default]
    Ask,
    /// Edits go through; commands still prompt.
    Edits,
    /// Read-only exploration.
    Plan,
    /// No prompts at all ("skip permissions").
    Auto,
    /// An automated reviewer answers prompts (where the harness has one).
    Guarded,
}

impl Posture {
    pub fn parse(s: &str) -> Option<Posture> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" | "default" => Some(Posture::Ask),
            "edits" | "acceptedits" => Some(Posture::Edits),
            "plan" => Some(Posture::Plan),
            "auto" | "skip" | "bypasspermissions" | "dontask" => Some(Posture::Auto),
            "guarded" => Some(Posture::Guarded),
            _ => None,
        }
    }
}

/// A harness's own permission mode, as the session's mode select shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionMode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
    /// The posture this mode realizes, if it maps to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posture: Option<Posture>,
}

/// What a harness can do. Every flag gates a control in the console and a
/// method on the handle; a missing capability is shown, never hidden.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessCapabilities {
    pub streaming: bool,
    pub interrupt: bool,
    pub mid_turn_inject: bool,
    pub permission_callback: bool,
    pub in_process_mcp: bool,
    pub resume: bool,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub set_model: bool,
    #[serde(default)]
    pub set_mode: bool,
    #[serde(default)]
    pub context_usage: bool,
    #[serde(default)]
    pub reload: bool,
    #[serde(default)]
    pub slash_commands: bool,
    #[serde(default)]
    pub skill_mentions: bool,
    #[serde(default)]
    pub subagents: bool,
    #[serde(default)]
    pub plugin_dirs: bool,
    #[serde(default)]
    pub replay_ack: bool,
    #[serde(default)]
    pub question_prompts: bool,
    #[serde(default)]
    pub always_allow: bool,
    #[serde(default)]
    pub transcript_on_disk: bool,
    #[serde(default)]
    pub cost_from_harness: bool,
    /// MCP servers (PROPOSALS-MCP.md): status per server, reconnect one,
    /// enable/disable one, start its auth flow, add one to the running
    /// session.
    #[serde(default)]
    pub mcp_status: bool,
    #[serde(default)]
    pub mcp_reconnect: bool,
    #[serde(default)]
    pub mcp_toggle: bool,
    #[serde(default)]
    pub mcp_auth: bool,
    #[serde(default)]
    pub mcp_add: bool,
}

/// An MCP server's state as the harness reports it (PROPOSALS-MCP.md §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpStatus {
    Connected,
    Failed,
    NeedsAuth,
    #[default]
    Pending,
    Disabled,
}

impl McpStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            McpStatus::Connected => "connected",
            McpStatus::Failed => "failed",
            McpStatus::NeedsAuth => "needs_auth",
            McpStatus::Pending => "pending",
            McpStatus::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServerState {
    pub name: String,
    pub status: McpStatus,
    /// The harness's failure text, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// project | user | local | plugin | claudeai | …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// stdio | http | sse | proxy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// stdio: the command line; http/sse: the url.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// serverInfo name/version once connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    /// The plugin that provides it, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
}

/// The answer to "authenticate": a URL the operator opens, when the flow
/// needs one.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub requires_user: bool,
}

/// What the runtime told us about itself: the model in use, the mode, the
/// models and commands/skills it offers, its context window. Built by the
/// adapter from the handshake and the init event.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default)]
    pub models: Vec<serde_json::Value>,
    #[serde(default)]
    pub commands: Vec<serde_json::Value>,
    #[serde(default)]
    pub skills: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// The harness's own handshake/init payloads, for the source view.
    #[serde(default)]
    pub raw: serde_json::Value,
}
