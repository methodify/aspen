//! The operator permission broker: silent tier by policy, everything else
//! becomes a `PermissionAsked` event held open until the console answers,
//! the CLI cancels, or a long timeout denies honestly.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};

use aspen_claude::broker::{BrokerDecision, DecidedBy, PermissionBroker, PermissionRequest};
use aspen_claude::session::{policy_opinion, PermissionPolicy};
use aspen_core::SessionEvent;

/// How long a prompt stays open before an honest deny. Generous: an operator
/// console can sit unattended, and the agent is told exactly what happened.
const OPERATOR_TIMEOUT: Duration = Duration::from_secs(30 * 60);

struct PendingPrompt {
    tool_name: String,
    original_input: Value,
    suggestions: Value,
    asked_at_epoch: f64,
    answer: oneshot::Sender<BrokerDecision>,
}

/// A snapshot of one open prompt, for inbox aggregation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub suggestions: Value,
    pub asked_at: f64,
    /// AskUserQuestion prompts are questions, not approvals.
    pub is_question: bool,
}

pub struct OperatorBroker {
    policy: PermissionPolicy,
    pending: Mutex<HashMap<String, PendingPrompt>>,
    /// Set once the session's broadcast exists (spawn wiring order).
    events: Mutex<Option<broadcast::Sender<SessionEvent>>>,
}

impl OperatorBroker {
    pub fn new(policy: PermissionPolicy) -> Self {
        Self {
            policy,
            pending: Mutex::new(HashMap::new()),
            events: Mutex::new(None),
        }
    }

    pub fn attach_events(&self, tx: broadcast::Sender<SessionEvent>) {
        *self.events.lock().unwrap() = Some(tx);
    }

    /// Console answer path. Returns false if the prompt is no longer open
    /// (answered elsewhere, cancelled, or timed out). `updated_permissions`
    /// carries "always allow" rules — echo the CLI's own suggestions back
    /// (reference §7.4); malformed entries degrade to allow-once.
    pub fn answer(
        &self,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<Value>,
        updated_permissions: Option<Value>,
    ) -> bool {
        let Some(p) = self.pending.lock().unwrap().remove(request_id) else {
            return false;
        };
        let decision = if allow {
            BrokerDecision::Allow {
                updated_input: updated_input.unwrap_or(p.original_input),
                updated_permissions: updated_permissions.filter(|v| !v.is_null()),
            }
        } else {
            BrokerDecision::Deny {
                message: message
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| "The operator declined this action.".into()),
            }
        };
        p.answer.send(decision).is_ok()
    }

    /// Every prompt currently held open, oldest first.
    pub fn open_prompts(&self) -> Vec<OpenPrompt> {
        let pending = self.pending.lock().unwrap();
        let mut out: Vec<OpenPrompt> = pending
            .iter()
            .map(|(id, p)| OpenPrompt {
                request_id: id.clone(),
                tool_name: p.tool_name.clone(),
                input: p.original_input.clone(),
                suggestions: p.suggestions.clone(),
                asked_at: p.asked_at_epoch,
                is_question: p.tool_name == "AskUserQuestion"
                    || p.original_input.get("questions").is_some(),
            })
            .collect();
        out.sort_by(|a, b| a.asked_at.total_cmp(&b.asked_at));
        out
    }

    fn emit(&self, ev: SessionEvent) {
        if let Some(tx) = self.events.lock().unwrap().as_ref() {
            let _ = tx.send(ev);
        }
    }
}

#[async_trait]
impl PermissionBroker for OperatorBroker {
    async fn decide(&self, req: PermissionRequest) -> (BrokerDecision, DecidedBy) {
        // Questions always reach the operator — a silently "allowed"
        // question is a question nobody answered (reference §7.6).
        let is_question = req.tool_name == "AskUserQuestion"
            || req
                .raw
                .get("input")
                .and_then(|i| i.get("questions"))
                .is_some();
        if !is_question && policy_opinion(self.policy, &req.tool_name, &req.raw) == Some(true) {
            return (
                BrokerDecision::Allow {
                    updated_input: req.input,
                    updated_permissions: None,
                },
                DecidedBy::Policy,
            );
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(
            req.request_id.clone(),
            PendingPrompt {
                tool_name: req.tool_name.clone(),
                original_input: req.input.clone(),
                suggestions: req.suggestions.clone(),
                asked_at_epoch: crate::store::now_epoch(),
                answer: tx,
            },
        );
        self.emit(SessionEvent::PermissionAsked {
            request_id: req.request_id.clone(),
            tool_name: req.tool_name.clone(),
            input: req.input,
            suggestions: req.suggestions,
        });

        match tokio::time::timeout(OPERATOR_TIMEOUT, rx).await {
            Ok(Ok(decision)) => (decision, DecidedBy::Operator),
            _ => {
                self.pending.lock().unwrap().remove(&req.request_id);
                (
                    BrokerDecision::Deny {
                        message: format!(
                            "The operator did not respond to the {} permission prompt in time. \
                             Proceed without it, or message @operator on the bus.",
                            req.tool_name
                        ),
                    },
                    DecidedBy::Operator,
                )
            }
        }
    }

    fn cancel(&self, request_id: &str) {
        if let Some(p) = self.pending.lock().unwrap().remove(request_id) {
            // The CLI no longer needs the answer; resolve so the decide task
            // finishes. The response we send is discarded upstream.
            let _ = p.answer.send(BrokerDecision::Deny {
                message: "cancelled by the runtime".into(),
            });
        }
    }
}
