//! Servicing: the node's self-update state machine, inventory, and the
//! rolling fleet rollout (docs/SERVICING.md).
//!
//! The daemon never replaces its own binary in-process. It *checks* the
//! release channel, *drains* (refuses new spawns, waits for the quiet
//! gate), then launches `aspen update --restart --unattended` detached and
//! lets that stop it. The updater keeps a rollback slot, health-checks the
//! new daemon, and leaves `update-outcome.json` for the next daemon to
//! report. Peers only ever *hint* that a release exists; every node checks
//! and verifies for itself.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::node::{NodeInner, TurnState};
use crate::release::{self, ReleaseInfo};
use crate::store::now_epoch;

/// A session must have been idle this long (and nothing spawned this
/// recently) before a restart is considered safe.
pub fn quiet_secs() -> f64 {
    std::env::var("ASPEN_QUIET_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300.0)
}
fn check_interval_secs() -> u64 {
    std::env::var("ASPEN_CHECK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6 * 3600)
}
/// A drain that has waited this long is flagged so the console escalates.
const OVERDUE_SECS: f64 = 24.0 * 3600.0;
/// Hints from peers trigger a check at most this often.
const HINT_MIN_GAP_SECS: f64 = 60.0;
/// The fleet event "agent" for node-level servicing events.
pub const NODE_AGENT: &str = "node";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeState {
    Ready,
    Draining {
        since: f64,
        /// policy | operator | peer:<name>
        by: String,
        /// quiet | now
        when: String,
        /// Why we haven't gone yet (empty = about to).
        waiting_on: Vec<String>,
        overdue: bool,
        target: String,
    },
    Updating {
        since: f64,
        by: String,
        target: String,
    },
}

impl NodeState {
    pub fn name(&self) -> &'static str {
        match self {
            NodeState::Ready => "ready",
            NodeState::Draining { .. } => "draining",
            NodeState::Updating { .. } => "updating",
        }
    }
    /// One line for rosters and status readouts.
    pub fn detail(&self) -> Option<String> {
        match self {
            NodeState::Ready => None,
            NodeState::Draining {
                waiting_on,
                when,
                overdue,
                ..
            } => Some(if waiting_on.is_empty() {
                format!("update ({when}) about to start")
            } else {
                format!(
                    "{}waiting on: {}",
                    if *overdue { "overdue · " } else { "" },
                    waiting_on.join(", ")
                )
            }),
            NodeState::Updating { since, .. } => Some(format!("updater running since {since:.0}")),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub at: f64,
    pub ok: bool,
    pub error: Option<String>,
}

/// What the updater leaves behind (`update-outcome.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub from: String,
    pub to: String,
    pub ok: bool,
    #[serde(default)]
    pub rolled_back: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub trigger: String,
    pub started_at: f64,
    pub finished_at: f64,
    /// Set once a daemon has recorded the fleet event for it.
    #[serde(default)]
    pub recorded: bool,
}

pub fn outcome_path(data_dir: &Path) -> PathBuf {
    data_dir.join("update-outcome.json")
}

pub fn read_outcome(data_dir: &Path) -> Option<Outcome> {
    std::fs::read_to_string(outcome_path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

pub fn write_outcome(data_dir: &Path, o: &Outcome) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(outcome_path(data_dir), serde_json::to_string_pretty(o)?)?;
    Ok(())
}

/// What is on this machine, for the roster and the console's node rows.
#[derive(Debug, Serialize)]
pub struct Inventory {
    pub os: &'static str,
    pub arch: &'static str,
    pub claude_version: Mutex<Option<String>>,
    pub started_at: f64,
    pub pid: u32,
}

impl Inventory {
    pub fn json(&self) -> Value {
        json!({
            "os": self.os,
            "arch": self.arch,
            "claude_version": *self.claude_version.lock().unwrap(),
            "started_at": self.started_at,
            "pid": self.pid,
        })
    }
}

/// The rolling fleet rollout's progress (lives on the node that runs it).
#[derive(Debug, Clone, Serialize, Default)]
pub struct Rollout {
    pub target: String,
    pub when: String,
    pub order: Vec<String>,
    pub done: Vec<String>,
    pub current: Option<String>,
    pub failed: Option<(String, String)>,
    pub stopped: bool,
    pub finished: bool,
    pub started_at: f64,
    pub finished_at: Option<f64>,
}

pub struct Servicing {
    /// The running binary's version.
    pub current: String,
    /// The binary to run the updater with (resolved at start: after a unix
    /// rename, /proc/self/exe reads "(deleted)").
    pub exe: Option<PathBuf>,
    pub available: Mutex<Option<ReleaseInfo>>,
    pub last_check: Mutex<Option<CheckResult>>,
    /// Versions already announced (event + hint), so each is once.
    announced: Mutex<HashSet<String>>,
    pub state: Mutex<NodeState>,
    pub last_spawn: Mutex<Option<f64>>,
    pub last_outcome: Mutex<Option<Outcome>>,
    pub inventory: Inventory,
    /// The updater child while it runs — so a failure *before* it stops us
    /// is noticed and the node returns to ready.
    updater: Mutex<Option<std::process::Child>>,
    pub check_now: tokio::sync::Notify,
    wake: tokio::sync::Notify,
    pub rollout: Mutex<Option<Rollout>>,
    rollout_cancel: AtomicBool,
}

impl Servicing {
    pub fn new(current: String, exe: Option<PathBuf>) -> Self {
        Self {
            current,
            exe,
            available: Mutex::new(None),
            last_check: Mutex::new(None),
            announced: Mutex::new(HashSet::new()),
            state: Mutex::new(NodeState::Ready),
            last_spawn: Mutex::new(None),
            last_outcome: Mutex::new(None),
            inventory: Inventory {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
                claude_version: Mutex::new(None),
                started_at: now_epoch(),
                pid: std::process::id(),
            },
            updater: Mutex::new(None),
            check_now: tokio::sync::Notify::new(),
            wake: tokio::sync::Notify::new(),
            rollout: Mutex::new(None),
            rollout_cancel: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> NodeState {
        self.state.lock().unwrap().clone()
    }

    /// New work is refused while an update is pending.
    pub fn accepting_spawns(&self) -> bool {
        matches!(self.state(), NodeState::Ready)
    }

    pub fn note_spawn(&self) {
        *self.last_spawn.lock().unwrap() = Some(now_epoch());
    }

    /// The newest release we know of, if newer than what runs.
    pub fn newer(&self) -> Option<ReleaseInfo> {
        self.available
            .lock()
            .unwrap()
            .clone()
            .filter(|r| release::is_newer(&r.version, &self.current))
    }

    /// We run something newer than what is published (a retracted tag).
    pub fn withdrawn(&self) -> bool {
        self.available
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| release::is_newer(&self.current, &r.version))
    }

    /// The compact form that rides the roster.
    pub fn roster_json(&self, policy_mode: &str) -> Value {
        let st = self.state();
        json!({
            "available": self.newer().map(|r| r.version),
            "withdrawn": self.withdrawn(),
            "state": st.name(),
            "state_detail": st.detail(),
            "policy": policy_mode,
            "inventory": self.inventory.json(),
            "last_outcome": self.last_outcome.lock().unwrap().as_ref().map(|o| json!({
                "from": o.from, "to": o.to, "ok": o.ok, "rolled_back": o.rolled_back,
                "error": o.error, "finished_at": o.finished_at,
            })),
        })
    }
}

/// Full status for GET /api/update and `aspen status`.
pub fn status_json(inner: &Arc<NodeInner>) -> Value {
    let s = &inner.servicing;
    let policy = inner
        .data_dir
        .as_deref()
        .map(crate::settings::load)
        .unwrap_or_default()
        .update;
    let available = s.available.lock().unwrap().clone();
    let newer = s.newer();
    let skipped = newer
        .as_ref()
        .is_some_and(|r| policy.skip.as_deref() == Some(r.version.as_str()));
    let soaked = newer.as_ref().map(|r| soaked(r, &policy));
    json!({
        "current": s.current,
        "sha": crate::federation::VERSION.get().map(|(_, s)| s.clone()),
        "available": newer,
        "latest": available.as_ref().map(|r| r.version.clone()),
        "behind": newer.is_some(),
        "withdrawn": s.withdrawn(),
        "skipped": skipped,
        "soaked": soaked,
        "last_check": *s.last_check.lock().unwrap(),
        "state": s.state(),
        "policy": policy,
        "policy_effective": {
            "auto": policy.auto(),
            "in_window": policy.in_window_now(),
            "quiet_secs": quiet_secs(),
        },
        "waiting_on": if matches!(s.state(), NodeState::Ready) { quiet_gate(inner, false) } else { Vec::new() },
        "inventory": s.inventory.json(),
        "last_outcome": *s.last_outcome.lock().unwrap(),
        "rollout": *s.rollout.lock().unwrap(),
        "exe": s.exe.as_ref().map(|p| p.to_string_lossy().into_owned()),
    })
}

fn soaked(r: &ReleaseInfo, policy: &crate::settings::UpdateSettings) -> bool {
    match (policy.soak_secs(), r.published_at) {
        (Some(soak), Some(published)) => now_epoch() - published >= soak as f64,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Why a restart is not safe right now (empty = safe). `force` skips the
/// session gates (the human said *now*).
pub fn quiet_gate(inner: &Arc<NodeInner>, force: bool) -> Vec<String> {
    if force {
        return Vec::new();
    }
    let quiet = quiet_secs();
    let now = now_epoch();
    let mut out = Vec::new();
    let sessions: Vec<Arc<crate::node::ManagedSession>> =
        inner.sessions.lock().unwrap().values().cloned().collect();
    for s in &sessions {
        if s.turn_state() == TurnState::Busy {
            out.push(format!("{} busy", s.name));
            continue;
        }
        let idle_since = s.summary.lock().unwrap().idle_since;
        if let Some(since) = idle_since {
            if now - since < quiet {
                out.push(format!("{} idle only {:.0}s", s.name, now - since));
            }
        }
        if let Some(b) = &s.broker {
            if !b.open_prompts().is_empty() {
                out.push(format!("{} has an open prompt", s.name));
            }
        }
    }
    if let Some(t) = *inner.servicing.last_spawn.lock().unwrap() {
        if now - t < quiet {
            out.push(format!("a session spawned {:.0}s ago", now - t));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Ask this node to update: drain, then go. `when` is "quiet" or "now".
pub fn request(inner: &Arc<NodeInner>, when: &str, by: &str) -> Result<NodeState> {
    let s = &inner.servicing;
    let when = match when {
        "now" => "now",
        _ => "quiet",
    };
    let target = s.newer().map(|r| r.version).ok_or_else(|| {
        anyhow!(
            "no newer release known (running {}; latest seen {})",
            s.current,
            s.available
                .lock()
                .unwrap()
                .as_ref()
                .map(|r| r.version.clone())
                .unwrap_or_else(|| "none — check first".into())
        )
    })?;
    if s.exe.is_none() {
        return Err(anyhow!(
            "this daemon has no known executable path to update"
        ));
    }
    let mut st = s.state.lock().unwrap();
    match &*st {
        NodeState::Updating { .. } => return Err(anyhow!("an update is already running")),
        NodeState::Draining { .. } if when != "now" => {
            // Already draining; a second "quiet" request is a no-op.
            return Ok(st.clone());
        }
        _ => {}
    }
    *st = NodeState::Draining {
        since: now_epoch(),
        by: by.to_owned(),
        when: when.to_owned(),
        waiting_on: Vec::new(),
        overdue: false,
        target: target.clone(),
    };
    let snapshot = st.clone();
    drop(st);
    let _ = inner.store.record_event(
        NODE_AGENT,
        "update_requested",
        json!({ "to": target, "when": when, "by": by }),
    );
    s.wake.notify_one();
    Ok(snapshot)
}

pub fn cancel(inner: &Arc<NodeInner>, by: &str) -> Result<bool> {
    let s = &inner.servicing;
    let mut st = s.state.lock().unwrap();
    match &*st {
        NodeState::Draining { target, .. } => {
            let target = target.clone();
            *st = NodeState::Ready;
            drop(st);
            let _ = inner.store.record_event(
                NODE_AGENT,
                "update_cancelled",
                json!({ "to": target, "by": by }),
            );
            Ok(true)
        }
        NodeState::Updating { .. } => Err(anyhow!(
            "the updater is already running; too late to cancel"
        )),
        NodeState::Ready => Ok(false),
    }
}

/// Check the release channel now (blocking network; run on a blocking
/// thread). Records the result, announces a newer version once (event +
/// hint to peers), and returns what it found.
pub fn check(inner: &Arc<NodeInner>) -> Result<ReleaseInfo> {
    let s = &inner.servicing;
    let ch = release::channel();
    let res = release::fetch(&ch, None);
    let mut lc = s.last_check.lock().unwrap();
    match &res {
        Ok(r) => {
            *lc = Some(CheckResult {
                at: now_epoch(),
                ok: true,
                error: None,
            });
            drop(lc);
            *s.available.lock().unwrap() = Some(r.clone());
            if release::is_newer(&r.version, &s.current)
                && s.announced.lock().unwrap().insert(r.version.clone())
                && !already_announced(inner, &r.version)
            {
                let _ = inner.store.record_event(
                    NODE_AGENT,
                    "update_available",
                    json!({ "version": r.version, "tag": r.tag, "current": s.current }),
                );
                hint_peers(inner, r);
            }
        }
        Err(e) => {
            *lc = Some(CheckResult {
                at: now_epoch(),
                ok: false,
                error: Some(format!("{e:#}")),
            });
        }
    }
    res
}

/// Was `update_available` for this version already recorded by an earlier
/// daemon process (the in-memory set only covers this one)?
fn already_announced(inner: &Arc<NodeInner>, version: &str) -> bool {
    let now = now_epoch();
    inner
        .store
        .events(now - 30.0 * 86400.0, now, Some(NODE_AGENT), 500)
        .unwrap_or_default()
        .iter()
        .any(|e| e.kind == "update_available" && e.detail["version"].as_str() == Some(version))
}

/// Tell linked peers a release exists. A hint, never authority: the peer
/// checks the channel itself.
fn hint_peers(inner: &Arc<NodeInner>, r: &ReleaseInfo) {
    let Some(mesh) = inner.mesh() else { return };
    let payload = json!({ "t": "update_hint", "version": r.version, "tag": r.tag });
    let peers: Vec<String> = mesh.links.lock().unwrap().keys().cloned().collect();
    for p in peers {
        let _ = mesh.send_to(&p, &payload);
    }
}

/// A peer said a release exists. Worth a look if it claims something newer
/// and we haven't checked in the last minute.
pub fn on_hint(inner: &Arc<NodeInner>, version: &str) {
    let s = &inner.servicing;
    if !release::is_newer(version, &s.current) {
        return;
    }
    tracing::info!(
        version,
        "update hint from a peer; checking the release channel"
    );
    let recent = s
        .last_check
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|c| now_epoch() - c.at < HINT_MIN_GAP_SECS);
    if !recent {
        s.check_now.notify_one();
    }
}

/// Policy: if auto, a newer release is known, it isn't snoozed, and it has
/// soaked, request a quiet drain. Idempotent.
fn evaluate_policy(inner: &Arc<NodeInner>) {
    let s = &inner.servicing;
    if !matches!(s.state(), NodeState::Ready) {
        return;
    }
    let policy = inner
        .data_dir
        .as_deref()
        .map(crate::settings::load)
        .unwrap_or_default()
        .update;
    if !policy.auto() {
        return;
    }
    let Some(r) = s.newer() else { return };
    if policy.skip.as_deref() == Some(r.version.as_str()) || !soaked(&r, &policy) {
        return;
    }
    let _ = request(inner, "quiet", "policy");
}

/// One drain tick: refresh what we're waiting on; launch when clear.
fn drain_tick(inner: &Arc<NodeInner>) {
    let s = &inner.servicing;
    consume_outcome(inner);
    // An updater that died before stopping us: back to ready, with its
    // reason (it writes the outcome file on failure).
    {
        let mut child = s.updater.lock().unwrap();
        if let Some(c) = child.as_mut() {
            if let Ok(Some(status)) = c.try_wait() {
                *child = None;
                drop(child);
                let outcome = inner.data_dir.as_deref().and_then(read_outcome);
                let msg = outcome
                    .as_ref()
                    .and_then(|o| o.error.clone())
                    .unwrap_or_else(|| format!("updater exited with {status}"));
                if let Some(o) = outcome {
                    *s.last_outcome.lock().unwrap() = Some(o);
                }
                let mut st = s.state.lock().unwrap();
                if let NodeState::Updating { target, .. } = &*st {
                    let target = target.clone();
                    *st = NodeState::Ready;
                    drop(st);
                    let _ = inner.store.record_event(
                        NODE_AGENT,
                        "update_failed",
                        json!({ "to": target, "error": msg }),
                    );
                }
            }
        }
    }
    let st = s.state();
    let NodeState::Draining {
        since,
        by,
        when,
        target,
        ..
    } = st
    else {
        evaluate_policy(inner);
        return;
    };
    let policy = inner
        .data_dir
        .as_deref()
        .map(crate::settings::load)
        .unwrap_or_default()
        .update;
    let force = when == "now";
    let mut waiting = quiet_gate(inner, force);
    if !force && by == "policy" && !policy.in_window_now() {
        waiting.push(format!(
            "outside update window {}",
            policy.window.as_deref().unwrap_or("")
        ));
    }
    if waiting.is_empty() {
        launch_updater(inner, &by, &target);
        return;
    }
    let overdue = now_epoch() - since > OVERDUE_SECS;
    let mut cur = s.state.lock().unwrap();
    if let NodeState::Draining {
        waiting_on,
        overdue: o,
        ..
    } = &mut *cur
    {
        *waiting_on = waiting;
        *o = overdue;
    }
}

/// Spawn `aspen update --restart --unattended` detached, output to
/// aspen.log, and move to `updating`. The child stops this daemon.
fn launch_updater(inner: &Arc<NodeInner>, by: &str, target: &str) {
    let s = &inner.servicing;
    let (Some(exe), Some(data_dir)) = (s.exe.clone(), inner.data_dir.clone()) else {
        let _ = cancel(inner, "system");
        return;
    };
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("aspen.log"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--data-dir")
        .arg(&data_dir)
        .args([
            "update",
            "--restart",
            "--unattended",
            "--trigger",
            by,
            "--version",
            &format!("v{target}"),
        ])
        .env_remove("ASPEN_DETACHED")
        .stdin(std::process::Stdio::null());
    match log {
        Ok(f) => {
            if let Ok(e) = f.try_clone() {
                cmd.stdout(f).stderr(e);
            }
        }
        Err(_) => {
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    match cmd.spawn() {
        Ok(child) => {
            *s.updater.lock().unwrap() = Some(child);
            *s.state.lock().unwrap() = NodeState::Updating {
                since: now_epoch(),
                by: by.to_owned(),
                target: target.to_owned(),
            };
            let _ = inner.store.record_event(
                NODE_AGENT,
                "update_started",
                json!({ "to": target, "by": by, "from": s.current }),
            );
            tracing::info!(
                target,
                by,
                "update: updater launched; this daemon will be stopped"
            );
        }
        Err(e) => {
            *s.state.lock().unwrap() = NodeState::Ready;
            let _ = inner.store.record_event(
                NODE_AGENT,
                "update_failed",
                json!({ "to": target, "error": format!("could not launch updater: {e}") }),
            );
        }
    }
}

/// Pick up `update-outcome.json` if the updater has written one we haven't
/// recorded. Idempotent; called at start and from the drain ticker — the
/// updater writes the file *after* it health-checks the new daemon, i.e.
/// after that daemon has already started.
pub fn consume_outcome(inner: &Arc<NodeInner>) {
    let Some(dir) = inner.data_dir.as_deref() else {
        return;
    };
    let Some(mut o) = read_outcome(dir) else {
        return;
    };
    if !o.recorded {
        let kind = if o.rolled_back {
            "update_rolled_back"
        } else if o.ok {
            "update_applied"
        } else {
            "update_failed"
        };
        let _ = inner.store.record_event(
            NODE_AGENT,
            kind,
            json!({
                "from": o.from, "to": o.to, "trigger": o.trigger,
                "error": o.error, "now_running": inner.servicing.current,
            }),
        );
        o.recorded = true;
        let _ = write_outcome(dir, &o);
    }
    let mut cur = inner.servicing.last_outcome.lock().unwrap();
    let changed = cur.as_ref().is_none_or(|c| c.finished_at != o.finished_at);
    if changed {
        *cur = Some(o);
    }
}

/// At daemon start: report what the previous updater did (once), and find
/// out what harness is installed.
pub fn on_start(inner: &Arc<NodeInner>) {
    consume_outcome(inner);
    let inner2 = inner.clone();
    tokio::task::spawn_blocking(move || {
        let v = crate::gitstate::quiet_command("claude")
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty());
        *inner2.servicing.inventory.claude_version.lock().unwrap() = v;
    });
}

/// The two loops: periodic/hinted release checks, and the drain ticker.
pub fn spawn_loops(inner: Arc<NodeInner>) {
    let checker = inner.clone();
    tokio::spawn(async move {
        // A beat after start so the listener is up before we talk out.
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            let policy = checker
                .data_dir
                .as_deref()
                .map(crate::settings::load)
                .unwrap_or_default()
                .update;
            if policy.checks() {
                let c = checker.clone();
                let _ = tokio::task::spawn_blocking(move || check(&c)).await;
                evaluate_policy(&checker);
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(check_interval_secs())) => {}
                _ = checker.servicing.check_now.notified() => {}
            }
        }
    });
    let drainer = inner.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
                _ = drainer.servicing.wake.notified() => {}
            }
            drain_tick(&drainer);
        }
    });
}

/// Force a check from an API call; returns the result.
pub async fn check_async(inner: Arc<NodeInner>) -> Result<ReleaseInfo> {
    let i = inner.clone();
    let r = tokio::task::spawn_blocking(move || check(&i))
        .await
        .map_err(|e| anyhow!("{e}"))??;
    evaluate_policy(&inner);
    Ok(r)
}

/// Tail this node's daemon log.
pub fn tail_log(data_dir: &Path, lines: usize) -> Vec<String> {
    let lines = lines.clamp(1, 2000);
    let Ok(text) = std::fs::read_to_string(data_dir.join("aspen.log")) else {
        return Vec::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].iter().map(|s| strip_ansi(s)).collect()
}

/// Logs written before color was turned off for detached daemons carry
/// escape codes; the console wants plain text.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------- fleet rollout

/// Rolling update of every linked peer, then this node. Serial, health
/// gated: a peer that doesn't come back on the target stops the rollout.
pub fn start_rollout(inner: &Arc<NodeInner>, when: &str) -> Result<Rollout> {
    let s = &inner.servicing;
    let target = s
        .newer()
        .map(|r| r.version)
        .ok_or_else(|| anyhow!("no newer release known — check first"))?;
    {
        let cur = s.rollout.lock().unwrap();
        if cur.as_ref().is_some_and(|r| !r.finished) {
            return Err(anyhow!("a rollout is already in progress"));
        }
    }
    let mesh = inner.mesh();
    let me = mesh
        .as_ref()
        .map(|m| m.identity.node.clone())
        .unwrap_or_else(|| "this node".into());
    let mut order: Vec<String> = mesh
        .as_ref()
        .map(|m| {
            let mut v: Vec<String> = m.links.lock().unwrap().keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    order.push(me.clone());
    let when = if when == "now" { "now" } else { "quiet" }.to_owned();
    let r = Rollout {
        target: target.clone(),
        when: when.clone(),
        order: order.clone(),
        started_at: now_epoch(),
        ..Default::default()
    };
    *s.rollout.lock().unwrap() = Some(r.clone());
    s.rollout_cancel.store(false, Ordering::SeqCst);
    let _ = inner.store.record_event(
        NODE_AGENT,
        "rollout_started",
        json!({ "to": target, "when": when, "order": order }),
    );
    let inner = inner.clone();
    tokio::spawn(async move { run_rollout(inner, order, me, target, when).await });
    Ok(r)
}

pub fn stop_rollout(inner: &Arc<NodeInner>) -> bool {
    let s = &inner.servicing;
    let active = s
        .rollout
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|r| !r.finished);
    if active {
        s.rollout_cancel.store(true, Ordering::SeqCst);
    }
    active
}

fn rollout_update(inner: &Arc<NodeInner>, f: impl FnOnce(&mut Rollout)) {
    if let Some(r) = inner.servicing.rollout.lock().unwrap().as_mut() {
        f(r);
    }
}

async fn run_rollout(
    inner: Arc<NodeInner>,
    order: Vec<String>,
    me: String,
    target: String,
    when: String,
) {
    const COMEBACK_SECS: f64 = 10.0 * 60.0;
    let finish = |inner: &Arc<NodeInner>, stopped: bool, failed: Option<(String, String)>| {
        rollout_update(inner, |r| {
            r.finished = true;
            r.stopped = stopped;
            r.failed = failed.clone();
            r.current = None;
            r.finished_at = Some(now_epoch());
        });
        let _ = inner.store.record_event(
            NODE_AGENT,
            "rollout_finished",
            json!({ "to": target, "stopped": stopped, "failed": failed }),
        );
    };
    for node in order {
        if inner.servicing.rollout_cancel.load(Ordering::SeqCst) {
            finish(&inner, true, None);
            return;
        }
        rollout_update(&inner, |r| r.current = Some(node.clone()));
        if node == me {
            // Last: ourselves. Request and return — the updater stops us.
            match request(&inner, &when, "rollout") {
                Ok(_) => {
                    rollout_update(&inner, |r| {
                        r.done.push(node.clone());
                        r.finished = true;
                        r.current = None;
                        r.finished_at = Some(now_epoch());
                    });
                }
                Err(e) => finish(&inner, false, Some((node, e.to_string()))),
            }
            return;
        }
        let Some(mesh) = inner.mesh() else {
            finish(&inner, false, Some((node, "not in a mesh".into())));
            return;
        };
        // Already there? Skip.
        let peer_version = |m: &crate::federation::MeshState| {
            m.health
                .lock()
                .unwrap()
                .get(&node)
                .and_then(|h| h.version.clone())
        };
        if peer_version(&mesh).as_deref() == Some(target.as_str()) {
            rollout_update(&inner, |r| r.done.push(node.clone()));
            continue;
        }
        let res = mesh
            .api_call(
                &node,
                "node_update",
                "",
                json!({ "when": when, "by": format!("rollout:{me}") }),
                std::time::Duration::from_secs(30),
            )
            .await;
        if let Err(e) = res {
            finish(
                &inner,
                false,
                Some((node, format!("update request failed: {e}"))),
            );
            return;
        }
        // Wait: while the peer drains (no timeout — the operator can stop
        // the rollout), then up to COMEBACK_SECS for it to report the
        // target version.
        let mut left_draining_at: Option<f64> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if inner.servicing.rollout_cancel.load(Ordering::SeqCst) {
                finish(&inner, true, None);
                return;
            }
            let Some(mesh) = inner.mesh() else { break };
            let (state, version) = {
                let h = mesh.health.lock().unwrap();
                let e = h.get(&node);
                (
                    e.and_then(|h| h.service_state.clone()),
                    e.and_then(|h| h.version.clone()),
                )
            };
            if version.as_deref() == Some(target.as_str()) && mesh.link_up(&node) {
                break;
            }
            let draining = mesh.link_up(&node) && state.as_deref() == Some("draining");
            if draining {
                left_draining_at = None;
                continue;
            }
            let t = *left_draining_at.get_or_insert_with(now_epoch);
            if now_epoch() - t > COMEBACK_SECS {
                finish(
                    &inner,
                    false,
                    Some((
                        node,
                        format!(
                            "did not come back on {target} within {} min (last seen v{})",
                            COMEBACK_SECS as u64 / 60,
                            version.unwrap_or_else(|| "?".into())
                        ),
                    )),
                );
                return;
            }
        }
        rollout_update(&inner, |r| r.done.push(node.clone()));
    }
    finish(&inner, false, None);
}
