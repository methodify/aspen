//! The permission seam, harness-neutral (PROPOSALS-HARNESSES.md §3.3).
//!
//! A runtime asks whether the agent may do something; the answer can take
//! milliseconds (policy) or minutes (a human at the console). Adapters
//! translate their own request shapes into `PermissionRequest` — with the
//! tool's *kind*, the prompt's kind, and the bounded decision set the
//! runtime accepts — and translate `BrokerDecision` back. The silent tier
//! (`PermissionPolicy`) reasons about kinds, never names.

use async_trait::async_trait;
use serde_json::Value;

use crate::harness::{DecisionOption, PromptKind, ToolKind};

/// How the node decides when no operator is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// Reads and searches silently; everything that writes, runs, or leaves
    /// the machine is denied with an honest message.
    ReadOnlyAuto,
    /// Allow everything (dev/smoke use only).
    AllowAll,
}

/// The silent tier: what a policy decides without anyone being asked. None
/// means "no opinion" (prompt-worthy); adapters add their own name-based
/// exceptions (the bus tools, a question tool) before consulting this.
pub fn policy_opinion(policy: PermissionPolicy, kind: ToolKind, prompt: PromptKind) -> Option<bool> {
    match policy {
        PermissionPolicy::AllowAll => Some(true),
        PermissionPolicy::ReadOnlyAuto => {
            // A question with nobody to answer it: allow-with-echo is honest
            // (the model is told the user did not answer).
            if prompt == PromptKind::Question {
                return Some(true);
            }
            Some(kind.is_read_only())
        }
    }
}

pub fn policy_deny_message(tool_name: &str) -> String {
    format!(
        "aspen policy: {tool_name} is not auto-allowed and no operator surface is attached \
         to approve it. Proceed without this tool, or tell the operator what you need."
    )
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub kind: ToolKind,
    pub prompt: PromptKind,
    pub input: Value,
    /// The harness's own suggestions payload (Claude `permission_suggestions`).
    pub suggestions: Value,
    /// The answers this prompt accepts, in the order to show them.
    pub decisions: Vec<DecisionOption>,
    /// For questions: the harness's question payload.
    pub questions: Value,
    /// The full request payload, untouched.
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub enum BrokerDecision {
    Allow {
        /// Echo of the input, or a modified one (answers ride here for
        /// question prompts).
        updated_input: Value,
        /// The harness payload of a broader grant ("always allow").
        updated_permissions: Option<Value>,
        /// Which offered decision this was, when the operator chose one.
        decision_id: Option<String>,
    },
    Deny {
        /// Shown to the model — a thoughtful message steers it.
        message: String,
        decision_id: Option<String>,
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
    /// The runtime no longer needs an answer (it raced a hook, the turn
    /// ended, another client answered). Implementations resolve any pending
    /// prompt for the id.
    fn cancel(&self, _request_id: &str) {}
}

/// Answers purely from policy — the headless default.
pub struct PolicyBroker(pub PermissionPolicy);

#[async_trait]
impl PermissionBroker for PolicyBroker {
    async fn decide(&self, req: PermissionRequest) -> (BrokerDecision, DecidedBy) {
        let allow = policy_opinion(self.0, req.kind, req.prompt).unwrap_or(false);
        let d = if allow {
            BrokerDecision::Allow {
                updated_input: req.input,
                updated_permissions: None,
                decision_id: None,
            }
        } else {
            BrokerDecision::Deny {
                message: policy_deny_message(&req.tool_name),
                decision_id: None,
            }
        };
        (d, DecidedBy::Policy)
    }
}
