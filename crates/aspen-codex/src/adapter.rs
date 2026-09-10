//! The Codex adapter: capabilities, the posture → (approval policy,
//! sandbox) table, spawn, the store (HARNESSES.md; PROPOSALS-HARNESSES.md
//! §3.3).

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use aspen_core::{
    AgentAdapter, Harness, HarnessCapabilities, PermissionMode, Posture, SessionEvent,
    SessionHandle, SessionStore, SpawnSpec,
};

use crate::session::{CodexConfig, CodexSession};
use crate::store::CodexStore;

pub fn capabilities() -> HarnessCapabilities {
    HarnessCapabilities {
        recap: false,
        streaming: true,
        interrupt: true,
        mid_turn_inject: true,
        permission_callback: true,
        in_process_mcp: false,
        resume: true,
        fork: true,
        set_model: true,
        set_mode: true,
        context_usage: true,
        reload: false,
        slash_commands: false,
        skill_mentions: true,
        subagents: true,
        plugin_dirs: false,
        replay_ack: false,
        question_prompts: true,
        always_allow: true,
        transcript_on_disk: true,
        cost_from_harness: false,
        mcp_status: true,
        mcp_reconnect: true,
        mcp_toggle: false,
        mcp_auth: true,
        mcp_add: false,
    }
}

/// A Codex "mode" as Aspen names it: an approval policy plus a sandbox
/// (CODEX_RUNTIME_REFERENCE.md §5). Ids are what the mode select shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub approval: &'static str,
    pub sandbox: &'static str,
    pub reviewer: &'static str,
    pub posture: Option<Posture>,
}

pub const MODES: &[Mode] = &[
    Mode {
        id: "on-request",
        label: "on request",
        hint: "workspace writes go through; anything outside the sandbox asks",
        approval: "on-request",
        sandbox: "workspace-write",
        reviewer: "user",
        posture: Some(Posture::Ask),
    },
    Mode {
        id: "untrusted",
        label: "untrusted",
        hint: "every command not on the trusted list asks",
        approval: "untrusted",
        sandbox: "workspace-write",
        reviewer: "user",
        posture: None,
    },
    Mode {
        id: "read-only",
        label: "read only",
        hint: "read-only sandbox; writes ask",
        approval: "on-request",
        sandbox: "read-only",
        reviewer: "user",
        posture: Some(Posture::Plan),
    },
    Mode {
        id: "full-access",
        label: "full access",
        hint: "no sandbox, no prompts",
        approval: "never",
        sandbox: "danger-full-access",
        reviewer: "user",
        posture: Some(Posture::Auto),
    },
    Mode {
        id: "auto-review",
        label: "auto review",
        hint: "Codex's reviewer answers the prompts",
        approval: "on-request",
        sandbox: "workspace-write",
        reviewer: "auto_review",
        posture: Some(Posture::Guarded),
    },
];

pub fn mode(id: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.id == id)
}

pub struct CodexAdapter {
    pub bin: String,
    store: Arc<CodexStore>,
    /// `codex --version`, probed once per daemon (no window on Windows).
    version: std::sync::Arc<std::sync::OnceLock<Option<String>>>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        let me = Self {
            bin: "codex".into(),
            store: Arc::new(CodexStore::new()),
            version: Default::default(),
        };
        me.probe_version();
        me
    }

    /// `codex --version` on its own thread (see the Claude adapter: a
    /// probe inside the first API request stalled every request behind it).
    fn probe_version(&self) {
        let cell = self.version.clone();
        let bin = self.bin.clone();
        std::thread::spawn(move || {
            let v = aspen_core::quiet_command(&bin)
                .arg("--version")
                .output()
                .ok()
                .and_then(|out| {
                    let s = String::from_utf8_lossy(&out.stdout);
                    // "codex-cli 0.153.4"
                    s.split_whitespace()
                        .last()
                        .map(str::to_owned)
                        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
                });
            let _ = cell.set(v);
        });
    }
    /// Is the binary on PATH?
    pub fn available(&self) -> bool {
        which(&self.bin).is_some()
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn which(bin: &str) -> Option<std::path::PathBuf> {
    if bin.contains(std::path::MAIN_SEPARATOR) {
        let p = std::path::PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(bin);
        if p.is_file() {
            return Some(p);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let p = dir.join(format!("{bin}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn harness(&self) -> Harness {
        Harness::Codex
    }
    fn capabilities(&self) -> HarnessCapabilities {
        capabilities()
    }
    fn permission_modes(&self) -> Vec<PermissionMode> {
        MODES
            .iter()
            .map(|m| PermissionMode {
                id: m.id.into(),
                label: m.label.into(),
                hint: m.hint.into(),
                posture: m.posture,
            })
            .collect()
    }
    fn mode_for_posture(&self, posture: Posture) -> Option<String> {
        let id = match posture {
            Posture::Ask | Posture::Edits => "on-request",
            Posture::Plan => "read-only",
            Posture::Auto => "full-access",
            Posture::Guarded => "auto-review",
        };
        Some(id.into())
    }
    async fn spawn(
        &self,
        spec: SpawnSpec,
    ) -> Result<(Arc<dyn SessionHandle>, mpsc::Receiver<SessionEvent>)> {
        let mode_id = spec
            .mode
            .clone()
            .filter(|m| mode(m).is_some())
            .or_else(|| spec.posture.and_then(|p| self.mode_for_posture(p)))
            .unwrap_or_else(|| "on-request".into());
        let cfg = CodexConfig {
            repo: spec.repo.clone(),
            session_id: spec.session_id,
            resume: spec.resume.clone(),
            fork: spec.fork,
            model: spec.model.clone(),
            mode: mode_id,
            policy: spec.policy,
            charter: spec.charter.clone(),
            extra_args: spec.extra_args.clone(),
            env: spec.env.clone(),
            codex_bin: self.bin.clone(),
            agent: spec.agent.clone(),
            bridge: spec.node_api.clone().map(|api| crate::session::Bridge {
                node_api: api,
                token: spec.bridge_token.clone(),
                aspen_bin: std::env::current_exe()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "aspen".into()),
                tools: spec.tools.clone(),
            }),
        };
        let broker: Arc<dyn aspen_core::PermissionBroker> = match spec.broker.clone() {
            Some(b) => b,
            None => Arc::new(aspen_core::PolicyBroker(spec.policy)),
        };
        let (session, rx) = CodexSession::start(cfg, broker).await?;
        Ok((session, rx))
    }
    fn store(&self) -> Arc<dyn SessionStore> {
        self.store.clone()
    }
    fn version(&self) -> Option<String> {
        self.version.get().cloned().flatten()
    }
    fn binary(&self) -> &'static str {
        "codex"
    }
}
