//! The permission broker seam.
//!
//! `can_use_tool` decisions can take milliseconds (policy) or minutes (a
//! human at the console). The session must never block its reader on one, so
//! decisions run in spawned tasks against this trait; the CLI's own prompt
//! can also be cancelled out from under us (`control_cancel_request`), which
//! `cancel` propagates.

use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub suggestions: Value,
    /// The full request payload — `decision_reason`, `blocked_path`,
    /// `agent_id`, whatever the build adds.
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub enum BrokerDecision {
    Allow {
        /// Echo of the input, or a modified one ("edit before run" /
        /// AskUserQuestion answers). REQUIRED by the wire contract.
        updated_input: Value,
        /// Optional `PermissionUpdate[]` ("always allow" rules).
        updated_permissions: Option<Value>,
    },
    Deny {
        /// Shown to the model — a thoughtful message steers it.
        message: String,
    },
}

/// Who settled it — surfaced in events so the UI can distinguish silent
/// policy from operator action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecidedBy {
    Policy,
    Operator,
}

#[async_trait]
pub trait PermissionBroker: Send + Sync {
    async fn decide(&self, req: PermissionRequest) -> (BrokerDecision, DecidedBy);
    /// The CLI no longer needs an answer (it raced a hook, or the turn
    /// ended). Implementations resolve any pending prompt for the id.
    fn cancel(&self, _request_id: &str) {}
}
