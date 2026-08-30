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
}

impl ManagedSession {
    pub fn turn_state(&self) -> TurnState {
        *self.turn_state.lock().unwrap()
    }
}

pub struct NodeInner {
    pub store: BusStore,
    pub sessions: Mutex<HashMap<String, Arc<ManagedSession>>>,
    pub delivery_tx: mpsc::UnboundedSender<String>,
}

impl NodeInner {
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
}

/// The auto-channel name for a repo: its directory name. (Two repos sharing
/// a basename collide; disambiguation is a later, deliberate feature.)
pub fn repo_channel(repo: &Path) -> String {
    repo.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into())
}

impl Node {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let store = BusStore::open(&data_dir.join("bus.db"))?;
        Ok(Self::with_store(store))
    }

    pub fn with_store(store: BusStore) -> Self {
        let (delivery_tx, delivery_rx) = mpsc::unbounded_channel::<String>();
        let inner = Arc::new(NodeInner {
            store,
            sessions: Mutex::new(HashMap::new()),
            delivery_tx,
        });
        tokio::spawn(delivery::run(inner.clone(), delivery_rx));
        Self { inner }
    }

    /// Spawn a named agent in a repo and join it to the bus.
    pub async fn spawn_agent(
        &self,
        name: &str,
        repo: PathBuf,
        opts: SpawnOpts,
    ) -> Result<Arc<ManagedSession>> {
        if self.inner.live(name).is_some() {
            return Err(anyhow!("an agent named @{name} is already running"));
        }
        let repo = repo
            .canonicalize()
            .map_err(|e| anyhow!("repo {}: {e}", repo.display()))?;
        let channel = repo_channel(&repo);

        let mut cfg = ClaudeConfig::new(repo.clone());
        cfg.model = opts.model.clone();
        cfg.resume = opts.resume.clone();
        cfg.permission_mode = opts.permission_mode.clone();
        cfg.policy = if opts.allow_all {
            PermissionPolicy::AllowAll
        } else {
            PermissionPolicy::ReadOnlyAuto
        };
        cfg.charter = Some(charter_text(name, &channel, opts.charter.as_deref()));

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

        self.inner.store.register_agent(
            name,
            &repo,
            &channel,
            &cfg.session_id.to_string(),
            opts.charter.as_deref(),
        )?;

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
        *sess.turn_state.lock().unwrap() = TurnState::Busy;
        sess.handle.send_user(text).await
    }

    pub async fn interrupt(&self, name: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.interrupt().await
    }

    pub async fn shutdown_agent(&self, name: &str) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        sess.handle.shutdown().await
    }

    pub fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<SessionEvent>> {
        self.inner.live(name).map(|s| s.events.subscribe())
    }

    /// Console answer to a pending permission prompt.
    pub fn answer_permission(
        &self,
        name: &str,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<serde_json::Value>,
    ) -> Result<()> {
        let sess = self
            .inner
            .live(name)
            .ok_or_else(|| anyhow!("no running agent named @{name}"))?;
        let broker = sess
            .broker
            .as_ref()
            .ok_or_else(|| anyhow!("@{name} was not spawned interactively"))?;
        if broker.answer(request_id, allow, message, updated_input) {
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
fn charter_text(name: &str, channel: &str, user_charter: Option<&str>) -> String {
    let mut t = format!(
        "You are @{name}, an agent session on the aspen mesh, in repo channel #{channel}. \
         Peers message you via the bus; those messages arrive prefixed with an [aspen bus] \
         header naming the sender. Reply to peers with the bus_send tool — never by writing \
         files at them. The human operator is @operator."
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
                // A boundary is a delivery opportunity for anything that
                // arrived for us while nothing could be written.
                inner.tick_delivery(&sess.name);
            }
            SessionEvent::TextDelta { .. } | SessionEvent::ToolUse { .. } => {
                *sess.turn_state.lock().unwrap() = TurnState::Busy;
            }
            SessionEvent::UserReplay { uuid } => {
                let _ = inner.store.mark_ingested(uuid);
            }
            SessionEvent::Exited { .. } => {
                inner.sessions.lock().unwrap().remove(&sess.name);
                let _ = sess.events.send(ev);
                break;
            }
            _ => {}
        }
        let _ = sess.events.send(ev); // no receivers is fine
    }
}
