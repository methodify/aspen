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
    original_input: Value,
    answer: oneshot::Sender<BrokerDecision>,
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
    /// (answered elsewhere, cancelled, or timed out).
    pub fn answer(
        &self,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<Value>,
    ) -> bool {
        let Some(p) = self.pending.lock().unwrap().remove(request_id) else {
            return false;
        };
        let decision = if allow {
            BrokerDecision::Allow {
                updated_input: updated_input.unwrap_or(p.original_input),
                updated_permissions: None,
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
                original_input: req.input.clone(),
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
