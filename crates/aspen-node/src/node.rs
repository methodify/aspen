//! The session manager: named agents, exact turn state, event fan-out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, mpsc};

use aspen_core::{
    AgentAdapter, Harness, PermissionPolicy, Posture, SessionEvent, SessionHandle, SessionStore,
};

use crate::delivery;
use crate::store::BusStore;

/// Exact turn state — derived from the wire, not inferred from registries or
/// transcript mtimes. `result` is the only idle signal (reference §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Idle,
    Busy,
}

/// The recap turn's collector: assistant text accumulates here and the
/// turn's result wakes the waiting request.
pub struct RecapCapture {
    pub text: String,
    pub done: Option<tokio::sync::oneshot::Sender<String>>,
}

/// What an agent is doing, for the operator's fleet view — accumulated from
/// the event stream since this process started (a revive starts fresh).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct WorkSummary {
    /// The operator's last ask (snippet) and when.
    pub last_ask: Option<String>,
    pub last_ask_at: Option<f64>,
    /// The runtime's last result text (snippet).
    pub last_reply: Option<String>,
    /// Turns completed since spawn.
    pub turns: u32,
    /// When the current idle began (None while busy).
    pub idle_since: Option<f64>,
    /// Cumulative cost as reported by the runtime's last result.
    pub cost_usd: Option<f64>,
    /// Context estimate from the last result's usage (tokens in the last
    /// request) and the model's window when reported.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    /// The model named in the latest assistant message — what the
    /// session is actually running on, whatever the select says.
    pub model: Option<String>,
    /// Files this session has edited/written (tool inputs with a path).
    pub files_touched: std::collections::BTreeSet<String>,
    pub tool_calls: u32,
}

pub struct ManagedSession {
    pub name: String,
    pub repo: PathBuf,
    pub channel: String,
    pub handle: Arc<dyn SessionHandle>,
    /// Which harness this process is (HARNESSES.md).
    pub harness: Harness,
    pub turn_state: Mutex<TurnState>,
    /// Fan-out to any number of observers (UI connections, dev harness).
    pub events: broadcast::Sender<SessionEvent>,
    /// Present when spawned interactively: the console can answer prompts.
    pub broker: Option<Arc<crate::permit::OperatorBroker>>,
    /// The runtime's last `system/init` inventory (skills/commands/mcp),
    /// captured whenever it arrives (with the first turn, per reference §4).
    pub inventory: Mutex<Option<serde_json::Value>>,
    /// When the current turn started (busy since), epoch seconds.
    pub busy_since: Mutex<Option<f64>>,
    /// The most recent tool the session invoked this turn.
    pub last_tool: Mutex<Option<String>>,
    /// When spawned as a fork: (parent session, fork point). The pump
    /// records lineage once the runtime announces the child's id.
    pub fork_from: Option<(String, Option<String>)>,
    pub summary: Mutex<WorkSummary>,
    /// Something the operator should know about how this process started
    /// (e.g. "forked: the session was live elsewhere").
    pub spawn_note: Mutex<Option<String>>,
    /// The plugins this process was started with (plugins.rs), so a newer
    /// cached version can be offered as a restart, never a surprise.
    pub plugins: Vec<crate::plugins::ActivePlugin>,
    /// Activities running at the last turn end (`kind:id` → label), so the
    /// next turn end can raise `activity_settled` for what is gone.
    pub running_acts: Mutex<HashMap<String, String>>,
    /// Activity counts (ACTIVITY.md) as of the last turn boundary — the
    /// fleet views read this instead of re-deriving the ledger from the
    /// transcript on every poll.
    pub activity_counts: Mutex<serde_json::Value>,
    /// When the counts were last derived (epoch), to throttle mid-turn refreshes.
    pub activity_refreshed_at: Mutex<f64>,
    /// The MCP picture as last learned (PROPOSALS-MCP.md §3.2): at init,
    /// at turn boundaries, on a harness push, on demand.
    pub mcp: Mutex<Vec<aspen_core::McpServerState>>,
    pub mcp_refreshed_at: Mutex<f64>,
    /// Activities the operator stopped from the console (by id or tool
    /// use id) with when: shown as stopped until the harness settles them.
    pub stopped_acts: Mutex<HashMap<String, f64>>,
    /// A recap in flight (PROPOSALS-2026-09-C.md §1): while set, the pump
    /// routes the side turn's events here instead of to observers and the
    /// ledger — the recap is an answer to the operator, not a turn of the
    /// conversation.
    pub recap: Mutex<Option<RecapCapture>>,
    /// When this process started — the ledger's stale cutoff (ACTIVITY.md).
    pub spawned_at: f64,
}

/// A transcript written this recently, by a process this node doesn't
/// manage, counts as live elsewhere.
const LIVE_ELSEWHERE_SECS: f64 = 120.0;

impl ManagedSession {
    pub fn turn_state(&self) -> TurnState {
        *self.turn_state.lock().unwrap()
    }

    pub fn mark_busy(&self) {
        *self.turn_state.lock().unwrap() = TurnState::Busy;
        let mut since = self.busy_since.lock().unwrap();
        if since.is_none() {
            *since = Some(crate::store::now_epoch());
        }
    }

    /// (busy_since_epoch, last_tool) for presence detail.
    pub fn presence_detail(&self) -> (Option<f64>, Option<String>) {
        (
            *self.busy_since.lock().unwrap(),
            self.last_tool.lock().unwrap().clone(),
        )
    }
}

pub struct NodeInner {
    pub store: BusStore,
    /// The harnesses this node can run (HARNESSES.md), by name.
    pub adapters: HashMap<Harness, Arc<dyn AgentAdapter>>,
    pub sessions: Mutex<HashMap<String, Arc<ManagedSession>>>,
    pub delivery_tx: mpsc::UnboundedSender<String>,
    /// Present when this node has joined a mesh (identity + cert on disk).
    /// Swappable so a node that started outside any mesh can join one
    /// live (`aspen mesh init/join` → reload), no restart.
    pub mesh: std::sync::RwLock<Option<Arc<crate::federation::MeshState>>>,
    /// The node data directory (trust store, keys). None for in-memory use.
    pub data_dir: Option<PathBuf>,
    /// Set when the daemon is going down: session exits during the ladder
    /// must not clear the agents' `live` mark (they'll be revived).
    pub shutting_down: std::sync::atomic::AtomicBool,
    /// Self-update state, inventory, rollout (servicing.rs).
    pub servicing: crate::servicing::Servicing,
    /// What the replication target has acknowledged (replicate.rs).
    pub replication: Mutex<crate::replicate::SourceState>,
    /// Dispatch an HTTP request into this node's own router — the `http`
    /// mesh op a console peer uses (RELAY.md §11). Set by the API server.
    pub http_gateway: std::sync::OnceLock<HttpGateway>,
    /// This node's own hosted relay, as a loopback URL, once the listener
    /// is bound: the node registers on it like any client, so a console
    /// (or a peer) that reaches the relay can reach its host too
    /// (RELAY.md §11).
    pub self_relay: std::sync::OnceLock<String>,
}

/// (method, path, body, headers) → `{status, content_type, body|body_b64}`.
pub type HttpGateway = Arc<
    dyn Fn(
            String,
            String,
            Option<String>,
            HashMap<String, String>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>>
        + Send
        + Sync,
>;

impl NodeInner {
    /// The adapter for a harness (claude always; codex when built in).
    pub fn adapter(&self, harness: Harness) -> Option<Arc<dyn AgentAdapter>> {
        self.adapters.get(&harness).cloned()
    }

    /// The session store for a harness; claude's when the harness is
    /// unknown here (rows from before v0.20, a harness not built in).
    pub fn store_for(&self, harness: Harness) -> Arc<dyn SessionStore> {
        self.adapters
            .get(&harness)
            .or_else(|| self.adapters.get(&Harness::Claude))
            .map(|a| a.store())
            .expect("the claude adapter is always registered")
    }

    /// Which harness an agent (by local key) runs on.
    /// This node's name (the mesh identity, else "this node").
    pub fn node_name(&self) -> String {
        self.mesh()
            .map(|m| m.identity.node.clone())
            .unwrap_or_else(|| "this node".into())
    }

    pub fn harness_of(&self, agent: &str) -> Harness {
        if let Some(s) = self.live(agent) {
            return s.harness;
        }
        self.store
            .agents()
            .ok()
            .and_then(|rows| {
                rows.into_iter()
                    .find(|a| a.name == agent)
                    .map(|a| a.harness)
            })
            .unwrap_or_default()
    }

    /// The mesh, if this node is in one (cheap Arc clone; may change at
    /// runtime via Node::reload_mesh).
    pub fn mesh(&self) -> Option<Arc<crate::federation::MeshState>> {
        self.mesh.read().unwrap().clone()
    }

    pub fn live(&self, name: &str) -> Option<Arc<ManagedSession>> {
        self.sessions.lock().unwrap().get(name).cloned()
    }

    /// Nudge the delivery engine to look at one recipient's pending mail.
    pub fn tick_delivery(&self, recipient: &str) {
        let _ = self.delivery_tx.send(recipient.to_owned());
    }
}

#[derive(Clone)]
pub struct Node {
    pub inner: Arc<NodeInner>,
}

#[derive(Debug, Clone, Default)]
pub struct SpawnOpts {
    /// Which harness to run (None: the row's, else the repo's default, else claude).
    pub harness: Option<Harness>,
    /// Aspen's posture (HARNESSES.md §3.3); `permission_mode` overrides it.
    pub posture: Option<Posture>,
    pub charter: Option<String>,
    pub model: Option<String>,
    pub resume: Option<String>,
    pub allow_all: bool,
    pub permission_mode: Option<String>,
    /// An operator surface exists: prompt instead of policy-denying, and
    /// route AskUserQuestion to the console.
    pub interactive: bool,
    /// Skip permission prompts entirely (`--dangerously-skip-permissions` /
    /// bypassPermissions mode). When None, the repo's stored default is used.
    pub skip_permissions: Option<bool>,
    /// Per-session harness CLI args (raw string; split at spawn). Appended
    /// after the harness defaults from settings.json.
    pub extra_args: Option<String>,
    /// With `resume`: branch to a fresh session id (the head moves to it).
    pub fork: bool,
    /// With `resume`: truncate history to this message first.
    pub resume_at: Option<String>,
    /// The operator's answer when the session to resume is live elsewhere:
    /// "fork" | "in_place". Absent → spawn refuses with `LiveElsewhere`.
    pub resume_choice: Option<String>,
}

/// Refusal: the session to resume is being written by a process this node
/// doesn't manage. The API turns this into a 409 the console asks about.
#[derive(Debug, Clone)]
pub struct LiveElsewhere {
    pub session: String,
    pub written_ago_secs: u64,
}
impl std::fmt::Display for LiveElsewhere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session {} was written {}s ago by a process this node doesn't manage — fork it, or resume in place anyway",
            &self.session[..8.min(self.session.len())],
            self.written_ago_secs
        )
    }
}
impl std::error::Error for LiveElsewhere {}

/// The one way a repo path enters or is looked up in the store. Resolves
/// symlinks/relative parts like canonicalize, but on Windows yields the
/// plain `C:\…` form rather than the `\\?\C:\…` verbatim form canonicalize
/// returns — discovery stores what Claude Code wrote (`C:\…`), and a lookup
/// in the other form matched zero rows (the "skip does nothing" bug).
/// A path that doesn't exist comes back as given.
pub fn normalize_repo(p: &Path) -> PathBuf {
    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The work summary as the API/roster carries it (files as a count + a
/// short list; busy_since/last_tool folded in).
/// Is a repo visible to peers of `mesh` (MESHES.md §exposure)? With one
/// mesh everything is; with more, only what the operator exposed.
pub fn repo_exposed(inner: &Arc<NodeInner>, repo: &Path, mesh: &str) -> bool {
    let Some(m) = inner.mesh() else { return true };
    if !m.is_multi() {
        return true;
    }
    inner
        .store
        .exposed_meshes(repo)
        .map(|v| v.iter().any(|x| x == mesh))
        .unwrap_or(false)
}

/// Is an agent (by local key) visible to peers of `mesh`?
pub fn agent_exposed(inner: &Arc<NodeInner>, agent: &str, mesh: &str) -> bool {
    let Ok(rows) = inner.store.agents() else {
        return false;
    };
    match rows.iter().find(|a| a.name == agent) {
        Some(a) => repo_exposed(inner, &a.repo, mesh),
        None => true,
    }
}

/// Joining a second mesh pins every existing repo to the first, so the
/// rule change leaks nothing; repos added later start unexposed.
fn pin_exposure_on_first_extra(inner: &Arc<NodeInner>) {
    let Some(m) = inner.mesh() else { return };
    if !m.is_multi() {
        return;
    }
    if inner.store.exposure_rows().unwrap_or(1) > 0 {
        return;
    }
    let primary = m.mesh_name();
    for r in inner.store.repos().unwrap_or_default() {
        let _ = inner
            .store
            .set_exposure(&r.path, std::slice::from_ref(&primary));
    }
    tracing::info!(mesh = %primary, "second mesh joined: existing repos pinned to the primary mesh");
}

/// Every running activity of every live session on this node, each tagged
/// with its agent's local key — the node's half of `GET /api/activities`.
pub fn fleet_activities(inner: &Arc<NodeInner>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(rows) = inner.store.agents() else {
        return out;
    };
    for row in rows {
        if inner.live(&row.name).is_none() {
            continue;
        }
        let Some(sid) = row.session_id.as_deref() else {
            continue;
        };
        for mut v in inner
            .store_for(row.harness)
            .activities(&row.repo, sid, row.last_spawned_at)
        {
            if v.get("status").and_then(|s| s.as_str()) != Some("running") {
                continue;
            }
            v["agent"] = serde_json::json!(row.name);
            out.push(v);
        }
    }
    out
}

/// Usage (USAGE.md) for every agent on this node: transcript totals plus
/// spend observed in [from, to] from the fleet trail. `agent` limits it
/// to one.
pub fn usage_rows(
    inner: &Arc<NodeInner>,
    from: f64,
    to: f64,
    agent: Option<&str>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(rows) = inner.store.agents() else {
        return out;
    };
    let observed = inner.store.observed_cost(from, to).unwrap_or_default();
    for row in rows {
        if let Some(a) = agent {
            if row.name != a {
                continue;
            }
        }
        let Some(sid) = row.session_id.as_deref() else {
            continue;
        };
        let u = inner.store_for(row.harness).usage(&row.repo, sid);
        let (win_cost, win_turns) = observed
            .iter()
            .find(|(a, _, _)| a == &row.name)
            .map(|(_, c, n)| (*c, *n))
            .unwrap_or((0.0, 0));
        let live = inner.live(&row.name);
        out.push(serde_json::json!({
            "agent": row.name,
            "repo": row.repo.to_string_lossy(),
            "channel": row.channel,
            "title": row.title,
            "session_id": sid,
            "live": live.is_some(),
            "harness": row.harness,
            "usage": u,
            "window": { "cost_usd": win_cost, "turns": win_turns },
        }));
    }
    out
}

/// Running-activity counts for a live session (activity.rs), from the
/// transcript on disk; cached by the file's size and mtime.
pub fn activity_counts(
    inner: &Arc<NodeInner>,
    harness: Harness,
    repo: &Path,
    session_id: Option<&str>,
    process_started: Option<f64>,
) -> serde_json::Value {
    inner
        .store_for(harness)
        .activity_counts(repo, session_id, process_started)
}

/// Every session on disk for a repo, across every harness this node runs,
/// newest first; each row says its harness.
pub fn enumerate_all(inner: &Arc<NodeInner>, repo: &Path) -> Vec<aspen_core::SessionInfo> {
    let mut out = Vec::new();
    for a in inner.adapters.values() {
        if let Ok(rows) = a.store().enumerate(repo) {
            out.extend(rows);
        }
    }
    out.sort_by(|a, b| b.modified_epoch.total_cmp(&a.modified_epoch));
    out
}

/// The daemon's local API address, for the tool bridge (`daemon.json`).
fn local_api_addr(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(data_dir.join("daemon.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let listen = v.get("listen")?.as_str()?;
    let addr: std::net::SocketAddr = listen.parse().ok()?;
    let host = if addr.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        addr.ip().to_string()
    };
    Some(format!("http://{host}:{}", addr.port()))
}

/// Counts of the session's MCP servers by status, for fleet rows.
pub fn mcp_summary(s: &ManagedSession) -> serde_json::Value {
    use aspen_core::McpStatus;
    let list = s.mcp.lock().unwrap();
    let n = |st: McpStatus| list.iter().filter(|m| m.status == st).count();
    serde_json::json!({
        "total": list.len(),
        "connected": n(McpStatus::Connected),
        "failed": n(McpStatus::Failed),
        "needs_auth": n(McpStatus::NeedsAuth),
        "pending": n(McpStatus::Pending),
        "disabled": n(McpStatus::Disabled),
    })
}

/// Ask the harness for the MCP picture, store it, raise notices for what
/// changed, and tell the session's listeners. Returns the picture.
pub async fn refresh_mcp(
    inner: &Arc<NodeInner>,
    sess: &Arc<ManagedSession>,
) -> Result<Vec<aspen_core::McpServerState>> {
    use aspen_core::McpStatus;
    if !sess.handle.capabilities().mcp_status {
        return Ok(sess.mcp.lock().unwrap().clone());
    }
    let now = tokio::time::timeout(
        std::time::Duration::from_secs(40),
        sess.handle.mcp_servers(),
    )
    .await
    .map_err(|_| anyhow!("the session did not answer an MCP status request within 40s"))??;
    let before = std::mem::replace(&mut *sess.mcp.lock().unwrap(), now.clone());
    // The first full picture says everything that is wrong at start (the
    // adapter's init list, if any, is only names and statuses).
    let first = *sess.mcp_refreshed_at.lock().unwrap() == 0.0;
    let before = if first { Vec::new() } else { before };
    *sess.mcp_refreshed_at.lock().unwrap() = crate::store::now_epoch();
    let link = format!("/session/{}", sess.name);
    for m in &now {
        let was = before.iter().find(|b| b.name == m.name).map(|b| b.status);
        let bad = matches!(m.status, McpStatus::Failed | McpStatus::NeedsAuth);
        let was_bad = matches!(was, Some(McpStatus::Failed) | Some(McpStatus::NeedsAuth));
        if bad && !was_bad {
            let what = match m.status {
                McpStatus::NeedsAuth => "needs authentication".to_owned(),
                _ => m
                    .error
                    .clone()
                    .map(|e| format!("failed: {e}"))
                    .unwrap_or_else(|| "failed".into()),
            };
            // Once per six hours per server and reason: a permanently
            // broken server toasts on first sight, not on every restart.
            let title = format!("MCP {} {what}", m.name);
            if !inner
                .store
                .notice_recent(&sess.name, "mcp_failed", &title, 6.0 * 3600.0)
            {
                crate::notify::raise(
                    inner,
                    &sess.name,
                    "mcp_failed",
                    &title,
                    m.plugin
                        .as_deref()
                        .map(|p| format!("from plugin {p}"))
                        .as_deref(),
                    Some(&link),
                );
            }
        } else if !bad && was_bad && m.status == McpStatus::Connected {
            crate::notify::raise(
                inner,
                &sess.name,
                "mcp_recovered",
                &format!("MCP {} connected", m.name),
                None,
                Some(&link),
            );
        }
    }
    let _ = sess.events.send(SessionEvent::McpChanged {
        servers: now.clone(),
    });
    Ok(now)
}

fn schedule_mcp_refresh(inner: &Arc<NodeInner>, sess: &Arc<ManagedSession>, delay_ms: u64) {
    let inner = inner.clone();
    let sess = sess.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        // Debounce: pushes and turn ends can cluster.
        let last = *sess.mcp_refreshed_at.lock().unwrap();
        if crate::store::now_epoch() - last < 1.0 {
            return;
        }
        if let Err(e) = refresh_mcp(&inner, &sess).await {
            tracing::debug!(agent = %sess.name, error = %e, "mcp refresh");
        }
    });
}

/// Epoch seconds → an ISO-8601 UTC timestamp (the ledger's format).
fn iso_of(epoch: f64) -> String {
    let secs = epoch.floor() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn summary_json(s: &ManagedSession) -> serde_json::Value {
    let w = s.summary.lock().unwrap().clone();
    let (busy_since, last_tool) = s.presence_detail();
    let files: Vec<&String> = w.files_touched.iter().collect();
    serde_json::json!({
        "last_ask": w.last_ask,
        "last_ask_at": w.last_ask_at,
        "last_reply": w.last_reply,
        "turns": w.turns,
        "idle_since": w.idle_since,
        "busy_since": busy_since,
        "last_tool": last_tool,
        "cost_usd": w.cost_usd,
        "model": w.model,
        "context_tokens": w.context_tokens,
        "context_window": w.context_window,
        "files_touched": files.len(),
        "files": files.iter().rev().take(8).collect::<Vec<_>>(),
        "tool_calls": w.tool_calls,
    })
}

/// First line-ish of a text, trimmed to `n` chars, for cards.
fn snippet(text: &str, n: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

/// The default handle for a repo: its directory basename. The store
/// assigns the real handle (suffixed on collision) — see
/// `BusStore::ensure_handle`; this is only the seed.
pub fn repo_channel(repo: &Path) -> String {
    repo.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into())
}

impl Node {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let store = BusStore::open(&data_dir.join("bus.db"))?;
        let files = crate::mesh::MeshFiles::new(data_dir);
        let mesh = match (files.load_identity()?, files.load_mesh()?) {
            (Some(identity), Some(mut config)) if identity.cert.is_some() => {
                config.peers = files.verified_peers()?;
                let st = crate::federation::MeshState::new(identity, config);
                *st.extra.write().unwrap() = files.load_extra_meshes()?;
                Some(Arc::new(st))
            }
            _ => None,
        };
        let node = Self::build(store, mesh, Some(data_dir.to_owned()));
        if node.inner.mesh().is_some() {
            pin_exposure_on_first_extra(&node.inner);
            crate::federation::ensure_dialers(node.inner.clone());
        }
        crate::gitstate::spawn_refresher(node.inner.clone(), 30);
        Ok(node)
    }

    /// Re-read mesh files and apply them to the running daemon: join a mesh
    /// if we weren't in one, or pick up new peers / relay if we were. Live
    /// links are preserved. Returns a one-line summary.
    pub fn reload_mesh(&self) -> Result<String> {
        let Some(data_dir) = self.inner.data_dir.as_deref() else {
            anyhow::bail!("node has no data dir");
        };
        let files = crate::mesh::MeshFiles::new(data_dir);
        let (identity, mut config) = match (files.load_identity()?, files.load_mesh()?) {
            (Some(id), Some(cfg)) if id.cert.is_some() => (id, cfg),
            _ => {
                // Left the mesh (or never joined): drop the live state so
                // links close and dialers stop.
                let had = self.inner.mesh.write().unwrap().take();
                return Ok(match had {
                    Some(m) => {
                        m.links.lock().unwrap().clear();
                        m.remote.lock().unwrap().clear();
                        format!("left mesh '{}': links closed", m.mesh_name())
                    }
                    None => "no certified mesh membership on disk".into(),
                });
            }
        };
        config.peers = files.verified_peers()?;
        let summary;
        if let Some(mesh) = self.inner.mesh() {
            let mut cur = mesh.config.write().unwrap();
            let before = cur.peers.len();
            let gone: Vec<String> = cur
                .peers
                .iter()
                .map(|p| p.cert.node.clone())
                .filter(|n| !config.peers.iter().any(|p| &p.cert.node == n))
                .collect();
            *cur = config;
            drop(cur);
            // A removed peer's live link closes now; its dialer sees it is
            // no longer configured and exits.
            for n in &gone {
                mesh.links.lock().unwrap().remove(n);
                mesh.remote.lock().unwrap().remove(n);
                mesh.dialing.lock().unwrap().remove(n);
            }
            *mesh.extra.write().unwrap() = files.load_extra_meshes()?;
            // Identity certs may have grown (a second mesh joined): the
            // live state keeps its identity, so reload extras' certs too.
            let cur = mesh.config.read().unwrap();
            summary = format!(
                "mesh '{}' config reloaded: {} peer(s) (was {}){}, relay {}",
                cur.mesh,
                cur.peers.len(),
                before,
                if gone.is_empty() {
                    String::new()
                } else {
                    format!(", removed {}", gone.join(", "))
                },
                cur.relay.as_deref().unwrap_or("none")
            );
        } else {
            let name = config.mesh.clone();
            let peers = config.peers.len();
            let state = crate::federation::MeshState::new(identity, config);
            *state.extra.write().unwrap() = files.load_extra_meshes()?;
            *self.inner.mesh.write().unwrap() = Some(Arc::new(state));
            summary = format!("joined mesh '{name}' live: {peers} peer(s)");
        }
        pin_exposure_on_first_extra(&self.inner);
        crate::federation::ensure_dialers(self.inner.clone());
        Ok(summary)
    }

    pub fn with_store(store: BusStore) -> Self {
        Self::build(store, None, None)
    }

    fn build(
        store: BusStore,
        mesh: Option<Arc<crate::federation::MeshState>>,
        data_dir: Option<PathBuf>,
    ) -> Self {
        let (delivery_tx, delivery_rx) = mpsc::unbounded_channel::<String>();
        let mut adapters: HashMap<Harness, Arc<dyn AgentAdapter>> = HashMap::new();
        adapters.insert(
            Harness::Claude,
            Arc::new(aspen_claude::ClaudeAdapter::new()),
        );
        // Codex is listed when its binary resolves (HARNESSES.md: a node
        // lists what it can run); `ASPEN_CODEX_BIN` overrides the name.
        let mut codex = aspen_codex::CodexAdapter::new();
        if let Ok(b) = std::env::var("ASPEN_CODEX_BIN") {
            if !b.trim().is_empty() {
                codex.bin = b;
            }
        }
        if codex.available() {
            adapters.insert(Harness::Codex, Arc::new(codex));
        }
        let inner = Arc::new(NodeInner {
            store,
            adapters,
            sessions: Mutex::new(HashMap::new()),
            delivery_tx,
            mesh: std::sync::RwLock::new(mesh),
            data_dir,
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            replication: Mutex::new(Default::default()),
            http_gateway: std::sync::OnceLock::new(),
            self_relay: std::sync::OnceLock::new(),
            servicing: crate::servicing::Servicing::new(
                crate::federation::VERSION
                    .get()
                    .map(|(v, _)| v.clone())
                    .unwrap_or_else(|| "0.0.0".into()),
                std::env::current_exe().ok(),
            ),
        });
        {
            // The delivery engine is one task for every agent; a panic
            // there would silently stop the bus. Say so, loudly.
            let handle = tokio::spawn(delivery::run(inner.clone(), delivery_rx));
            tokio::spawn(async move {
                if let Err(e) = handle.await {
                    if e.is_panic() {
                        tracing::error!("delivery engine panicked: bus delivery is stopped until the daemon restarts");
                    }
                }
            });
        }
        // Harness defaults from settings.json become this node's synced
        // row once (PROPOSALS-MCP.md §5.4); the file stops mattering.
        if let Some(dd) = inner.data_dir.as_deref() {
            let me = inner.node_name();
            for (h, cfg) in crate::settings::load(dd).harness {
                if cfg.args.trim().is_empty() {
                    continue;
                }
                let scope = format!("node:{me}");
                let have = inner
                    .store
                    .harness_defaults(true)
                    .unwrap_or_default()
                    .iter()
                    .any(|d| d.scope == scope && d.harness == h);
                if !have {
                    let _ = inner
                        .store
                        .upsert_harness_default(&crate::store::HarnessDefault {
                            scope,
                            harness: h,
                            args: cfg.args.trim().to_owned(),
                            updated_at: inner.store.hlc_now(),
                            deleted: false,
                        });
                }
            }
        }
        Self { inner }
    }

    /// Spawn a named agent in a repo and join it to the bus.
    /// `name` is the bare agent name (`arch`) or an existing key
    /// (`arch@nonlinear`, as revive passes it). The agent's key is always
    /// `bare@<repo handle>`; that key is the name everywhere below.
    pub async fn spawn_agent(
        &self,
        name: &str,
        repo: PathBuf,
        opts: SpawnOpts,
    ) -> Result<Arc<ManagedSession>> {
        let repo =
            dunce::canonicalize(&repo).map_err(|e| anyhow!("repo {}: {e}", repo.display()))?;
        let channel = self.inner.store.ensure_handle(&repo)?;
        let bare = crate::addr::bare(name).to_owned();
        if let Some(given) = crate::addr::repo_of(name) {
            if given != channel {
                return Err(anyhow!(
                    "{name} names repo '{given}' but {} is #{channel}",
                    repo.display()
                ));
            }
        }
        let key = crate::addr::local_key(&bare, &channel);
        let name = key.as_str();
        if self.inner.live(name).is_some() {
            return Err(anyhow!("an agent named {name} is already running"));
        }
        // Draining for an update: no new work until the node is back.
        if !self.inner.servicing.accepting_spawns() {
            return Err(anyhow!(
                "node is updating ({}); start sessions once it is back, or cancel the update",
                self.inner.servicing.state().name()
            ));
        }
        self.inner.servicing.note_spawn();

        // Resolve skip-permissions: explicit request wins, else the repo's
        // stored default, else off.
        let skip = opts.skip_permissions.unwrap_or_else(|| {
            self.inner
                .store
                .repo(&repo)
                .ok()
                .flatten()
                .map(|r| r.skip_permissions)
                .unwrap_or(false)
        });

        // Which harness: the caller's, else what this agent ran on before,
        // else the repo's default, else claude (HARNESSES.md).
        let harness = opts.harness.unwrap_or_else(|| {
            self.inner
                .store
                .agents()
                .ok()
                .and_then(|rows| rows.into_iter().find(|a| a.name == name).map(|a| a.harness))
                .or_else(|| self.inner.store.repo_default_harness(&repo))
                .unwrap_or_default()
        });

        // Resuming in place a session that is being written right now by
        // a process we don't manage (a terminal, the desktop app, another
        // node) would put two writers on one transcript. Never decide that
        // silently: refuse with LiveElsewhere so the console can ask, and
        // act only on an explicit choice — fork (history kept, new id) or
        // resume in place anyway.
        let mut opts = opts;
        let mut spawn_note: Option<String> = None;
        if let (Some(sid), false) = (opts.resume.as_deref(), opts.fork) {
            // "Ours" = a process this node manages right now, or one it
            // managed when it last went down: the store's `live` mark
            // survives a daemon restart precisely so auto-revive can bring
            // the session back — and that session wrote its transcript
            // seconds ago, by this node's own hand. Without the mark, every
            // restart refused its own agents (seen on the mac, 2026-09-06).
            let marked_live = self.inner.store.agents_marked_live().unwrap_or_default();
            let ours = self
                .inner
                .store
                .agents()
                .unwrap_or_default()
                .iter()
                .any(|a| {
                    a.session_id.as_deref() == Some(sid)
                        && (self.inner.live(&a.name).is_some() || marked_live.contains(&a.name))
                });
            let mtime = self.inner.store_for(harness).modified(&repo, sid);
            let ago = mtime.map(|t| crate::store::now_epoch() - t);
            let recent = ago.is_some_and(|a| a < LIVE_ELSEWHERE_SECS);
            if recent && !ours {
                match opts.resume_choice.as_deref() {
                    Some("fork") => {
                        opts.fork = true;
                        spawn_note = Some(format!(
                            "forked at your request: session {} was being written elsewhere",
                            &sid[..8.min(sid.len())]
                        ));
                    }
                    Some("in_place") => {
                        spawn_note = Some(format!(
                            "resumed in place at your request; session {} was being written elsewhere — two processes share this transcript",
                            &sid[..8.min(sid.len())]
                        ));
                    }
                    _ => {
                        return Err(LiveElsewhere {
                            session: sid.to_owned(),
                            written_ago_secs: ago.unwrap_or(0.0) as u64,
                        }
                        .into());
                    }
                }
            }
        }
        let opts = opts;

        let adapter = self
            .inner
            .adapter(harness)
            .ok_or_else(|| anyhow!("harness {harness} is not available on this node"))?;
        let caps = adapter.capabilities();
        let policy = if opts.allow_all {
            PermissionPolicy::AllowAll
        } else {
            PermissionPolicy::ReadOnlyAuto
        };
        // Posture: an explicit "skip permissions" on this request is the auto
        // posture; else the caller's posture; else the repo's stored skip
        // default (auto) or nothing (the adapter's default, ask).
        let posture = if opts.skip_permissions == Some(true) {
            Some(Posture::Auto)
        } else if opts.posture.is_some() {
            opts.posture
        } else if skip {
            Some(Posture::Auto)
        } else {
            None
        };
        let node_name = self
            .inner
            .mesh()
            .map(|m| m.identity.node.clone())
            .unwrap_or_else(|| "this node".into());
        let mut charter = charter_text(name, &channel, &node_name, opts.charter.as_deref());
        // Links are instructions: what the operator wired, explained.
        let guidance = crate::topology::guidance(&self.inner, name);
        if !guidance.is_empty() {
            charter.push_str(
                "\n\nYour neighborhood on the bus (declared by the operator; bus_status shows it live):\n",
            );
            charter.push_str(&guidance);
        }
        // Harness defaults (PROPOSALS-MCP.md §5): this node's synced row,
        // else the mesh row, else the legacy settings.json — then this
        // session's args.
        let defaults = self
            .inner
            .store
            .effective_harness_default(&self.inner.node_name(), harness.as_str())
            .map(|(a, _)| a)
            .or_else(|| {
                self.inner
                    .data_dir
                    .as_deref()
                    .map(crate::settings::load)
                    .unwrap_or_default()
                    .harness
                    .get(harness.as_str())
                    .map(|h| h.args.clone())
            })
            .unwrap_or_default();
        let extra_args = crate::settings::split_args(&defaults, opts.extra_args.as_deref())?;
        // Plugins by scope (plugins.rs): every enclosing rule's plugin, at
        // its cached version, as plugin dirs — for harnesses that take them.
        let (active_plugins, missing_plugins) = match (
            self.inner.data_dir.as_deref(),
            self.inner.store.plugin_rules(false),
        ) {
            (Some(dd), Ok(rules)) if !rules.is_empty() && caps.plugin_dirs => {
                let node = self
                    .inner
                    .mesh()
                    .map(|m| m.identity.node.clone())
                    .unwrap_or_else(|| "local".into());
                crate::plugins::resolve(dd, &rules, &node, &repo, name)
            }
            _ => (Vec::new(), Vec::new()),
        };
        let plugin_dirs: Vec<String> = active_plugins.iter().map(|a| a.path.clone()).collect();
        if !missing_plugins.is_empty() {
            let note = format!(
                "plugins not cached yet, started without: {}",
                missing_plugins.join(", ")
            );
            spawn_note = Some(match spawn_note {
                Some(prev) => format!("{prev}; {note}"),
                None => note,
            });
        }

        let tools = crate::tools::build_tools(self.inner.clone(), name.to_owned());
        let op_broker = opts
            .interactive
            .then(|| Arc::new(crate::permit::OperatorBroker::new(policy)));
        let session_id = aspen_core::SessionId::new();
        let bridge_token = self
            .inner
            .data_dir
            .as_deref()
            .and_then(|d| std::fs::read_to_string(d.join("api-token")).ok())
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        let node_api = self.inner.data_dir.as_deref().and_then(local_api_addr);
        let spec = aspen_core::SpawnSpec {
            repo: repo.clone(),
            session_id,
            resume: opts.resume.clone(),
            fork: opts.fork,
            resume_at: opts.resume_at.clone(),
            model: opts.model.clone(),
            mode: opts.permission_mode.clone(),
            posture,
            policy,
            charter: Some(charter),
            extra_args,
            plugin_dirs,
            tools: Some(tools),
            broker: op_broker
                .clone()
                .map(|b| b as Arc<dyn aspen_core::PermissionBroker>),
            agent: name.to_owned(),
            bridge_token: bridge_token.clone(),
            node_api: node_api.clone(),
            env: {
                // The session's identity for everything the harness
                // starts — hooks, MCP servers, scripts — so a plugin can
                // know which agent it serves without asking.
                let mut env = vec![
                    ("ASPEN_AGENT".to_owned(), name.to_owned()),
                    (
                        "ASPEN_AGENT_NAME".to_owned(),
                        crate::addr::bare(name).to_owned(),
                    ),
                    ("ASPEN_CHANNEL".to_owned(), channel.clone()),
                    ("ASPEN_NODE".to_owned(), self.inner.node_name()),
                    ("ASPEN_SESSION_ID".to_owned(), session_id.to_string()),
                ];
                if let Some(api) = &node_api {
                    env.push(("ASPEN_NODE_API".to_owned(), api.clone()));
                }
                if let Some(t) = &bridge_token {
                    env.push(("ASPEN_NODE_TOKEN".to_owned(), t.clone()));
                }
                env
            },
        };
        let (handle, adapter_rx) = adapter.spawn(spec).await?;

        // On resume the runtime keeps the resumed session's id — register
        // that, not the fresh id Aspen generated and never used.
        let effective_session_id = opts
            .resume
            .clone()
            .unwrap_or_else(|| session_id.to_string());
        self.inner.store.register_agent(
            name,
            &repo,
            &channel,
            &effective_session_id,
            opts.charter.as_deref(),
            opts.extra_args.as_deref(),
            harness,
        )?;
        let _ = self.inner.store.set_agent_live(name, true);
        // A fork's own id arrives with the runtime's first turn; until
        // then the row points at the parent, and a revive must fork again.
        let _ = self.inner.store.set_fork_pending(name, opts.fork);
        let _ = self.inner.store.record_event(
            name,
            if opts.fork {
                "branch"
            } else if opts.resume.is_some() {
                "revive"
            } else {
                "spawn"
            },
            serde_json::json!({
                "session": effective_session_id,
                "from": opts.resume,
                "at": opts.resume_at,
            }),
        );
        // Remember the repo (and, when the operator asked to skip here,
        // adopt that as the repo's default going forward).
        let _ = self.inner.store.add_repo(&repo, opts.skip_permissions);

        let (events_tx, _) = broadcast::channel(4096);
        if let Some(b) = &op_broker {
            b.attach_events(events_tx.clone());
        }
        let managed = Arc::new(ManagedSession {
            plugins: active_plugins,
            harness,
            running_acts: Mutex::new(HashMap::new()),
            activity_counts: Mutex::new(
                serde_json::json!({ "running": 0, "agents": 0, "tasks": 0, "workflows": 0, "monitors": 0 }),
            ),
            activity_refreshed_at: Mutex::new(0.0),
            mcp: Mutex::new(Vec::new()),
            mcp_refreshed_at: Mutex::new(0.0),
            stopped_acts: Mutex::new(HashMap::new()),
            recap: Mutex::new(None),
            spawned_at: crate::store::now_epoch(),
            name: name.to_owned(),
            repo,
            channel,
            handle,
            turn_state: Mutex::new(TurnState::Idle),
            events: events_tx,
            broker: op_broker,
            inventory: Mutex::new(None),
            busy_since: Mutex::new(None),
            last_tool: Mutex::new(None),
            fork_from: if opts.fork {
                opts.resume.clone().map(|p| (p, opts.resume_at.clone()))
            } else {
                None
            },
            spawn_note: Mutex::new(spawn_note.clone()),
            summary: Mutex::new(WorkSummary {
                idle_since: Some(crate::store::now_epoch()),
                ..Default::default()
            }),
        });
        self.inner
            .sessions
            .lock()
            .unwrap()
            .insert(name.to_owned(), managed.clone());

        // The pump is the session's lifeline: if it ever panics, say so and
        // take the session out of the live map rather than leave a ghost
        // that looks busy forever.
        {
            let inner = self.inner.clone();
            let managed = managed.clone();
            let handle = tokio::spawn(pump(inner.clone(), managed.clone(), adapter_rx));
            tokio::spawn(async move {
                if let Err(e) = handle.await {
                    if e.is_panic() {
                        tracing::error!(agent = %managed.name, "session pump panicked; marking the session down");
                        inner.sessions.lock().unwrap().remove(&managed.name);
                        let _ = inner.store.set_agent_live(&managed.name, false);
                        let _ = inner.store.record_event(
                            &managed.name,
                            "pump_panic",
                            serde_json::json!({}),
                        );
                        crate::federation::broadcast_roster(&inner);
                    }
                }
            });
        }

        // Anything held for this agent while it was down delivers at session
        // start (plumb's "next session start" rule).
        self.inner.tick_delivery(name);
        crate::federation::broadcast_roster(&self.inner);
        Ok(managed)
    }

    /// Operator input into a session. Pending notices ride along first — the
    /// one lane a notice may use besides another delivery.
    pub async fn send_operator_message(&self, name: &str, text: String) -> Result<String> {
        self.send_operator_message_with(name, text, Vec::new())
            .await
    }

    /// An operator message with pasted attachments (PROPOSALS §4). Every
    /// attachment is saved under the session's attachment dir (so it is
    /// viewable and referable later). Raster images are sent as image
    /// blocks at their marker's position in the text; other files keep
    /// their marker, rewritten to the saved path, so the agent reads
    /// them with its own tools.
    pub async fn send_operator_message_with(
        &self,
        name: &str,
        text: String,
        attachments: Vec<crate::artifacts::Attachment>,
    ) -> Result<String> {
        if let Ok(row) = self.agent_row(name) {
            if let Some(to) = row.moved_to {
                return Err(anyhow!("@{name} moved — it is now @{to}"));
            }
        }
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        if !attachments.is_empty() {
            let sid = self
                .agent_row(name)?
                .session_id
                .unwrap_or_else(|| name.to_owned());
            let dir = self
                .inner
                .data_dir
                .as_deref()
                .map(|d| crate::artifacts::attachments_dir(d, &sid))
                .ok_or_else(|| anyhow!("node has no data dir for attachments"))?;
            let (content, plain) =
                crate::artifacts::compose_with_attachments(&dir, &text, &attachments)?;
            delivery::flush_notices(&self.inner, &sess).await;
            {
                let mut s = sess.summary.lock().unwrap();
                s.last_ask = Some(snippet(&plain, 160));
                s.last_ask_at = Some(crate::store::now_epoch());
                s.idle_since = None;
            }
            let _ = self.inner.store.record_event(
                name,
                "ask",
                serde_json::json!({ "from": "operator", "text": snippet(&plain, 200), "attachments": attachments.len() }),
            );
            sess.mark_busy();
            return sess.handle.send_user_content(content).await;
        }
        delivery::flush_notices(&self.inner, &sess).await;
        {
            let mut s = sess.summary.lock().unwrap();
            s.last_ask = Some(snippet(&text, 160));
            s.last_ask_at = Some(crate::store::now_epoch());
            s.idle_since = None;
        }
        let _ = self.inner.store.record_event(
            name,
            "ask",
            serde_json::json!({ "from": "operator", "text": snippet(&text, 200) }),
        );
        sess.mark_busy();
        sess.handle.send_user(text).await
    }

    pub async fn interrupt(&self, name: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.interrupt().await
    }

    /// Operator-initiated stop: clears the live mark so the agent is not
    /// revived at the next daemon start. (The daemon's own shutdown ladder
    /// uses `shutdown_for_restart` and keeps the mark.)
    pub async fn shutdown_agent(&self, name: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let _ = self.inner.store.set_agent_live(name, false);
        sess.handle.shutdown().await
    }

    /// Stop a session because the daemon is going down — the agent stays
    /// marked live and comes back at the next `aspen up`.
    pub async fn shutdown_for_restart(&self, name: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.shutdown().await
    }

    pub fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<SessionEvent>> {
        self.inner.live(name).map(|s| s.events.subscribe())
    }

    /// Bring a registered-but-down agent back by resuming its session. The
    /// conversation, not the process, is the identity.
    ///
    /// `resume_choice` answers the live-elsewhere gate the same way a start
    /// does ("fork" | "in_place"); absent, a transcript written moments ago
    /// by a process this node doesn't manage is refused with
    /// [`LiveElsewhere`] so the operator can choose.
    pub async fn revive_agent(
        &self,
        name: &str,
        interactive: bool,
        resume_choice: Option<String>,
    ) -> Result<Arc<ManagedSession>> {
        if self.inner.live(name).is_some() {
            return Err(anyhow!("@{name} is already running"));
        }
        let rows = self.inner.store.agents()?;
        let row = rows
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow!("no agent named @{name} on record"))?;
        // A session that never had a turn wrote no transcript; `-r` on it
        // fails with "No conversation found". Nothing to resume ⇒ start
        // fresh under the same name/repo/charter.
        let resume = row
            .session_id
            .clone()
            .filter(|sid| self.inner.store_for(row.harness).exists(&row.repo, sid));
        let opts = SpawnOpts {
            harness: Some(row.harness),
            charter: row.charter.clone(),
            resume,
            // A fork that never announced its id is forked again from the
            // parent (nothing of its own exists yet); resuming in place
            // would put it on the parent's transcript.
            fork: row.fork_pending,
            interactive,
            extra_args: row.extra_args.clone(),
            resume_choice,
            ..Default::default()
        };
        self.spawn_agent(name, row.repo.clone(), opts).await
    }

    /// Branch here: leave a bookmark on the current head and move the agent
    /// to a fresh fork of it (optionally from an earlier message). The
    /// process is restarted on the fork; live marks are kept so the agent
    /// revives on the new head from now on.
    /// Branch here. `as_name` = None: the name follows the fork and the tip
    /// is left as a bookmark (carry). `as_name` = Some: the fork becomes a
    /// NEW agent with that name and this one keeps its session (split).
    pub async fn branch_agent(
        &self,
        name: &str,
        label: Option<&str>,
        at_message: Option<&str>,
        as_name: Option<&str>,
    ) -> Result<Arc<ManagedSession>> {
        let rows = self.inner.store.agents()?;
        let row = rows
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow!("no agent named {name} on record"))?;
        let head = row
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("{name} has no session to branch from"))?;
        if !self.inner.store_for(row.harness).exists(&row.repo, &head) {
            return Err(anyhow!(
                "{name}'s session has no transcript yet — nothing to branch from"
            ));
        }
        if let Some(as_name) = as_name {
            return self.split_agent(name, as_name, &head, at_message).await;
        }
        // Bookmark the tip we're leaving.
        self.inner.store.add_bookmark(
            name,
            &head,
            None,
            label
                .or(row.title.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty()),
            "branch",
        )?;
        self.fork_to(name, &head, at_message).await
    }

    /// Resume a bookmark: bookmark the current tip (reason "swap"), then
    /// fork from the bookmark's session/point and make that the head.
    pub async fn resume_bookmark(
        &self,
        name: &str,
        id: i64,
        as_name: Option<&str>,
    ) -> Result<Arc<ManagedSession>> {
        let bm = self
            .inner
            .store
            .bookmark(name, id)?
            .ok_or_else(|| anyhow!("no bookmark {id} for {name}"))?;
        if let Some(as_name) = as_name {
            return self
                .split_agent(name, as_name, &bm.session_id, bm.message_uuid.as_deref())
                .await;
        }
        let rows = self.inner.store.agents()?;
        let row = rows
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow!("no agent named {name} on record"))?;
        if let Some(head) = &row.session_id {
            if head != &bm.session_id {
                self.inner
                    .store
                    .add_bookmark(name, head, None, row.title.as_deref(), "swap")?;
            }
        }
        self.fork_to(name, &bm.session_id, bm.message_uuid.as_deref())
            .await
    }

    /// Split: a NEW agent `as_name` (bare, same repo) starts as a fork of
    /// `from` at `at_message`; `name` keeps its session untouched. Both are
    /// siblings from here — the lineage table links the fork to its parent.
    pub async fn split_agent(
        &self,
        name: &str,
        as_name: &str,
        from: &str,
        at_message: Option<&str>,
    ) -> Result<Arc<ManagedSession>> {
        let rows = self.inner.store.agents()?;
        let row = rows
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow!("no agent named {name} on record"))?;
        let bare = crate::addr::bare(as_name.trim());
        if bare.is_empty()
            || !bare
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(anyhow!(
                "'{as_name}' is not a valid agent name (letters, digits, - and _)"
            ));
        }
        if bare == crate::addr::bare(name) {
            return Err(anyhow!("choose a different name for the split, or branch without one to carry {name} to the fork"));
        }
        let key = crate::addr::local_key(bare, &row.channel);
        if rows.iter().any(|a| a.name == key) {
            return Err(anyhow!("an agent named {key} already exists in this repo"));
        }
        let interactive = self
            .inner
            .live(name)
            .map(|s| s.broker.is_some())
            .unwrap_or(true);
        let opts = SpawnOpts {
            harness: Some(row.harness),
            charter: row.charter.clone(),
            resume: Some(from.to_owned()),
            fork: true,
            resume_at: at_message.map(str::to_owned),
            interactive,
            extra_args: row.extra_args.clone(),
            ..Default::default()
        };
        let sess = self.spawn_agent(bare, row.repo.clone(), opts).await?;
        let _ = self.inner.store.record_event(
            &sess.name,
            "split",
            serde_json::json!({ "from": name, "session": from, "at": at_message }),
        );
        Ok(sess)
    }

    /// Stop the running process (if any) and relaunch as a fork of `from`.
    async fn fork_to(
        &self,
        name: &str,
        from: &str,
        at_message: Option<&str>,
    ) -> Result<Arc<ManagedSession>> {
        let rows = self.inner.store.agents()?;
        let row = rows
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow!("no agent named {name} on record"))?;
        let interactive = self
            .inner
            .live(name)
            .map(|s| s.broker.is_some())
            .unwrap_or(true);
        if self.inner.live(name).is_some() {
            // Keep the live mark: this is a restart, not an operator stop.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.shutdown_for_restart(name),
            )
            .await;
            // Wait for the process to actually leave the roster.
            for _ in 0..50 {
                if self.inner.live(name).is_none() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        let opts = SpawnOpts {
            harness: Some(row.harness),
            charter: row.charter.clone(),
            resume: Some(from.to_owned()),
            fork: true,
            resume_at: at_message.map(str::to_owned),
            interactive,
            extra_args: row.extra_args.clone(),
            ..Default::default()
        };
        self.spawn_agent(name, row.repo.clone(), opts).await
    }

    /// Recover repos from Claude Code's on-disk session store and add the
    /// new ones to this node's registry. Returns (path, session count,
    /// newly added).
    pub fn discover_repos(&self) -> Result<Vec<(PathBuf, usize, bool)>> {
        let known: std::collections::HashSet<PathBuf> = self
            .inner
            .store
            .repos()?
            .into_iter()
            .map(|r| r.path)
            .collect();
        let mut out = Vec::new();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let adapters: Vec<Arc<dyn AgentAdapter>> = self.inner.adapters.values().cloned().collect();
        for adapter in adapters {
            for (found, sessions) in adapter.store().discover_repos() {
                let path = normalize_repo(&found);
                if !seen.insert(path.clone()) {
                    continue;
                }
                let added = !known.contains(&path);
                if added {
                    self.inner.store.add_repo(&path, None)?;
                }
                out.push((path, sessions, added));
            }
        }
        Ok(out)
    }

    /// The trust gate's decision surface: what a repo would auto-run, and
    /// whether the operator has already trusted it. Enforcement happens in
    /// the API layer so dev/CLI flows stay unchanged.
    pub fn trust_state(&self, repo: &Path) -> (crate::trust::RepoAutorun, bool) {
        let autorun = crate::trust::inspect(repo);
        let trusted = self
            .inner
            .data_dir
            .as_ref()
            .map(|d| crate::trust::TrustStore::new(d).is_trusted(repo))
            .unwrap_or(true);
        (autorun, trusted)
    }

    pub fn record_trust(&self, repo: &Path) -> Result<()> {
        let d = self
            .inner
            .data_dir
            .as_ref()
            .ok_or_else(|| anyhow!("no data dir on this node"))?;
        crate::trust::TrustStore::new(d).trust(repo)
    }

    pub fn revoke_trust(&self, repo: &Path) -> Result<()> {
        let d = self
            .inner
            .data_dir
            .as_ref()
            .ok_or_else(|| anyhow!("no data dir on this node"))?;
        crate::trust::TrustStore::new(d).revoke(repo)
    }

    pub fn set_title(&self, name: &str, title: Option<&str>) -> Result<()> {
        self.inner.store.set_agent_title(name, title)
    }

    pub fn set_charter(&self, name: &str, charter: Option<&str>) -> Result<()> {
        self.inner.store.set_agent_charter(name, charter)
    }

    /// The runtime's own view of a session: handshake (commands, models,
    /// output style, account) plus the `system/init` inventory (tools,
    /// skills, MCP servers, plugins as loaded). Never parsed from disk.
    fn agent_row(&self, name: &str) -> Result<crate::store::AgentRow> {
        self.inner
            .store
            .agents()?
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow!("no agent named @{name} on record"))
    }

    /// Files this agent's session named in tool calls, newest first.
    pub fn artifacts(&self, name: &str) -> Result<Vec<crate::artifacts::Artifact>> {
        let row = self.agent_row(name)?;
        Ok(row
            .session_id
            .as_deref()
            .map(|sid| {
                crate::artifacts::touched_paths_for(
                    row.harness,
                    &self.inner.store_for(row.harness).main_path(&row.repo, sid),
                )
            })
            .unwrap_or_default())
    }

    /// Resolve a path the operator wants to see for this agent, under the
    /// serving rule (artifacts.rs).
    pub fn agent_file(&self, name: &str, path: &str) -> Result<std::path::PathBuf> {
        let row = self.agent_row(name)?;
        crate::artifacts::resolve(
            self.inner.data_dir.as_deref(),
            &row.repo,
            row.session_id.as_deref(),
            path,
            row.harness,
        )
    }

    pub fn file_stat(&self, name: &str, path: &str) -> Result<serde_json::Value> {
        let p = self.agent_file(name, path)?;
        Ok(crate::artifacts::stat(&p))
    }

    /// One chunk of a served file, base64 on the wire.
    pub fn file_read(
        &self,
        name: &str,
        path: &str,
        offset: u64,
        len: u64,
    ) -> Result<serde_json::Value> {
        let p = self.agent_file(name, path)?;
        let bytes = crate::artifacts::read_chunk(&p, offset, len)?;
        Ok(serde_json::json!({
            "offset": offset,
            "len": bytes.len(),
            "data": aspen_wire::b64::encode(&bytes),
        }))
    }

    // ------------------------------------------------------- migration

    fn my_node_name(&self) -> String {
        self.inner
            .mesh()
            .map(|m| m.identity.node.clone())
            .unwrap_or_else(|| "local".into())
    }

    /// Stage a bundle for `name` (migrate.rs). For a move, the session is
    /// stopped first so the transcript is final; the live mark is cleared
    /// so this node does not revive it.
    pub async fn session_export(
        &self,
        name: &str,
        opts: &crate::migrate::ExportOpts,
    ) -> Result<(String, crate::migrate::Manifest)> {
        let row = self.agent_row(name)?;
        if let Some(to) = &row.moved_to {
            return Err(anyhow!("@{name} already moved — it is now @{to}"));
        }
        // Nothing to carry yet (a session that has not produced a
        // transcript): refuse before stopping anything.
        if let Some(sid) = row.session_id.as_deref() {
            if !self.inner.store_for(row.harness).exists(&row.repo, sid) {
                return Err(anyhow!(
                    "@{name} has no transcript yet (session {sid}); give it a first prompt or stop it instead"
                ));
            }
        } else {
            return Err(anyhow!("@{name} has no session to migrate"));
        }
        if opts.mode == "move" && self.inner.live(name).is_some() {
            self.shutdown_agent(name).await?;
            // Let the process close its transcript.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let data_dir = self
            .inner
            .data_dir
            .clone()
            .ok_or_else(|| anyhow!("node has no data dir"))?;
        let node = self.my_node_name();
        let store = self.inner.store.clone();
        let row2 = row.clone();
        let opts2 = opts.clone();
        let (dir, manifest) = tokio::task::spawn_blocking(move || {
            crate::migrate::export(&data_dir, &node, &row2, &store, &opts2)
        })
        .await??;
        let _ = dir;
        Ok((manifest.bundle_id.clone(), manifest))
    }

    /// What a target needs to preflight a move: the repo's identity.
    /// Start a session here from a replica of `agent@from_node` held on
    /// this node (REPLICATION.md): stage it as a bundle, import as a copy
    /// (a fork — the original may still run), spawn it.
    pub async fn pull_from_replica(
        &self,
        from_node: &str,
        agent: &str,
        opts: &crate::migrate::ImportOpts,
    ) -> Result<serde_json::Value> {
        let inner = self.inner.clone();
        let data_dir = inner
            .data_dir
            .clone()
            .ok_or_else(|| anyhow!("no data dir"))?;
        let r = crate::replicate::find(&inner, from_node, agent)
            .ok_or_else(|| anyhow!("no replica of @{agent}@{from_node} held here"))?;
        if r.main.is_none() {
            return Err(anyhow!(
                "the replica of @{agent}@{from_node} has no main transcript yet"
            ));
        }
        let mut o = opts.clone();
        o.mode = Some("copy".into());
        let dd = data_dir.clone();
        let store = inner.store.clone();
        let (report, dir) = tokio::task::spawn_blocking(
            move || -> Result<(crate::migrate::ImportReport, PathBuf)> {
                let dir = crate::replicate::stage_bundle(&dd, &r)?;
                let rep = crate::migrate::import(&dd, &store, &dir, &o)?;
                Ok((rep, dir))
            },
        )
        .await??;
        let _ = std::fs::remove_dir_all(&dir);
        let repo = PathBuf::from(&report.repo);
        let revived = self
            .spawn_agent(
                &report.name,
                repo,
                SpawnOpts {
                    resume: Some(report.session_id.clone()),
                    fork: true,
                    interactive: true,
                    ..Default::default()
                },
            )
            .await;
        let (ok, note) = match revived {
            Ok(_) => (true, None),
            Err(e) => (false, Some(format!("{e:#}"))),
        };
        let _ = inner.store.record_event(
            &report.name,
            "migrated",
            serde_json::json!({ "from": from_node, "mode": "replica", "session_id": report.session_id }),
        );
        Ok(serde_json::json!({
            "name": report.name,
            "repo": report.repo,
            "session_id": report.session_id,
            "mode": "replica",
            "files": report.files,
            "notes": report.notes,
            "revived": ok,
            "revive_note": note,
        }))
    }

    /// Start a session from a template (PLUGINS.md §templates), with
    /// overrides from the caller (`name`, `repo`, `charter`, `model`,
    /// `extra_args`, `skip_permissions`, `title`). The trust gate is the
    /// API layer's, as for any spawn. Session-scope
    /// plugin rules are written for the new name first, so the process
    /// starts with its `--plugin-dir`s. Board placement is the console's.
    pub async fn spawn_from_template(
        &self,
        id: &str,
        overrides: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let t = self
            .inner
            .store
            .templates(false)?
            .into_iter()
            .find(|t| t.id == id || t.name == id)
            .ok_or_else(|| anyhow!("no template {id:?}"))?;
        let spec = &t.spec;
        let ov = |k: &str| overrides.get(k).filter(|v| !v.is_null());
        let name = ov("name")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| spec.get("name").and_then(|v| v.as_str()).map(str::to_owned))
            .ok_or_else(|| anyhow!("a name is required (the template names none)"))?;
        // Repo: an override path, else the template's repo (a handle, a
        // basename, or an origin URL) resolved against this node's repos.
        let repo_ref = ov("repo")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| spec.get("repo").and_then(|v| v.as_str()).map(str::to_owned))
            .ok_or_else(|| anyhow!("a repo is required (the template names none)"))?;
        let repo = if Path::new(&repo_ref).is_absolute() {
            PathBuf::from(&repo_ref)
        } else {
            let repos = self.inner.store.repos()?;
            repos
                .iter()
                .find(|r| r.handle == repo_ref)
                .or_else(|| repos.iter().find(|r| r.path.file_name().map(|n| n.to_string_lossy() == repo_ref).unwrap_or(false)))
                .or_else(|| repos.iter().find(|r| crate::migrate::git_origin(&r.path).as_deref() == Some(repo_ref.as_str())))
                .map(|r| r.path.clone())
                .ok_or_else(|| anyhow!("no repo on this node matches {repo_ref:?} (handle, basename or origin); give a path"))?
        };
        let handle = self.inner.store.ensure_handle(&repo)?;
        let key = format!("{name}@{handle}");
        // Plugins: session-scope rules for the new address.
        if let Some(plugins) = spec.get("plugins").and_then(|p| p.as_array()) {
            let now = self.inner.store.hlc_now();
            for pl in plugins {
                let (Some(m), Some(pn)) = (
                    pl.get("marketplace").and_then(|v| v.as_str()),
                    pl.get("plugin").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let rule = crate::plugins::Rule {
                    id: format!("tpl-{}-{}-{}", t.id, pn, key).replace(['@', '/'], "_"),
                    marketplace: m.to_owned(),
                    plugin: pn.to_owned(),
                    scope_kind: "session".into(),
                    scope: key.clone(),
                    enabled: true,
                    pin: pl.get("pin").and_then(|v| v.as_str()).map(str::to_owned),
                    updated_at: now,
                    deleted: false,
                };
                let _ = self.inner.store.upsert_plugin_rule(&rule);
            }
            crate::plugins::spawn_sync(self.inner.clone(), None);
        }
        let pick_str = |k: &str| {
            ov(k)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .or_else(|| spec.get(k).and_then(|v| v.as_str()).map(str::to_owned))
        };
        let skip = ov("skip_permissions")
            .and_then(|v| v.as_bool())
            .or_else(|| {
                spec.get("permission")
                    .and_then(|v| v.as_str())
                    .map(|p| p == "skip")
            });
        let opts = SpawnOpts {
            harness: pick_str("harness").and_then(|h| Harness::parse(&h)),
            charter: pick_str("charter"),
            model: pick_str("model"),
            extra_args: pick_str("extra_args"),
            skip_permissions: skip,
            interactive: true,
            ..Default::default()
        };
        let sess = self.spawn_agent(&name, repo, opts).await?;
        if let Some(title) = pick_str("title") {
            let _ = self.inner.store.set_agent_title(&sess.name, Some(&title));
        }
        let _ = self.inner.store.record_event(
            &sess.name,
            "template",
            serde_json::json!({ "template": t.name, "id": t.id }),
        );
        Ok(serde_json::json!({
            "name": sess.name,
            "bare": crate::addr::bare(&sess.name),
            "template": t.name,
            "board": spec.get("board").cloned().unwrap_or(serde_json::Value::Null),
        }))
    }

    /// What a move would carry (MIGRATION.md preflight): tier sizes, the
    /// harness version here, whether the tree is dirty, whether the
    /// session is busy.
    pub fn session_preflight(&self, name: &str) -> Result<serde_json::Value> {
        let spec = self.session_spec(name)?;
        let row = self.agent_row(name)?;
        let sid = spec.session_id.clone();
        let files = self.inner.store_for(row.harness).files(&row.repo, &sid);
        let mut a = 0u64;
        let mut b = 0u64;
        let main = self.inner.store_for(row.harness).main_path(&row.repo, &sid);
        for (_rel, p) in &files {
            let n = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            if p == &main {
                a += n;
            } else {
                b += n;
            }
        }
        let mem = crate::memory::memory_dir(&row.repo);
        let c: u64 = crate::memory::read_dir_files(&mem)
            .values()
            .map(|(_, t)| t.len() as u64)
            .sum();
        let git = crate::gitstate::get(&row.repo);
        let live = self.inner.live(name);
        Ok(serde_json::json!({
            "agent": spec,
            "tiers": { "A": a, "B": b, "C": c },
            "files": files.len(),
            "harness": self.inner.adapter(row.harness).and_then(|a| a.version()),
            "harness_name": row.harness,
            "dirty": git.as_ref().map(|g| g.dirty).unwrap_or(0),
            "branch": git.as_ref().and_then(|g| g.branch.clone()),
            "busy": live.as_ref().map(|m| matches!(m.turn_state(), TurnState::Busy)).unwrap_or(false),
            "live": live.is_some(),
        }))
    }

    pub fn session_spec(&self, name: &str) -> Result<crate::migrate::AgentSpec> {
        let row = self.agent_row(name)?;
        if let Some(to) = &row.moved_to {
            return Err(anyhow!("@{name} already moved — it is now @{to}"));
        }
        let sid = row
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("@{name} has no session to migrate"))?;
        Ok(crate::migrate::AgentSpec {
            name: row.name.clone(),
            channel: row.channel.clone(),
            session_id: sid,
            charter: row.charter.clone(),
            title: row.title.clone(),
            extra_args: row.extra_args.clone(),
            repo_basename: row
                .repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            repo_origin: crate::migrate::git_origin(&row.repo),
            harness: row.harness,
        })
    }

    fn bundle_dir(&self, bundle_id: &str) -> Result<PathBuf> {
        if bundle_id.is_empty() || bundle_id.contains(['/', '\\', '.']) {
            return Err(anyhow!("bad bundle id"));
        }
        let data_dir = self
            .inner
            .data_dir
            .clone()
            .ok_or_else(|| anyhow!("node has no data dir"))?;
        let dir = crate::migrate::staging_root(&data_dir).join(bundle_id);
        if !dir.is_dir() {
            return Err(anyhow!("no such bundle {bundle_id}"));
        }
        Ok(dir)
    }

    /// One chunk of a staged bundle file, base64.
    pub fn bundle_read(
        &self,
        bundle_id: &str,
        rel: &str,
        offset: u64,
        len: u64,
    ) -> Result<serde_json::Value> {
        let dir = self.bundle_dir(bundle_id)?;
        if rel.contains("..") {
            return Err(anyhow!("bad path"));
        }
        let bytes = crate::artifacts::read_chunk(&dir.join(rel), offset, len)?;
        Ok(
            serde_json::json!({ "offset": offset, "len": bytes.len(), "data": aspen_wire::b64::encode(&bytes) }),
        )
    }

    pub fn bundle_done(&self, bundle_id: &str) -> Result<()> {
        let dir = self.bundle_dir(bundle_id)?;
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    /// The source's last step of a move: tombstone the row, re-address
    /// its undelivered bus rows to the new home.
    pub fn session_moved(
        &self,
        name: &str,
        to_node: &str,
        new_name: &str,
    ) -> Result<serde_json::Value> {
        let row = self.agent_row(name)?;
        let _ = row;
        // The tombstone carries the full new address: the name may have
        // changed with the target repo's handle.
        let full = format!("{new_name}@{to_node}");
        self.inner.store.set_moved_to(name, Some(&full))?;
        let n = self
            .inner
            .store
            .rehome_pending(name, &full, &format!("@{full}"))
            .unwrap_or(0);
        let _ = self.inner.store.record_event(
            name,
            "moved",
            serde_json::json!({ "to": to_node, "as": new_name, "rehomed": n }),
        );
        crate::federation::broadcast_roster(&self.inner);
        Ok(serde_json::json!({ "rehomed": n }))
    }

    /// The target side of a move/copy: fetch a bundle from `from_node`
    /// over the mesh, install it here, revive it, and (for a move) tell
    /// the source it is gone.
    pub async fn pull_session(
        &self,
        from_node: &str,
        agent: &str,
        opts: &crate::migrate::ImportOpts,
    ) -> Result<serde_json::Value> {
        let mesh = self
            .inner
            .mesh()
            .ok_or_else(|| anyhow!("this node is not in a mesh"))?;
        let mode = opts.mode.clone().unwrap_or_else(|| "move".into());
        let t = std::time::Duration::from_secs(120);
        // 0. Preflight here before the source is touched: the target repo
        // must resolve, or the operator gets the error with nothing
        // stopped anywhere.
        let spec_v = mesh
            .api_call(from_node, "session_spec", agent, serde_json::json!({}), t)
            .await
            .map_err(|e| anyhow!("asking {from_node} about @{agent}: {e}"))?;
        if let Some(err) = spec_v.get("error").and_then(|e| e.as_str()) {
            return Err(anyhow!("on {from_node}: {err}"));
        }
        let spec: crate::migrate::AgentSpec = serde_json::from_value(spec_v)?;
        let target_repo = match &opts.repo {
            Some(r) => PathBuf::from(r),
            None => {
                crate::migrate::find_counterpart(&self.inner.store, &spec).ok_or_else(|| {
                    anyhow!(
                        "no repo on {} matches {} ({}); pass one",
                        self.my_node_name(),
                        spec.repo_basename,
                        spec.repo_origin.as_deref().unwrap_or("no origin")
                    )
                })?
            }
        };
        if !target_repo.is_dir() {
            return Err(anyhow!(
                "target repo does not exist on {}: {}",
                self.my_node_name(),
                target_repo.display()
            ));
        }
        let mut opts = opts.clone();
        opts.repo = Some(target_repo.to_string_lossy().into_owned());
        let opts = &opts;
        // Anything failing past this point in a move brings the source
        // back: its session was stopped for the export.
        let (mesh_r, mode_r, from_r, agent_r) = (
            mesh.clone(),
            mode.clone(),
            from_node.to_owned(),
            agent.to_owned(),
        );
        let restore = move |why: anyhow::Error| {
            let (mesh_r, mode_r, from_r, agent_r) = (
                mesh_r.clone(),
                mode_r.clone(),
                from_r.clone(),
                agent_r.clone(),
            );
            async move {
                if mode_r == "move" {
                    let _ = mesh_r
                        .api_call(
                            &from_r,
                            "revive",
                            &agent_r,
                            serde_json::json!({ "resume_choice": "in_place" }),
                            t,
                        )
                        .await;
                    anyhow!("{why:#} — the session was restarted on {from_r}")
                } else {
                    why
                }
            }
        };
        // 1. Export on the source (stops the session for a move).
        let exported = mesh
            .api_call(
                from_node,
                "session_export",
                agent,
                serde_json::json!({ "mode": mode }),
                t,
            )
            .await
            .map_err(|e| anyhow!("export on {from_node}: {e}"))?;
        if let Some(err) = exported.get("error").and_then(|e| e.as_str()) {
            return Err(anyhow!("export on {from_node}: {err}"));
        }
        let bundle_id = exported
            .get("bundle_id")
            .and_then(|b| b.as_str())
            .ok_or_else(|| anyhow!("export returned no bundle id"))?
            .to_owned();
        let manifest: crate::migrate::Manifest = serde_json::from_value(
            exported
                .get("manifest")
                .cloned()
                .ok_or_else(|| anyhow!("export returned no manifest"))?,
        )?;
        // 2. Fetch every file, chunked, into local staging.
        let data_dir = self
            .inner
            .data_dir
            .clone()
            .ok_or_else(|| anyhow!("node has no data dir"))?;
        let local_dir = crate::migrate::staging_root(&data_dir).join(format!("in-{bundle_id}"));
        std::fs::create_dir_all(&local_dir)?;
        std::fs::write(
            local_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        let mut total = 0u64;
        for f in &manifest.files {
            let out = local_dir.join(&f.rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut buf: Vec<u8> = Vec::with_capacity(f.size as usize);
            let mut offset = 0u64;
            while offset < f.size {
                let chunk = match mesh
                    .api_call(
                        from_node,
                        "bundle_read",
                        agent,
                        serde_json::json!({ "bundle_id": bundle_id, "rel": f.rel, "offset": offset, "len": crate::artifacts::READ_CHUNK }),
                        t,
                    )
                    .await
                {
                    Ok(c) => c,
                    Err(e) => return Err(restore(anyhow!("reading {} from {from_node}: {e}", f.rel)).await),
                };
                let data = chunk.get("data").and_then(|d| d.as_str()).unwrap_or("");
                let bytes = match aspen_wire::b64::decode(data) {
                    Ok(b) => b,
                    Err(e) => return Err(restore(anyhow!("bad chunk: {e}")).await),
                };
                if bytes.is_empty() {
                    break;
                }
                offset += bytes.len() as u64;
                buf.extend_from_slice(&bytes);
            }
            total += buf.len() as u64;
            std::fs::write(&out, &buf)?;
        }
        let _ = mesh
            .api_call(
                from_node,
                "bundle_done",
                agent,
                serde_json::json!({ "bundle_id": bundle_id }),
                t,
            )
            .await;
        // 3. Install.
        let store = self.inner.store.clone();
        let dir2 = local_dir.clone();
        let opts2 = opts.clone();
        let dd = data_dir.clone();
        let report = match tokio::task::spawn_blocking(move || {
            crate::migrate::import(&dd, &store, &dir2, &opts2)
        })
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(restore(e).await),
            Err(e) => return Err(restore(anyhow!("{e}")).await),
        };
        let _ = std::fs::remove_dir_all(&local_dir);
        // 4. Revive: in place for a move (ours now: mark live first), as a
        // fork for a copy (the source keeps its id).
        let revived = if mode == "copy" {
            let row = self.agent_row(&report.name)?;
            let sopts = SpawnOpts {
                harness: Some(row.harness),
                charter: row.charter.clone(),
                resume: Some(report.session_id.clone()),
                fork: true,
                interactive: true,
                extra_args: row.extra_args.clone(),
                ..Default::default()
            };
            self.spawn_agent(&report.name, row.repo.clone(), sopts)
                .await
                .map(|_| true)
        } else {
            let _ = self.inner.store.set_agent_live(&report.name, true);
            self.revive_agent(&report.name, true, None)
                .await
                .map(|_| true)
        };
        let revive_note = match &revived {
            Ok(_) => None,
            Err(e) => Some(format!("installed but not started: {e:#}")),
        };
        // 5. Tell the source.
        let mut rehomed = 0u64;
        if mode == "move" {
            let me = self.my_node_name();
            match mesh
                .api_call(
                    from_node,
                    "session_moved",
                    agent,
                    serde_json::json!({ "to": me, "as": report.name }),
                    t,
                )
                .await
            {
                Ok(v) => rehomed = v.get("rehomed").and_then(|n| n.as_u64()).unwrap_or(0),
                Err(e) => {
                    tracing::warn!(error = %e, "source did not acknowledge the move; its row is not tombstoned")
                }
            }
        }
        let _ = self.inner.store.record_event(
            &report.name,
            "migrated",
            serde_json::json!({ "from": from_node, "mode": mode, "bytes": total }),
        );
        Ok(serde_json::json!({
            "name": report.name,
            "node": self.my_node_name(),
            "repo": report.repo,
            "session_id": report.session_id,
            "mode": mode,
            "files": report.files,
            "bytes": total,
            "memory_conflicts": report.memory_conflicts,
            "notes": report.notes,
            "residue": report.residue,
            "revived": revived.is_ok(),
            "revive_note": revive_note,
            "rehomed": rehomed,
            "harness_version": manifest.harness_version,
        }))
    }

    /// Export to a `.aspen-session` file (tar of the bundle).
    pub async fn session_export_file(
        &self,
        name: &str,
        out: &std::path::Path,
        tiers: Option<Vec<String>>,
    ) -> Result<crate::migrate::Manifest> {
        let opts = crate::migrate::ExportOpts {
            mode: "export".into(),
            tiers,
        };
        let (bundle_id, manifest) = self.session_export(name, &opts).await?;
        let dir = self.bundle_dir(&bundle_id)?;
        crate::migrate::pack(&dir, out)?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(manifest)
    }

    /// Import a `.aspen-session` file; registers the agent, not live.
    pub async fn session_import_file(
        &self,
        file: &std::path::Path,
        opts: &crate::migrate::ImportOpts,
    ) -> Result<crate::migrate::ImportReport> {
        let data_dir = self
            .inner
            .data_dir
            .clone()
            .ok_or_else(|| anyhow!("node has no data dir"))?;
        let dir =
            crate::migrate::staging_root(&data_dir).join(format!("file-{}", uuid::Uuid::new_v4()));
        crate::migrate::unpack(file, &dir)?;
        let store = self.inner.store.clone();
        let dir2 = dir.clone();
        let opts2 = opts.clone();
        let report = tokio::task::spawn_blocking(move || {
            crate::migrate::import(&data_dir, &store, &dir2, &opts2)
        })
        .await??;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(report)
    }

    pub fn runtime_info(&self, name: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let info = sess.handle.runtime_info();
        Ok(serde_json::json!({
            "handshake": info.raw,
            "runtime": info,
            "harness": sess.harness,
            "capabilities": sess.handle.capabilities(),
            "modes": self.inner.adapter(sess.harness).map(|a| a.permission_modes()).unwrap_or_default(),
            "inventory": sess.inventory.lock().unwrap().clone(),
        }))
    }

    /// Rich context breakdown from the runtime (poll at turn end).
    pub async fn context_usage(&self, name: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.context_usage().await
    }

    /// Switch a session's model (takes effect next turn).
    pub async fn set_model(&self, name: &str, model: Option<&str>) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.set_model(model).await
    }

    /// Live-switch a session's permission mode.
    pub async fn set_permission_mode(&self, name: &str, mode: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.set_mode(mode).await
    }

    /// Every permission prompt / question currently held open on THIS node,
    /// tagged with the agent holding it.
    pub fn open_prompts(&self) -> Vec<(String, crate::permit::OpenPrompt)> {
        let sessions: Vec<Arc<ManagedSession>> = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        let mut out = Vec::new();
        for s in sessions {
            if let Some(b) = &s.broker {
                for p in b.open_prompts() {
                    out.push((s.name.clone(), p));
                }
            }
        }
        out.sort_by(|a, b| a.1.asked_at.total_cmp(&b.1.asked_at));
        out
    }

    /// Reload a live session's plugins/skills/commands from disk.
    pub async fn reload_plugins(&self, name: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.reload().await
    }

    /// The session's MCP servers; `refresh` asks the harness now
    /// (PROPOSALS-MCP.md §6.2), else the last picture.
    pub async fn mcp_list(&self, name: &str, refresh: bool) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let servers = if refresh || sess.mcp.lock().unwrap().is_empty() {
            refresh_mcp(&self.inner, &sess).await?
        } else {
            sess.mcp.lock().unwrap().clone()
        };
        let caps = sess.handle.capabilities();
        Ok(serde_json::json!({
            "servers": servers,
            "refreshed_at": *sess.mcp_refreshed_at.lock().unwrap(),
            "can": { "status": caps.mcp_status, "reconnect": caps.mcp_reconnect, "toggle": caps.mcp_toggle, "auth": caps.mcp_auth, "add": caps.mcp_add },
        }))
    }

    /// A one-line recap of the session from the harness itself
    /// (PROPOSALS-2026-09-C.md §1): Claude's `/recap` runs as a side
    /// query — nothing enters the conversation, the transcript gains only
    /// a local-command marker — and its answer comes back here. Refused
    /// while a turn is running: the recap would queue behind it and answer
    /// late; refused for a harness without one, where the digest is what
    /// there is.
    pub async fn recap(&self, name: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        if !sess.handle.capabilities().recap {
            return Err(anyhow!(
                "unsupported: {} has no recap of its own",
                sess.harness
            ));
        }
        if sess.turn_state() == TurnState::Busy {
            return Err(anyhow!(
                "busy: @{name} is mid-turn; ask again when it is idle"
            ));
        }
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        {
            let mut cap = sess.recap.lock().unwrap();
            if cap.is_some() {
                return Err(anyhow!("busy: a recap is already being asked"));
            }
            *cap = Some(RecapCapture {
                text: String::new(),
                done: Some(tx),
            });
        }
        let started = std::time::Instant::now();
        if let Err(e) = sess.handle.send_user("/recap".to_owned()).await {
            sess.recap.lock().unwrap().take();
            return Err(e);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(90), rx).await {
            Ok(Ok(text)) => Ok(serde_json::json!({
                "text": text,
                "took_ms": started.elapsed().as_millis() as u64,
                "at": crate::store::now_epoch(),
            })),
            Ok(Err(_)) => {
                sess.recap.lock().unwrap().take();
                Err(anyhow!("the session ended before it answered"))
            }
            Err(_) => {
                sess.recap.lock().unwrap().take();
                Err(anyhow!("no recap within 90s"))
            }
        }
    }

    pub async fn mcp_reconnect(&self, name: &str, server: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let r = sess.handle.mcp_reconnect(server).await;
        let servers = refresh_mcp(&self.inner, &sess).await.unwrap_or_default();
        match r {
            Ok(()) => Ok(serde_json::json!({ "ok": true, "servers": servers })),
            Err(e) => {
                Ok(serde_json::json!({ "ok": false, "error": e.to_string(), "servers": servers }))
            }
        }
    }

    pub async fn mcp_toggle(
        &self,
        name: &str,
        server: &str,
        enabled: bool,
    ) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.mcp_toggle(server, enabled).await?;
        let servers = refresh_mcp(&self.inner, &sess).await.unwrap_or_default();
        Ok(serde_json::json!({ "ok": true, "servers": servers }))
    }

    pub async fn mcp_auth(&self, name: &str, server: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let a = sess.handle.mcp_authenticate(server).await?;
        Ok(serde_json::to_value(a)?)
    }

    pub async fn mcp_add(
        &self,
        name: &str,
        server: &str,
        config: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.mcp_add(server, config).await?;
        let servers = refresh_mcp(&self.inner, &sess).await.unwrap_or_default();
        Ok(serde_json::json!({ "ok": true, "servers": servers }))
    }

    /// The session's activity ledger with the console's own stops
    /// applied (a stopped row reads "stopped" until the harness says so).
    pub async fn activities(&self, name: &str) -> Result<Vec<serde_json::Value>> {
        let row = self.agent_row(name)?;
        let Some(sid) = row.session_id.clone() else {
            return Ok(Vec::new());
        };
        let st = self.inner.store_for(row.harness);
        let (repo, since) = (row.repo.clone(), row.last_spawned_at);
        let mut acts = tokio::task::spawn_blocking(move || st.activities(&repo, &sid, since))
            .await
            .unwrap_or_default();
        if let Some(sess) = self.inner.live(name) {
            let stopped = sess.stopped_acts.lock().unwrap().clone();
            for a in acts.iter_mut() {
                let id = a
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_owned();
                let tu = a
                    .get("tool_use_id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_owned();
                if let Some(at) = stopped.get(&id).or_else(|| stopped.get(&tu)) {
                    if a.get("status").and_then(|s| s.as_str()) == Some("running") {
                        a["status"] = serde_json::json!("stopped");
                        a["ended_at"] = serde_json::json!(iso_of(*at));
                        a["detail"]["stopped_by"] = serde_json::json!("operator");
                    }
                }
            }
        }
        acts.reverse();
        Ok(acts)
    }

    /// The session's child processes (PROPOSALS-MCP.md §6.4): each with
    /// the ledger row it matches, if any — the rest are what hooks and
    /// plugins started outside the tool stream.
    pub async fn processes(&self, name: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let Some(pid) = sess.handle.pid() else {
            return Ok(serde_json::json!({ "pid": null, "processes": [] }));
        };
        let acts = {
            let row = self.agent_row(name)?;
            let st = self.inner.store_for(row.harness);
            match row.session_id.as_deref() {
                Some(sid) => {
                    let (repo, sid, since) =
                        (row.repo.clone(), sid.to_owned(), row.last_spawned_at);
                    tokio::task::spawn_blocking(move || st.activities(&repo, &sid, since))
                        .await
                        .unwrap_or_default()
                }
                None => Vec::new(),
            }
        };
        let procs = tokio::task::spawn_blocking(move || crate::procs::descendants(pid))
            .await
            .unwrap_or_default();
        let list: Vec<serde_json::Value> = procs
            .iter()
            .map(|p| {
                let matched = acts.iter().find(|a| {
                    a.get("status").and_then(|s| s.as_str()) == Some("running")
                        && a.get("detail").and_then(|d| d.get("command")).and_then(|c| c.as_str()).is_some_and(|c| crate::procs::matches_script(&p.cmdline, c))
                });
                serde_json::json!({
                    "pid": p.pid, "parent": p.parent, "cmdline": p.cmdline, "age_secs": p.age_secs,
                    "activity": matched.map(|a| serde_json::json!({ "id": a.get("id"), "kind": a.get("kind"), "label": a.get("label") })),
                })
            })
            .collect();
        Ok(serde_json::json!({ "pid": pid, "processes": list }))
    }

    /// Stop a background activity by terminating the process its script
    /// runs in, or a bare child process by pid. Returns what was stopped.
    pub async fn stop_process(
        &self,
        name: &str,
        activity_id: Option<&str>,
        pid: Option<u32>,
    ) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let root = sess
            .handle
            .pid()
            .ok_or_else(|| anyhow!("the session's process id is not known"))?;
        let procs = tokio::task::spawn_blocking(move || crate::procs::descendants(root))
            .await
            .unwrap_or_default();
        let target = if let Some(pid) = pid {
            procs
                .iter()
                .find(|p| p.pid == pid)
                .cloned()
                .ok_or_else(|| anyhow!("pid {pid} is not a child of this session"))?
        } else {
            let id = activity_id.ok_or_else(|| anyhow!("an activity id or a pid is needed"))?;
            let row = self.agent_row(name)?;
            let st = self.inner.store_for(row.harness);
            let sid = row
                .session_id
                .clone()
                .ok_or_else(|| anyhow!("no session"))?;
            let (repo, since) = (row.repo.clone(), row.last_spawned_at);
            let acts = tokio::task::spawn_blocking(move || st.activities(&repo, &sid, since))
                .await
                .unwrap_or_default();
            let act = acts
                .iter()
                .find(|a| {
                    a.get("id").and_then(|i| i.as_str()) == Some(id)
                        || a.get("tool_use_id").and_then(|i| i.as_str()) == Some(id)
                })
                .ok_or_else(|| anyhow!("no activity {id}"))?;
            let script = act
                .get("detail")
                .and_then(|d| d.get("command"))
                .and_then(|c| c.as_str())
                .ok_or_else(|| anyhow!("that activity has no script to match a process by"))?;
            procs
                .iter()
                .find(|p| crate::procs::matches_script(&p.cmdline, script))
                .cloned()
                .ok_or_else(|| anyhow!("no running process under this session matches the script; it may have already ended"))?
        };
        let tpid = target.pid;
        tokio::task::spawn_blocking(move || crate::procs::terminate(tpid)).await??;
        if let Some(id) = activity_id {
            sess.stopped_acts
                .lock()
                .unwrap()
                .insert(id.to_owned(), crate::store::now_epoch());
        }
        let _ = self.inner.store.record_event(name, "process_stopped", serde_json::json!({ "pid": target.pid, "cmdline": target.cmdline, "activity": activity_id }));
        Ok(serde_json::json!({ "ok": true, "pid": target.pid, "cmdline": target.cmdline }))
    }

    /// Reload every live session running in a given repo (after a skill edit).
    pub async fn reload_repo(&self, repo: &Path) -> usize {
        let targets: Vec<Arc<ManagedSession>> = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.repo == repo)
            .cloned()
            .collect();
        let mut n = 0;
        for s in targets {
            if s.handle.reload().await.is_ok() {
                n += 1;
            }
        }
        n
    }

    /// Console answer to a pending permission prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn answer_permission(
        &self,
        name: &str,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<serde_json::Value>,
        updated_permissions: Option<serde_json::Value>,
    ) -> Result<()> {
        self.answer_permission_with(
            name,
            request_id,
            allow,
            message,
            updated_input,
            updated_permissions,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn answer_permission_with(
        &self,
        name: &str,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<serde_json::Value>,
        updated_permissions: Option<serde_json::Value>,
        decision_id: Option<String>,
    ) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let broker = sess
            .broker
            .as_ref()
            .ok_or_else(|| anyhow!("@{name} was not spawned interactively"))?;
        if broker.answer_with(
            request_id,
            allow,
            message,
            updated_input,
            updated_permissions,
            decision_id,
        ) {
            Ok(())
        } else {
            Err(anyhow!(
                "prompt {request_id} is no longer open (answered, cancelled, or timed out)"
            ))
        }
    }
}

/// The charter preamble every Aspen agent gets, ahead of any user-provided
/// charter: who you are on the bus, in the runtime's own system prompt.
fn charter_text(key: &str, channel: &str, node: &str, user_charter: Option<&str>) -> String {
    let bare = crate::addr::bare(key);
    let mut t = format!(
        "You are {key} — agent '{bare}' in repo channel #{channel} on node '{node}' of the aspen mesh. \
         Agents are named per repo, so address peers as name@repo (name alone reaches a peer in \
         your own repo; add @node only when the same repo exists on several nodes). Peers message \
         you via the bus; those messages arrive prefixed with an [aspen bus] header naming the \
         sender. Reply to peers with the bus_send tool — never by writing files at them. The human \
         operator is @operator."
    );
    if let Some(c) = user_charter {
        t.push_str("\n\nYour charter:\n");
        t.push_str(c);
    }
    t
}

/// Per-session event pump: updates exact turn state, correlates ingestion
/// acks into the trail, fans events out, and nudges delivery at boundaries.
async fn pump(
    inner: Arc<NodeInner>,
    sess: Arc<ManagedSession>,
    mut rx: tokio::sync::mpsc::Receiver<SessionEvent>,
) {
    while let Some(ev) = rx.recv().await {
        // A recap in flight: the side turn is not the conversation. Its
        // text goes to the request that asked; its result ends the capture;
        // nothing reaches observers, the ledger or the notices.
        let capturing = sess.recap.lock().unwrap().is_some();
        if capturing {
            match &ev {
                SessionEvent::TurnEnded { result_text, .. } => {
                    let cap = sess.recap.lock().unwrap().take();
                    if let Some(mut c) = cap {
                        let text = result_text
                            .as_deref()
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| c.text.trim().to_owned());
                        if let Some(tx) = c.done.take() {
                            let _ = tx.send(text);
                        }
                    }
                    *sess.turn_state.lock().unwrap() = TurnState::Idle;
                    continue;
                }
                SessionEvent::AssistantMessage { raw, .. } => {
                    if let Some(blocks) = raw.pointer("/message/content").and_then(|c| c.as_array())
                    {
                        let mut cap = sess.recap.lock().unwrap();
                        if let Some(c) = cap.as_mut() {
                            for b in blocks {
                                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                    c.text.push_str(t);
                                }
                            }
                        }
                    }
                    continue;
                }
                SessionEvent::Exited { .. } => {
                    // The process died mid-recap: drop the capture (the
                    // request sees the closed channel) and handle the exit.
                    sess.recap.lock().unwrap().take();
                }
                _ => continue,
            }
        }
        match &ev {
            SessionEvent::TurnEnded {
                total_cost_usd,
                result_text,
                raw,
                ..
            } => {
                *sess.turn_state.lock().unwrap() = TurnState::Idle;
                *sess.busy_since.lock().unwrap() = None;
                *sess.last_tool.lock().unwrap() = None;
                {
                    let prev_cost = sess.summary.lock().unwrap().cost_usd;
                    let _ = inner.store.record_event(
                        &sess.name,
                        "turn",
                        serde_json::json!({
                            "duration_ms": raw.get("duration_ms").and_then(|d| d.as_u64()),
                            "cost_usd": total_cost_usd,
                            "cost_delta": match (total_cost_usd, prev_cost) {
                                (Some(c), Some(p)) => Some((c - p).max(0.0)),
                                (Some(c), None) => Some(*c),
                                _ => None,
                            },
                            "reply": result_text.as_deref().map(|x| snippet(x, 160)),
                            "tokens": raw.get("usage").map(|u| {
                                let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                                n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens")
                            }),
                        }),
                    );
                    let mut s = sess.summary.lock().unwrap();
                    s.turns += 1;
                    s.idle_since = Some(crate::store::now_epoch());
                    if total_cost_usd.is_some() {
                        s.cost_usd = *total_cost_usd;
                    }
                    if let Some(txt) = result_text.as_deref().filter(|x| !x.trim().is_empty()) {
                        s.last_reply = Some(snippet(txt, 200));
                    }
                    // Context estimate: tokens in the last request.
                    if let Some(u) = raw.get("usage") {
                        let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                        let total = n("input_tokens")
                            + n("cache_read_input_tokens")
                            + n("cache_creation_input_tokens");
                        if total > 0 {
                            s.context_tokens = Some(total);
                        }
                    }
                    if let Some(mu) = raw.get("modelUsage").and_then(|m| m.as_object()) {
                        if let Some(w) = mu
                            .values()
                            .filter_map(|v| v.get("contextWindow").and_then(|c| c.as_u64()))
                            .max()
                        {
                            s.context_window = Some(w);
                        }
                    }
                }
                // Notices (NOTIFICATIONS.md): the turn ended; anything that
                // was running at the last boundary and is gone now settled.
                {
                    let link = format!("/session/{}", sess.name);
                    let first = result_text
                        .as_deref()
                        .map(|t| t.trim())
                        .filter(|t| !t.is_empty())
                        .map(|t| snippet(t, 200));
                    crate::notify::raise(
                        &inner,
                        &sess.name,
                        "turn_ended",
                        &format!("@{} finished a turn", sess.name),
                        first.as_deref(),
                        Some(&link),
                    );
                    let sid = inner
                        .store
                        .agents()
                        .ok()
                        .and_then(|rows| rows.into_iter().find(|a| a.name == sess.name))
                        .and_then(|a| a.session_id);
                    // Deriving the ledger reads the whole transcript: off
                    // the runtime workers, and cached for the fleet views.
                    let (now_running, counts) = {
                        let inner2 = inner.clone();
                        let harness = sess.harness;
                        let repo = sess.repo.clone();
                        let sid2 = sid.clone();
                        let since = Some(sess.spawned_at);
                        tokio::task::spawn_blocking(move || {
                            let ids = crate::notify::running_ids(
                                &inner2,
                                harness,
                                &repo,
                                sid2.as_deref(),
                                since,
                            );
                            let counts =
                                activity_counts(&inner2, harness, &repo, sid2.as_deref(), since);
                            (ids, counts)
                        })
                        .await
                        .unwrap_or_default()
                    };
                    *sess.activity_counts.lock().unwrap() = counts;
                    let mut prev = sess.running_acts.lock().unwrap();
                    for (key, label) in prev.iter() {
                        if !now_running.contains_key(key) {
                            let kind = key.split(':').next().unwrap_or("task");
                            crate::notify::raise(
                                &inner,
                                &sess.name,
                                "activity_settled",
                                &format!("{kind} settled for @{}", sess.name),
                                Some(label),
                                Some(&link),
                            );
                        }
                    }
                    *prev = now_running;
                }
                // The MCP picture at the boundary (PROPOSALS-MCP.md §6.2).
                schedule_mcp_refresh(&inner, &sess, 300);
                // A boundary is a delivery opportunity for anything that
                // arrived for us while nothing could be written.
                inner.tick_delivery(&sess.name);
            }
            SessionEvent::TextDelta { .. } => {
                sess.mark_busy();
            }
            SessionEvent::AssistantMessage { raw, .. } => {
                // The runtime names the model on every message; "default"
                // in the select resolves to this.
                if let Some(m) = raw
                    .pointer("/message/model")
                    .and_then(|m| m.as_str())
                    .filter(|m| !m.is_empty())
                {
                    sess.summary.lock().unwrap().model = Some(m.to_owned());
                }
            }
            SessionEvent::ToolUse {
                tool_name,
                input,
                tool_kind,
                ..
            } => {
                // A subagent/task/workflow started mid-turn: refresh the
                // cached activity counts (throttled; off the workers) so
                // the fleet chips move before the turn ends.
                if *tool_kind == aspen_core::ToolKind::Agent {
                    let now = crate::store::now_epoch();
                    let due = {
                        let mut at = sess.activity_refreshed_at.lock().unwrap();
                        if now - *at > 5.0 {
                            *at = now;
                            true
                        } else {
                            false
                        }
                    };
                    if due {
                        let inner2 = inner.clone();
                        let sess2 = sess.clone();
                        tokio::spawn(async move {
                            let sid = inner2
                                .store
                                .agents()
                                .ok()
                                .and_then(|rows| rows.into_iter().find(|a| a.name == sess2.name))
                                .and_then(|a| a.session_id);
                            let (harness, repo, since) =
                                (sess2.harness, sess2.repo.clone(), Some(sess2.spawned_at));
                            let counts = tokio::task::spawn_blocking(move || {
                                activity_counts(&inner2, harness, &repo, sid.as_deref(), since)
                            })
                            .await;
                            if let Ok(c) = counts {
                                *sess2.activity_counts.lock().unwrap() = c;
                            }
                        });
                    }
                }
                {
                    let path = ["file_path", "notebook_path", "path"]
                        .iter()
                        .find_map(|k| input.get(k).and_then(|v| v.as_str()))
                        .map(str::to_owned);
                    let cmd = input
                        .get("command")
                        .and_then(|v| v.as_str())
                        .map(|c| snippet(c, 80));
                    let _ = inner.store.record_event(
                        &sess.name,
                        "tool",
                        serde_json::json!({ "name": tool_name, "path": path, "command": cmd }),
                    );
                    let mut s = sess.summary.lock().unwrap();
                    s.tool_calls += 1;
                    for k in ["file_path", "notebook_path", "path"] {
                        if let Some(p) = input.get(k).and_then(|v| v.as_str()) {
                            if matches!(
                                tool_name.as_str(),
                                "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
                            ) {
                                s.files_touched.insert(p.to_owned());
                            }
                        }
                    }
                }
                sess.mark_busy();
                *sess.last_tool.lock().unwrap() = Some(tool_name.clone());
            }
            SessionEvent::PermissionAsked {
                tool_name, input, ..
            } => {
                let _ = inner.store.record_event(
                    &sess.name,
                    "prompt",
                    serde_json::json!({ "tool": tool_name }),
                );
                let link = format!("/session/{}", sess.name);
                if tool_name == "AskUserQuestion" {
                    let q = input
                        .get("questions")
                        .and_then(|q| q.as_array())
                        .and_then(|a| a.first())
                        .and_then(|q| q.get("question"))
                        .and_then(|q| q.as_str())
                        .map(|q| snippet(q, 200));
                    crate::notify::raise(
                        &inner,
                        &sess.name,
                        "question",
                        &format!("@{} has a question", sess.name),
                        q.as_deref(),
                        Some(&link),
                    );
                } else {
                    let what = input
                        .get("command")
                        .or_else(|| input.get("file_path"))
                        .or_else(|| input.get("path"))
                        .and_then(|v| v.as_str())
                        .map(|v| snippet(v, 160));
                    crate::notify::raise(
                        &inner,
                        &sess.name,
                        "permission",
                        &format!("@{} needs permission: {tool_name}", sess.name),
                        what.as_deref(),
                        Some(&link),
                    );
                }
            }
            SessionEvent::UserReplay { uuid } => {
                let _ = inner.store.mark_ingested(uuid);
            }
            SessionEvent::McpChanged { servers } => {
                // The adapter's own picture (Claude's init): keep it; the
                // full one follows from the refresh below.
                let mut cur = sess.mcp.lock().unwrap();
                if cur.is_empty() {
                    *cur = servers.clone();
                }
            }
            SessionEvent::Status { raw }
                if raw.get("type").and_then(|t| t.as_str()) == Some("mcp_startup") =>
            {
                schedule_mcp_refresh(&inner, &sess, 800);
            }
            SessionEvent::RuntimeInit { raw, .. } => {
                *sess.inventory.lock().unwrap() = Some(raw.clone());
                schedule_mcp_refresh(&inner, &sess, 1500);
                // The runtime's announced id is the head. On a fork it is
                // new — move the agent to it and record where it came from.
                if let Some(announced) = raw.get("session_id").and_then(|s| s.as_str()) {
                    let current = inner
                        .store
                        .agents()
                        .ok()
                        .and_then(|rows| rows.into_iter().find(|a| a.name == sess.name))
                        .and_then(|a| a.session_id);
                    if current.as_deref() != Some(announced) {
                        let _ = inner.store.set_agent_session(&sess.name, announced);
                        if let Some((parent, at)) = &sess.fork_from {
                            let _ = inner.store.record_lineage(
                                &sess.name,
                                announced,
                                parent,
                                at.as_deref(),
                            );
                            tracing::info!(agent = %sess.name, parent = %parent, child = %announced, "branched");
                        }
                    }
                }
            }
            SessionEvent::Exited { code } => {
                inner.sessions.lock().unwrap().remove(&sess.name);
                let _ = inner.store.set_agent_exit(&sess.name, *code);
                let _ = inner.store.record_event(
                    &sess.name,
                    "exit",
                    serde_json::json!({
                        "code": code,
                        "daemon_shutdown": inner.shutting_down.load(std::sync::atomic::Ordering::SeqCst),
                    }),
                );
                if !inner
                    .shutting_down
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    // Died on its own (or operator stop): not a revive
                    // candidate. During daemon shutdown the mark stays.
                    let _ = inner.store.set_agent_live(&sess.name, false);
                    if *code != Some(0) {
                        crate::notify::raise(
                            &inner,
                            &sess.name,
                            "exited",
                            &format!("@{} exited", sess.name),
                            Some(&match code {
                                Some(c) => format!("exit code {c}"),
                                None => "killed by a signal".to_owned(),
                            }),
                            Some(&format!("/session/{}", sess.name)),
                        );
                    }
                }
                let _ = sess.events.send(ev);
                crate::federation::broadcast_roster(&inner);
                break;
            }
            _ => {}
        }
        let _ = sess.events.send(ev); // no receivers is fine
    }
}
