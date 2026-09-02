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
}

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
}

/// The one way a repo path enters or is looked up in the store. Resolves
/// symlinks/relative parts like canonicalize, but on Windows yields the
/// plain `C:\…` form rather than the `\\?\C:\…` verbatim form canonicalize
/// returns — discovery stores what Claude Code wrote (`C:\…`), and a lookup
/// in the other form matched zero rows (the "skip does nothing" bug).
/// A path that doesn't exist comes back as given.
pub fn normalize_repo(p: &Path) -> PathBuf {
    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
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
            _ => return Ok("no certified mesh membership on disk".into()),
        };
        config.peers = files.verified_peers()?;
        let summary;
        if let Some(mesh) = self.inner.mesh() {
            let mut cur = mesh.config.write().unwrap();
            let before = cur.peers.len();
            *cur = config;
            summary = format!(
                "mesh '{}' config reloaded: {} peer(s) (was {}), relay {}",
                cur.mesh,
                cur.peers.len(),
                before,
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
        cfg.charter = Some(charter_text(
            name,
            &channel,
            &node_name,
            opts.charter.as_deref(),
        ));
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
        // Remember the repo (and, when the operator asked to skip here,
        // adopt that as the repo's default going forward).
        let _ = self.inner.store.add_repo(&repo, opts.skip_permissions);

        let (events_tx, _) = broadcast::channel(4096);
        if let Some(b) = &op_broker {
            b.attach_events(events_tx.clone());
        }
        let managed = Arc::new(ManagedSession {
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
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        delivery::flush_notices(&self.inner, &sess).await;
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
    pub async fn revive_agent(&self, name: &str, interactive: bool) -> Result<Arc<ManagedSession>> {
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
            interactive,
            extra_args: row.extra_args.clone(),
            ..Default::default()
        };
        self.spawn_agent(name, row.repo.clone(), opts).await
    }

    /// Branch here: leave a bookmark on the current head and move the agent
    /// to a fresh fork of it (optionally from an earlier message). The
    /// process is restarted on the fork; live marks are kept so the agent
    /// revives on the new head from now on.
    pub async fn branch_agent(
        &self,
        name: &str,
        label: Option<&str>,
        at_message: Option<&str>,
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
    pub async fn resume_bookmark(&self, name: &str, id: i64) -> Result<Arc<ManagedSession>> {
        let bm = self
            .inner
            .store
            .bookmark(name, id)?
            .ok_or_else(|| anyhow!("no bookmark {id} for {name}"))?;
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
            SessionEvent::TurnEnded { .. } => {
                *sess.turn_state.lock().unwrap() = TurnState::Idle;
                *sess.busy_since.lock().unwrap() = None;
                *sess.last_tool.lock().unwrap() = None;
                // A boundary is a delivery opportunity for anything that
                // arrived for us while nothing could be written.
                inner.tick_delivery(&sess.name);
            }
            SessionEvent::TextDelta { .. } => {
                sess.mark_busy();
            }
            SessionEvent::ToolUse { tool_name, .. } => {
                sess.mark_busy();
                *sess.last_tool.lock().unwrap() = Some(tool_name.clone());
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
            SessionEvent::Exited { .. } => {
                inner.sessions.lock().unwrap().remove(&sess.name);
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
