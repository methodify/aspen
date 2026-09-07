//! The session manager: named agents, exact turn state, event fan-out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, mpsc};

use aspen_claude::{ClaudeConfig, ClaudeSession, PermissionPolicy};
use aspen_core::{SessionEvent, SessionHandle};

use crate::delivery;
use crate::store::BusStore;

/// Exact turn state — derived from the wire, not inferred from registries or
/// transcript mtimes. `result` is the only idle signal (reference §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Idle,
    Busy,
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
    /// Files this session has edited/written (tool inputs with a path).
    pub files_touched: std::collections::BTreeSet<String>,
    pub tool_calls: u32,
}

pub struct ManagedSession {
    pub name: String,
    pub repo: PathBuf,
    pub channel: String,
    pub handle: Arc<ClaudeSession>,
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
}

impl NodeInner {
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
/// Running-activity counts for a live session (activity.rs), from the
/// transcript on disk; cached by the file's size and mtime.
pub fn activity_counts(
    repo: &Path,
    session_id: Option<&str>,
    process_started: Option<f64>,
) -> aspen_claude::activity::ActivityCounts {
    match session_id {
        Some(sid) => aspen_claude::activity::counts(&aspen_claude::activity::activities_for(
            repo,
            sid,
            process_started,
        )),
        None => aspen_claude::activity::ActivityCounts::default(),
    }
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
                Some(Arc::new(crate::federation::MeshState::new(
                    identity, config,
                )))
            }
            _ => None,
        };
        let node = Self::build(store, mesh, Some(data_dir.to_owned()));
        if node.inner.mesh().is_some() {
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
            let state = Arc::new(crate::federation::MeshState::new(identity, config));
            *self.inner.mesh.write().unwrap() = Some(state);
            summary = format!("joined mesh '{name}' live: {peers} peer(s)");
        }
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
        let inner = Arc::new(NodeInner {
            store,
            sessions: Mutex::new(HashMap::new()),
            delivery_tx,
            mesh: std::sync::RwLock::new(mesh),
            data_dir,
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            servicing: crate::servicing::Servicing::new(
                crate::federation::VERSION
                    .get()
                    .map(|(v, _)| v.clone())
                    .unwrap_or_else(|| "0.0.0".into()),
                std::env::current_exe().ok(),
            ),
        });
        tokio::spawn(delivery::run(inner.clone(), delivery_rx));
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
            let mtime = std::fs::metadata(aspen_claude::transcript::transcript_path(&repo, sid))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
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
                            "resumed in place at your request while session {} was being written elsewhere — two processes share this transcript",
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

        let mut cfg = ClaudeConfig::new(repo.clone());
        cfg.model = opts.model.clone();
        cfg.resume = opts.resume.clone();
        cfg.fork = opts.fork;
        cfg.resume_at = opts.resume_at.clone();
        // bypassPermissions makes the CLI skip can_use_tool entirely; an
        // explicit permission_mode still overrides it if given.
        cfg.permission_mode = opts
            .permission_mode
            .clone()
            .or_else(|| skip.then(|| "bypassPermissions".to_string()));
        cfg.policy = if opts.allow_all {
            PermissionPolicy::AllowAll
        } else {
            PermissionPolicy::ReadOnlyAuto
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
        cfg.charter = Some(charter);
        // Harness defaults (settings.json, read live) + this session's args.
        let defaults = self
            .inner
            .data_dir
            .as_deref()
            .map(crate::settings::load)
            .unwrap_or_default()
            .harness
            .get("claude")
            .map(|h| h.args.clone())
            .unwrap_or_default();
        cfg.extra_args = crate::settings::split_args(&defaults, opts.extra_args.as_deref())?;
        // Plugins by scope (plugins.rs): every enclosing rule's plugin, at
        // its cached version, as --plugin-dir.
        let (active_plugins, missing_plugins) = match (
            self.inner.data_dir.as_deref(),
            self.inner.store.plugin_rules(false),
        ) {
            (Some(dd), Ok(rules)) if !rules.is_empty() => {
                let node = self
                    .inner
                    .mesh()
                    .map(|m| m.identity.node.clone())
                    .unwrap_or_else(|| "local".into());
                crate::plugins::resolve(dd, &rules, &node, &repo, name)
            }
            _ => (Vec::new(), Vec::new()),
        };
        for a in &active_plugins {
            cfg.extra_args.push("--plugin-dir".into());
            cfg.extra_args.push(a.path.clone());
        }
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

        let mcp = crate::tools::build_mcp(self.inner.clone(), name.to_owned());
        let op_broker = opts
            .interactive
            .then(|| Arc::new(crate::permit::OperatorBroker::new(cfg.policy)));
        let (handle, adapter_rx) = match &op_broker {
            Some(b) => {
                let b: Arc<dyn aspen_claude::broker::PermissionBroker> = b.clone();
                ClaudeSession::spawn_with_broker(cfg.clone(), mcp, b).await?
            }
            None => ClaudeSession::spawn(cfg.clone(), mcp).await?,
        };

        // On resume the runtime keeps the resumed session's id — register
        // that, not the fresh uuid the config generated and never used.
        let effective_session_id = opts
            .resume
            .clone()
            .unwrap_or_else(|| cfg.session_id.to_string());
        self.inner.store.register_agent(
            name,
            &repo,
            &channel,
            &effective_session_id,
            opts.charter.as_deref(),
            opts.extra_args.as_deref(),
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

        tokio::spawn(pump(self.inner.clone(), managed.clone(), adapter_rx));

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
            .filter(|sid| aspen_claude::transcript::transcript_path(&row.repo, sid).is_file());
        let opts = SpawnOpts {
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
        if !aspen_claude::transcript::transcript_path(&row.repo, &head).is_file() {
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
        for found in aspen_claude::transcript::discover_repos() {
            let path = normalize_repo(&found.path);
            let added = !known.contains(&path);
            if added {
                self.inner.store.add_repo(&path, None)?;
            }
            out.push((path, found.sessions, added));
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
            .map(|sid| crate::artifacts::touched_paths(&row.repo, sid))
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
        Ok(serde_json::json!({
            "handshake": sess.handle.handshake.get(),
            "inventory": sess.inventory.lock().unwrap().clone(),
        }))
    }

    /// Rich context breakdown from the runtime (poll at turn end).
    pub async fn context_usage(&self, name: &str) -> Result<serde_json::Value> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.get_context_usage().await
    }

    /// Switch a session's model (takes effect next turn).
    pub async fn set_model(&self, name: &str, model: Option<&str>) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.set_model(model).await.map(|_| ())
    }

    /// Live-switch a session's permission mode.
    pub async fn set_permission_mode(&self, name: &str, mode: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.set_permission_mode(mode).await.map(|_| ())
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
        sess.handle.reload_plugins().await
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
            if s.handle.reload_plugins().await.is_ok() {
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
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let broker = sess
            .broker
            .as_ref()
            .ok_or_else(|| anyhow!("@{name} was not spawned interactively"))?;
        if broker.answer(
            request_id,
            allow,
            message,
            updated_input,
            updated_permissions,
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
                // A boundary is a delivery opportunity for anything that
                // arrived for us while nothing could be written.
                inner.tick_delivery(&sess.name);
            }
            SessionEvent::TextDelta { .. } => {
                sess.mark_busy();
            }
            SessionEvent::ToolUse {
                tool_name, input, ..
            } => {
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
            SessionEvent::PermissionAsked { tool_name, .. } => {
                let _ = inner.store.record_event(
                    &sess.name,
                    "prompt",
                    serde_json::json!({ "tool": tool_name }),
                );
            }
            SessionEvent::UserReplay { uuid } => {
                let _ = inner.store.mark_ingested(uuid);
            }
            SessionEvent::RuntimeInit { raw, .. } => {
                *sess.inventory.lock().unwrap() = Some(raw.clone());
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
