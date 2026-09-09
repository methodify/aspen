//! The Claude Code harness behind the seam (docs/HARNESSES.md): the
//! `AgentAdapter` that maps a neutral `SpawnSpec` onto `ClaudeConfig`, the
//! `SessionStore` over the JSONL transcripts, tool classification by
//! Claude's tool names, and the permission modes → posture table.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use aspen_core::{
    AgentAdapter, DecisionOption, DecisionScope, Harness, HarnessCapabilities, PermissionMode, Posture,
    ProjectDirs, PromptKind, SessionEvent, SessionHandle, SessionInfo, SessionOrigin, SessionStore,
    SpawnSpec, ToolKind,
};

use crate::session::{ClaudeConfig, ClaudeSession};

/// What a Claude tool does (PROPOSALS-HARNESSES.md §3.2).
pub fn classify(tool_name: &str, input: &Value) -> ToolKind {
    match tool_name {
        "Bash" | "PowerShell" | "Shell" => ToolKind::Shell,
        "Write" | "NotebookEdit" => ToolKind::FileWrite,
        "Edit" | "MultiEdit" => ToolKind::FileEdit,
        "Read" | "NotebookRead" | "TaskOutput" => ToolKind::FileRead,
        "Glob" | "Grep" | "LS" => ToolKind::Search,
        "WebSearch" | "WebFetch" => ToolKind::Web,
        "Agent" | "Task" | "Workflow" | "ScheduleWakeup" | "CronCreate" | "Monitor" => ToolKind::Agent,
        "AskUserQuestion" => ToolKind::Question,
        n if n.starts_with("mcp__") => ToolKind::Mcp,
        _ => {
            if input.get("questions").is_some() {
                ToolKind::Question
            } else {
                ToolKind::Other
            }
        }
    }
}

/// The adapter's one-line reading of a call: summary, path, command.
pub fn describe(tool_name: &str, input: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let path = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input.get(k).and_then(|v| v.as_str()))
        .map(str::to_owned);
    let command = input.get("command").and_then(|v| v.as_str()).map(str::to_owned);
    let summary = command
        .clone()
        .or_else(|| path.clone())
        .or_else(|| input.get("pattern").and_then(|v| v.as_str()).map(str::to_owned))
        .or_else(|| input.get("query").and_then(|v| v.as_str()).map(str::to_owned))
        .or_else(|| input.get("description").and_then(|v| v.as_str()).map(str::to_owned))
        .or_else(|| input.get("prompt").and_then(|v| v.as_str()).map(|p| p.chars().take(120).collect()));
    let _ = tool_name;
    (summary, path, command)
}

/// The answers a Claude prompt takes: allow / always allow (when the CLI
/// suggested rules) / deny.
pub fn decisions_for(kind: PromptKind, suggestions: &Value) -> Vec<DecisionOption> {
    if kind == PromptKind::Question {
        return vec![DecisionOption { id: "answer".into(), label: "answer".into(), allow: true, scope: DecisionScope::Once, payload: None }];
    }
    let mut v = vec![DecisionOption { id: "allow".into(), label: "allow".into(), allow: true, scope: DecisionScope::Once, payload: None }];
    if suggestions.as_array().is_some_and(|a| !a.is_empty()) {
        v.push(DecisionOption {
            id: "always".into(),
            label: "always allow".into(),
            allow: true,
            scope: DecisionScope::Always,
            payload: Some(suggestions.clone()),
        });
    }
    v.push(DecisionOption { id: "deny".into(), label: "deny".into(), allow: false, scope: DecisionScope::Once, payload: None });
    v
}

pub struct ClaudeAdapter {
    pub bin: String,
    store: Arc<ClaudeStore>,
    /// `claude --version`, probed once per daemon: the API asks on every
    /// console poll, and a spawn per poll flashed a console on Windows.
    version: std::sync::OnceLock<Option<String>>,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self { bin: "claude".into(), store: Arc::new(ClaudeStore), version: std::sync::OnceLock::new() }
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn capabilities() -> HarnessCapabilities {
    HarnessCapabilities {
        streaming: true,
        interrupt: true,
        mid_turn_inject: true,
        permission_callback: true,
        in_process_mcp: true,
        resume: true,
        fork: true,
        set_model: true,
        set_mode: true,
        context_usage: true,
        reload: true,
        slash_commands: true,
        skill_mentions: false,
        subagents: true,
        plugin_dirs: true,
        replay_ack: true,
        question_prompts: true,
        always_allow: true,
        transcript_on_disk: true,
        cost_from_harness: true,
        mcp_status: true,
        mcp_reconnect: true,
        mcp_toggle: true,
        mcp_auth: true,
        mcp_add: true,
    }
}

/// One entry of `mcp_status`'s reply (or of `system/init`'s `mcp_servers`)
/// in the neutral shape (CLAUDE_RUNTIME_REFERENCE.md; PROPOSALS-MCP.md §2.1).
pub fn mcp_state_from(v: &Value) -> aspen_core::McpServerState {
    use aspen_core::McpStatus;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    let status = match s("status").as_deref() {
        Some("connected") => McpStatus::Connected,
        Some("failed") => McpStatus::Failed,
        Some("needs-auth") | Some("needs_auth") => McpStatus::NeedsAuth,
        Some("disabled") => McpStatus::Disabled,
        _ => McpStatus::Pending,
    };
    let cfg = v.get("config").cloned().unwrap_or(Value::Null);
    let transport = cfg.get("type").and_then(|t| t.as_str()).map(|t| match t {
        "claudeai-proxy" => "proxy".to_owned(),
        other => other.to_owned(),
    });
    let command = cfg
        .get("command")
        .and_then(|c| c.as_str())
        .map(|c| {
            let args: Vec<&str> = cfg.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            if args.is_empty() { c.to_owned() } else { format!("{c} {}", args.join(" ")) }
        })
        .or_else(|| cfg.get("url").and_then(|u| u.as_str()).map(str::to_owned));
    let server = v.get("serverInfo").and_then(|i| {
        let n = i.get("name").and_then(|x| x.as_str())?;
        Some(match i.get("version").and_then(|x| x.as_str()) {
            Some(ver) => format!("{n} {ver}"),
            None => n.to_owned(),
        })
    });
    let tools = v
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_owned)).collect())
        .unwrap_or_default();
    let scope = s("scope");
    let plugin = scope.as_deref().and_then(|sc| sc.strip_prefix("plugin:").map(str::to_owned)).or_else(|| s("pluginName")).or_else(|| s("plugin"));
    aspen_core::McpServerState {
        name: s("name").unwrap_or_default(),
        status,
        error: s("error"),
        scope,
        transport,
        command,
        server,
        tools,
        plugin,
    }
}

#[async_trait]
impl AgentAdapter for ClaudeAdapter {
    fn harness(&self) -> Harness {
        Harness::Claude
    }
    fn capabilities(&self) -> HarnessCapabilities {
        capabilities()
    }
    fn permission_modes(&self) -> Vec<PermissionMode> {
        vec![
            PermissionMode { id: "default".into(), label: "default".into(), hint: "prompt for writes and commands".into(), posture: Some(Posture::Ask) },
            PermissionMode { id: "acceptEdits".into(), label: "accept edits".into(), hint: "edits go through; commands prompt".into(), posture: Some(Posture::Edits) },
            PermissionMode { id: "plan".into(), label: "plan".into(), hint: "read-only exploration".into(), posture: Some(Posture::Plan) },
            PermissionMode { id: "bypassPermissions".into(), label: "bypass permissions".into(), hint: "no prompts at all".into(), posture: Some(Posture::Auto) },
            PermissionMode { id: "dontAsk".into(), label: "don't ask".into(), hint: "deny anything that would prompt".into(), posture: None },
        ]
    }
    fn mode_for_posture(&self, posture: Posture) -> Option<String> {
        match posture {
            Posture::Ask => Some("default".into()),
            Posture::Edits => Some("acceptEdits".into()),
            Posture::Plan => Some("plan".into()),
            Posture::Auto => Some("bypassPermissions".into()),
            Posture::Guarded => None,
        }
    }
    async fn spawn(&self, spec: SpawnSpec) -> Result<(Arc<dyn SessionHandle>, tokio::sync::mpsc::Receiver<SessionEvent>)> {
        let mut cfg = ClaudeConfig::new(spec.repo.clone());
        cfg.session_id = spec.session_id;
        cfg.claude_bin = self.bin.clone();
        cfg.model = spec.model.clone();
        cfg.resume = spec.resume.clone();
        cfg.fork = spec.fork;
        cfg.resume_at = spec.resume_at.clone();
        cfg.permission_mode = spec
            .mode
            .clone()
            .or_else(|| spec.posture.and_then(|p| self.mode_for_posture(p)).filter(|m| m != "default"));
        cfg.policy = spec.policy;
        cfg.charter = spec.charter.clone();
        cfg.extra_args = spec.extra_args.clone();
        for d in &spec.plugin_dirs {
            cfg.extra_args.push("--plugin-dir".into());
            cfg.extra_args.push(d.clone());
        }
        let mcp = match &spec.tools {
            Some(t) => crate::mcp::McpServer::from_provider(t.clone()),
            None => crate::mcp::McpServer::new(),
        };
        let (handle, rx) = match &spec.broker {
            Some(b) => ClaudeSession::spawn_with_broker(cfg, mcp, b.clone()).await?,
            None => ClaudeSession::spawn(cfg, mcp).await?,
        };
        Ok((handle as Arc<dyn SessionHandle>, rx))
    }
    fn store(&self) -> Arc<dyn SessionStore> {
        self.store.clone()
    }
    fn version(&self) -> Option<String> {
        self.version
            .get_or_init(|| {
                aspen_core::quiet_command(&self.bin)
                    .arg("--version")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
            })
            .clone()
    }
    fn binary(&self) -> &'static str {
        "claude"
    }
}

/// The JSONL transcript store.
pub struct ClaudeStore;

impl SessionStore for ClaudeStore {
    fn harness(&self) -> Harness {
        Harness::Claude
    }
    fn exists(&self, repo: &Path, sid: &str) -> bool {
        crate::transcript::transcript_path(repo, sid).is_file()
    }
    fn modified(&self, repo: &Path, sid: &str) -> Option<f64> {
        std::fs::metadata(crate::transcript::transcript_path(repo, sid))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
    }
    fn enumerate(&self, repo: &Path) -> Result<Vec<SessionInfo>> {
        Ok(crate::transcript::enumerate_sessions(repo)?
            .into_iter()
            .map(|s| SessionInfo {
                session_id: s.session_id,
                title: s.title,
                entrypoint: s.entrypoint,
                modified_epoch: s.modified_epoch,
                user_messages: s.user_messages,
                harness: Harness::Claude,
            })
            .collect())
    }
    fn rehydrate(&self, repo: &Path, sid: &str) -> Result<Vec<Value>> {
        crate::transcript::rehydrate(repo, sid)
    }
    fn rehydrate_after(&self, repo: &Path, sid: &str, after: &str) -> Result<(Vec<Value>, bool)> {
        crate::transcript::rehydrate_after(repo, sid, after)
    }
    fn rehydrate_file(&self, path: &Path) -> Result<Vec<Value>> {
        crate::transcript::rehydrate_file(path)
    }
    fn origin(&self, repo: &Path, sid: &str) -> Option<SessionOrigin> {
        let o = crate::transcript::session_origin(repo, sid);
        Some(SessionOrigin {
            forked_from: o.forked_from,
            first_ref: o.first_uuid,
            last_entrypoint: o.last_entrypoint,
            last_ts: o.last_ts,
        })
    }
    fn files(&self, repo: &Path, sid: &str) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let main = crate::transcript::transcript_path(repo, sid);
        if main.is_file() {
            out.push((format!("{sid}.jsonl"), main.clone()));
        }
        let Some(project) = main.parent() else { return out };
        let sub = project.join(sid);
        if sub.is_dir() {
            let mut stack = vec![(sub.clone(), String::new())];
            while let Some((dir, prefix)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    let name = e.file_name().to_string_lossy().to_string();
                    let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                    if p.is_dir() {
                        stack.push((p, rel));
                    } else if p.is_file() {
                        out.push((format!("{sid}/{rel}"), p));
                    }
                }
            }
        }
        out
    }
    fn main_path(&self, repo: &Path, sid: &str) -> PathBuf {
        crate::transcript::transcript_path(repo, sid)
    }
    fn usage(&self, repo: &Path, sid: &str) -> Value {
        serde_json::to_value(&*crate::usage::session_usage(repo, sid)).unwrap_or(Value::Null)
    }
    fn activities(&self, repo: &Path, sid: &str, since: Option<f64>) -> Vec<Value> {
        crate::activity::activities_for(repo, sid, since)
            .into_iter()
            .filter_map(|a| serde_json::to_value(a).ok())
            .collect()
    }
    fn activity_counts(&self, repo: &Path, sid: Option<&str>, since: Option<f64>) -> Value {
        let c = match sid {
            Some(s) => crate::activity::counts(&crate::activity::activities_for(repo, s, since)),
            None => crate::activity::ActivityCounts::default(),
        };
        serde_json::to_value(c).unwrap_or(Value::Null)
    }
    fn subagent(&self, repo: &Path, sid: &str, agent_id: &str) -> Result<Vec<Value>> {
        crate::transcript::rehydrate_file(&crate::activity::subagent_transcript(repo, sid, agent_id))
    }
    fn project_dirs(&self, repo: &Path) -> ProjectDirs {
        let home = crate::transcript::claude_home();
        let encoded = crate::transcript::project_slug(repo);
        let project_dir = home.join("projects").join(&encoded);
        ProjectDirs {
            memory_dir: Some(project_dir.join("memory")),
            artifact_roots: vec![project_dir.clone(), home.join("image-cache"), home.join("plans")],
            project_dir: Some(project_dir),
            encoded: Some(encoded),
            home: Some(home),
        }
    }
    fn discover_repos(&self) -> Vec<(PathBuf, usize)> {
        crate::transcript::discover_repos()
            .into_iter()
            .map(|d| (d.path, d.sessions))
            .collect()
    }
}

/// Neutral usage shape for `TurnEnded.usage` from a `result` frame.
pub fn turn_usage(raw: &Value) -> Value {
    match raw.get("usage") {
        Some(u) => {
            let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            json!({
                "input": n("input_tokens"),
                "output": n("output_tokens"),
                "cache_read": n("cache_read_input_tokens"),
                "cache_create": n("cache_creation_input_tokens"),
                "cumulative": false,
            })
        }
        None => Value::Null,
    }
}
