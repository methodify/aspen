//! The protocol client: owns the handshake, control correlation, the four
//! CLI-initiated request subtypes, permissions, and event normalization.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use aspen_core::{AdapterCapabilities, Harness, PromptKind, RuntimeInfo, SessionEvent, SessionHandle, SessionId};
pub use aspen_core::permission::PermissionPolicy;

use crate::mcp::McpServer;
use crate::normalize::normalize;
use crate::process::{self, SpawnSpec};

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub repo: PathBuf,
    pub session_id: SessionId,
    pub resume: Option<String>,
    /// With resume: branch to a fresh session id instead of appending.
    pub fork: bool,
    /// With resume: truncate history to this message first.
    pub resume_at: Option<String>,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub policy: PermissionPolicy,
    pub claude_bin: String,
    /// Injected as `appendSystemPrompt` at initialize — the session's
    /// charter (who you are on the bus), when set.
    pub charter: Option<String>,
    /// Extra CLI arguments appended verbatim after the protocol flags
    /// (harness defaults + per-session args, already split).
    pub extra_args: Vec<String>,
}

impl ClaudeConfig {
    pub fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            session_id: SessionId::new(),
            resume: None,
            fork: false,
            resume_at: None,
            permission_mode: None,
            model: None,
            policy: PermissionPolicy::ReadOnlyAuto,
            claude_bin: "claude".into(),
            charter: None,
            extra_args: Vec::new(),
        }
    }
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;

/// Everything learned from the `initialize` response (commands, models,
/// account, current mode) — populated once the handshake completes.
#[derive(Default)]
pub struct HandshakeInfo(Mutex<Option<Value>>);

impl HandshakeInfo {
    pub fn get(&self) -> Option<Value> {
        self.0.lock().unwrap().clone()
    }
}

pub struct ClaudeSession {
    id: SessionId,
    stdin_tx: mpsc::Sender<String>,
    pending: Pending,
    kill_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub handshake: Arc<HandshakeInfo>,
    pid: Option<u32>,
}

impl ClaudeSession {
    /// Spawn with the config's policy as the broker (no operator surface).
    pub async fn spawn(
        cfg: ClaudeConfig,
        mcp: McpServer,
    ) -> Result<(Arc<Self>, mpsc::Receiver<SessionEvent>)> {
        let broker: Arc<dyn crate::broker::PermissionBroker> = Arc::new(PolicyBroker(cfg.policy));
        Self::spawn_with_broker(cfg, mcp, broker).await
    }

    /// Spawn, wire the loops, and perform the `initialize` handshake.
    /// Returns the handle plus the normalized event stream.
    pub async fn spawn_with_broker(
        cfg: ClaudeConfig,
        mcp: McpServer,
        broker: Arc<dyn crate::broker::PermissionBroker>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<SessionEvent>)> {
        let mut spec = SpawnSpec::new(cfg.repo.clone());
        spec.claude_bin = cfg.claude_bin.clone();
        spec.permission_mode = cfg.permission_mode.clone();
        spec.model = cfg.model.clone();
        spec.extra_args = cfg.extra_args.clone();
        if let Some(r) = &cfg.resume {
            spec.resume = Some(r.clone());
            spec.fork_session = cfg.fork;
            spec.resume_at = cfg.resume_at.clone();
        } else {
            spec.session_id = Some(cfg.session_id.to_string());
        }

        let proc = process::spawn(&spec)?;
        let (events_tx, events_rx) = mpsc::channel::<SessionEvent>(4096);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (kill_tx, kill_rx) = oneshot::channel::<()>();

        let pid = proc.child.id();
        let session = Arc::new(Self {
            id: cfg.session_id,
            stdin_tx: proc.stdin_tx.clone(),
            pending: pending.clone(),
            kill_tx: Mutex::new(Some(kill_tx)),
            handshake: Arc::new(HandshakeInfo::default()),
            pid,
        });

        // Child supervisor: waits for exit or a kill order.
        {
            let events_tx = events_tx.clone();
            let pending = pending.clone();
            let mut child = proc.child;
            tokio::spawn(async move {
                let code = tokio::select! {
                    status = child.wait() => status.ok().and_then(|s| s.code()),
                    _ = kill_rx => {
                        let _ = child.kill().await;
                        None
                    }
                };
                // Reject everything in flight — the pre-attached-catch
                // lesson: awaiting callers see the error; nobody hangs.
                let drained: Vec<_> = pending.lock().unwrap().drain().collect();
                for (_, tx) in drained {
                    let _ = tx.send(Err("session process exited".into()));
                }
                let _ = events_tx.send(SessionEvent::Exited { code }).await;
            });
        }

        // Stderr: the debug channel, surfaced not swallowed.
        {
            let events_tx = events_tx.clone();
            let mut stderr_rx = proc.stderr_rx;
            tokio::spawn(async move {
                while let Some(line) = stderr_rx.recv().await {
                    tracing::debug!(target: "claude_stderr", "{line}");
                    let _ = events_tx.send(SessionEvent::Stderr { line }).await;
                }
            });
        }

        // The single choke point: one reader, one router.
        {
            let session = session.clone();
            let mut stdout_rx = proc.stdout_rx;
            let events_tx = events_tx.clone();
            let mcp = Arc::new(mcp);
            let broker = broker.clone();
            tokio::spawn(async move {
                while let Some(line) = stdout_rx.recv().await {
                    let frame: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        // Trust the CLI's stdout guard, but verify: skip,
                        // never crash (reference §2.3).
                        Err(_) => {
                            tracing::warn!(line, "unparseable stdout line skipped");
                            continue;
                        }
                    };
                    session.route_frame(frame, &events_tx, &broker, &mcp).await;
                }
            });
        }

        // Handshake. `system/init` will NOT arrive until the first turn
        // (reference §4.1) — nothing here blocks on it.
        let mut init_req = json!({
            "subtype": "initialize",
            "sdkMcpServers": [crate::mcp::SERVER_NAME],
        });
        if let Some(charter) = &cfg.charter {
            init_req["appendSystemPrompt"] = json!(charter);
        }
        let resp = session
            .request(init_req, Duration::from_secs(60))
            .await
            .context("initialize handshake")?;
        *session.handshake.0.lock().unwrap() = Some(resp);

        Ok((session, events_rx))
    }

    /// Send one control request and await its correlated response.
    /// Timeouts are mandatory: a dead child must not leak promises.
    pub async fn request(&self, request: Value, timeout: Duration) -> Result<Value> {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);
        let frame = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        });
        // The timeout covers the write too: a child that stopped reading
        // stdin fills the channel, and a bare write would wait forever.
        match tokio::time::timeout(timeout, async {
            self.write(frame).await?;
            Ok::<_, anyhow::Error>(rx.await)
        })
        .await
        {
            Ok(Err(e)) => {
                self.pending.lock().unwrap().remove(&request_id);
                Err(e)
            }
            Ok(Ok(Ok(Ok(v)))) => Ok(v),
            Ok(Ok(Ok(Err(e)))) => Err(anyhow!("control error: {e}")),
            Ok(Ok(Err(_))) => Err(anyhow!("session closed before response")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&request_id);
                Err(anyhow!("control request timed out"))
            }
        }
    }

    async fn write(&self, frame: Value) -> Result<()> {
        let line = serde_json::to_string(&frame)?;
        self.stdin_tx
            .send(line)
            .await
            .map_err(|_| anyhow!("session stdin closed"))
    }

    async fn respond_success(&self, request_id: &str, response: Value) {
        let _ = self
            .write(json!({
                "type": "control_response",
                "response": { "subtype": "success", "request_id": request_id, "response": response },
            }))
            .await;
    }

    async fn respond_error(&self, request_id: &str, error: &str) {
        let _ = self
            .write(json!({
                "type": "control_response",
                "response": { "subtype": "error", "request_id": request_id, "error": error },
            }))
            .await;
    }

    async fn route_frame(
        self: &Arc<Self>,
        frame: Value,
        events_tx: &mpsc::Sender<SessionEvent>,
        broker: &Arc<dyn crate::broker::PermissionBroker>,
        mcp: &Arc<McpServer>,
    ) {
        match frame.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "control_response" => {
                let resp = frame.get("response").cloned().unwrap_or(Value::Null);
                let rid = resp
                    .get("request_id")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_owned();
                if let Some(tx) = self.pending.lock().unwrap().remove(&rid) {
                    let result = match resp.get("subtype").and_then(|s| s.as_str()) {
                        Some("success") => Ok(resp.get("response").cloned().unwrap_or(Value::Null)),
                        _ => Err(resp
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("unknown control error")
                            .to_owned()),
                    };
                    let _ = tx.send(result);
                }
            }
            "control_request" => {
                self.handle_cli_request(frame, events_tx, broker, mcp).await;
            }
            "control_cancel_request" => {
                // The CLI raced a hook or moved on: close any prompt the
                // broker holds open (reference §3, invariant 5).
                if let Some(rid) = frame.get("request_id").and_then(|r| r.as_str()) {
                    broker.cancel(rid);
                }
            }
            _ => {
                for ev in normalize(frame) {
                    let _ = events_tx.send(ev).await;
                }
            }
        }
    }

    /// The four subtypes the CLI originates (reference §6). Handle all;
    /// never hang any — an unanswered request wedges the CLI forever.
    async fn handle_cli_request(
        self: &Arc<Self>,
        frame: Value,
        events_tx: &mpsc::Sender<SessionEvent>,
        broker: &Arc<dyn crate::broker::PermissionBroker>,
        mcp: &Arc<McpServer>,
    ) {
        let request_id = frame
            .get("request_id")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_owned();
        let request = frame.get("request").cloned().unwrap_or(Value::Null);
        let subtype = request
            .get("subtype")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        match subtype {
            "can_use_tool" => {
                // A decision can take minutes at a console. NEVER block the
                // reader on it — spawn, decide, respond.
                let session = self.clone();
                let events_tx = events_tx.clone();
                let broker = broker.clone();
                tokio::spawn(async move {
                    let tool_name = request
                        .get("tool_name")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_owned();
                    let input = request.get("input").cloned().unwrap_or(json!({}));
                    let suggestions = request
                        .get("permission_suggestions")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let kind = crate::adapter::classify(&tool_name, &input);
                    let prompt = if kind == aspen_core::ToolKind::Question {
                        PromptKind::Question
                    } else {
                        PromptKind::Permission
                    };
                    let req = crate::broker::PermissionRequest {
                        request_id: request_id.clone(),
                        tool_name,
                        kind,
                        prompt,
                        decisions: crate::adapter::decisions_for(prompt, &suggestions),
                        questions: if prompt == PromptKind::Question { input.get("questions").cloned().unwrap_or(Value::Null) } else { Value::Null },
                        input,
                        suggestions,
                        raw: request,
                    };
                    let tool_name = req.tool_name.clone();
                    let (decision, by) = broker.decide(req).await;
                    let allowed = matches!(decision, crate::broker::BrokerDecision::Allow { .. });
                    let _ = events_tx
                        .send(SessionEvent::PermissionSettled {
                            request_id: request_id.clone(),
                            tool_name,
                            allowed,
                            by_policy: by == crate::broker::DecidedBy::Policy,
                        })
                        .await;
                    match decision {
                        crate::broker::BrokerDecision::Allow {
                            updated_input,
                            updated_permissions,
                            ..
                        } => {
                            // updatedInput is REQUIRED on allow; envelope is
                            // snake_case but this payload is camelCase
                            // (reference §3 invariant 2 — real, not a typo).
                            let mut payload =
                                json!({ "behavior": "allow", "updatedInput": updated_input });
                            if let Some(p) = updated_permissions {
                                payload["updatedPermissions"] = p;
                            }
                            session.respond_success(&request_id, payload).await;
                        }
                        crate::broker::BrokerDecision::Deny { message, .. } => {
                            session
                                .respond_success(
                                    &request_id,
                                    json!({ "behavior": "deny", "message": message }),
                                )
                                .await;
                        }
                    }
                });
            }
            "hook_callback" => {
                // `{}` is a valid HookJSONOutput; broken hooks must not
                // wedge a turn.
                self.respond_success(&request_id, json!({})).await;
            }
            "mcp_message" => {
                // Bus tools touch the store and disk: off the router, off
                // the runtime workers.
                let message = request.get("message").cloned().unwrap_or(Value::Null);
                let session = self.clone();
                let mcp = mcp.clone();
                tokio::spawn(async move {
                    let reply = tokio::task::spawn_blocking(move || mcp.handle(&message)).await.unwrap_or(Value::Null);
                    session.respond_success(&request_id, json!({ "mcp_response": reply })).await;
                });
            }
            "elicitation" => {
                self.respond_success(&request_id, json!({ "action": "decline" }))
                    .await;
            }
            other => {
                // Unknown subtype: error rather than silence — the CLI's own
                // dispatcher does the same for us.
                self.respond_error(&request_id, &format!("hub: unsupported subtype {other:?}"))
                    .await;
            }
        }
    }

    /// Re-read plugins/skills/commands from disk (reference §12.4). Returns
    /// the refreshed inventory the CLI reports.
    pub async fn reload_plugins(&self) -> Result<Value> {
        self.request(
            json!({ "subtype": "reload_plugins" }),
            Duration::from_secs(30),
        )
        .await
    }

    /// Rich context breakdown (reference §9): percentage, per-category
    /// tokens, a pre-computed visual grid. Poll at turn end, never mid-turn.
    pub async fn get_context_usage(&self) -> Result<Value> {
        self.request(
            json!({ "subtype": "get_context_usage" }),
            Duration::from_secs(30),
        )
        .await
    }

    /// Switch model; takes effect next turn (say so in the UI). Pass None
    /// or "default" to reset.
    pub async fn set_model(&self, model: Option<&str>) -> Result<Value> {
        self.request(
            json!({ "subtype": "set_model", "model": model }),
            Duration::from_secs(30),
        )
        .await
    }

    /// Live-switch the session-wide permission posture (reference §7.1).
    pub async fn set_permission_mode(&self, mode: &str) -> Result<Value> {
        self.request(
            json!({ "subtype": "set_permission_mode", "mode": mode }),
            Duration::from_secs(30),
        )
        .await
    }

    /// End-of-session ladder (reference §4.3): `end_session` → stdin EOF →
    /// kill. Transcripts persist in all cases.
    pub async fn shutdown_ladder(&self) {
        let end = self
            .request(json!({ "subtype": "end_session" }), Duration::from_secs(5))
            .await;
        if end.is_err() {
            tracing::debug!("end_session unavailable or timed out; closing stdin");
        }
        // Give the CLI a moment to drain and exit on its own…
        tokio::time::sleep(Duration::from_millis(1500)).await;
        // …then order the kill (no-op if it already exited).
        if let Some(tx) = self.kill_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}

/// The silent tier for Claude tool names: the neutral policy on the tool's
/// kind, plus Aspen's own bus tools (safe by construction — an agent that
/// cannot speak is stranded) and questions (allow-with-echo is honest when
/// nobody can answer).
pub fn policy_opinion(policy: PermissionPolicy, tool_name: &str, request: &Value) -> Option<bool> {
    if policy == PermissionPolicy::ReadOnlyAuto && tool_name.starts_with("mcp__aspen__") {
        return Some(true);
    }
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let kind = crate::adapter::classify(tool_name, &input);
    let prompt = if kind == aspen_core::ToolKind::Question {
        PromptKind::Question
    } else {
        PromptKind::Permission
    };
    aspen_core::permission::policy_opinion(policy, kind, prompt)
}

pub use aspen_core::permission::policy_deny_message;

/// Broker that answers purely from policy — the dev/headless default.
pub use aspen_core::permission::PolicyBroker;

#[async_trait]
impl SessionHandle for ClaudeSession {
    fn id(&self) -> SessionId {
        self.id
    }

    fn harness(&self) -> Harness {
        Harness::Claude
    }

    fn capabilities(&self) -> AdapterCapabilities {
        crate::adapter::capabilities()
    }

    fn runtime_info(&self) -> RuntimeInfo {
        let hs = self.handshake.get().unwrap_or(Value::Null);
        RuntimeInfo {
            model: None,
            mode: None,
            models: hs.get("models").and_then(|m| m.as_array()).cloned().unwrap_or_default(),
            commands: hs.get("commands").and_then(|m| m.as_array()).cloned().unwrap_or_default(),
            skills: Vec::new(),
            context_window: None,
            raw: hs,
        }
    }

    async fn set_model(&self, model: Option<&str>) -> Result<()> {
        ClaudeSession::set_model(self, model).await.map(|_| ())
    }

    async fn set_mode(&self, mode: &str) -> Result<()> {
        self.set_permission_mode(mode).await.map(|_| ())
    }

    async fn context_usage(&self) -> Result<Value> {
        self.get_context_usage().await
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    async fn mcp_servers(&self) -> Result<Vec<aspen_core::McpServerState>> {
        let v = self.request(json!({ "subtype": "mcp_status" }), Duration::from_secs(30)).await?;
        Ok(v.get("mcpServers")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().map(crate::adapter::mcp_state_from).collect())
            .unwrap_or_default())
    }

    async fn mcp_reconnect(&self, name: &str) -> Result<()> {
        // A failed reconnect answers with a control error carrying the
        // reason ("Connection closed"): that text is the diagnosis.
        self.request(json!({ "subtype": "mcp_reconnect", "serverName": name }), Duration::from_secs(60))
            .await
            .map(|_| ())
            .map_err(|e| anyhow!("{}", e.to_string().trim_start_matches("control error: ")))
    }

    async fn mcp_toggle(&self, name: &str, enabled: bool) -> Result<()> {
        self.request(json!({ "subtype": "mcp_toggle", "serverName": name, "enabled": enabled }), Duration::from_secs(30))
            .await
            .map(|_| ())
    }

    async fn mcp_authenticate(&self, name: &str) -> Result<aspen_core::McpAuth> {
        let v = self.request(json!({ "subtype": "mcp_authenticate", "serverName": name }), Duration::from_secs(60)).await?;
        Ok(aspen_core::McpAuth {
            url: v.get("authUrl").and_then(|u| u.as_str()).map(str::to_owned),
            requires_user: v.get("requiresUserAction").and_then(|b| b.as_bool()).unwrap_or(true),
        })
    }

    async fn mcp_add(&self, name: &str, config: Value) -> Result<()> {
        let v = self
            .request(json!({ "subtype": "mcp_set_servers", "servers": { name: config } }), Duration::from_secs(60))
            .await?;
        if let Some(errs) = v.get("errors").and_then(|e| e.as_object()).filter(|e| !e.is_empty()) {
            let text: Vec<String> = errs.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or(&v.to_string()))).collect();
            anyhow::bail!("{}", text.join("; "));
        }
        Ok(())
    }

    async fn reload(&self) -> Result<Value> {
        self.reload_plugins().await
    }

    async fn send_user(&self, text: String) -> Result<String> {
        // Generate the uuid BEFORE sending (the replay ack can race any
        // post-send bookkeeping). Callers correlate on UserReplay events.
        let uuid = Uuid::new_v4().to_string();
        self.write(json!({
            "type": "user",
            "message": { "role": "user", "content": text },
            "parent_tool_use_id": null,
            "uuid": uuid,
        }))
        .await?;
        Ok(uuid)
    }

    async fn send_user_content(&self, content: Value) -> Result<String> {
        let uuid = Uuid::new_v4().to_string();
        self.write(json!({
            "type": "user",
            "message": { "role": "user", "content": content },
            "parent_tool_use_id": null,
            "uuid": uuid,
        }))
        .await?;
        Ok(uuid)
    }

    async fn interrupt(&self) -> Result<()> {
        self.request(json!({ "subtype": "interrupt" }), Duration::from_secs(30))
            .await
            .map(|_| ())
    }

    async fn shutdown(&self) -> Result<()> {
        self.shutdown_ladder().await;
        Ok(())
    }
}
