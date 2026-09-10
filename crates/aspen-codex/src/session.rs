//! One Codex session: an app-server process, one thread, turns driven by
//! `turn/start` (and `turn/steer` mid-turn), approvals brokered through
//! the neutral `PermissionBroker`, notifications normalized into
//! `SessionEvent` (CODEX_RUNTIME_REFERENCE.md §2–§6).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use aspen_core::permission::{
    BrokerDecision, DecidedBy, PermissionBroker, PermissionPolicy, PermissionRequest,
};
use aspen_core::{
    AdapterCapabilities, DecisionOption, DecisionScope, Harness, PromptKind, RuntimeInfo,
    SessionEvent, SessionHandle, SessionId, ToolKind,
};

use crate::adapter::{mode, Mode};
use crate::normalize;
use crate::rpc::{Inbound, ProcessSpec, RpcClient};

/// How the session reaches the bus tools: the node's API through the
/// `aspen mcp` stdio bridge, registered as the `aspen` MCP server.
#[derive(Clone)]
pub struct Bridge {
    pub node_api: String,
    pub token: Option<String>,
    pub aspen_bin: String,
    /// The provider itself, for the tool list the bridge advertises
    /// (the node serves the calls; the bridge lists what it forwards).
    pub tools: Option<Arc<dyn aspen_core::ToolProvider>>,
}

#[derive(Clone)]
pub struct CodexConfig {
    pub repo: PathBuf,
    pub session_id: SessionId,
    pub resume: Option<String>,
    pub fork: bool,
    pub model: Option<String>,
    /// A mode id from `adapter::MODES`.
    pub mode: String,
    pub policy: PermissionPolicy,
    pub charter: Option<String>,
    pub extra_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub codex_bin: String,
    pub agent: String,
    pub bridge: Option<Bridge>,
}

const CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub struct CodexSession {
    id: SessionId,
    rpc: Arc<RpcClient>,
    thread_id: Mutex<String>,
    current_turn: Mutex<Option<String>>,
    model: Mutex<Option<String>>,
    mode: Mutex<&'static Mode>,
    last_usage: Mutex<Value>,
    runtime: Mutex<RuntimeInfo>,
    /// Items seen at `item/started`, by id — approvals name only the item.
    items: Mutex<HashMap<String, Value>>,
    /// Text of the turn's last agent message, for `TurnEnded.result_text`.
    last_text: Mutex<Option<String>>,
    broker: Arc<dyn PermissionBroker>,
    events: mpsc::Sender<SessionEvent>,
}

fn sandbox_policy(mode: &Mode) -> Value {
    match mode.sandbox {
        "read-only" => json!({ "type": "readOnly", "networkAccess": false }),
        "danger-full-access" => json!({ "type": "dangerFullAccess" }),
        _ => {
            json!({ "type": "workspaceWrite", "writableRoots": [], "networkAccess": false, "excludeTmpdirEnvVar": false, "excludeSlashTmp": false })
        }
    }
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl CodexSession {
    pub async fn start(
        cfg: CodexConfig,
        broker: Arc<dyn PermissionBroker>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<SessionEvent>)> {
        let mode_now =
            mode(&cfg.mode).ok_or_else(|| anyhow!("unknown codex mode {:?}", cfg.mode))?;
        let cwd = cfg.repo.canonicalize().unwrap_or_else(|_| cfg.repo.clone());
        let cwd_str = cwd.to_string_lossy().into_owned();

        // Trust for the cwd is a command-line override, never a config.toml
        // write: Aspen's trust gate is the only trust decision.
        let mut overrides = vec![(
            format!("projects.{}.trust_level", toml_str(&cwd_str)),
            toml_str("trusted"),
        )];
        let mut env = vec![(
            "CODEX_INTERNAL_ORIGINATOR_OVERRIDE".to_owned(),
            "aspen".to_owned(),
        )];
        env.extend(cfg.env.iter().cloned());
        if let Some(b) = &cfg.bridge {
            overrides.push(("mcp_servers.aspen.command".into(), toml_str(&b.aspen_bin)));
            overrides.push(("mcp_servers.aspen.args".into(), "[\"mcp\"]".into()));
            let mut kv = vec![
                format!("ASPEN_NODE_API = {}", toml_str(&b.node_api)),
                format!("ASPEN_AGENT = {}", toml_str(&cfg.agent)),
            ];
            if let Some(t) = &b.token {
                kv.push(format!("ASPEN_NODE_TOKEN = {}", toml_str(t)));
            }
            overrides.push((
                "mcp_servers.aspen.env".into(),
                format!("{{ {} }}", kv.join(", ")),
            ));
            // The bridge is a tiny process; do not wait long on it.
            overrides.push(("mcp_servers.aspen.startup_timeout_sec".into(), "20".into()));
        }
        let spec = ProcessSpec {
            codex_bin: cfg.codex_bin.clone(),
            cwd: cwd.clone(),
            config_overrides: overrides,
            extra_env: env,
            extra_args: cfg.extra_args.clone(),
        };
        let (rpc, mut inbound) = RpcClient::spawn(&spec)?;
        let (events_tx, events_rx) = mpsc::channel::<SessionEvent>(4096);

        // Handshake.
        rpc.call(
            "initialize",
            json!({ "clientInfo": { "name": "aspen", "title": "Aspen", "version": env!("CARGO_PKG_VERSION") }, "capabilities": null }),
            Duration::from_secs(30),
        )
        .await
        .context("codex app-server initialize")?;
        rpc.notify("initialized", json!({})).await?;

        // The thread: fresh, resumed, or forked.
        let mut params = json!({
            "cwd": cwd_str,
            "approvalPolicy": mode_now.approval,
            "approvalsReviewer": mode_now.reviewer,
            "sandbox": mode_now.sandbox,
        });
        if let Some(m) = &cfg.model {
            params["model"] = json!(m);
        }
        if let Some(c) = &cfg.charter {
            params["developerInstructions"] = json!(c);
        }
        let method = match (&cfg.resume, cfg.fork) {
            (Some(id), false) => {
                params["threadId"] = json!(id);
                "thread/resume"
            }
            (Some(id), true) => {
                params["threadId"] = json!(id);
                "thread/fork"
            }
            (None, _) => "thread/start",
        };
        let started = rpc
            .call(method, params, CALL_TIMEOUT)
            .await
            .with_context(|| format!("codex {method}"))?;
        let thread_id = started
            .get("thread")
            .and_then(|t| t.get("id"))
            .and_then(|i| i.as_str())
            .ok_or_else(|| anyhow!("codex {method}: no thread id in response"))?
            .to_owned();
        let model_now = started
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::to_owned);

        let session = Arc::new(Self {
            id: cfg.session_id,
            rpc: rpc.clone(),
            thread_id: Mutex::new(thread_id.clone()),
            current_turn: Mutex::new(None),
            model: Mutex::new(cfg.model.clone()),
            mode: Mutex::new(mode_now),
            last_usage: Mutex::new(Value::Null),
            runtime: Mutex::new(RuntimeInfo {
                model: model_now.clone(),
                mode: Some(mode_now.id.into()),
                raw: json!({ "thread": started, "method": method }),
                ..Default::default()
            }),
            items: Mutex::new(HashMap::new()),
            last_text: Mutex::new(None),
            broker: broker.clone(),
            events: events_tx.clone(),
        });

        // Inventory, best effort: models and skills.
        if let Ok(models) = rpc
            .call(
                "model/list",
                json!({ "includeHidden": false }),
                Duration::from_secs(20),
            )
            .await
        {
            if let Some(list) = models.get("data").and_then(|d| d.as_array()) {
                session.runtime.lock().unwrap().models = list
                    .iter()
                    .map(|m| json!({ "value": m.get("model"), "displayName": m.get("displayName"), "description": m.get("description"), "isDefault": m.get("isDefault") }))
                    .collect();
            }
        }
        if let Ok(skills) = rpc
            .call(
                "skills/list",
                json!({ "cwds": [cwd_str] }),
                Duration::from_secs(20),
            )
            .await
        {
            let mut out = Vec::new();
            for entry in skills
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default()
            {
                for s in entry
                    .get("skills")
                    .and_then(|s| s.as_array())
                    .cloned()
                    .unwrap_or_default()
                {
                    if s.get("enabled") == Some(&Value::Bool(false)) {
                        continue;
                    }
                    out.push(json!({ "name": s.get("name"), "description": s.get("shortDescription").or_else(|| s.get("description")), "path": s.get("path"), "scope": s.get("scope") }));
                }
            }
            session.runtime.lock().unwrap().skills = out;
        }

        let init_raw = {
            let rt = session.runtime.lock().unwrap();
            json!({
                "session_id": thread_id, "model": rt.model, "mode": rt.mode,
                "harness": "codex", "models": rt.models, "skills": rt.skills,
                "thread": started,
            })
        };
        let _ = events_tx
            .send(SessionEvent::RuntimeInit {
                session_id: thread_id.clone(),
                model: model_now,
                raw: init_raw,
            })
            .await;

        // The router: notifications normalized, requests brokered.
        {
            let session = session.clone();
            let events_tx = events_tx.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound.recv().await {
                    match msg {
                        Inbound::Notification { method, params } => {
                            session.on_notification(&method, params, &events_tx).await
                        }
                        Inbound::Request { id, method, params } => {
                            let session = session.clone();
                            let events_tx = events_tx.clone();
                            let broker = broker.clone();
                            // Decisions can take minutes; never block the router.
                            tokio::spawn(async move {
                                session
                                    .on_request(id, &method, params, &events_tx, &broker)
                                    .await
                            });
                        }
                        Inbound::Stderr(line) => {
                            let _ = events_tx.send(SessionEvent::Stderr { line }).await;
                        }
                        Inbound::Exited(code) => {
                            let _ = events_tx.send(SessionEvent::Exited { code }).await;
                            break;
                        }
                    }
                }
            });
        }

        Ok((session, events_rx))
    }

    fn thread(&self) -> String {
        self.thread_id.lock().unwrap().clone()
    }

    async fn on_notification(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        tx: &mpsc::Sender<SessionEvent>,
    ) {
        let send = |ev: SessionEvent| async move {
            let _ = tx.send(ev).await;
        };
        match method {
            "item/agentMessage/delta" => {
                if let Some(d) = params.get("delta").and_then(|d| d.as_str()) {
                    send(SessionEvent::TextDelta {
                        text: d.to_owned(),
                        thinking: false,
                    })
                    .await;
                }
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                if let Some(d) = params.get("delta").and_then(|d| d.as_str()) {
                    send(SessionEvent::TextDelta {
                        text: d.to_owned(),
                        thinking: true,
                    })
                    .await;
                }
            }
            "item/started" => {
                let item = params.get("item").cloned().unwrap_or(Value::Null);
                let ty = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                    self.items
                        .lock()
                        .unwrap()
                        .insert(id.to_owned(), item.clone());
                }
                match ty {
                    "agentMessage" | "reasoning" | "userMessage" | "plan" | "hookPrompt"
                    | "contextCompaction" => {}
                    // Extensions (clock.sleep while a question waits, …) are
                    // status, not tool cards.
                    "extension" | "sleep" => {
                        send(SessionEvent::Status { raw: json!({ "type": "extension", "kind": item.get("kind"), "duration_ms": item.get("durationMs") }) }).await;
                    }
                    _ => {
                        if let Some(ev) = normalize::tool_use_of(&item) {
                            send(ev).await;
                        }
                    }
                }
            }
            "item/completed" => {
                let item = params.get("item").cloned().unwrap_or(Value::Null);
                let ty = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "agentMessage" => {
                        let text = item
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_owned();
                        if !text.is_empty() {
                            *self.last_text.lock().unwrap() = Some(text);
                        }
                        let model = self.runtime.lock().unwrap().model.clone();
                        send(normalize::assistant_message_of(&item, model.as_deref())).await;
                        // An async question (CODEX_RUNTIME_REFERENCE.md §6.3):
                        // the model asked in its message and now waits; the
                        // answer is ordinary user input. Surface it as a
                        // question prompt through the broker.
                        if let Some(qs) = item
                            .get("questions")
                            .and_then(|q| q.as_array())
                            .filter(|q| !q.is_empty())
                        {
                            self.async_question(
                                item.get("id")
                                    .and_then(|i| i.as_str())
                                    .unwrap_or("q")
                                    .to_owned(),
                                qs.clone(),
                            );
                        }
                    }
                    "plan" => {
                        send(SessionEvent::Status {
                            raw: json!({ "type": "plan", "text": item.get("text") }),
                        })
                        .await
                    }
                    "userMessage" | "reasoning" | "hookPrompt" | "contextCompaction"
                    | "extension" | "sleep" => {}
                    _ => {
                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                            self.items.lock().unwrap().remove(id);
                        }
                        if let Some(ev) = normalize::tool_result_of(&item) {
                            send(ev).await;
                        }
                    }
                }
            }
            "turn/started" => {
                if let Some(id) = params
                    .get("turn")
                    .and_then(|t| t.get("id"))
                    .and_then(|i| i.as_str())
                {
                    *self.current_turn.lock().unwrap() = Some(id.to_owned());
                }
                *self.last_text.lock().unwrap() = None;
                send(SessionEvent::Status { raw: json!({ "type": "turn_started", "turn": params.get("turn").and_then(|t| t.get("id")) }) }).await;
            }
            "turn/completed" => {
                *self.current_turn.lock().unwrap() = None;
                let turn = params.get("turn").cloned().unwrap_or(Value::Null);
                let status = turn
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("completed")
                    .to_owned();
                let duration_ms = turn.get("durationMs").and_then(|d| d.as_u64());
                let mut result_text = self.last_text.lock().unwrap().clone();
                if let Some(msg) = turn
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                {
                    result_text = Some(match result_text {
                        Some(t) => format!("{t}\n\n{msg}"),
                        None => msg.to_owned(),
                    });
                }
                let usage = normalize::turn_usage(&self.last_usage.lock().unwrap());
                let subtype = match status.as_str() {
                    "completed" => "success".to_owned(),
                    other => other.to_owned(),
                };
                send(SessionEvent::TurnEnded {
                    subtype,
                    duration_ms,
                    total_cost_usd: None,
                    result_text,
                    raw: json!({ "type": "result", "harness": "codex", "turn": turn, "status": status }),
                    usage,
                })
                .await;
            }
            "thread/tokenUsage/updated" => {
                let tu = params.get("tokenUsage").cloned().unwrap_or(Value::Null);
                if let Some(w) = tu.get("modelContextWindow").and_then(|w| w.as_u64()) {
                    self.runtime.lock().unwrap().context_window = Some(w);
                }
                *self.last_usage.lock().unwrap() = tu.clone();
                send(SessionEvent::Status {
                    raw: json!({ "type": "token_usage", "tokenUsage": tu }),
                })
                .await;
            }
            "error" => {
                let msg = params
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("error");
                let retry = params
                    .get("willRetry")
                    .and_then(|r| r.as_bool())
                    .unwrap_or(false);
                send(SessionEvent::Stderr {
                    line: format!("codex: {msg}{}", if retry { " (retrying)" } else { "" }),
                })
                .await;
                send(SessionEvent::Status {
                    raw: json!({ "type": "error", "message": msg, "will_retry": retry }),
                })
                .await;
            }
            "thread/settings/updated" => {
                if let Some(m) = params
                    .get("threadSettings")
                    .and_then(|s| s.get("model"))
                    .and_then(|m| m.as_str())
                {
                    self.runtime.lock().unwrap().model = Some(m.to_owned());
                }
                send(SessionEvent::Status {
                    raw: json!({ "type": "settings", "settings": params.get("threadSettings") }),
                })
                .await;
            }
            "thread/compacted" => {
                send(SessionEvent::Status {
                    raw: json!({ "type": "compacted" }),
                })
                .await
            }
            "mcpServer/startupStatus/updated" => {
                // Push from Codex (PROPOSALS-MCP.md §2.2); the node re-lists
                // on it so the picture carries tools and auth state.
                send(SessionEvent::Status { raw: json!({ "type": "mcp_startup", "name": params.get("name"), "status": params.get("status"), "error": params.get("error") }) }).await;
            }
            "model/rerouted" => {
                send(SessionEvent::Status {
                    raw: json!({ "type": "model_rerouted", "detail": params }),
                })
                .await
            }
            "warning" | "guardianWarning" | "configWarning" | "deprecationNotice" => {
                let msg = params
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_owned();
                send(SessionEvent::Stderr {
                    line: format!("codex {method}: {msg}"),
                })
                .await;
            }
            // Output deltas, status flips, rate limits, diffs: kept, never
            // interpreted (the source view shows them).
            _ => {
                send(SessionEvent::Raw {
                    raw: json!({ "method": method, "params": params }),
                })
                .await
            }
        }
    }

    async fn on_request(
        &self,
        id: Value,
        method: &str,
        params: Value,
        tx: &mpsc::Sender<SessionEvent>,
        broker: &Arc<dyn PermissionBroker>,
    ) {
        let request_id = match &id {
            Value::Number(n) => format!("codex-{n}"),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        match method {
            "item/commandExecution/requestApproval" => {
                let item_id = params
                    .get("itemId")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_owned();
                let item = self
                    .items
                    .lock()
                    .unwrap()
                    .get(&item_id)
                    .cloned()
                    .unwrap_or(Value::Null);
                let command = params
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(str::to_owned)
                    .or_else(|| Some(normalize::command_of(&item)))
                    .unwrap_or_default();
                let actions_cmd = params
                    .get("commandActions")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|a| a.get("command"))
                    .and_then(|c| c.as_str())
                    .map(str::to_owned);
                let shown = actions_cmd.unwrap_or(command);
                let amendment = params
                    .get("proposedExecpolicyAmendment")
                    .cloned()
                    .filter(|a| !a.is_null());
                let kind = if params.get("kind").and_then(|k| k.as_str()) == Some("network") {
                    ToolKind::Web
                } else {
                    normalize::shell_kind(
                        &json!({ "commandActions": params.get("commandActions") }),
                    )
                };
                let available: Vec<String> = params
                    .get("availableDecisions")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|d| match d {
                                Value::String(s) => s.clone(),
                                Value::Object(o) => o.keys().next().cloned().unwrap_or_default(),
                                _ => String::new(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let has = |d: &str| available.is_empty() || available.iter().any(|a| a == d);
                let mut decisions = vec![DecisionOption {
                    id: "accept".into(),
                    label: "allow".into(),
                    allow: true,
                    scope: DecisionScope::Once,
                    payload: None,
                }];
                if has("acceptForSession") {
                    decisions.push(DecisionOption {
                        id: "acceptForSession".into(),
                        label: "allow for this session".into(),
                        allow: true,
                        scope: DecisionScope::Session,
                        payload: None,
                    });
                }
                if let Some(a) = &amendment {
                    if has("acceptWithExecpolicyAmendment") {
                        let toks: Vec<&str> = a
                            .as_array()
                            .map(|p| p.iter().filter_map(|x| x.as_str()).collect())
                            .unwrap_or_default();
                        // `/bin/bash -lc '<script>'` → show the script.
                        let prefix = if toks.len() == 3 && toks[1] == "-lc" {
                            toks[2].to_owned()
                        } else {
                            toks.join(" ")
                        };
                        let prefix: String = prefix.chars().take(60).collect();
                        decisions.push(DecisionOption {
                            id: "acceptWithExecpolicyAmendment".into(),
                            label: format!("always allow `{prefix}`"),
                            allow: true,
                            scope: DecisionScope::Always,
                            payload: Some(a.clone()),
                        });
                    }
                }
                decisions.push(DecisionOption {
                    id: "decline".into(),
                    label: "deny".into(),
                    allow: false,
                    scope: DecisionScope::Once,
                    payload: None,
                });
                let req = PermissionRequest {
                    request_id: request_id.clone(),
                    tool_name: "commandExecution".into(),
                    kind,
                    prompt: PromptKind::Permission,
                    input: json!({ "command": shown, "cwd": params.get("cwd"), "reason": params.get("reason"), "network": params.get("networkApprovalContext") }),
                    suggestions: amendment.clone().unwrap_or(Value::Null),
                    decisions,
                    questions: Value::Null,
                    raw: params.clone(),
                };
                let (decision, by) = broker.decide(req).await;
                self.settled(tx, &request_id, "commandExecution", &decision, by)
                    .await;
                let result = match decision {
                    BrokerDecision::Allow {
                        decision_id,
                        updated_permissions,
                        ..
                    } => match decision_id.as_deref() {
                        Some("acceptForSession") => json!("acceptForSession"),
                        Some("acceptWithExecpolicyAmendment") | Some("always") => {
                            match updated_permissions.or(amendment) {
                                Some(a) => {
                                    json!({ "acceptWithExecpolicyAmendment": { "execpolicy_amendment": a } })
                                }
                                None => json!("acceptForSession"),
                            }
                        }
                        _ => json!("accept"),
                    },
                    BrokerDecision::Deny { .. } => json!("decline"),
                };
                let _ = self.rpc.respond(id, json!({ "decision": result })).await;
            }
            "item/fileChange/requestApproval" => {
                let item_id = params
                    .get("itemId")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_owned();
                let item = self
                    .items
                    .lock()
                    .unwrap()
                    .get(&item_id)
                    .cloned()
                    .unwrap_or(Value::Null);
                let changes = item.get("changes").cloned().unwrap_or(json!([]));
                let first = changes
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|c| c.get("path"))
                    .and_then(|p| p.as_str())
                    .map(str::to_owned);
                let decisions = vec![
                    DecisionOption {
                        id: "accept".into(),
                        label: "allow".into(),
                        allow: true,
                        scope: DecisionScope::Once,
                        payload: None,
                    },
                    DecisionOption {
                        id: "acceptForSession".into(),
                        label: "allow for this session".into(),
                        allow: true,
                        scope: DecisionScope::Session,
                        payload: None,
                    },
                    DecisionOption {
                        id: "decline".into(),
                        label: "deny".into(),
                        allow: false,
                        scope: DecisionScope::Once,
                        payload: None,
                    },
                ];
                let req = PermissionRequest {
                    request_id: request_id.clone(),
                    tool_name: "fileChange".into(),
                    kind: ToolKind::FileEdit,
                    prompt: PromptKind::Permission,
                    input: json!({ "file_path": first, "changes": changes, "reason": params.get("reason"), "grant_root": params.get("grantRoot") }),
                    suggestions: Value::Null,
                    decisions,
                    questions: Value::Null,
                    raw: params.clone(),
                };
                let (decision, by) = broker.decide(req).await;
                self.settled(tx, &request_id, "fileChange", &decision, by)
                    .await;
                let result = match decision {
                    BrokerDecision::Allow { decision_id, .. }
                        if decision_id.as_deref() == Some("acceptForSession") =>
                    {
                        "acceptForSession"
                    }
                    BrokerDecision::Allow { .. } => "accept",
                    BrokerDecision::Deny { .. } => "decline",
                };
                let _ = self.rpc.respond(id, json!({ "decision": result })).await;
            }
            "item/tool/requestUserInput" => {
                // Codex questions → the console's question card shape (the
                // Claude `questions[]` form), answers keyed by question text
                // back to Codex ids.
                let qs = params
                    .get("questions")
                    .and_then(|q| q.as_array())
                    .cloned()
                    .unwrap_or_default();
                let questions: Vec<Value> = qs
                    .iter()
                    .map(|q| {
                        json!({
                            "question": q.get("question"),
                            "header": q.get("header"),
                            "multiSelect": false,
                            "options": q.get("options").and_then(|o| o.as_array()).map(|o| o.iter().map(|x| json!({ "label": x.get("label"), "description": x.get("description") })).collect::<Vec<_>>()).unwrap_or_default(),
                            "id": q.get("id"),
                            "isOther": q.get("isOther"),
                            "isSecret": q.get("isSecret"),
                        })
                    })
                    .collect();
                let req = PermissionRequest {
                    request_id: request_id.clone(),
                    tool_name: "requestUserInput".into(),
                    kind: ToolKind::Question,
                    prompt: PromptKind::Question,
                    input: json!({ "questions": questions }),
                    suggestions: Value::Null,
                    decisions: vec![DecisionOption {
                        id: "answer".into(),
                        label: "answer".into(),
                        allow: true,
                        scope: DecisionScope::Once,
                        payload: None,
                    }],
                    questions: json!(questions),
                    raw: params.clone(),
                };
                let (decision, by) = broker.decide(req).await;
                self.settled(tx, &request_id, "requestUserInput", &decision, by)
                    .await;
                let mut answers = serde_json::Map::new();
                if let BrokerDecision::Allow { updated_input, .. } = &decision {
                    let given = updated_input.get("answers").and_then(|a| a.as_object());
                    for q in &qs {
                        let qid = q.get("id").and_then(|i| i.as_str()).unwrap_or("");
                        let qtext = q.get("question").and_then(|t| t.as_str()).unwrap_or("");
                        let v = given.and_then(|g| g.get(qtext).or_else(|| g.get(qid)));
                        let list: Vec<String> = match v {
                            Some(Value::String(s)) => vec![s.clone()],
                            Some(Value::Array(a)) => a
                                .iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect(),
                            _ => Vec::new(),
                        };
                        if !list.is_empty() {
                            answers.insert(qid.to_owned(), json!({ "answers": list }));
                        }
                    }
                }
                let _ = self.rpc.respond(id, json!({ "answers": answers })).await;
            }
            "item/permissions/requestApproval" => {
                let req = PermissionRequest {
                    request_id: request_id.clone(),
                    tool_name: "permissions".into(),
                    kind: ToolKind::Other,
                    prompt: PromptKind::Permission,
                    input: json!({ "permissions": params.get("permissions"), "reason": params.get("reason"), "cwd": params.get("cwd") }),
                    suggestions: Value::Null,
                    decisions: vec![
                        DecisionOption {
                            id: "accept".into(),
                            label: "grant for this turn".into(),
                            allow: true,
                            scope: DecisionScope::Once,
                            payload: None,
                        },
                        DecisionOption {
                            id: "acceptForSession".into(),
                            label: "grant for this session".into(),
                            allow: true,
                            scope: DecisionScope::Session,
                            payload: None,
                        },
                        DecisionOption {
                            id: "decline".into(),
                            label: "deny".into(),
                            allow: false,
                            scope: DecisionScope::Once,
                            payload: None,
                        },
                    ],
                    questions: Value::Null,
                    raw: params.clone(),
                };
                let (decision, by) = broker.decide(req).await;
                self.settled(tx, &request_id, "permissions", &decision, by)
                    .await;
                let result = match decision {
                    BrokerDecision::Allow { decision_id, .. } => {
                        let mut granted = serde_json::Map::new();
                        if let Some(p) = params.get("permissions").and_then(|p| p.as_object()) {
                            for (k, v) in p {
                                if !v.is_null() {
                                    granted.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        let scope = if decision_id.as_deref() == Some("acceptForSession") {
                            "session"
                        } else {
                            "turn"
                        };
                        json!({ "permissions": granted, "scope": scope })
                    }
                    BrokerDecision::Deny { .. } => json!({ "permissions": {}, "scope": "turn" }),
                };
                let _ = self.rpc.respond(id, result).await;
            }
            "mcpServer/elicitation/request" => {
                let meta = params.get("_meta").cloned().unwrap_or(Value::Null);
                let server = params
                    .get("serverName")
                    .and_then(|s| s.as_str())
                    .unwrap_or("mcp")
                    .to_owned();
                if meta.get("codex_approval_kind").and_then(|k| k.as_str()) == Some("mcp_tool_call")
                {
                    // Codex asks for MCP tool calls through an elicitation
                    // (CODEX_RUNTIME_REFERENCE.md §6.4). Named like Claude's
                    // MCP tools so the bus tools auto-allow by name.
                    let tool = meta
                        .get("tool_name")
                        .and_then(|t| t.as_str())
                        .map(str::to_owned)
                        .or_else(|| {
                            params
                                .get("message")
                                .and_then(|m| m.as_str())
                                .and_then(|m| m.split('"').nth(1))
                                .map(str::to_owned)
                        });
                    let tool = tool.unwrap_or_else(|| "tool".into());
                    let persist: Vec<String> = meta
                        .get("persist")
                        .and_then(|p| p.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut decisions = vec![DecisionOption {
                        id: "accept".into(),
                        label: "allow".into(),
                        allow: true,
                        scope: DecisionScope::Once,
                        payload: None,
                    }];
                    if persist.iter().any(|p| p == "session") {
                        decisions.push(DecisionOption {
                            id: "session".into(),
                            label: "allow for this session".into(),
                            allow: true,
                            scope: DecisionScope::Session,
                            payload: None,
                        });
                    }
                    if persist.iter().any(|p| p == "always") {
                        decisions.push(DecisionOption {
                            id: "always".into(),
                            label: "always allow".into(),
                            allow: true,
                            scope: DecisionScope::Always,
                            payload: None,
                        });
                    }
                    decisions.push(DecisionOption {
                        id: "decline".into(),
                        label: "deny".into(),
                        allow: false,
                        scope: DecisionScope::Once,
                        payload: None,
                    });
                    let name = format!("mcp__{server}__{tool}");
                    let req = PermissionRequest {
                        request_id: request_id.clone(),
                        tool_name: name.clone(),
                        kind: ToolKind::Mcp,
                        prompt: PromptKind::Permission,
                        input: meta.get("tool_params").cloned().unwrap_or(json!({})),
                        suggestions: Value::Null,
                        decisions,
                        questions: Value::Null,
                        raw: params.clone(),
                    };
                    let (decision, by) = broker.decide(req).await;
                    self.settled(tx, &request_id, &name, &decision, by).await;
                    let result = match decision {
                        BrokerDecision::Allow { decision_id, .. } => {
                            let mut m = serde_json::Map::new();
                            match decision_id.as_deref() {
                                Some("session") => {
                                    m.insert("persist".into(), json!("session"));
                                }
                                Some("always") => {
                                    m.insert("persist".into(), json!("always"));
                                }
                                _ => {}
                            }
                            json!({ "action": "accept", "content": {}, "_meta": Value::Object(m) })
                        }
                        BrokerDecision::Deny { .. } => {
                            json!({ "action": "decline", "content": null, "_meta": null })
                        }
                    };
                    let _ = self.rpc.respond(id, result).await;
                } else {
                    // A server's own form/url elicitation: not surfaced yet
                    // (PROPOSALS-HARNESSES.md §5) — decline honestly.
                    let _ = self
                        .rpc
                        .respond(
                            id,
                            json!({ "action": "decline", "content": null, "_meta": null }),
                        )
                        .await;
                    let _ = tx.send(SessionEvent::Status { raw: json!({ "type": "elicitation_declined", "server": server, "mode": params.get("mode") }) }).await;
                }
            }
            other => {
                let _ = self
                    .rpc
                    .respond_error(id, &format!("aspen: unsupported server request {other:?}"))
                    .await;
            }
        }
    }

    /// Route an async agent question through the broker; when answered,
    /// the answers go back as user input (steering the waiting turn).
    fn async_question(self: &Arc<Self>, id: String, qs: Vec<Value>) {
        let session = self.clone();
        tokio::spawn(async move {
            let questions: Vec<Value> = qs
                .iter()
                .map(|q| {
                    json!({
                        "question": q.get("title").or_else(|| q.get("question")),
                        "header": q.get("header").cloned().unwrap_or(json!("Question")),
                        "multiSelect": false,
                        "options": q.get("options").and_then(|o| o.as_array()).map(|o| o.iter().map(|x| match x {
                            Value::String(s) => json!({ "label": s, "description": "" }),
                            other => json!({ "label": other.get("label"), "description": other.get("description").cloned().unwrap_or(json!("")) }),
                        }).collect::<Vec<_>>()).unwrap_or_default(),
                    })
                })
                .collect();
            let request_id = format!("codex-q-{id}");
            let req = PermissionRequest {
                request_id: request_id.clone(),
                tool_name: "question".into(),
                kind: ToolKind::Question,
                prompt: PromptKind::Question,
                input: json!({ "questions": questions }),
                suggestions: Value::Null,
                decisions: vec![DecisionOption {
                    id: "answer".into(),
                    label: "answer".into(),
                    allow: true,
                    scope: DecisionScope::Once,
                    payload: None,
                }],
                questions: json!(questions),
                raw: json!({ "item": id, "questions": qs }),
            };
            let (decision, by) = session.broker.decide(req).await;
            session
                .settled(&session.events, &request_id, "question", &decision, by)
                .await;
            if let BrokerDecision::Allow { updated_input, .. } = decision {
                let mut lines = Vec::new();
                if let Some(ans) = updated_input.get("answers").and_then(|a| a.as_object()) {
                    for (q, v) in ans {
                        let text = match v {
                            Value::String(s) => s.clone(),
                            Value::Array(a) => a
                                .iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                            other => other.to_string(),
                        };
                        lines.push(format!("{q}: {text}"));
                    }
                }
                if let Some(r) = updated_input
                    .get("response")
                    .and_then(|r| r.as_str())
                    .filter(|r| !r.trim().is_empty())
                {
                    lines.push(r.to_owned());
                }
                if lines.is_empty() {
                    lines.push("(no answer given)".into());
                }
                let _ = session
                    .start_or_steer(
                        json!([{ "type": "text", "text": lines.join("\n"), "text_elements": [] }]),
                    )
                    .await;
            }
        });
    }

    async fn settled(
        &self,
        tx: &mpsc::Sender<SessionEvent>,
        request_id: &str,
        tool: &str,
        decision: &BrokerDecision,
        by: DecidedBy,
    ) {
        let _ = tx
            .send(SessionEvent::PermissionSettled {
                request_id: request_id.to_owned(),
                tool_name: tool.to_owned(),
                allowed: matches!(decision, BrokerDecision::Allow { .. }),
                by_policy: by == DecidedBy::Policy,
            })
            .await;
    }

    /// Text → Codex input blocks. A `$name` token naming a known skill
    /// becomes a skill block (Codex's own mention form) beside the text.
    fn input_blocks(&self, text: &str) -> Value {
        let skills = self.runtime.lock().unwrap().skills.clone();
        let mut blocks = vec![json!({ "type": "text", "text": text, "text_elements": [] })];
        for tok in text.split_whitespace() {
            let Some(name) = tok.strip_prefix('$') else {
                continue;
            };
            let name =
                name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
            if name.is_empty() {
                continue;
            }
            if let Some(sk) = skills
                .iter()
                .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(name))
            {
                if let Some(path) = sk.get("path").and_then(|p| p.as_str()) {
                    if !blocks.iter().any(|b| {
                        b.get("type") == Some(&json!("skill"))
                            && b.get("name") == Some(&json!(name))
                    }) {
                        blocks.push(json!({ "type": "skill", "name": name, "path": path }));
                    }
                }
            }
        }
        json!(blocks)
    }

    fn turn_params(&self, input: Value) -> Value {
        let mode = *self.mode.lock().unwrap();
        let mut p = json!({
            "threadId": self.thread(),
            "input": input,
            "approvalPolicy": mode.approval,
            "approvalsReviewer": mode.reviewer,
            "sandboxPolicy": sandbox_policy(mode),
        });
        if let Some(m) = self.model.lock().unwrap().clone() {
            p["model"] = json!(m);
        }
        p
    }

    async fn start_or_steer(&self, input: Value) -> Result<String> {
        let uuid = Uuid::new_v4().to_string();
        let in_turn = self.current_turn.lock().unwrap().clone();
        if let Some(turn) = in_turn {
            // Mid-turn: steer the running turn; if Codex refuses (it just
            // ended), fall through to a new turn.
            let r = self
                .rpc
                .call("turn/steer", json!({ "threadId": self.thread(), "input": input, "expectedTurnId": turn, "clientUserMessageId": uuid }), CALL_TIMEOUT)
                .await;
            if r.is_ok() {
                return Ok(uuid);
            }
            tracing::debug!(error = ?r.err(), "turn/steer refused; starting a new turn");
        }
        let mut params = self.turn_params(input);
        params["clientUserMessageId"] = json!(uuid);
        let resp = self.rpc.call("turn/start", params, CALL_TIMEOUT).await?;
        if let Some(id) = resp
            .get("turn")
            .and_then(|t| t.get("id"))
            .and_then(|i| i.as_str())
        {
            *self.current_turn.lock().unwrap() = Some(id.to_owned());
        }
        Ok(uuid)
    }
}

#[async_trait]
impl SessionHandle for CodexSession {
    fn id(&self) -> SessionId {
        self.id
    }
    fn harness(&self) -> Harness {
        Harness::Codex
    }
    fn capabilities(&self) -> AdapterCapabilities {
        crate::adapter::capabilities()
    }
    fn runtime_info(&self) -> RuntimeInfo {
        self.runtime.lock().unwrap().clone()
    }

    async fn send_user(&self, text: String) -> Result<String> {
        self.start_or_steer(self.input_blocks(&text)).await
    }

    /// Claude-shaped content blocks (`text`, `image` with base64 source)
    /// → Codex `UserInput` (text, `image` data URLs).
    async fn send_user_content(&self, content: Value) -> Result<String> {
        let blocks = content.as_array().cloned().unwrap_or_default();
        let mut input = Vec::new();
        for b in blocks {
            match b.get("type").and_then(|t| t.as_str()) {
                Some("text") => input.push(json!({ "type": "text", "text": b.get("text").and_then(|t| t.as_str()).unwrap_or(""), "text_elements": [] })),
                Some("image") => {
                    let src = b.get("source").cloned().unwrap_or(Value::Null);
                    if src.get("type").and_then(|t| t.as_str()) == Some("base64") {
                        let media = src.get("media_type").and_then(|m| m.as_str()).unwrap_or("image/png");
                        let data = src.get("data").and_then(|d| d.as_str()).unwrap_or("");
                        input.push(json!({ "type": "image", "url": format!("data:{media};base64,{data}") }));
                    } else if let Some(p) = b.get("path").and_then(|p| p.as_str()) {
                        input.push(json!({ "type": "localImage", "path": p }));
                    }
                }
                _ => {}
            }
        }
        if input.is_empty() {
            anyhow::bail!("nothing to send");
        }
        self.start_or_steer(json!(input)).await
    }

    async fn interrupt(&self) -> Result<()> {
        let turn = self.current_turn.lock().unwrap().clone();
        let Some(turn) = turn else {
            return Ok(());
        };
        self.rpc
            .call(
                "turn/interrupt",
                json!({ "threadId": self.thread(), "turnId": turn }),
                Duration::from_secs(30),
            )
            .await
            .map(|_| ())
    }

    async fn shutdown(&self) -> Result<()> {
        let turn = self.current_turn.lock().unwrap().clone();
        if let Some(turn) = turn {
            let _ = self
                .rpc
                .call(
                    "turn/interrupt",
                    json!({ "threadId": self.thread(), "turnId": turn }),
                    Duration::from_secs(5),
                )
                .await;
        }
        self.rpc.kill();
        Ok(())
    }

    async fn set_model(&self, model: Option<&str>) -> Result<()> {
        let m = model
            .filter(|m| !m.is_empty() && *m != "default")
            .map(str::to_owned);
        *self.model.lock().unwrap() = m.clone();
        if let Some(m) = m {
            self.runtime.lock().unwrap().model = Some(m);
        }
        Ok(())
    }

    async fn set_mode(&self, mode_id: &str) -> Result<()> {
        let m = mode(mode_id).ok_or_else(|| {
            anyhow!(
                "unknown codex mode {mode_id:?}; one of {}",
                crate::adapter::MODES
                    .iter()
                    .map(|m| m.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        *self.mode.lock().unwrap() = m;
        self.runtime.lock().unwrap().mode = Some(m.id.into());
        Ok(())
    }

    fn pid(&self) -> Option<u32> {
        self.rpc.pid
    }

    async fn mcp_servers(&self) -> Result<Vec<aspen_core::McpServerState>> {
        let v = self
            .rpc
            .call(
                "mcpServerStatus/list",
                json!({ "threadId": self.thread(), "detail": "toolsAndAuthOnly" }),
                Duration::from_secs(30),
            )
            .await?;
        Ok(v.get("data")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().map(codex_mcp_state).collect())
            .unwrap_or_default())
    }

    async fn mcp_reconnect(&self, _name: &str) -> Result<()> {
        // Codex reloads all servers from config; there is no per-server call.
        self.rpc
            .call(
                "config/mcpServer/reload",
                json!({}),
                Duration::from_secs(60),
            )
            .await
            .map(|_| ())
    }

    async fn mcp_authenticate(&self, name: &str) -> Result<aspen_core::McpAuth> {
        let v = self
            .rpc
            .call(
                "mcpServer/oauth/login",
                json!({ "name": name }),
                Duration::from_secs(60),
            )
            .await?;
        Ok(aspen_core::McpAuth {
            url: v
                .get("authorizationUrl")
                .or_else(|| v.get("authUrl"))
                .or_else(|| v.get("url"))
                .and_then(|u| u.as_str())
                .map(str::to_owned),
            requires_user: true,
        })
    }

    async fn context_usage(&self) -> Result<Value> {
        let tu = self.last_usage.lock().unwrap().clone();
        let window = tu.get("modelContextWindow").and_then(|w| w.as_u64());
        let last = tu.get("last").cloned().unwrap_or(Value::Null);
        let used = last.get("inputTokens").and_then(|v| v.as_u64()).map(|i| {
            i + last
                .get("outputTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        });
        Ok(json!({
            "harness": "codex",
            "usedTokens": used,
            "maxTokens": window,
            "categories": [
                { "name": "cached input", "tokens": last.get("cachedInputTokens") },
                { "name": "fresh input", "tokens": last.get("inputTokens").and_then(|v| v.as_u64()).map(|i| i.saturating_sub(last.get("cachedInputTokens").and_then(|v| v.as_u64()).unwrap_or(0))) },
                { "name": "output", "tokens": last.get("outputTokens") },
                { "name": "reasoning", "tokens": last.get("reasoningOutputTokens") },
            ],
            "total": tu.get("total"),
        }))
    }
}

/// One `mcpServerStatus/list` row in the neutral shape (PROPOSALS-MCP.md §3.5).
fn codex_mcp_state(v: &Value) -> aspen_core::McpServerState {
    use aspen_core::McpStatus;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    let status = match s("runtimeStatus").as_deref() {
        Some("connected") => McpStatus::Connected,
        Some("failed") | Some("cancelled") => McpStatus::Failed,
        Some("authenticationRequired") => McpStatus::NeedsAuth,
        Some("disabled") => McpStatus::Disabled,
        _ => McpStatus::Pending,
    };
    let tools: Vec<String> = v
        .get("tools")
        .and_then(|t| t.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let server = v.get("serverInfo").and_then(|i| {
        let n = i.get("name").and_then(|x| x.as_str())?;
        Some(match i.get("version").and_then(|x| x.as_str()) {
            Some(ver) => format!("{n} {ver}"),
            None => n.to_owned(),
        })
    });
    aspen_core::McpServerState {
        name: s("name").unwrap_or_default(),
        status,
        error: s("toolsError"),
        scope: v
            .get("pluginId")
            .and_then(|p| p.as_str())
            .map(|_| "plugin".to_owned()),
        transport: None,
        command: None,
        server,
        tools,
        plugin: s("pluginId"),
    }
}
