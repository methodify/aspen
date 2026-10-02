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
    /// What `bin` launches: the executable a shell would run, and any
    /// environment the npm launcher would have set.
    launch: Launch,
    store: Arc<CodexStore>,
    /// `codex --version`, probed once per daemon (no window on Windows).
    version: std::sync::Arc<std::sync::OnceLock<Option<String>>>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self::with_bin("codex")
    }

    /// The adapter for a binary name or path (`ASPEN_CODEX_BIN`).
    pub fn with_bin(bin: &str) -> Self {
        let launch = resolve(bin).unwrap_or_else(|| Launch {
            path: bin.into(),
            env: Vec::new(),
        });
        let me = Self {
            bin: bin.into(),
            launch,
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
        let launch = self.launch.clone();
        std::thread::spawn(move || {
            let v = aspen_core::quiet_command(&launch.path)
                .envs(launch.env.iter().map(|(k, v)| (k, v)))
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
        resolve(&self.bin).is_some()
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// What a binary name launches.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub path: String,
    pub env: Vec<(String, String)>,
}

/// Resolve the Codex binary the way the operator's shell would: the first
/// PATH entry that has it. On Windows a process spawn by bare name finds
/// only `codex.exe`, but an npm install puts only `codex.cmd`/`codex.ps1`
/// shims on PATH — so the spawn skipped the operator's npm Codex and ran
/// an older one further down PATH (the Codex app's), which the backend
/// refused newer models for (2026-10-02). A shim resolves to the native
/// `codex.exe` its package ships, with the variables its launcher sets.
pub fn resolve(bin: &str) -> Option<Launch> {
    let plain = |p: std::path::PathBuf| Launch {
        path: p.to_string_lossy().into_owned(),
        env: Vec::new(),
    };
    if bin.contains('/') || bin.contains(std::path::MAIN_SEPARATOR) {
        let p = std::path::PathBuf::from(bin);
        return p.is_file().then(|| plain(p));
    }
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    resolve_in(bin, &dirs, cfg!(windows))
}

fn resolve_in(bin: &str, dirs: &[std::path::PathBuf], windows: bool) -> Option<Launch> {
    for dir in dirs {
        if !windows {
            let p = dir.join(bin);
            if p.is_file() {
                return Some(Launch {
                    path: p.to_string_lossy().into_owned(),
                    env: Vec::new(),
                });
            }
            continue;
        }
        let exe = dir.join(format!("{bin}.exe"));
        if exe.is_file() {
            return Some(Launch {
                path: exe.to_string_lossy().into_owned(),
                env: Vec::new(),
            });
        }
        let shim = ["cmd", "ps1", "bat"]
            .iter()
            .any(|ext| dir.join(format!("{bin}.{ext}")).is_file());
        if shim {
            if let Some(l) = npm_native(dir) {
                return Some(l);
            }
        }
    }
    None
}

/// The native binary inside an npm-installed `@openai/codex` next to its
/// shim, in the layouts its launcher (`bin/codex.js`) looks in.
fn npm_native(dir: &std::path::Path) -> Option<Launch> {
    let pkg = dir.join("node_modules").join("@openai").join("codex");
    if !pkg.is_dir() {
        return None;
    }
    let (plat, triple) = if std::env::consts::ARCH == "aarch64" {
        ("codex-win32-arm64", "aarch64-pc-windows-msvc")
    } else {
        ("codex-win32-x64", "x86_64-pc-windows-msvc")
    };
    let vendor = |root: std::path::PathBuf| root.join("vendor").join(triple);
    let nested = pkg.join("node_modules").join("@openai").join(plat);
    let hoisted = dir.join("node_modules").join("@openai").join(plat);
    let candidates = [
        vendor(nested).join("bin").join("codex.exe"),
        vendor(hoisted).join("bin").join("codex.exe"),
        vendor(pkg.clone()).join("bin").join("codex.exe"),
        vendor(pkg.clone()).join("codex").join("codex.exe"),
    ];
    let exe = candidates.into_iter().find(|p| p.is_file())?;
    let root = std::fs::canonicalize(&pkg).unwrap_or(pkg);
    Some(Launch {
        path: exe.to_string_lossy().into_owned(),
        env: vec![
            ("CODEX_MANAGED_BY_NPM".into(), "1".into()),
            (
                "CODEX_MANAGED_PACKAGE_ROOT".into(),
                root.to_string_lossy().into_owned(),
            ),
        ],
    })
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
            env: {
                let mut env = self.launch.env.clone();
                env.extend(spec.env.iter().cloned());
                env
            },
            codex_bin: self.launch.path.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"").unwrap();
    }

    fn triple() -> (&'static str, &'static str) {
        if std::env::consts::ARCH == "aarch64" {
            ("codex-win32-arm64", "aarch64-pc-windows-msvc")
        } else {
            ("codex-win32-x64", "x86_64-pc-windows-msvc")
        }
    }

    #[test]
    fn an_npm_shim_earlier_on_path_wins_over_a_later_exe() {
        let t = tempfile::tempdir().unwrap();
        let npm = t.path().join("nodejs");
        let app = t.path().join("app");
        touch(&npm.join("codex.cmd"));
        let (plat, tr) = triple();
        let exe = npm
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("node_modules")
            .join("@openai")
            .join(plat)
            .join("vendor")
            .join(tr)
            .join("bin")
            .join("codex.exe");
        touch(&exe);
        touch(&app.join("codex.exe"));
        let l = resolve_in("codex", &[npm.clone(), app], true).unwrap();
        assert_eq!(l.path, exe.to_string_lossy());
        assert!(l
            .env
            .iter()
            .any(|(k, v)| k == "CODEX_MANAGED_BY_NPM" && v == "1"));
    }

    #[test]
    fn a_shim_without_its_native_binary_is_passed_over() {
        let t = tempfile::tempdir().unwrap();
        let npm = t.path().join("nodejs");
        let app = t.path().join("app");
        touch(&npm.join("codex.cmd"));
        touch(&app.join("codex.exe"));
        let l = resolve_in("codex", &[npm, app.clone()], true).unwrap();
        assert_eq!(l.path, app.join("codex.exe").to_string_lossy());
        assert!(l.env.is_empty());
    }

    #[test]
    fn elsewhere_the_first_plain_binary_on_path() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        touch(&b.join("codex"));
        let l = resolve_in("codex", &[a, b.clone()], false).unwrap();
        assert_eq!(l.path, b.join("codex").to_string_lossy());
    }
}
