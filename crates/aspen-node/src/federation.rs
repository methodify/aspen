//! Node↔node federation: authenticated links carrying sealed envelopes.
//!
//! Link lifecycle: plaintext `hello` (cert + fresh nonce) both ways → each
//! side proves key possession by returning the peer's nonce inside a sealed
//! envelope → link up → roster exchange → bus traffic. Everything after
//! hello is a `SealedEnvelope`: signed by the sender, encrypted to the
//! recipient, so the transport underneath (tailnet, LAN, later a relay) is
//! never trusted.
//!
//! Cross-node bus delivery is store-and-forward, at-least-once: rows stay
//! pending on the origin until the home node confirms insertion into ITS
//! store (`bus_ack`); duplicate forwards are absorbed by a uuid unique
//! index. The trail on the origin shows `federated:<node>`; the home node's
//! trail shows the local delivery lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use aspen_wire::identity::{NodeCert, NodeIdentity};
use aspen_wire::SealedEnvelope;

use crate::mesh::MeshConfig;
use crate::node::{NodeInner, TurnState};
use crate::store::StoredMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAgent {
    pub name: String,
    pub channel: String,
    pub live: bool,
    pub turn_state: Option<String>,
    /// Work summary + title, so a peer's fleet view is as rich as ours.
    #[serde(default)]
    pub summary: Option<Value>,
    #[serde(default)]
    pub title: Option<String>,
    /// Running-activity counts (ACTIVITY.md), live sessions only.
    #[serde(default)]
    pub activities: Option<Value>,
    /// Which runtime the session runs on (HARNESSES.md).
    #[serde(default)]
    pub harness: aspen_core::Harness,
}

pub struct MeshState {
    pub identity: NodeIdentity,
    /// Interior-mutable so `aspen mesh peers-add` / `relay` take effect in
    /// the running daemon without a restart (see Node::reload_mesh).
    pub config: std::sync::RwLock<MeshConfig>,
    /// Additional meshes (MESHES.md): each with its own root, peers,
    /// relays and policy. Node names are unique across all of them.
    pub extra: std::sync::RwLock<Vec<MeshConfig>>,
    /// node → the mesh its live link was made in.
    pub link_mesh: Mutex<HashMap<String, String>>,
    /// Peers with a dialer task already running (idempotent ensure_dialers).
    pub dialing: Mutex<std::collections::HashSet<String>>,
    /// Relay URLs with a client task running (idempotent ensure_dialers).
    pub relay_running: Mutex<std::collections::HashSet<String>>,
    /// Connected relays: url → since (epoch), and the socket writer — for
    /// the mailbox (Store frames go to whichever relay is up).
    pub relay_up: Mutex<HashMap<String, (f64, mpsc::UnboundedSender<String>)>>,
    /// Bus rows handed to a relay mailbox: uuid → when. Re-handed after a
    /// while if still pending (the mailbox may have been full or lost).
    pub mailed: Mutex<HashMap<String, f64>>,
    /// The roster ticker has been started.
    pub ticker_started: std::sync::atomic::AtomicBool,
    /// The mailbox re-hand ticker has been started.
    pub mail_ticker_started: std::sync::atomic::AtomicBool,
    /// Per-relay last error, for the console.
    pub relay_errors: Mutex<HashMap<String, (String, f64)>>,
    /// Live relay sessions: url → how to start/stop per-peer links over it
    /// and who the relay says is present. Lets a direct link supersede a
    /// relay link, and a lost direct link fall back to a relay.
    pub relay_sessions: Mutex<HashMap<String, RelaySession>>,
    /// Relay URLs learned from peers' advertisements (not configured; not
    /// persisted; the console shows them as discovered).
    pub discovered_relays: Mutex<HashMap<String, String>>,
    /// node → how its live link was made: "direct" or "relay:<url>".
    pub link_kind: Mutex<HashMap<String, String>>,
    /// node name → sender of already-serialized wire frames.
    pub links: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    /// node name → last roster it sent us.
    pub remote: Mutex<HashMap<String, Vec<RemoteAgent>>>,
    /// Outstanding api_req calls we made, keyed by request id.
    pub pending_api: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Value>>>,
    /// Event subscriptions WE serve to peers: sub id → forwarder task.
    pub served_subs: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    /// Event subscriptions we REQUESTED: sub id → (serving peer, consumer).
    pub remote_subs: Mutex<HashMap<String, (String, mpsc::UnboundedSender<Value>)>>,
    /// Epoch seconds when the (first) relay session came up; None when
    /// none is up. Kept for the status readout; relay_up has the detail.
    pub relay_connected_at: Mutex<Option<f64>>,
    /// Per-peer diagnostics for the console: why isn't X linked?
    pub health: Mutex<HashMap<String, PeerHealth>>,
    /// What we have learned about every URL we dial (peers' candidates,
    /// relays): consecutive failures, when we may try it again, when it
    /// last worked. Backoff lives here, so a dead candidate is tried less
    /// and less (to a cap) and a proven one is tried first.
    pub reach: Mutex<HashMap<String, UrlReach>>,
}

/// What an op does to this node (MESHES.md §capabilities).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Observe,
    Control,
    Spawn,
    Trust,
}

/// Classify a mesh op. Anything not listed as a read is control.
pub fn op_capability(op: &str) -> Capability {
    match op {
        "transcript" | "activities" | "subagent" | "artifacts" | "file_stat" | "file_read"
        | "runtime" | "context" | "bookmarks" | "plugins_effective" | "boards"
        | "plugin_registry" | "plugins_registry_view" | "templates" | "needs" | "node_repos"
        | "node_sessions" | "history" | "node_update_status" | "node_logs" | "adoptions"
        | "usage" | "fleet_activities" | "notices" | "memory_files" | "memory_conflicts"
        | "replica_offsets" | "session_spec" | "session_preflight" | "node_preflight_target"
        | "sub" | "http" => Capability::Observe,
        "spawn" | "template_spawn" => Capability::Spawn,
        "adoption" | "node_repo_skip" => Capability::Trust,
        _ => Capability::Control,
    }
}

/// Reachability memory for one dial URL.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UrlReach {
    pub fails: u32,
    /// Epoch seconds before which this URL is not tried.
    pub until: f64,
    pub last_ok: Option<f64>,
    pub last_error: Option<String>,
}

/// Connect timeout for any dial (peer or relay). A black-holed address
/// otherwise holds the dialer for the OS's own timeout (75s on macOS), and
/// every other candidate for that peer waits behind it.
pub const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Backoff caps: a URL the operator configured is retried at least this
/// often; one we merely learned (advertised, discovered) may sleep longer.
pub const BACKOFF_CAP_CONFIGURED: f64 = 60.0;
pub const BACKOFF_CAP_LEARNED: f64 = 600.0;

/// One live relay session, as other parts of the node see it.
pub struct RelaySession {
    pub tx: mpsc::UnboundedSender<String>,
    pub peer_ins: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
    pub present: std::collections::HashSet<String>,
    /// The node hosting this relay, when it said (embedded relays do).
    pub host: Option<String>,
}

/// What a node tells peers about how to reach it (rides the roster).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Advertised {
    /// Federation dial URLs peers may try directly.
    #[serde(default)]
    pub dial_urls: Vec<String>,
    /// "wsl-nat": every address here is WSL's NAT-internal one and no
    /// `aspen config advertise` URL is set — other machines cannot dial
    /// this node without a forwarded port or a relay (RELAY.md §9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Relay endpoints this node hosts that peers may rendezvous at.
    #[serde(default)]
    pub relay_urls: Vec<String>,
}

/// What we know about a peer's link, for humans.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PeerHealth {
    pub last_error: Option<String>,
    pub last_error_at: Option<f64>,
    pub last_up: Option<f64>,
    pub last_down: Option<f64>,
    pub last_roster: Option<f64>,
    /// The peer's daemon version/sha, from its roster.
    pub version: Option<String>,
    pub sha: Option<String>,
    /// Fingerprint of the cert the peer presented (short hash of its key).
    pub fingerprint: Option<String>,
    /// Servicing, from the peer's roster: a newer release it knows of, its
    /// state (ready/draining/updating) with detail, policy mode, inventory
    /// (os/arch/claude version/started_at), and its last update outcome.
    pub update_available: Option<String>,
    pub service_state: Option<String>,
    pub service_detail: Option<String>,
    pub policy: Option<String>,
    pub inventory: Option<Value>,
    pub last_outcome: Option<Value>,
    /// The peer holds the mesh's root key (certify happens there).
    pub has_root: Option<bool>,
    /// Where the peer says it can be reached (from its roster).
    pub advertised: Option<Advertised>,
}

/// Federation frame protocol. Bump only when a frame format changes
/// incompatibly; a peer on a different number is refused at hello with a
/// health error that says so (docs/SERVICING.md §9).
pub const PROTOCOL: u32 = 1;

/// The daemon's own version stamp, set by the binary at startup (the node
/// crate can't see the bin crate's version).
pub static VERSION: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

/// Short, human fingerprint of a public key: first 8 hex of its sha256.
pub fn fingerprint(key: &[u8]) -> String {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(key);
    h.iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

impl MeshState {
    pub fn note_relay(&self, url: &str, err: Option<String>) {
        let mut e = self.relay_errors.lock().unwrap();
        match err {
            Some(msg) => {
                e.insert(url.to_owned(), (msg, crate::store::now_epoch()));
            }
            None => {
                e.remove(url);
            }
        }
    }

    pub fn note(&self, peer: &str, f: impl FnOnce(&mut PeerHealth)) {
        let mut h = self.health.lock().unwrap();
        f(h.entry(peer.to_owned()).or_default());
    }

    /// A dial to `url` succeeded: clear its backoff, remember it as proven.
    pub fn url_ok(&self, url: &str) {
        let mut r = self.reach.lock().unwrap();
        let e = r.entry(url.to_owned()).or_default();
        e.fails = 0;
        e.until = 0.0;
        e.last_ok = Some(crate::store::now_epoch());
        e.last_error = None;
    }

    /// A dial to `url` failed: back off exponentially (5s doubling) up to
    /// `cap` seconds. Returns the consecutive-failure count, so the caller
    /// can log the first loudly and the rest quietly.
    pub fn url_failed(&self, url: &str, err: &str, cap: f64) -> u32 {
        let mut r = self.reach.lock().unwrap();
        let e = r.entry(url.to_owned()).or_default();
        e.fails = e.fails.saturating_add(1);
        let wait = (5.0 * 2f64.powi(e.fails.saturating_sub(1).min(20) as i32)).min(cap);
        e.until = crate::store::now_epoch() + wait;
        e.last_error = Some(err.to_owned());
        e.fails
    }

    /// The candidates we may try right now, best first: the one that last
    /// worked, then the never-failed, then the rest by fewest failures.
    /// A URL inside its backoff window is left out.
    pub fn order_urls(&self, cands: &[String]) -> Vec<String> {
        let now = crate::store::now_epoch();
        let r = self.reach.lock().unwrap();
        let mut v: Vec<(String, f64, u32)> = cands
            .iter()
            .filter_map(|u| {
                let e = r.get(u).cloned().unwrap_or_default();
                (e.until <= now).then(|| (u.clone(), e.last_ok.unwrap_or(0.0), e.fails))
            })
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.2.cmp(&b.2)));
        v.into_iter().map(|t| t.0).collect()
    }

    /// Any of `cands` never tried at all? Those are worth a probe at once,
    /// even while a relay link carries the peer.
    pub fn has_untried(&self, cands: &[String]) -> bool {
        let r = self.reach.lock().unwrap();
        cands.iter().any(|u| !r.contains_key(u))
    }

    /// Seconds until the soonest of `cands` leaves its backoff window
    /// (0 when one is available now; 5 when none are known).
    pub fn next_try_in(&self, cands: &[String]) -> f64 {
        let now = crate::store::now_epoch();
        let r = self.reach.lock().unwrap();
        cands
            .iter()
            .map(|u| r.get(u).map(|e| (e.until - now).max(0.0)).unwrap_or(0.0))
            .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))))
            .unwrap_or(5.0)
    }

    /// Reach memory for a set of URLs, for the console.
    pub fn reach_of(&self, cands: &[String]) -> Vec<Value> {
        let now = crate::store::now_epoch();
        let r = self.reach.lock().unwrap();
        cands
            .iter()
            .map(|u| {
                let e = r.get(u).cloned().unwrap_or_default();
                json!({
                    "url": u,
                    "fails": e.fails,
                    "retry_in_secs": (e.until - now).max(0.0).round(),
                    "last_ok": e.last_ok,
                    "last_error": e.last_error,
                })
            })
            .collect()
    }
}

impl MeshState {
    pub fn new(identity: NodeIdentity, config: MeshConfig) -> Self {
        Self {
            identity,
            config: std::sync::RwLock::new(config),
            extra: std::sync::RwLock::new(Vec::new()),
            link_mesh: Mutex::new(HashMap::new()),
            dialing: Mutex::new(std::collections::HashSet::new()),
            relay_running: Mutex::new(std::collections::HashSet::new()),
            relay_up: Mutex::new(HashMap::new()),
            mailed: Mutex::new(HashMap::new()),
            ticker_started: std::sync::atomic::AtomicBool::new(false),
            mail_ticker_started: std::sync::atomic::AtomicBool::new(false),
            relay_errors: Mutex::new(HashMap::new()),
            relay_sessions: Mutex::new(HashMap::new()),
            discovered_relays: Mutex::new(HashMap::new()),
            link_kind: Mutex::new(HashMap::new()),
            links: Mutex::new(HashMap::new()),
            remote: Mutex::new(HashMap::new()),
            pending_api: Mutex::new(HashMap::new()),
            served_subs: Mutex::new(HashMap::new()),
            remote_subs: Mutex::new(HashMap::new()),
            relay_connected_at: Mutex::new(None),
            health: Mutex::new(HashMap::new()),
            reach: Mutex::new(HashMap::new()),
        }
    }

    /// Remote API call over the federation link, correlated by id.
    pub async fn api_call(
        &self,
        node: &str,
        op: &str,
        agent: &str,
        body: Value,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_api.lock().unwrap().insert(id.clone(), tx);
        let sent = self.send_to(
            node,
            &json!({ "t": "api_req", "id": id, "op": op, "agent": agent, "body": body }),
        );
        if let Err(e) = sent {
            self.pending_api.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => {
                if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
                    Ok(v.get("body").cloned().unwrap_or(Value::Null))
                } else {
                    Err(anyhow!(
                        "{}",
                        v.get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("remote error")
                    ))
                }
            }
            Ok(Err(_)) => Err(anyhow!("link to {node:?} dropped mid-call")),
            Err(_) => {
                self.pending_api.lock().unwrap().remove(&id);
                Err(anyhow!("remote call to {node:?} timed out"))
            }
        }
    }
}

impl MeshState {
    /// Every peer across every mesh (names are unique across them).
    pub fn peers(&self) -> Vec<crate::mesh::PeerConfig> {
        let mut v = self.config.read().unwrap().peers.clone();
        for m in self.extra.read().unwrap().iter() {
            v.extend(m.peers.iter().cloned());
        }
        v
    }
    /// Every mesh config, the primary first.
    pub fn configs(&self) -> Vec<MeshConfig> {
        let mut v = vec![self.config.read().unwrap().clone()];
        v.extend(self.extra.read().unwrap().iter().cloned());
        v
    }
    pub fn mesh_names(&self) -> Vec<String> {
        self.configs().into_iter().map(|c| c.mesh).collect()
    }
    pub fn is_multi(&self) -> bool {
        !self.extra.read().unwrap().is_empty()
    }
    /// The mesh a peer is configured in, else the mesh its link was made
    /// in (a peer met through a relay before any config listed it).
    pub fn mesh_of_peer(&self, node: &str) -> Option<String> {
        for c in self.configs() {
            if c.peers.iter().any(|p| p.cert.node == node) {
                return Some(c.mesh);
            }
        }
        self.link_mesh.lock().unwrap().get(node).cloned()
    }
    pub fn root_for(&self, mesh: &str) -> Option<Vec<u8>> {
        self.configs().into_iter().find(|c| c.mesh == mesh).map(|c| c.root_public)
    }
    /// "full" | "observe" for a mesh (MESHES.md §capabilities).
    pub fn policy_of(&self, mesh: &str) -> String {
        let primary = self.config.read().unwrap().mesh.clone();
        self.configs()
            .into_iter()
            .find(|c| c.mesh == mesh)
            .map(|c| c.policy_or(if c.mesh == primary { "full" } else { "observe" }))
            .unwrap_or_else(|| "observe".into())
    }
    /// The mesh a relay URL belongs to: the config listing it, else the
    /// mesh of the peer that advertised it, else the primary.
    pub fn relay_mesh(&self, url: &str) -> String {
        for c in self.configs() {
            if c.relay_urls().iter().any(|u| u == url) {
                return c.mesh;
            }
        }
        if let Some(from) = self.discovered_relays.lock().unwrap().get(url).cloned() {
            if let Some(m) = self.mesh_of_peer(&from) {
                return m;
            }
        }
        self.mesh_name()
    }
    /// What a peer may do here: every capability for a "full" mesh; reads
    /// only for "observe"; a console peer reads and controls, never spawns
    /// or trusts (MESHES.md).
    pub fn allows(&self, peer: &str, cap: Capability) -> bool {
        if peer.starts_with("console-") {
            return matches!(cap, Capability::Observe | Capability::Control);
        }
        let mesh = self.mesh_of_peer(peer).unwrap_or_else(|| self.mesh_name());
        match self.policy_of(&mesh).as_str() {
            "full" => true,
            _ => matches!(cap, Capability::Observe),
        }
    }
    /// First configured relay (status readouts); `relay_urls` has them all.
    pub fn relay_url(&self) -> Option<String> {
        self.relay_urls().into_iter().next()
    }
    pub fn relay_urls(&self) -> Vec<String> {
        let mut v = self.config.read().unwrap().relay_urls();
        for m in self.extra.read().unwrap().iter() {
            for u in m.relay_urls() {
                if !v.contains(&u) {
                    v.push(u);
                }
            }
        }
        v
    }
    pub fn mesh_name(&self) -> String {
        self.config.read().unwrap().mesh.clone()
    }
    pub fn root_public(&self) -> Vec<u8> {
        self.config.read().unwrap().root_public.clone()
    }

    pub fn link_up(&self, node: &str) -> bool {
        self.links.lock().unwrap().contains_key(node)
    }

    /// Every peer with a link up right now.
    pub fn up_peers(&self) -> Vec<String> {
        self.links.lock().unwrap().keys().cloned().collect()
    }

    /// Where a bare agent name is homed remotely, if anywhere.
    pub fn find_remote(&self, agent: &str) -> Option<(String, RemoteAgent)> {
        let remote = self.remote.lock().unwrap();
        for (node, agents) in remote.iter() {
            if let Some(a) = agents.iter().find(|a| a.name == agent) {
                return Some((node.clone(), a.clone()));
            }
        }
        None
    }

    pub fn remote_channel_members(&self, channel: &str) -> Vec<(String, String)> {
        let remote = self.remote.lock().unwrap();
        let mut out = Vec::new();
        for (node, agents) in remote.iter() {
            for a in agents.iter().filter(|a| a.channel == channel) {
                out.push((a.name.clone(), node.clone()));
            }
        }
        out
    }

    fn peer_cert(&self, node: &str) -> Option<NodeCert> {
        // Across every mesh (MESHES.md): names are unique across them.
        self.peers()
            .into_iter()
            .find(|p| p.cert.node == node)
            .map(|p| p.cert)
    }

    /// Seal and queue a payload for a peer. Err if no live link.
    pub fn send_to(&self, node: &str, payload: &Value) -> Result<()> {
        let cert = self
            .peer_cert(node)
            .ok_or_else(|| anyhow!("no cert on file for node {node:?}"))?;
        let env = SealedEnvelope::seal(&self.identity, &cert, payload.to_string().as_bytes())?;
        let frame = serde_json::to_string(&env)?;
        let links = self.links.lock().unwrap();
        let tx = links
            .get(node)
            .ok_or_else(|| anyhow!("no live link to node {node:?}"))?;
        tx.send(frame)
            .map_err(|_| anyhow!("link to {node:?} closed"))
    }
}

// ------------------------------------------------------------------ payloads

fn bus_payload(m: &StoredMessage, dest_node: &str) -> Value {
    // A recipient qualified with the destination node travels bare — on
    // that node it IS the local name.
    let recipient = crate::addr::strip_node(&m.recipient, dest_node);
    json!({
        "t": "bus",
        "uuid": m.uuid, "thread": m.thread, "sender": m.sender,
        "recipient": recipient, "to_display": m.to_display,
        "urgency": m.urgency, "body": m.body, "record_ref": m.record_ref,
        "created_at": m.created_at,
    })
}

/// This node's reachable addresses, for the roster: the listen address
/// when it is beyond loopback (as hostname and as every non-loopback IPv4),
/// plus anything the operator set with `aspen config advertise`. A
/// loopback-only node advertises nothing — it is a spoke by its own choice.
pub fn advertised(inner: &Arc<NodeInner>) -> Advertised {
    let mut out = Advertised::default();
    let Some(dir) = inner.data_dir.as_deref() else {
        return out;
    };
    let listen = std::fs::read_to_string(dir.join("daemon.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["listen"].as_str().map(str::to_owned));
    let port = listen
        .as_deref()
        .and_then(|l| l.parse::<std::net::SocketAddr>().ok())
        .filter(|a| !a.ip().is_loopback())
        .map(|a| a.port());
    if let Some(port) = port {
        let mut hosts: Vec<String> = Vec::new();
        let mut v4s: Vec<std::net::Ipv4Addr> = Vec::new();
        if let Ok(ifs) = if_addrs::get_if_addrs() {
            for i in ifs {
                if i.is_loopback() {
                    continue;
                }
                if let std::net::IpAddr::V4(v4) = i.ip() {
                    if !v4.is_link_local() {
                        v4s.push(v4);
                    }
                }
            }
        }
        // The hostname goes first — but only when it names THIS machine
        // for others: it must resolve to one of our own IPv4 addresses. A
        // WSL node carries the Windows host's name, which resolves to the
        // Windows side (IPv6 at that); advertising it sent every peer to
        // the wrong box, and the relay client chased it forever.
        if let Some(h) = hostname() {
            if hostname_is_ours(&h, port, &v4s) {
                hosts.push(h);
            }
        }
        hosts.extend(v4s.iter().map(|v| v.to_string()));
        for h in hosts {
            let dial = format!("ws://{h}:{port}/api/federation/ws");
            if !out.dial_urls.contains(&dial) {
                out.dial_urls.push(dial);
            }
            out.relay_urls
                .push(format!("ws://{h}:{port}/api/federation/relay"));
        }
    }
    let configured = crate::settings::load(dir).advertise_urls();
    for u in &configured {
        if !out.dial_urls.contains(u) {
            out.dial_urls.push(u.clone());
        }
        let relay = u.replacen("/api/federation/ws", "/api/federation/relay", 1);
        if relay != *u && !out.relay_urls.contains(&relay) {
            out.relay_urls.push(relay);
        }
    }
    if configured.is_empty() && is_wsl() {
        out.hint = Some("wsl-nat".into());
    } else if !crate::winfw::block_profiles().is_empty() {
        // Windows: a Block rule names this executable (winfw.rs); the
        // addresses above are real but a Public-profile peer times out.
        out.hint = Some("win-firewall-block".into());
    }
    out
}

/// WSL2's virtual NIC sits behind Windows' NAT: its addresses mean
/// nothing to other machines.
fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

/// Does `host` resolve to one of our own non-loopback IPv4 addresses?
/// Resolution is cached for five minutes (it runs on every roster).
fn hostname_is_ours(host: &str, port: u16, ours: &[std::net::Ipv4Addr]) -> bool {
    use std::net::ToSocketAddrs;
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, (std::time::Instant, bool)>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, ok)) = cache.lock().unwrap().get(host) {
        if at.elapsed() < std::time::Duration::from_secs(300) {
            return *ok;
        }
    }
    let ok = (host, port)
        .to_socket_addrs()
        .map(|it| {
            it.filter_map(|a| match a.ip() {
                std::net::IpAddr::V4(v4) => Some(v4),
                _ => None,
            })
            .any(|v4| ours.contains(&v4))
        })
        .unwrap_or(false);
    if !ok {
        tracing::info!(
            host,
            "hostname does not resolve to this node's own IPv4 address; not advertising it"
        );
    }
    cache
        .lock()
        .unwrap()
        .insert(host.to_owned(), (std::time::Instant::now(), ok));
    ok
}

fn hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            crate::gitstate::quiet_command("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_owned())
        })
        .filter(|s| !s.is_empty())
}

pub fn roster_payload(inner: &Arc<NodeInner>) -> Value {
    roster_payload_for(inner, None)
}

/// The roster a peer in `mesh` receives: only agents whose repo is
/// exposed to that mesh (MESHES.md §exposure); mesh-wide digests only for
/// the primary mesh, so boards, plugins and templates never cross meshes.
pub fn roster_payload_for(inner: &Arc<NodeInner>, mesh: Option<&str>) -> Value {
    let agents = inner.store.agents().unwrap_or_default();
    let primary = inner.mesh().map(|m| m.mesh_name());
    let for_primary = mesh.is_none() || mesh == primary.as_deref();
    let list: Vec<Value> = agents
        .iter()
        .filter(|a| match mesh {
            Some(m) => crate::node::repo_exposed(inner, &a.repo, m),
            None => true,
        })
        .map(|a| {
            let live = inner.live(&a.name);
            json!({
                "name": a.name,
                "channel": a.channel,
                "title": a.title,
                "live": live.is_some(),
                "turn_state": live.as_ref().map(|s| match s.turn_state() {
                    TurnState::Idle => "idle",
                    TurnState::Busy => "busy",
                }),
                "summary": live.as_ref().map(|s| crate::node::summary_json(s)),
                "activities": live.as_ref().map(|_| crate::node::activity_counts(inner, a.harness, &a.repo, a.session_id.as_deref(), a.last_spawned_at)),
                "harness": a.harness,
            })
        })
        .collect();
    let (version, sha) = VERSION.get().cloned().unwrap_or_default();
    let policy = inner
        .data_dir
        .as_deref()
        .map(crate::settings::load)
        .unwrap_or_default()
        .update;
    let mode = policy.mode.as_deref().unwrap_or("notify");
    let has_root = inner
        .data_dir
        .as_deref()
        .map(crate::mesh::MeshFiles::new)
        .and_then(|f| f.load_root().ok().flatten())
        .is_some();
    json!({
        "t": "roster", "agents": list, "version": version, "sha": sha,
        "servicing": inner.servicing.roster_json(mode),
        "has_root": has_root,
        "advertised": advertised(inner),
        "boards_digest": if for_primary { Some(inner.store.boards_digest()) } else { None },
        "plugins_digest": if for_primary { Some(inner.store.plugin_registry_digest()) } else { None },
        "templates_digest": if for_primary { Some(inner.store.templates_digest()) } else { None },
        "memory": if for_primary { crate::memory::roster_digests(inner) } else { None },
        "meshes": inner.mesh().map(|m| m.mesh_names()),
    })
}

/// Push the current roster to every connected peer (spawns/exits/timer).
pub fn broadcast_roster(inner: &Arc<NodeInner>) {
    let Some(mesh) = inner.mesh() else { return };
    let peers: Vec<String> = mesh.links.lock().unwrap().keys().cloned().collect();
    let mut by_mesh: HashMap<Option<String>, Value> = HashMap::new();
    for p in peers {
        let m = mesh.mesh_of_peer(&p);
        let payload = by_mesh
            .entry(m.clone())
            .or_insert_with(|| roster_payload_for(inner, m.as_deref()))
            .clone();
        if let Err(e) = mesh.send_to(&p, &payload) {
            tracing::debug!(peer = %p, error = %e, "roster push failed");
        }
    }
}

/// Forward every pending row for `recipient` to its home `node`.
/// Rows stay pending until the peer acks storage (at-least-once).
pub fn forward_pending(inner: &Arc<NodeInner>, recipient: &str, node: &str) {
    let Some(mesh) = inner.mesh() else { return };
    let Ok(pending) = inner.store.pending_for(recipient) else {
        return;
    };
    for m in &pending {
        if let Err(e) = mesh.send_to(node, &bus_payload(m, node)) {
            tracing::debug!(peer = %node, error = %e, "bus forward failed; stays pending");
            return;
        }
    }
}

// ----------------------------------------------------------------- the link

#[derive(Serialize, Deserialize)]
struct Hello {
    hello: NodeCert,
    #[serde(with = "aspen_wire::b64")]
    nonce: Vec<u8>,
    /// Absent on pre-servicing daemons → treated as 1.
    #[serde(default = "proto_one")]
    proto: u32,
    /// Certs for every additional mesh the sender is in (MESHES.md);
    /// `hello` is the primary. The receiver picks one for a mesh it shares.
    #[serde(default)]
    certs: Vec<NodeCert>,
}
fn proto_one() -> u32 {
    1
}

/// Run one authenticated link over a pair of text-frame channels. The
/// transport adapters (axum WS server side, tungstenite client side) bridge
/// to these channels; this function is transport-blind.
pub async fn run_link(
    inner: Arc<NodeInner>,
    out_tx: mpsc::UnboundedSender<String>,
    mut in_rx: mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    let mesh = inner
        .mesh()
        .ok_or_else(|| anyhow!("this node has not joined a mesh"))?;

    // 1. Hello out.
    let my_nonce: Vec<u8> = {
        use rand_core::RngCore;
        let mut n = [0u8; 32];
        rand_core::OsRng.fill_bytes(&mut n);
        n.to_vec()
    };
    let my_cert = mesh
        .identity
        .cert
        .clone()
        .ok_or_else(|| anyhow!("node identity has no certificate"))?;
    out_tx
        .send(serde_json::to_string(&Hello {
            hello: my_cert,
            nonce: my_nonce.clone(),
            proto: PROTOCOL,
            certs: mesh.identity.certs.clone(),
        })?)
        .map_err(|_| anyhow!("link closed before hello"))?;

    // 2. Hello in: verify the peer's cert against OUR trusted root. A
    // handshake that stalls (the other side gave up, a crossed attempt)
    // must not hold the peer's slot forever.
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, in_rx.recv())
        .await
        .map_err(|_| anyhow!("handshake timed out waiting for the peer's hello"))?
        .ok_or_else(|| anyhow!("link closed before peer hello"))?;
    let peer_hello: Hello = serde_json::from_str(&first)?;
    // Pick the peer's cert for a mesh we share (MESHES.md): the primary
    // `hello` first, then any extra; each verified against THAT mesh's
    // root. No shared mesh = no link.
    let mut offered: Vec<NodeCert> = vec![peer_hello.hello.clone()];
    offered.extend(peer_hello.certs.iter().cloned());
    let mut chosen: Option<(NodeCert, String)> = None;
    let mut last_err: Option<anyhow::Error> = None;
    for c in &offered {
        let Some(root) = mesh.root_for(&c.mesh) else { continue };
        match c.verify_against(&root) {
            Ok(()) => {
                chosen = Some((c.clone(), c.mesh.clone()));
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let Some((peer_cert, link_mesh)) = chosen else {
        let who = peer_hello.hello.node.clone();
        let msg = match last_err {
            Some(e) => format!("cert from '{who}' does not verify against this mesh's root: {e}"),
            None => format!(
                "'{who}' is in mesh {} — this node is in {}",
                offered.iter().map(|c| c.mesh.as_str()).collect::<Vec<_>>().join("/"),
                mesh.mesh_names().join("/")
            ),
        };
        mesh.note(&who, |h| {
            h.last_error = Some(msg.clone());
            h.last_error_at = Some(crate::store::now_epoch());
        });
        bail!("{msg}");
    };
    mesh.link_mesh
        .lock()
        .unwrap()
        .insert(peer_cert.node.clone(), link_mesh.clone());
    mesh.note(&peer_cert.node, |h| {
        h.fingerprint = Some(fingerprint(&peer_cert.ed_public));
    });
    if peer_hello.proto != PROTOCOL {
        let msg = format!(
            "peer '{}' speaks federation protocol {}, this node speaks {} — update the older side",
            peer_cert.node, peer_hello.proto, PROTOCOL
        );
        mesh.note(&peer_cert.node, |h| {
            h.last_error = Some(msg.clone());
            h.last_error_at = Some(crate::store::now_epoch());
        });
        bail!("{msg}");
    }
    if peer_cert.node == mesh.identity.node {
        bail!("peer presented this node's own name");
    }
    if mesh.peer_cert(&peer_cert.node).map(|c| c.ed_public) != Some(peer_cert.ed_public.clone()) {
        // A valid root-signed cert we don't have on file yet — typical for a
        // peer met through the relay (the join bundle only carried the
        // certifier's). Certs are public facts: record it, in memory for
        // send_to (envelopes are sealed to the peer's cert) and on disk so
        // it is a known peer from now on (no dial URL: reached via relay or
        // inbound).
        tracing::info!(peer = %peer_cert.node, mesh = %link_mesh, "peer cert not on file; recording it (root signature verified)");
        {
            let primary = mesh.config.read().unwrap().mesh.clone();
            if link_mesh == primary {
                let mut cfg = mesh.config.write().unwrap();
                cfg.peers.retain(|p| p.cert.node != peer_cert.node);
                cfg.peers.push(crate::mesh::PeerConfig {
                    cert: peer_cert.clone(),
                    url: None,
                });
            } else {
                let mut ex = mesh.extra.write().unwrap();
                if let Some(cfg) = ex.iter_mut().find(|c| c.mesh == link_mesh) {
                    cfg.peers.retain(|p| p.cert.node != peer_cert.node);
                    cfg.peers.push(crate::mesh::PeerConfig {
                        cert: peer_cert.clone(),
                        url: None,
                    });
                }
            }
        }
        // A console peer (RELAY.md §11) lives only for its link: it is
        // never a configured peer on disk.
        if !peer_cert.node.starts_with("console-") {
            if let Some(dir) = inner.data_dir.as_deref() {
                if let Err(e) = crate::mesh::MeshFiles::new(dir).add_peer(peer_cert.clone(), None) {
                    tracing::warn!(peer = %peer_cert.node, error = %e, "could not persist peer cert");
                }
            }
        }
    }

    // 3. Prove key possession both ways: return their nonce sealed.
    let auth_out = SealedEnvelope::seal(
        &mesh.identity,
        &peer_cert,
        json!({ "t": "auth", "nonce": aspen_wire::b64::encode(&peer_hello.nonce) })
            .to_string()
            .as_bytes(),
    )?;
    out_tx
        .send(serde_json::to_string(&auth_out)?)
        .map_err(|_| anyhow!("link closed during auth"))?;

    let auth_frame = tokio::time::timeout(HANDSHAKE_TIMEOUT, in_rx.recv())
        .await
        .map_err(|_| anyhow!("handshake timed out waiting for the peer's auth"))?
        .ok_or_else(|| anyhow!("link closed before peer auth"))?;
    let env: SealedEnvelope = serde_json::from_str(&auth_frame)?;
    let payload: Value = serde_json::from_slice(&env.open(&mesh.identity, &peer_cert)?)?;
    if payload.get("t").and_then(|t| t.as_str()) != Some("auth")
        || payload.get("nonce").and_then(|n| n.as_str())
            != Some(aspen_wire::b64::encode(&my_nonce)).as_deref()
    {
        bail!("peer failed nonce proof");
    }

    // 4. Link up. Kind: a relay link announced itself as pending; anything
    // else is direct (dialed or inbound). A direct link supersedes a relay
    // one — drop the relay-side channel so that session ends.
    let peer = peer_cert.node.clone();
    let kind = mesh
        .link_kind
        .lock()
        .unwrap()
        .remove(&format!("pending:{peer}"))
        .unwrap_or_else(|| "direct".into());
    if kind == "direct" {
        let sessions = mesh.relay_sessions.lock().unwrap();
        for s in sessions.values() {
            if s.peer_ins.lock().unwrap().remove(&peer).is_some() {
                tracing::info!(peer = %peer, "direct link supersedes the relay link");
            }
        }
    } else if mesh
        .link_kind
        .lock()
        .unwrap()
        .get(&peer)
        .is_some_and(|k| k == "direct")
        && mesh.link_up(&peer)
    {
        // A direct link already carries this peer: don't replace it.
        bail!("direct link already up; relay link not needed");
    }
    mesh.link_kind
        .lock()
        .unwrap()
        .insert(peer.clone(), kind.clone());
    mesh.links
        .lock()
        .unwrap()
        .insert(peer.clone(), out_tx.clone());
    tracing::info!(peer = %peer, kind = %kind, "federation link up");
    mesh.note(&peer, |h| {
        h.last_up = Some(crate::store::now_epoch());
        h.last_error = None;
        h.last_error_at = None;
    });
    let _ = mesh.send_to(&peer, &roster_payload_for(&inner, mesh.mesh_of_peer(&peer).as_deref()));
    // Anything pending for agents homed there can move now.
    let homed: Vec<String> = mesh
        .remote
        .lock()
        .unwrap()
        .get(&peer)
        .map(|v| v.iter().map(|a| a.name.clone()).collect())
        .unwrap_or_default();
    for name in homed {
        inner.tick_delivery(&name);
    }

    // 5. Steady state.
    let result = link_loop(&inner, &mesh, &peer, &peer_cert, &mut in_rx).await;

    // 6. Teardown (only if the registered link is still ours).
    let mut links = mesh.links.lock().unwrap();
    let ours = links.get(&peer).is_some_and(|tx| tx.same_channel(&out_tx));
    if ours {
        links.remove(&peer);
        mesh.link_kind.lock().unwrap().remove(&peer);
    }
    drop(links);
    if !ours {
        // Superseded (a direct link replaced this relay link): nothing else
        // to tear down — the live link's state stays.
        return result;
    }
    // A direct link that dropped: fall back to any relay where the peer is
    // present (lower name starts it; the peer's side does the same).
    if kind == "direct" && mesh.identity.node.as_str() < peer.as_str() {
        let sessions = mesh.relay_sessions.lock().unwrap();
        for (url, s) in sessions.iter() {
            if s.present.contains(&peer) {
                start_relay_link(&inner, &mesh.identity.node, &peer, url, &s.tx, &s.peer_ins);
                tracing::info!(peer = %peer, relay = %url, "direct link lost; falling back to relay");
                break;
            }
        }
    }
    mesh.remote.lock().unwrap().remove(&peer);
    mesh.link_mesh.lock().unwrap().remove(&peer);
    // Consumers of subscriptions served over this link learn immediately
    // (their channel closes) rather than waiting on silence.
    mesh.remote_subs
        .lock()
        .unwrap()
        .retain(|_, (served_by, _)| served_by != &peer);
    tracing::info!(peer = %peer, "federation link down");
    mesh.note(&peer, |h| h.last_down = Some(crate::store::now_epoch()));
    // Pending mail for agents homed there takes the mailbox from now on —
    // don't wait for the periodic tick.
    for r in inner.store.pending_recipients().unwrap_or_default() {
        if crate::addr::node_of(&r) == Some(peer.as_str()) {
            inner.tick_delivery(&r);
        }
    }
    result
}

async fn link_loop(
    inner: &Arc<NodeInner>,
    mesh: &Arc<MeshState>,
    peer: &str,
    peer_cert: &NodeCert,
    in_rx: &mut mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    while let Some(frame) = in_rx.recv().await {
        let env: SealedEnvelope = match serde_json::from_str(&frame) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(peer, error = %e, "unparseable federation frame skipped");
                continue;
            }
        };
        let payload: Value = match env
            .open(&mesh.identity, peer_cert)
            .and_then(|b| Ok(serde_json::from_slice(&b)?))
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(peer, error = %e, "envelope rejected");
                continue;
            }
        };
        match payload.get("t").and_then(|t| t.as_str()).unwrap_or("") {
            "bus" if !mesh.allows(peer, Capability::Control) => {
                tracing::info!(peer, "bus row from an observe-only peer dropped");
            }
            "bus" => {
                let uuid = payload.get("uuid").and_then(|u| u.as_str()).unwrap_or("");
                let sender = payload
                    .get("sender")
                    .and_then(|s| s.as_str())
                    .unwrap_or("?");
                // Cross-node sender identity gains its home node unless it
                // already carries one (`name@repo` → `name@repo@peer`).
                let sender = if sender == "operator" || crate::addr::node_of(sender).is_some() {
                    sender.to_owned()
                } else {
                    format!("{sender}@{peer}")
                };
                let recipient = payload
                    .get("recipient")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let inserted = inner.store.insert_federated(
                    uuid,
                    &sender,
                    recipient,
                    payload
                        .get("to_display")
                        .and_then(|s| s.as_str())
                        .unwrap_or(""),
                    payload
                        .get("urgency")
                        .and_then(|s| s.as_str())
                        .unwrap_or("normal"),
                    payload.get("body").and_then(|s| s.as_str()).unwrap_or(""),
                    payload.get("thread").and_then(|s| s.as_str()),
                    payload.get("record_ref").and_then(|s| s.as_str()),
                    payload.get("created_at").and_then(|c| c.as_f64()),
                );
                match inserted {
                    Ok(_) => {
                        inner.tick_delivery(recipient);
                        let _ = mesh.send_to(peer, &json!({ "t": "bus_ack", "uuid": uuid }));
                    }
                    Err(e) => tracing::warn!(peer, error = %e, "federated insert failed"),
                }
            }
            "bus_ack" => {
                if let Some(uuid) = payload.get("uuid").and_then(|u| u.as_str()) {
                    let _ = inner
                        .store
                        .mark_delivered_by_uuid(uuid, &format!("federated:{peer}"));
                }
            }
            "roster" => {
                let agents: Vec<RemoteAgent> = payload
                    .get("agents")
                    .and_then(|a| serde_json::from_value(a.clone()).ok())
                    .unwrap_or_default();
                let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
                mesh.remote.lock().unwrap().insert(peer.to_owned(), agents);
                {
                    let v = payload
                        .get("version")
                        .and_then(|x| x.as_str())
                        .map(str::to_owned);
                    let s = payload
                        .get("sha")
                        .and_then(|x| x.as_str())
                        .map(str::to_owned);
                    let svc = payload.get("servicing").cloned();
                    let has_root = payload.get("has_root").and_then(|b| b.as_bool());
                    let adv: Option<Advertised> = payload
                        .get("advertised")
                        .and_then(|a| serde_json::from_value(a.clone()).ok());
                    if let Some(a) = &adv {
                        // A peer's advertised relays are one relay under
                        // several names. Take the first only, and none at
                        // all when we already reach that peer by a
                        // configured URL (its relay is at the same place —
                        // set it with `aspen mesh relay` if wanted).
                        let dialed = mesh
                            .peers()
                            .iter()
                            .any(|p| p.cert.node == peer && p.url.is_some());
                        let mine = advertised(inner);
                        // What the peer no longer advertises is stale.
                        mesh.discovered_relays
                            .lock()
                            .unwrap()
                            .retain(|u, from| from != peer || a.relay_urls.contains(u));
                        if !dialed {
                            let mut disc = mesh.discovered_relays.lock().unwrap();
                            if !disc.values().any(|from| from == peer) {
                                if let Some(u) =
                                    a.relay_urls.iter().find(|u| !mine.relay_urls.contains(u))
                                {
                                    disc.insert(u.clone(), peer.to_owned());
                                }
                            }
                        }
                    }
                    // Boards ride the mesh: a peer whose digest differs
                    // from ours is pulled (last writer wins per board).
                    if let Some(d) = payload.get("boards_digest").and_then(|d| d.as_str()) {
                        if d != inner.store.boards_digest() {
                            let inner2 = inner.clone();
                            let peer2 = peer.to_owned();
                            tokio::spawn(async move {
                                sync_boards_from(&inner2, &peer2).await;
                            });
                        }
                    }
                    if let Some(d) = payload.get("templates_digest").and_then(|d| d.as_str()) {
                        if d != inner.store.templates_digest() {
                            let inner2 = inner.clone();
                            let peer2 = peer.to_owned();
                            tokio::spawn(async move {
                                sync_templates_from(&inner2, &peer2).await;
                            });
                        }
                    }
                    if let Some(m) = payload.get("memory").and_then(|m| m.as_object()) {
                        if let Some(mine) = crate::memory::roster_digests(inner) {
                            for (key, d) in m {
                                if mine.get(key).is_some() && mine.get(key) != Some(d) {
                                    let inner2 = inner.clone();
                                    let peer2 = peer.to_owned();
                                    let key2 = key.clone();
                                    tokio::spawn(async move {
                                        crate::memory::sync_from(&inner2, &peer2, &key2).await;
                                    });
                                }
                            }
                        }
                    }
                    if let Some(d) = payload.get("plugins_digest").and_then(|d| d.as_str()) {
                        if d != inner.store.plugin_registry_digest() {
                            let inner2 = inner.clone();
                            let peer2 = peer.to_owned();
                            tokio::spawn(async move {
                                sync_plugin_registry_from(&inner2, &peer2).await;
                            });
                        }
                    }
                    mesh.note(peer, |h| {
                        h.last_roster = Some(crate::store::now_epoch());
                        if has_root.is_some() {
                            h.has_root = has_root;
                        }
                        if adv.is_some() {
                            h.advertised = adv;
                        }
                        if v.is_some() {
                            h.version = v;
                        }
                        if s.is_some() {
                            h.sha = s;
                        }
                        if let Some(svc) = &svc {
                            let g =
                                |k: &str| svc.get(k).and_then(|x| x.as_str()).map(str::to_owned);
                            h.update_available = g("available");
                            h.service_state = g("state");
                            h.service_detail = g("state_detail");
                            h.policy = g("policy");
                            h.inventory = svc.get("inventory").cloned().filter(|v| !v.is_null());
                            h.last_outcome =
                                svc.get("last_outcome").cloned().filter(|v| !v.is_null());
                        }
                    });
                }
                // Channel members from before scoped names (`main@node`)
                // can only be resolved once we see that node's roster.
                let _ = inner.store.heal_legacy_remote_members(peer, &names);
                // New addresses may have arrived: dialers for peers we can
                // now reach directly, clients for relays peers host.
                ensure_dialers(inner.clone());
                for name in names {
                    // Remote keys are addressed here as key@node; a bare
                    // key can also be homed there (delivery finds it).
                    inner.tick_delivery(&format!("{name}@{peer}"));
                    inner.tick_delivery(&name);
                }
            }
            "update_hint" => {
                // A hint, never authority: we check the channel ourselves.
                if let Some(v) = payload.get("version").and_then(|v| v.as_str()) {
                    crate::servicing::on_hint(inner, v);
                }
            }
            "api_req" => {
                // Serve the peer's console. Spawned: ops like spawn/revive
                // take seconds and must not stall the link.
                let id = payload
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_owned();
                let op = payload
                    .get("op")
                    .and_then(|o| o.as_str())
                    .unwrap_or("")
                    .to_owned();
                let agent = payload
                    .get("agent")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_owned();
                let body = payload.get("body").cloned().unwrap_or(Value::Null);
                let inner = inner.clone();
                let mesh = mesh.clone();
                let peer = peer.to_owned();
                // The capability layer (MESHES.md): what this peer's mesh
                // policy grants, checked before dispatch; spawn/trust from a
                // peer is audited on the fleet trail.
                let cap = op_capability(&op);
                if !mesh.allows(&peer, cap) {
                    let m = mesh.mesh_of_peer(&peer).unwrap_or_default();
                    let reply = json!({ "t": "api_res", "id": id, "ok": false,
                        "error": format!("forbidden: {cap:?} not granted to mesh '{m}' here (policy {})", mesh.policy_of(&m)) });
                    let _ = mesh.send_to(&peer, &reply);
                    continue;
                }
                if matches!(cap, Capability::Spawn | Capability::Trust) {
                    let _ = inner.store.record_event(
                        "node",
                        "remote_control",
                        json!({ "peer": peer, "mesh": mesh.mesh_of_peer(&peer), "op": op, "agent": agent }),
                    );
                }
                tokio::spawn(async move {
                    // Exposure (MESHES.md): a peer sees and acts on only what
                    // its mesh is allowed to; checked before and after.
                    let peer_mesh = mesh.mesh_of_peer(&peer).unwrap_or_else(|| mesh.mesh_name());
                    let res = match exposure_precheck(&inner, &peer_mesh, &op, &agent, &body) {
                        Err(e) => Err(e),
                        Ok(()) => serve_api_req(&inner, &peer, &op, &agent, body)
                            .await
                            .map(|v| exposure_filter(&inner, &peer_mesh, &op, v)),
                    };
                    let reply = match res {
                        Ok(body) => json!({ "t": "api_res", "id": id, "ok": true, "body": body }),
                        Err(e) => {
                            json!({ "t": "api_res", "id": id, "ok": false, "error": e.to_string() })
                        }
                    };
                    let _ = mesh.send_to(&peer, &reply);
                });
            }
            "api_res" => {
                if let Some(id) = payload.get("id").and_then(|i| i.as_str()) {
                    if let Some(tx) = mesh.pending_api.lock().unwrap().remove(id) {
                        let _ = tx.send(payload);
                    }
                }
            }
            "sub" => {
                let id = payload
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_owned();
                let agent = payload
                    .get("agent")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_owned();
                // A console peer may ask for an agent homed on another
                // node (`bare@node`): subscribe there on its behalf and
                // relay the frames (RELAY.md §11).
                if inner.live(&agent).is_none() {
                    if let Some((bare, home)) = agent.rsplit_once('@').filter(|(_, h)| mesh.peers().iter().any(|p| p.cert.node == *h)) {
                        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
                        let up_id = uuid::Uuid::new_v4().to_string();
                        mesh.remote_subs.lock().unwrap().insert(up_id.clone(), (home.to_owned(), tx));
                        if mesh.send_to(home, &json!({ "t": "sub", "id": up_id, "agent": bare })).is_err() {
                            mesh.remote_subs.lock().unwrap().remove(&up_id);
                            let _ = mesh.send_to(peer, &json!({ "t": "sub_end", "id": id, "reason": format!("no live link to {home}") }));
                            continue;
                        }
                        let mesh2 = mesh.clone();
                        let peer2 = peer.to_owned();
                        let id2 = id.clone();
                        let home2 = home.to_owned();
                        let up_id2 = up_id.clone();
                        let task = tokio::spawn(async move {
                            while let Some(f) = rx.recv().await {
                                let t = f.get("t").and_then(|t| t.as_str()).unwrap_or("");
                                if t == "sub_end" {
                                    break;
                                }
                                if mesh2.send_to(&peer2, &json!({ "t": "ev", "id": id2, "ev": f.get("ev").cloned().unwrap_or(Value::Null) })).is_err() {
                                    break;
                                }
                            }
                            mesh2.remote_subs.lock().unwrap().remove(&up_id2);
                            let _ = mesh2.send_to(&home2, &json!({ "t": "unsub", "id": up_id2 }));
                            let _ = mesh2.send_to(&peer2, &json!({ "t": "sub_end", "id": id2 }));
                        });
                        mesh.served_subs.lock().unwrap().insert(id.clone(), task);
                        continue;
                    }
                }
                let Some(sess) = inner.live(&agent) else {
                    let _ = mesh.send_to(
                        peer,
                        &json!({ "t": "sub_end", "id": id, "reason": "no such live agent" }),
                    );
                    continue;
                };
                let mut rx = sess.events.subscribe();
                let mesh2 = mesh.clone();
                let peer2 = peer.to_owned();
                let id2 = id.clone();
                let task = tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(ev) => {
                                let Ok(ev_json) = serde_json::to_value(&ev) else {
                                    continue;
                                };
                                if mesh2
                                    .send_to(
                                        &peer2,
                                        &json!({ "t": "ev", "id": id2, "ev": ev_json }),
                                    )
                                    .is_err()
                                {
                                    break;
                                }
                                if matches!(ev, aspen_core::SessionEvent::Exited { .. }) {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                    let _ = mesh2.send_to(&peer2, &json!({ "t": "sub_end", "id": id2 }));
                    mesh2.served_subs.lock().unwrap().remove(&id2);
                });
                mesh.served_subs.lock().unwrap().insert(id, task);
            }
            "unsub" => {
                if let Some(id) = payload.get("id").and_then(|i| i.as_str()) {
                    if let Some(task) = mesh.served_subs.lock().unwrap().remove(id) {
                        task.abort();
                    }
                }
            }
            "ev" | "sub_end" => {
                if let Some(id) = payload.get("id").and_then(|i| i.as_str()) {
                    let is_end = payload.get("t").and_then(|t| t.as_str()) == Some("sub_end");
                    let dead = {
                        let subs = mesh.remote_subs.lock().unwrap();
                        subs.get(id)
                            .map(|(_, tx)| tx.send(payload.clone()).is_err())
                    };
                    if dead == Some(true) || is_end {
                        mesh.remote_subs.lock().unwrap().remove(id);
                        if dead == Some(true) && !is_end {
                            let _ = mesh.send_to(peer, &json!({ "t": "unsub", "id": id }));
                        }
                    }
                }
            }
            other => tracing::debug!(peer, other, "unknown federation payload ignored"),
        }
    }
    Ok(())
}

/// Execute a peer console's request against this node. The op vocabulary
/// mirrors the local REST API; every op is scoped to one named agent.
/// Refuse before dispatch what a peer's mesh may not touch: a local agent
/// in an unexposed repo; a spawn into one; mesh-wide tables from a mesh
/// that is not the primary (boards, plugins, templates, memory never
/// cross meshes). With one mesh nothing is filtered.
fn exposure_precheck(inner: &Arc<NodeInner>, peer_mesh: &str, op: &str, agent: &str, body: &Value) -> Result<()> {
    let Some(mesh) = inner.mesh() else { return Ok(()) };
    if !mesh.is_multi() {
        return Ok(());
    }
    let primary = mesh.mesh_name();
    if peer_mesh != primary
        && matches!(
            op,
            "boards" | "plugin_registry" | "plugins_sync" | "plugins_registry_view" | "templates"
                | "template_spawn" | "memory_files" | "memory_conflicts" | "memory_resolve"
                | "inbox_read" | "link_add" | "link_del" | "node_update" | "node_update_cancel"
                | "node_update_policy" | "node_evacuate" | "adoption" | "adoptions" | "node_logs"
        )
    {
        return Err(anyhow!("{op}: mesh-wide state and node servicing belong to this node's primary mesh"));
    }
    if !agent.is_empty() && !crate::node::agent_exposed(inner, agent, peer_mesh) {
        return Err(anyhow!("@{agent} is not exposed to mesh '{peer_mesh}'"));
    }
    if matches!(op, "spawn" | "node_sessions" | "node_repo_skip" | "node_repo_rename" | "node_repo_forget") {
        if let Some(r) = body.get("repo").or_else(|| body.get("path")).and_then(|r| r.as_str()) {
            let path = crate::node::normalize_repo(std::path::Path::new(r));
            if !crate::node::repo_exposed(inner, &path, peer_mesh) {
                return Err(anyhow!("{r} is not exposed to mesh '{peer_mesh}'"));
            }
        }
    }
    Ok(())
}

/// Drop from a response what a peer's mesh may not see: repo rows and
/// anything tagged with an unexposed agent.
fn exposure_filter(inner: &Arc<NodeInner>, peer_mesh: &str, op: &str, v: Value) -> Value {
    let Some(mesh) = inner.mesh() else { return v };
    if !mesh.is_multi() {
        return v;
    }
    let agent_ok = |item: &Value| -> bool {
        for k in ["agent", "of_agent", "name"] {
            if let Some(a) = item.get(k).and_then(|a| a.as_str()) {
                if op == "node_repos" || op == "node_discover" {
                    break;
                }
                return crate::node::agent_exposed(inner, a, peer_mesh);
            }
        }
        true
    };
    let repo_ok = |item: &Value| -> bool {
        match item.get("path").and_then(|p| p.as_str()) {
            Some(p) => crate::node::repo_exposed(inner, std::path::Path::new(p), peer_mesh),
            None => true,
        }
    };
    match (op, v) {
        ("node_repos" | "node_discover", Value::Array(items)) => {
            Value::Array(items.into_iter().filter(|i| repo_ok(i)).collect())
        }
        ("fleet_activities" | "usage" | "adoptions", Value::Array(items)) => {
            Value::Array(items.into_iter().filter(|i| agent_ok(i)).collect())
        }
        ("history", mut v) => {
            for k in ["events", "messages"] {
                if let Some(Value::Array(items)) = v.get(k).cloned() {
                    v[k] = Value::Array(items.into_iter().filter(|i| agent_ok(i)).collect());
                }
            }
            v
        }
        ("needs", mut v) => {
            for k in ["prompts", "adoptions"] {
                if let Some(Value::Array(items)) = v.get(k).cloned() {
                    v[k] = Value::Array(items.into_iter().filter(|i| agent_ok(i)).collect());
                }
            }
            // Operator mail and memory conflicts are this operator's.
            v["inbox"] = json!([]);
            v["memory"] = json!([]);
            v
        }
        ("notices", mut v) => {
            if let Some(Value::Array(items)) = v.get("notices").cloned() {
                v["notices"] = Value::Array(items.into_iter().filter(|i| agent_ok(i)).collect());
            }
            v
        }
        (_, v) => v,
    }
}

async fn serve_api_req(
    inner: &Arc<NodeInner>,
    peer: &str,
    op: &str,
    agent: &str,
    body: Value,
) -> Result<Value> {
    let node = crate::node::Node {
        inner: inner.clone(),
    };
    match op {
        "message" => {
            let text = body
                .get("text")
                .and_then(|t| t.as_str())
                .ok_or_else(|| anyhow!("missing text"))?;
            let attachments: Vec<crate::artifacts::Attachment> = body
                .get("attachments")
                .and_then(|a| serde_json::from_value(a.clone()).ok())
                .unwrap_or_default();
            let uuid = node
                .send_operator_message_with(agent, text.to_owned(), attachments)
                .await?;
            Ok(json!({ "uuid": uuid }))
        }
        "interrupt" => {
            node.interrupt(agent).await?;
            Ok(json!({}))
        }
        "shutdown" => {
            node.shutdown_agent(agent).await?;
            Ok(json!({}))
        }
        "revive" => {
            let choice = body
                .get("resume_choice")
                .and_then(|c| c.as_str())
                .map(str::to_owned);
            match node.revive_agent(agent, true, choice).await {
                Ok(_) => Ok(json!({})),
                // Same structured reply as a start: the caller maps it back
                // to the fork / in-place question.
                Err(e) => match e.downcast_ref::<crate::node::LiveElsewhere>() {
                    Some(le) => Ok(json!({
                        "live_elsewhere": { "session": le.session, "written_ago_secs": le.written_ago_secs },
                    })),
                    None => Err(e),
                },
            }
        }
        "branch" => {
            let sess = node
                .branch_agent(
                    agent,
                    body.get("label").and_then(|l| l.as_str()),
                    body.get("at").and_then(|a| a.as_str()),
                    body.get("as").and_then(|a| a.as_str()),
                )
                .await?;
            Ok(json!({ "name": sess.name }))
        }
        "bookmarks" => {
            let head = node
                .inner
                .store
                .agents()?
                .into_iter()
                .find(|a| a.name == agent)
                .and_then(|a| a.session_id);
            let lineage = head
                .as_deref()
                .and_then(|h| node.inner.store.lineage_of(h).ok())
                .unwrap_or_default();
            Ok(json!({
                "head": head,
                "lineage": lineage.iter().map(|(p, at)| json!({ "session_id": p, "fork_message": at })).collect::<Vec<_>>(),
                "bookmarks": node.inner.store.bookmarks(agent)?,
            }))
        }
        "resume_bookmark" => {
            let id = body
                .get("id")
                .and_then(|i| i.as_i64())
                .ok_or_else(|| anyhow!("missing id"))?;
            let sess = node
                .resume_bookmark(agent, id, body.get("as").and_then(|a| a.as_str()))
                .await?;
            Ok(json!({ "name": sess.name }))
        }
        "delete_bookmark" => {
            let id = body
                .get("id")
                .and_then(|i| i.as_i64())
                .ok_or_else(|| anyhow!("missing id"))?;
            node.inner.store.delete_bookmark(agent, id)?;
            Ok(json!({}))
        }
        "reload" => node.reload_plugins(agent).await,
        "runtime" => node.runtime_info(agent),
        "artifacts" => Ok(json!(node.artifacts(agent)?)),
        "fleet_activities" => Ok(json!(crate::node::fleet_activities(&node.inner))),
        "http" => {
            // A console peer's request, dispatched into this node's own
            // router (RELAY.md §11). Only /api/ paths; the gateway sets the
            // node token, since the link already authenticated the caller.
            let gw = inner
                .http_gateway
                .get()
                .cloned()
                .ok_or_else(|| anyhow!("no http gateway on this node (headless?)"))?;
            let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("GET").to_owned();
            let path = body.get("path").and_then(|p| p.as_str()).unwrap_or("/").to_owned();
            if !path.starts_with("/api/") {
                return Err(anyhow!("http op serves /api/ paths only"));
            }
            let b = body.get("body").and_then(|b| b.as_str()).map(str::to_owned);
            let headers: HashMap<String, String> = body
                .get("headers")
                .and_then(|h| serde_json::from_value(h.clone()).ok())
                .unwrap_or_default();
            Ok(gw(method, path, b, headers).await)
        }
        "templates" => Ok(json!(node.inner.store.templates(true)?)),
        "template_spawn" => {
            let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let overrides = body.get("overrides").cloned().unwrap_or(Value::Null);
            node.spawn_from_template(&id, &overrides).await
        }
        "replica_offsets" => crate::replicate::offsets(&node.inner, peer, agent),
        "memory_files" => {
            let key = body.get("key").and_then(|k| k.as_str()).unwrap_or("").to_owned();
            let inner = node.inner.clone();
            tokio::task::spawn_blocking(move || crate::memory::files_for_key(&inner, &key)).await?
        }
        "memory_conflicts" => Ok(json!(crate::memory::conflicts(&node.inner))),
        "memory_resolve" => {
            let repo = std::path::PathBuf::from(body.get("repo").and_then(|v| v.as_str()).unwrap_or(""));
            let rel = body.get("rel").and_then(|v| v.as_str()).unwrap_or("");
            let copy = body.get("copy").and_then(|v| v.as_str()).unwrap_or("");
            let choice = body.get("choice").and_then(|v| v.as_str()).unwrap_or("");
            crate::memory::resolve(&node.inner, &repo, rel, copy, choice)?;
            Ok(json!({ "ok": true }))
        }
        "replica_append" => {
            let inner = node.inner.clone();
            let peer = peer.to_owned();
            let agent = agent.to_owned();
            tokio::task::spawn_blocking(move || crate::replicate::append(&inner, &peer, &agent, &body)).await?
        }
        "usage" => {
            let from = body.get("from").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let to = body.get("to").and_then(|v| v.as_f64()).unwrap_or(f64::MAX);
            let one = if agent.is_empty() { None } else { Some(agent) };
            Ok(json!(crate::node::usage_rows(&node.inner, from, to, one)))
        }
        "notices" => {
            let since = body.get("since").and_then(|v| v.as_i64()).unwrap_or(-1);
            if since < 0 {
                return Ok(json!({ "head": node.inner.store.notices_head(), "notices": [] }));
            }
            let rows = node.inner.store.notices_since(since, 200)?;
            let out: Vec<Value> = rows.iter().map(|n| crate::notify::notice_json(n, None)).collect();
            Ok(json!({ "head": rows.last().map(|n| n.id).unwrap_or(since), "notices": out }))
        }
        "activities" => {
            let rows = node.inner.store.agents()?;
            let row = rows
                .iter()
                .find(|a| a.name == agent)
                .ok_or_else(|| anyhow!("no agent named @{agent}"))?;
            let sid = row
                .session_id
                .as_deref()
                .ok_or_else(|| anyhow!("no session on record"))?;
            let mut acts = node.inner.store_for(row.harness).activities(&row.repo, sid, row.last_spawned_at);
            acts.reverse();
            Ok(json!(acts))
        }
        "subagent" => {
            let rows = node.inner.store.agents()?;
            let row = rows
                .iter()
                .find(|a| a.name == agent)
                .ok_or_else(|| anyhow!("no agent named @{agent}"))?;
            let sid = row
                .session_id
                .as_deref()
                .ok_or_else(|| anyhow!("no session on record"))?;
            let id = body
                .get("id")
                .and_then(|i| i.as_str())
                .ok_or_else(|| anyhow!("missing id"))?;
            if id.contains(['/', '\\', '.']) {
                return Err(anyhow!("bad agent id"));
            }
            Ok(json!(node.inner.store_for(row.harness).subagent(&row.repo, sid, id).unwrap_or_default()))
        }
        "boards" => Ok(json!(node.inner.store.boards(true)?)),
        "plugin_registry" => Ok(json!({
            "marketplaces": node.inner.store.marketplaces(true)?,
            "rules": node.inner.store.plugin_rules(true)?,
        })),
        "plugins_sync" => {
            let c = crate::plugins::spawn_sync(
                node.inner.clone(),
                body.get("marketplace")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned),
            )
            .await?;
            Ok(json!(c))
        }
        "plugins_registry_view" => Ok(crate::plugins::registry_json(&node.inner)),
        "plugins_effective" => {
            let dd = node
                .inner
                .data_dir
                .clone()
                .ok_or_else(|| anyhow!("no data dir"))?;
            let rows = node.inner.store.agents()?;
            let row = rows
                .iter()
                .find(|a| a.name == agent)
                .ok_or_else(|| anyhow!("no agent named @{agent}"))?;
            let rules = node.inner.store.plugin_rules(false)?;
            let me = node
                .inner
                .mesh()
                .map(|m| m.identity.node.clone())
                .unwrap_or_else(|| "local".into());
            let (active, missing) = crate::plugins::resolve(&dd, &rules, &me, &row.repo, &row.name);
            let running = node
                .inner
                .live(&row.name)
                .map(|m| m.plugins.clone())
                .unwrap_or_default();
            Ok(
                json!({ "would_start_with": active, "missing": missing, "running": running, "updates": crate::plugins::updates_for(&dd, &running) }),
            )
        }
        // Migration (migrate.rs): the source stages a bundle and serves
        // it in chunks; the target pulls, installs, and reports back.
        "session_spec" => Ok(json!(node.session_spec(agent)?)),
        "session_export" => {
            let opts: crate::migrate::ExportOpts =
                serde_json::from_value(body.clone()).unwrap_or_default();
            let (bundle_id, manifest) = node.session_export(agent, &opts).await?;
            Ok(json!({ "bundle_id": bundle_id, "manifest": manifest }))
        }
        "bundle_read" => node.bundle_read(
            body.get("bundle_id").and_then(|b| b.as_str()).unwrap_or(""),
            body.get("rel").and_then(|r| r.as_str()).unwrap_or(""),
            body.get("offset").and_then(|o| o.as_u64()).unwrap_or(0),
            body.get("len")
                .and_then(|l| l.as_u64())
                .unwrap_or(crate::artifacts::READ_CHUNK),
        ),
        "bundle_done" => {
            node.bundle_done(body.get("bundle_id").and_then(|b| b.as_str()).unwrap_or(""))?;
            Ok(json!({}))
        }
        "session_moved" => node.session_moved(
            agent,
            body.get("to").and_then(|t| t.as_str()).unwrap_or(""),
            body.get("as").and_then(|t| t.as_str()).unwrap_or(agent),
        ),
        "session_pull" => {
            let from = body
                .get("from")
                .and_then(|f| f.as_str())
                .ok_or_else(|| anyhow!("missing from"))?
                .to_owned();
            let opts: crate::migrate::ImportOpts =
                serde_json::from_value(body.clone()).unwrap_or_default();
            node.pull_session(&from, agent, &opts).await
        }
        "file_stat" => node.file_stat(
            agent,
            body.get("path").and_then(|p| p.as_str()).unwrap_or(""),
        ),
        "file_read" => node.file_read(
            agent,
            body.get("path").and_then(|p| p.as_str()).unwrap_or(""),
            body.get("offset").and_then(|o| o.as_u64()).unwrap_or(0),
            body.get("len")
                .and_then(|l| l.as_u64())
                .unwrap_or(crate::artifacts::READ_CHUNK),
        ),
        "context" => node.context_usage(agent).await,
        "set_model" => {
            node.set_model(agent, body.get("model").and_then(|m| m.as_str()))
                .await?;
            Ok(json!({}))
        }
        "set_mode" => {
            node.set_permission_mode(
                agent,
                body.get("mode")
                    .and_then(|m| m.as_str())
                    .ok_or_else(|| anyhow!("missing mode"))?,
            )
            .await?;
            Ok(json!({}))
        }
        "title" => {
            node.set_title(agent, body.get("title").and_then(|v| v.as_str()))?;
            Ok(json!({}))
        }
        "charter" => {
            node.set_charter(agent, body.get("charter").and_then(|v| v.as_str()))?;
            Ok(json!({}))
        }
        // Node-level ops (the `agent` field is ignored):
        "needs" => {
            let prompts: Vec<Value> = node
                .open_prompts()
                .into_iter()
                .map(|(agent, p)| {
                    json!({
                        "agent": agent, "request_id": p.request_id,
                        "tool_name": p.tool_name, "input": p.input,
                        "suggestions": p.suggestions, "asked_at": p.asked_at,
                        "is_question": p.is_question,
                        "prompt_kind": p.prompt_kind, "tool_kind": p.tool_kind,
                        "decisions": p.decisions, "questions": p.questions,
                    })
                })
                .collect();
            let inbox: Vec<Value> = inner
                .store
                .pending_for("operator")
                .unwrap_or_default()
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id, "sender": m.sender, "recipient": m.recipient,
                        "to_display": m.to_display, "urgency": m.urgency,
                        "body": m.body, "thread": m.thread, "record": m.record_ref,
                        "created_at": m.created_at,
                    })
                })
                .collect();
            let adoptions: Vec<Value> = inner
                .store
                .adoptions(true)
                .unwrap_or_default()
                .iter()
                .map(crate::adoption::adoption_json)
                .collect();
            Ok(json!({
                "prompts": prompts, "inbox": inbox, "adoptions": adoptions,
                "memory": crate::memory::conflicts(inner),
            }))
        }
        "inbox_read" => {
            let rows = inner.store.pending_for("operator")?;
            let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
            inner.store.mark_delivered(&ids, "operator-ui", None)?;
            Ok(json!({}))
        }
        "permission" => {
            node.answer_permission(
                agent,
                body.get("request_id")
                    .and_then(|r| r.as_str())
                    .ok_or_else(|| anyhow!("missing request_id"))?,
                body.get("allow").and_then(|a| a.as_bool()).unwrap_or(false),
                body.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned),
                body.get("updated_input").cloned().filter(|v| !v.is_null()),
                body.get("updated_permissions")
                    .cloned()
                    .filter(|v| !v.is_null()),
            )?;
            Ok(json!({}))
        }
        "transcript" => {
            let rows = inner.store.agents()?;
            let row = rows
                .iter()
                .find(|a| a.name == agent)
                .ok_or_else(|| anyhow!("no agent named @{agent}"))?;
            let sid = row
                .session_id
                .as_ref()
                .ok_or_else(|| anyhow!("no session on record"))?;
            let st = node.inner.store_for(row.harness);
            if let Some(after) = body.get("after").and_then(|a| a.as_str()) {
                let (items, found) = st.rehydrate_after(&row.repo, sid, after).unwrap_or_default();
                return Ok(json!({ "items": items, "after_found": found }));
            }
            let items = st.rehydrate(&row.repo, sid).unwrap_or_default();
            Ok(json!(items))
        }
        // -------------------------------------------------- node-scoped ops
        // These ignore `agent` — they act on the node, for the Library's
        // mesh-wide view. Repos/sessions recovered here register on THIS
        // node; a peer that loses the mesh link stops seeing them, which is
        // the intended "remote content lives on its owning node" model.
        "node_repos" => {
            let repos = node.inner.store.repos()?;
            let agents = node.inner.store.agents().unwrap_or_default();
            Ok(json!(repos
                .iter()
                .map(|r| {
                    let sessions = crate::node::enumerate_all(&node.inner, &r.path)
                        .iter()
                        .filter(|si| si.user_messages > 0)
                        .count();
                    let live = agents
                        .iter()
                        .filter(|a| a.repo == r.path && node.inner.live(&a.name).is_some())
                        .count();
                    json!({
                        "path": r.path.to_string_lossy(),
                        "handle": r.handle,
                        "git": crate::gitstate::get(&r.path),
                        "skip_permissions": r.skip_permissions,
                        "sessions": sessions,
                        "live": live,
                    })
                })
                .collect::<Vec<_>>()))
        }
        "node_discover" => {
            let found = node.discover_repos()?;
            Ok(json!(found
                .iter()
                .map(|(path, sessions, added)| json!({
                    "path": path, "sessions": sessions, "added": added,
                }))
                .collect::<Vec<_>>()))
        }
        "history" => {
            let g = |k: &str| body.get(k).and_then(|v| v.as_f64());
            let to = g("to").unwrap_or_else(crate::store::now_epoch);
            let from = g("from").unwrap_or(to - 86400.0);
            let n = body.get("n").and_then(|v| v.as_i64()).unwrap_or(2000);
            let agent = body.get("agent").and_then(|a| a.as_str());
            let events = node.inner.store.events(from, to, agent, n)?;
            let messages: Vec<Value> = node
                .inner
                .store
                .messages_between(from, to, n)?
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id, "uuid": m.uuid, "thread": m.thread, "sender": m.sender,
                        "recipient": m.recipient, "to_display": m.to_display, "urgency": m.urgency,
                        "body": m.body, "record": m.record_ref, "created_at": m.created_at,
                        "delivered_at": m.delivered_at, "delivered_via": m.delivered_via,
                        "ingested_at": m.ingested_at, "post": m.post,
                    })
                })
                .collect();
            Ok(json!({ "events": events, "messages": messages }))
        }
        "link_add" => {
            let g = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            let (Some(src), Some(dst)) = (g("src"), g("dst")) else {
                return Err(anyhow!("missing src/dst"));
            };
            node.inner.store.add_link(
                &src,
                &dst,
                body.get("two_way")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
                g("purpose").as_deref(),
                g("urgency").as_deref(),
            )?;
            Ok(json!({ "ok": true }))
        }
        "link_del" => {
            let g = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            let (Some(src), Some(dst)) = (g("src"), g("dst")) else {
                return Err(anyhow!("missing src/dst"));
            };
            node.inner.store.delete_link_by_ends(&src, &dst)?;
            Ok(json!({ "ok": true }))
        }
        "node_repo_skip" => {
            let path = body
                .get("path")
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("missing path"))?;
            let skip = body
                .get("skip_permissions")
                .and_then(|b| b.as_bool())
                .ok_or_else(|| anyhow!("missing skip_permissions"))?;
            node.inner.store.set_repo_skip(
                &crate::node::normalize_repo(std::path::Path::new(path)),
                skip,
            )?;
            Ok(json!({ "ok": true }))
        }
        "node_repo_rename" => {
            let path = body
                .get("path")
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("missing path"))?;
            let handle = body
                .get("handle")
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("missing handle"))?;
            let live: Vec<String> = inner.sessions.lock().unwrap().keys().cloned().collect();
            node.inner.store.rename_handle(
                &crate::node::normalize_repo(std::path::Path::new(path)),
                handle,
                &live,
            )?;
            Ok(json!({ "ok": true }))
        }
        "node_repo_forget" => {
            let path = body
                .get("path")
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("missing path"))?;
            node.inner
                .store
                .remove_repo(&crate::node::normalize_repo(std::path::Path::new(path)))?;
            Ok(json!({ "ok": true }))
        }
        "node_sessions" => {
            let repo = body
                .get("repo")
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("missing repo"))?;
            let path = std::path::Path::new(repo);
            let mcc = crate::mcc::read(path);
            let rows = crate::node::enumerate_all(&node.inner, path);
            Ok(json!(rows
                .iter()
                .map(|si| {
                    let m = mcc.get(&si.session_id);
                    json!({
                        "session_id": si.session_id,
                        "title": si.title,
                        "entrypoint": si.entrypoint,
                        "modified": si.modified_epoch,
                        "user_messages": si.user_messages,
                        "harness": si.harness,
                        "mcc_name": m.map(|m| m.name.clone()),
                        "mcc_args": m.and_then(|m| m.args.clone()),
                        "mcc_skip": m.map(|m| m.skip_permissions),
                    })
                })
                .collect::<Vec<_>>()))
        }
        "spawn" => {
            // Body is a spawn request; run it here and return the agent name.
            let name = body
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| anyhow!("missing name"))?
                .to_owned();
            let repo = body
                .get("repo")
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("missing repo"))?;
            let opts = crate::node::SpawnOpts {
                charter: body
                    .get("charter")
                    .and_then(|c| c.as_str())
                    .map(str::to_owned),
                model: body
                    .get("model")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned),
                resume: body
                    .get("resume")
                    .and_then(|r| r.as_str())
                    .map(str::to_owned),
                allow_all: body
                    .get("allow_all")
                    .and_then(|a| a.as_bool())
                    .unwrap_or(false),
                interactive: true,
                skip_permissions: body.get("skip_permissions").and_then(|s| s.as_bool()),
                extra_args: body
                    .get("extra_args")
                    .and_then(|a| a.as_str())
                    .filter(|a| !a.trim().is_empty())
                    .map(str::to_owned),
                resume_choice: body
                    .get("resume_choice")
                    .and_then(|a| a.as_str())
                    .map(str::to_owned),
                harness: body.get("harness").and_then(|h| h.as_str()).and_then(aspen_core::Harness::parse),
                posture: body.get("posture").and_then(|h| h.as_str()).and_then(aspen_core::Posture::parse),
                ..Default::default()
            };
            let ack = body.get("acknowledge_trust").and_then(|a| a.as_bool()) == Some(true);
            let repo_path = crate::node::normalize_repo(std::path::Path::new(repo));
            let (autorun, trusted) = node.trust_state(&repo_path);
            if ack {
                let _ = node.record_trust(&repo_path);
            } else if !trusted && autorun.has_autorun {
                // Mirror the local 428 as a structured error the caller maps
                // back to a trust prompt.
                return Ok(json!({
                    "trust_required": true,
                    "autorun": autorun,
                }));
            }
            let sess = match node
                .spawn_agent(&name, std::path::PathBuf::from(repo), opts)
                .await
            {
                Ok(s) => s,
                // Mirror the local 409 as a structured reply the caller maps
                // back to the fork / in-place question.
                Err(e) => {
                    if let Some(le) = e.downcast_ref::<crate::node::LiveElsewhere>() {
                        return Ok(json!({
                            "live_elsewhere": { "session": le.session, "written_ago_secs": le.written_ago_secs },
                        }));
                    }
                    return Err(e);
                }
            };
            let key = sess.name.clone();
            if let Some(title) = body
                .get("title")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
            {
                let _ = node.inner.store.set_agent_title(&key, Some(title));
            }
            Ok(json!({ "name": key }))
        }
        // ------------------------------------------------- servicing ops
        // Control-class (a peer makes this machine fetch and run a binary,
        // from the release channel this machine verifies itself). Own-mesh
        // peers only — the `service` capability once the capability layer
        // exists (DESIGN §8.1).
        "node_evacuate" => {
            let to = body.get("to").and_then(|t| t.as_str()).unwrap_or("");
            let by = body.get("by").and_then(|b| b.as_str()).unwrap_or("peer");
            let st = crate::servicing::evacuate(inner, to, by)?;
            Ok(serde_json::to_value(st)?)
        }
        "session_preflight" => node.session_preflight(agent),
        "node_preflight_target" => {
            // Can this node receive the session described by `spec`?
            let spec: crate::migrate::AgentSpec = serde_json::from_value(body.get("spec").cloned().unwrap_or(Value::Null))?;
            let counterpart = crate::migrate::find_counterpart(&inner.store, &spec);
            let adapter = inner.adapters.get(&spec.harness);
            Ok(json!({
                "counterpart": counterpart.map(|p| p.to_string_lossy().into_owned()),
                "harness": adapter.and_then(|a| a.version()),
                "harness_name": spec.harness,
                "state": inner.servicing.state().name(),
                "accepting": inner.servicing.accepting_spawns() && adapter.is_some(),
                "error": if adapter.is_none() { Some(format!("{} is not available on this node", spec.harness)) } else { None },
            }))
        }
        "node_update" => {
            let when = body.get("when").and_then(|w| w.as_str()).unwrap_or("quiet");
            let by = body
                .get("by")
                .and_then(|b| b.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| "peer".into());
            let st = crate::servicing::request(inner, when, &by)?;
            Ok(serde_json::to_value(st)?)
        }
        "node_update_cancel" => {
            let by = body.get("by").and_then(|b| b.as_str()).unwrap_or("peer");
            let cancelled = crate::servicing::cancel(inner, by)?;
            Ok(json!({ "ok": true, "cancelled": cancelled }))
        }
        "node_update_check" => {
            let r = crate::servicing::check_async(inner.clone()).await?;
            Ok(
                json!({ "ok": true, "latest": r.version, "behind": inner.servicing.newer().is_some() }),
            )
        }
        "node_update_status" => Ok(crate::servicing::status_json(inner)),
        "adoptions" => Ok(json!(inner
            .store
            .adoptions(true)?
            .iter()
            .map(crate::adoption::adoption_json)
            .collect::<Vec<_>>())),
        "adoption" => {
            let id = body
                .get("id")
                .and_then(|i| i.as_i64())
                .ok_or_else(|| anyhow!("missing id"))?;
            let how = body
                .get("action")
                .and_then(|a| a.as_str())
                .ok_or_else(|| anyhow!("missing action"))?;
            crate::adoption::resolve(&node, id, how, body.get("name").and_then(|n| n.as_str()))
                .await
        }
        "node_update_policy" => {
            let policy: crate::settings::UpdateSettings =
                serde_json::from_value(body.get("policy").cloned().unwrap_or(Value::Null))
                    .map_err(|e| anyhow!("bad policy: {e}"))?;
            policy.validate()?;
            let dir = inner
                .data_dir
                .as_deref()
                .ok_or_else(|| anyhow!("node has no data dir"))?;
            let mut s = crate::settings::load(dir);
            s.update = policy;
            crate::settings::save(dir, &s)?;
            Ok(json!({ "ok": true }))
        }
        "node_logs" => {
            let n = body.get("lines").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
            let dir = inner
                .data_dir
                .as_deref()
                .ok_or_else(|| anyhow!("node has no data dir"))?;
            Ok(json!({ "lines": crate::servicing::tail_log(dir, n) }))
        }
        other => Err(anyhow!("unknown remote op {other:?}")),
    }
}

// ----------------------------------------------------------------- dialing

/// Dial every configured peer with a URL, forever, with backoff. Idempotent:
/// safe to call again after the config changes — only peers without a
/// running dialer get one, the roster ticker starts once, and the relay
/// client starts when a relay is (newly) configured.
pub fn ensure_dialers(inner: Arc<NodeInner>) {
    let Some(mesh) = inner.mesh() else { return };
    for peer in mesh.peers() {
        let name = peer.cert.node.clone();
        // Candidates: the configured URL, then whatever the peer advertises.
        if dial_candidates(&mesh, &name).is_empty() {
            continue;
        }
        if !mesh.dialing.lock().unwrap().insert(name.clone()) {
            continue; // already dialing this peer
        }
        let inner = inner.clone();
        tokio::spawn(async move {
            loop {
                // Stop when the mesh is gone or this peer was removed.
                let Some(m) = inner.mesh() else { break };
                if !m.peers().iter().any(|p| p.cert.node == name) {
                    m.dialing.lock().unwrap().remove(&name);
                    break;
                }
                let candidates = dial_candidates(&m, &name);
                if candidates.is_empty() {
                    m.dialing.lock().unwrap().remove(&name);
                    break;
                }
                // Only one DIRECT link per peer; a relay link doesn't stop
                // us — a direct one supersedes it (run_link handles that).
                let already = m
                    .link_kind
                    .lock()
                    .unwrap()
                    .get(&name)
                    .is_some_and(|k| k == "direct");
                // Best candidate first; none = all backing off. The
                // configured URL is retried at least every minute, a
                // learned one may sleep up to ten.
                let configured = m
                    .peers()
                    .iter()
                    .find(|p| p.cert.node == name)
                    .and_then(|p| p.url.clone());
                let ready = m.order_urls(&candidates);
                if !already && !ready.is_empty() {
                    let url = ready[0].clone();
                    let cap = if configured.as_deref() == Some(url.as_str()) {
                        BACKOFF_CAP_CONFIGURED
                    } else {
                        BACKOFF_CAP_LEARNED
                    };
                    let dialed =
                        tokio::time::timeout(DIAL_TIMEOUT, tokio_tungstenite::connect_async(&url))
                            .await
                            .map_err(|_| {
                                anyhow!("connect timed out after {}s", DIAL_TIMEOUT.as_secs())
                            })
                            .and_then(|r| r.map_err(|e| anyhow!("{e}")));
                    match dialed {
                        Ok((ws, _)) => {
                            m.url_ok(&url);
                            tracing::info!(peer = %name, url = %url, "direct dial connected");
                            let (mut sink, mut stream) = ws.split();
                            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
                            let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
                            let writer = tokio::spawn(async move {
                                while let Some(f) = out_rx.recv().await {
                                    if sink
                                        .send(tokio_tungstenite::tungstenite::Message::text(f))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            });
                            let reader = tokio::spawn(async move {
                                while let Some(Ok(msg)) = stream.next().await {
                                    if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
                                        if in_tx.send(t.to_string()).is_err() {
                                            break;
                                        }
                                    }
                                }
                            });
                            let _ = run_link(inner.clone(), out_tx, in_rx).await;
                            writer.abort();
                            reader.abort();
                        }
                        Err(e) => {
                            let n = m.url_failed(&url, &e.to_string(), cap);
                            // First failure of a URL is news; the rest are
                            // the backoff doing its job.
                            if n == 1 {
                                tracing::info!(peer = %name, url = %url, error = %e, "direct dial failed; backing off");
                            } else {
                                tracing::debug!(peer = %name, url = %url, fails = n, error = %e, "direct dial failed");
                            }
                            // A relay link is fine; only complain when
                            // there is no link at all.
                            if !m.link_up(&name) {
                                m.note(&name, |h| {
                                    h.last_error = Some(format!("dial {url} failed: {e}"));
                                    h.last_error_at = Some(crate::store::now_epoch());
                                });
                            }
                        }
                    }
                }
                // Pace: a relay link carrying the peer → probe for a direct
                // one every 30s; otherwise as soon as a candidate is out of
                // backoff (at least 1s, at most 30s between looks).
                // Decide on a fresh candidate list: the peer's advertisement
                // usually lands over the relay link during the first dial.
                let carried = inner.mesh().is_some_and(|m| m.link_up(&name));
                let (untried, next_in) = inner
                    .mesh()
                    .map(|m| {
                        let fresh = dial_candidates(&m, &name);
                        (m.has_untried(&fresh), m.next_try_in(&fresh))
                    })
                    .unwrap_or((false, 5.0));
                let wait = if carried && !untried {
                    30.0
                } else {
                    next_in.clamp(1.0, 30.0)
                };
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
            }
        });
    }
    // Periodic roster refresh so turn states stay roughly current.
    if !mesh
        .ticker_started
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        let inner2 = inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                broadcast_roster(&inner2);
            }
        });
    }

    // Keep a client on every configured rendezvous relay — the fallback
    // for peers with no direct path, and the mailbox for peers that are
    // offline. A client whose URL is removed from the config exits.
    let mut relay_urls = mesh.relay_urls();
    for u in mesh.discovered_relays.lock().unwrap().keys() {
        if !relay_urls.contains(u) {
            relay_urls.push(u.clone());
        }
    }
    for relay_url in relay_urls {
        if mesh.relay_running.lock().unwrap().insert(relay_url.clone()) {
            spawn_relay_client(inner.clone(), relay_url);
        }
    }
    // Pending mail for peers with no live link: hand it to a relay mailbox,
    // and re-hand it if it stays pending (mailbox full/lost, peer never
    // present). The delivery engine does the same on every tick.
    if !mesh
        .mail_ticker_started
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        let inner3 = inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                for r in inner3.store.pending_recipients().unwrap_or_default() {
                    inner3.tick_delivery(&r);
                }
            }
        });
    }
}

/// Direct dial URLs for a peer: the configured one first, then what the
/// peer advertises (deduped, never our own address).
pub fn dial_candidates(mesh: &MeshState, peer: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(u) = mesh
        .peers()
        .iter()
        .find(|p| p.cert.node == peer)
        .and_then(|p| p.url.clone())
    {
        out.push(u);
    }
    if let Some(a) = mesh
        .health
        .lock()
        .unwrap()
        .get(peer)
        .and_then(|h| h.advertised.clone())
    {
        for u in a.dial_urls {
            if !out.contains(&u) {
                out.push(u);
            }
        }
    }
    out
}

/// Maintain one relay connection, muxing per-peer federation links over it.
fn spawn_relay_client(inner: Arc<NodeInner>, relay_url: String) {
    tokio::spawn(async move {
        loop {
            let Some(mesh) = inner.mesh() else { break };
            let wanted = mesh.relay_urls().contains(&relay_url)
                || mesh
                    .discovered_relays
                    .lock()
                    .unwrap()
                    .contains_key(&relay_url);
            if !wanted {
                mesh.relay_running.lock().unwrap().remove(&relay_url);
                break;
            }
            // The relay under every name we know for it: the configured
            // URL plus the other addresses its host advertises (one relay,
            // many names — a hostname that resolves badly from here must
            // not keep us off a relay reachable by IP).
            let cands = relay_candidates(&mesh, &relay_url);
            let ready = mesh.order_urls(&cands);
            let Some(dial) = ready.first().cloned() else {
                let wait = mesh.next_try_in(&cands).clamp(1.0, 60.0);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
                continue;
            };
            let cap = if dial == relay_url {
                BACKOFF_CAP_CONFIGURED
            } else {
                BACKOFF_CAP_LEARNED
            };
            match relay_session(&inner, &relay_url, &dial).await {
                Ok(()) => {
                    // Clean end (relay closed): try again shortly.
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                Err(e) => {
                    let was_up = mesh.relay_up.lock().unwrap().contains_key(&relay_url);
                    let n = mesh.url_failed(&dial, &e.to_string(), cap);
                    if n == 1 || was_up {
                        tracing::info!(relay = %relay_url, dial = %dial, error = %e, "relay session ended; backing off");
                    } else {
                        tracing::debug!(relay = %relay_url, dial = %dial, fails = n, error = %e, "relay dial failed");
                    }
                    mesh.note_relay(&relay_url, Some(format!("{e}")));
                    let wait = mesh.next_try_in(&cands).clamp(1.0, 60.0);
                    tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
                }
            }
        }
    });
}

/// Pull a peer's boards and merge them (newer wins per id, tombstones
/// included). If anything changed, our digest changes and the next roster
/// lets everyone else pull from us.
async fn sync_boards_from(inner: &Arc<NodeInner>, peer: &str) {
    let Some(mesh) = inner.mesh() else { return };
    let Ok(v) = mesh
        .api_call(
            peer,
            "boards",
            "",
            json!({}),
            std::time::Duration::from_secs(20),
        )
        .await
    else {
        return;
    };
    let Ok(boards) = serde_json::from_value::<Vec<crate::store::Board>>(v) else {
        return;
    };
    let mut changed = false;
    for b in &boards {
        if inner.store.upsert_board(b).unwrap_or(false) {
            changed = true;
        }
    }
    if changed {
        tracing::info!(peer, n = boards.len(), "boards synced from peer");
        broadcast_roster(inner);
    }
}

async fn sync_templates_from(inner: &Arc<NodeInner>, peer: &str) {
    let Some(mesh) = inner.mesh() else { return };
    let Ok(v) = mesh
        .api_call(peer, "templates", "", json!({}), std::time::Duration::from_secs(20))
        .await
    else {
        return;
    };
    let Ok(rows) = serde_json::from_value::<Vec<crate::store::Template>>(v) else {
        return;
    };
    let mut changed = false;
    for t in &rows {
        if inner.store.upsert_template(t).unwrap_or(false) {
            changed = true;
        }
    }
    if changed {
        tracing::info!(peer, n = rows.len(), "templates synced from peer");
        broadcast_roster(inner);
    }
}

/// Pull a peer's plugin registry and merge (newer wins per row). A change
/// re-broadcasts our digest and triggers a sync so activated plugins get
/// cached here.
async fn sync_plugin_registry_from(inner: &Arc<NodeInner>, peer: &str) {
    let Some(mesh) = inner.mesh() else { return };
    let Ok(v) = mesh
        .api_call(
            peer,
            "plugin_registry",
            "",
            json!({}),
            std::time::Duration::from_secs(20),
        )
        .await
    else {
        return;
    };
    let mut changed = false;
    if let Some(ms) = v
        .get("marketplaces")
        .and_then(|m| serde_json::from_value::<Vec<crate::plugins::Marketplace>>(m.clone()).ok())
    {
        for m in &ms {
            if inner.store.upsert_marketplace(m).unwrap_or(false) {
                changed = true;
                if m.deleted {
                    let _ = crate::plugins::remove_marketplace(inner, &m.name);
                }
            }
        }
    }
    if let Some(rs) = v
        .get("rules")
        .and_then(|r| serde_json::from_value::<Vec<crate::plugins::Rule>>(r.clone()).ok())
    {
        for r in &rs {
            if inner.store.upsert_plugin_rule(r).unwrap_or(false) {
                changed = true;
            }
        }
    }
    if changed {
        tracing::info!(peer, "plugin registry synced from peer");
        broadcast_roster(inner);
        // spawn_blocking runs at once; the handle is not awaited here.
        drop(crate::plugins::spawn_sync(inner.clone(), None));
    }
}

/// Every URL we know for the relay at `relay_url`: itself, then the other
/// relay URLs advertised by whichever peer advertises this one (that peer
/// hosts it). Order is by reach memory at dial time.
pub fn relay_candidates(mesh: &MeshState, relay_url: &str) -> Vec<String> {
    let mut out = vec![relay_url.to_owned()];
    let health = mesh.health.lock().unwrap();
    for h in health.values() {
        if let Some(a) = &h.advertised {
            if a.relay_urls.iter().any(|u| u == relay_url) {
                for u in &a.relay_urls {
                    if !out.contains(u) {
                        out.push(u.clone());
                    }
                }
            }
        }
    }
    out
}

/// One relay session. `relay_url` is the relay's identity (the configured
/// or discovered URL, which keys every table); `dial` is the address used
/// this time — the same, or an alternate name for the same host.
async fn relay_session(inner: &Arc<NodeInner>, relay_url: &str, dial: &str) -> Result<()> {
    use aspen_wire::relay::{Challenge, Register};

    let mesh = inner.mesh().ok_or_else(|| anyhow!("no mesh"))?.clone();
    let (ws, _) = tokio::time::timeout(DIAL_TIMEOUT, tokio_tungstenite::connect_async(dial))
        .await
        .map_err(|_| anyhow!("connect timed out after {}s", DIAL_TIMEOUT.as_secs()))??;
    mesh.url_ok(dial);
    if dial != relay_url {
        tracing::info!(relay = %relay_url, dial = %dial, "relay reached under an alternate address");
    }
    let (mut sink, mut stream) = ws.split();

    // Challenge → Register.
    let first = stream
        .next()
        .await
        .ok_or_else(|| anyhow!("relay closed before challenge"))??;
    let challenge: Challenge = serde_json::from_str(first.to_text()?)?;
    let relay_mesh = mesh.relay_mesh(relay_url);
    let cert = mesh
        .identity
        .cert_for(&relay_mesh)
        .cloned()
        .ok_or_else(|| anyhow!("node not certified in mesh {relay_mesh}"))?;
    let reg = Register {
        mesh: relay_mesh.clone(),
        node: mesh.identity.node.clone(),
        challenge_sig: mesh
            .identity
            .sign_relay_challenge(&relay_mesh, &challenge.nonce)?,
        cert,
    };
    sink.send(tokio_tungstenite::tungstenite::Message::text(
        serde_json::to_string(&reg)?,
    ))
    .await?;

    let now = crate::store::now_epoch();
    *mesh.relay_connected_at.lock().unwrap() = Some(now);
    mesh.note_relay(relay_url, None);

    // Mux: outbound relay frames + per-peer inbound channels.
    let (relay_tx, mut relay_rx) = mpsc::unbounded_channel::<String>();
    let peer_ins: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    mesh.relay_up
        .lock()
        .unwrap()
        .insert(relay_url.to_owned(), (now, relay_tx.clone()));
    mesh.relay_sessions.lock().unwrap().insert(
        relay_url.to_owned(),
        RelaySession {
            tx: relay_tx.clone(),
            peer_ins: peer_ins.clone(),
            present: std::collections::HashSet::new(),
            host: None,
        },
    );
    // Anything pending for peers we have no link to can go to the mailbox.
    for r in inner.store.pending_recipients().unwrap_or_default() {
        inner.tick_delivery(&r);
    }

    // Writer task: everything queued for the relay socket.
    let writer = tokio::spawn(async move {
        while let Some(frame) = relay_rx.recv().await {
            if sink
                .send(tokio_tungstenite::tungstenite::Message::text(frame))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Which peers we dial vs accept: lower node name dials, to avoid double
    // links (both sides otherwise start one).
    let me = mesh.identity.node.clone();

    let result = relay_read_loop(
        inner,
        &mesh,
        &me,
        relay_url,
        &relay_tx,
        &peer_ins,
        &mut stream,
    )
    .await;
    mesh.relay_sessions.lock().unwrap().remove(relay_url);
    {
        let mut up = mesh.relay_up.lock().unwrap();
        up.remove(relay_url);
        if up.is_empty() {
            *mesh.relay_connected_at.lock().unwrap() = None;
        }
    }
    writer.abort();
    // Drop all per-peer links routed over this relay.
    peer_ins.lock().unwrap().clear();
    result
}

#[allow(clippy::too_many_arguments)]
async fn relay_read_loop(
    inner: &Arc<NodeInner>,
    mesh: &Arc<MeshState>,
    me: &str,
    relay_url: &str,
    relay_tx: &mpsc::UnboundedSender<String>,
    peer_ins: &Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
    stream: &mut (impl futures_util::Stream<
        Item = std::result::Result<
            tokio_tungstenite::tungstenite::Message,
            tokio_tungstenite::tungstenite::Error,
        >,
    > + Unpin),
) -> Result<()> {
    use aspen_wire::relay::RelayFrame;

    // Keepalive: a relay that restarts (a worker deploy, a host bounce)
    // drops our socket without a close frame; TCP alone never tells us.
    // Ping every 20s; anything inbound within 45s keeps the session, else
    // it is dead and we reconnect (and re-register, and re-link).
    let pinger = {
        let tx = relay_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(RELAY_PING_SECS)).await;
                if tx.send("ping".into()).is_err() {
                    break;
                }
            }
        })
    };
    let result = loop {
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(RELAY_SILENCE_SECS),
            stream.next(),
        )
        .await;
        let msg = match next {
            Ok(Some(m)) => m,
            Ok(None) => break Ok(()),
            Err(_) => {
                break Err(anyhow!(
                    "relay silent for {RELAY_SILENCE_SECS}s (no pong) — reconnecting"
                ))
            }
        };
        let msg = match msg {
            Ok(m) => m,
            Err(e) => break Err(e.into()),
        };
        let text = match msg.to_text() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if text == "pong" || text == "ping" {
            continue;
        }
        let frame: RelayFrame = match serde_json::from_str(text) {
            Ok(f) => f,
            Err(_) => continue,
        };
        match frame {
            RelayFrame::Welcome { peers, host } => {
                // The same relay under another address? Keep the session
                // that got there first; this one is a duplicate — and if
                // it came from discovery, forget the URL.
                if let Some(h) = &host {
                    let dup = mesh
                        .relay_sessions
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|(u, s)| u != relay_url && s.host.as_deref() == Some(h.as_str()));
                    if dup {
                        mesh.discovered_relays.lock().unwrap().remove(relay_url);
                        return Err(anyhow!(
                            "relay at {relay_url} is node '{h}', already reached by another session"
                        ));
                    }
                }
                if let Some(s) = mesh.relay_sessions.lock().unwrap().get_mut(relay_url) {
                    s.present = peers.iter().cloned().collect();
                    s.host = host;
                }
                for p in peers {
                    if me < p.as_str() && !p.starts_with("console-") && !mesh.link_up(&p) {
                        start_relay_link(inner, me, &p, relay_url, relay_tx, peer_ins);
                    }
                }
            }
            RelayFrame::Presence { node, online } => {
                if let Some(s) = mesh.relay_sessions.lock().unwrap().get_mut(relay_url) {
                    if online {
                        s.present.insert(node.clone());
                    } else {
                        s.present.remove(&node);
                    }
                }
                if online {
                    if me < node.as_str() && !mesh.link_up(&node) {
                        start_relay_link(inner, me, &node, relay_url, relay_tx, peer_ins);
                    }
                } else {
                    // Gone: drop the link riding this relay now, so pending
                    // mail takes the mailbox instead of a dead session.
                    if peer_ins.lock().unwrap().remove(&node).is_some() {
                        tracing::info!(peer = %node, "peer left the relay; link closed");
                    }
                }
            }
            RelayFrame::Route {
                from: Some(from),
                data,
                ..
            } => {
                // Only a HELLO may start a link. A stray mid-handshake or
                // sealed frame (from a session that just died) used to
                // spawn a fresh link that then choked on it — and its
                // failure produced the next stray frame: a cascade of
                // dead links every 100 ms (seen live). Now: a hello from a
                // peer that dials us replaces whatever link we had with
                // it (it is telling us it started over); a hello from a
                // peer WE dial is a crossing — ignore it, our attempt is
                // the one that counts; anything else goes to the live
                // link or the floor.
                let is_hello = serde_json::from_str::<Hello>(&data).is_ok();
                let existing = peer_ins.lock().unwrap().get(&from).cloned();
                // A console peer always dials us (RELAY.md §11), whatever
                // its name sorts as.
                let i_dial = me < from.as_str() && !from.starts_with("console-");
                match (is_hello, existing) {
                    // We dial this peer: its hello is the reply to our
                    // attempt (feed it) — or, with no attempt in flight, a
                    // stray from an older session (ignore; presence drives
                    // our next attempt).
                    (true, Some(tx)) if i_dial => {
                        let _ = tx.send(data);
                    }
                    (true, None) if i_dial => {
                        tracing::info!(peer = %from, "hello from a peer we dial with no attempt in flight; ignored");
                    }
                    (true, existing) => {
                        if existing.is_some() {
                            tracing::info!(peer = %from, "peer restarted its relay link; replacing ours");
                            peer_ins.lock().unwrap().remove(&from);
                        }
                        let tx = start_relay_link(inner, me, &from, relay_url, relay_tx, peer_ins);
                        let _ = tx.send(data);
                    }
                    (false, Some(tx)) => {
                        let _ = tx.send(data);
                    }
                    (false, None) => {
                        tracing::debug!(peer = %from, "stray relay frame with no link; dropped");
                    }
                }
            }
            RelayFrame::Undeliverable { to } => {
                peer_ins.lock().unwrap().remove(&to);
            }
            RelayFrame::Rejected { reason } => {
                return Err(anyhow!("relay rejected this node: {reason}"));
            }
            RelayFrame::Mail { from, id, data } => {
                receive_mail(inner, mesh, &from, &id, &data);
            }
            RelayFrame::MailboxFull { to } => {
                tracing::warn!(peer = %to, "relay mailbox full; rows stay pending");
                // Forget our hand-off so the next tick tries again later.
                mesh.mailed
                    .lock()
                    .unwrap()
                    .retain(|_, at| crate::store::now_epoch() - *at > MAIL_RETRY_SECS);
            }
            _ => {}
        }
    };
    pinger.abort();
    result
}

/// A link handshake (hello → auth) must complete within this.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Relay keepalive cadence (docs/RELAY.md §8).
const RELAY_PING_SECS: u64 = 20;
const RELAY_SILENCE_SECS: u64 = 45;

/// How long a handed-off row waits before being handed off again if it is
/// still pending (the mailbox was full, the relay lost it, the ack got lost).
const MAIL_RETRY_SECS: f64 = 10.0 * 60.0;

/// Seal `payload` to `peer` and hand it to a connected relay's mailbox.
/// Err when no relay is up or the peer's cert isn't on file.
pub fn mailbox_send(mesh: &MeshState, peer: &str, id: &str, payload: &Value) -> Result<()> {
    use aspen_wire::relay::RelayFrame;
    let cert = mesh
        .peer_cert(peer)
        .ok_or_else(|| anyhow!("no cert on file for node {peer:?}"))?;
    let env = SealedEnvelope::seal(&mesh.identity, &cert, payload.to_string().as_bytes())?;
    let frame = serde_json::to_string(&RelayFrame::Store {
        to: peer.to_owned(),
        id: id.to_owned(),
        data: serde_json::to_string(&env)?,
    })?;
    let up = mesh.relay_up.lock().unwrap();
    let (_, tx) = up
        .values()
        .next()
        .ok_or_else(|| anyhow!("no relay connected"))?;
    tx.send(frame).map_err(|_| anyhow!("relay writer closed"))
}

/// Pending rows for `recipient` (homed on `node`, no live link): hand each
/// to a relay mailbox, at most once per MAIL_RETRY_SECS per row. Rows stay
/// pending until the home node's bus_ack arrives — by link or by mail.
pub fn mail_pending(inner: &Arc<NodeInner>, recipient: &str, node: &str) {
    let Some(mesh) = inner.mesh() else { return };
    if mesh.relay_up.lock().unwrap().is_empty() {
        return;
    }
    let Ok(pending) = inner.store.pending_for(recipient) else {
        return;
    };
    let now = crate::store::now_epoch();
    for m in &pending {
        {
            let mut mailed = mesh.mailed.lock().unwrap();
            if mailed
                .get(&m.uuid)
                .is_some_and(|at| now - at < MAIL_RETRY_SECS)
            {
                continue;
            }
            mailed.insert(m.uuid.clone(), now);
        }
        if let Err(e) = mailbox_send(&mesh, node, &m.uuid, &bus_payload(m, node)) {
            tracing::debug!(peer = %node, error = %e, "mailbox hand-off failed; stays pending");
            mesh.mailed.lock().unwrap().remove(&m.uuid);
            return;
        }
        tracing::info!(peer = %node, uuid = %m.uuid, "bus row handed to relay mailbox");
    }
}

/// A mailbox delivery: open it with the sender's cert (on file — a peer we
/// have met) and treat it as the bus / bus_ack frame it is.
fn receive_mail(inner: &Arc<NodeInner>, mesh: &Arc<MeshState>, from: &str, id: &str, data: &str) {
    let Some(cert) = mesh.peer_cert(from) else {
        tracing::warn!(peer = %from, "mail from a node with no cert on file; dropped");
        return;
    };
    let env: SealedEnvelope = match serde_json::from_str(data) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(peer = %from, error = %e, "unparseable mail dropped");
            return;
        }
    };
    let payload: Value = match env
        .open(&mesh.identity, &cert)
        .and_then(|b| Ok(serde_json::from_slice(&b)?))
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(peer = %from, error = %e, "mail envelope rejected");
            return;
        }
    };
    match payload.get("t").and_then(|t| t.as_str()).unwrap_or("") {
        "bus" => {
            let uuid = payload.get("uuid").and_then(|u| u.as_str()).unwrap_or("");
            let sender = payload
                .get("sender")
                .and_then(|s| s.as_str())
                .unwrap_or("?");
            let sender = if sender == "operator" || crate::addr::node_of(sender).is_some() {
                sender.to_owned()
            } else {
                format!("{sender}@{from}")
            };
            let recipient = payload
                .get("recipient")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let g = |k: &str| payload.get(k).and_then(|s| s.as_str());
            let inserted = inner.store.insert_federated(
                uuid,
                &sender,
                recipient,
                g("to_display").unwrap_or(""),
                g("urgency").unwrap_or("normal"),
                g("body").unwrap_or(""),
                g("thread"),
                g("record_ref"),
                payload.get("created_at").and_then(|c| c.as_f64()),
            );
            match inserted {
                Ok(_) => {
                    inner.tick_delivery(recipient);
                    tracing::info!(peer = %from, uuid, "bus row received by mail");
                }
                Err(e) => tracing::warn!(peer = %from, error = %e, "mail insert failed"),
            }
            // Ack by link if one is up, else back through the mailbox.
            let ack = json!({ "t": "bus_ack", "uuid": uuid });
            if mesh.send_to(from, &ack).is_err() {
                let _ = mailbox_send(mesh, from, &format!("ack:{id}"), &ack);
            }
        }
        "bus_ack" => {
            if let Some(uuid) = payload.get("uuid").and_then(|u| u.as_str()) {
                let _ = inner
                    .store
                    .mark_delivered_by_uuid(uuid, &format!("federated:{from} (mail)"));
                mesh.mailed.lock().unwrap().remove(uuid);
            }
        }
        other => tracing::debug!(peer = %from, other, "unknown mail payload ignored"),
    }
}

/// Start one federation link that rides the relay to `peer`: wrap outbound
/// frames as Route{to:peer}, feed inbound Route data in. Returns the inbound
/// sender registered for the peer.
fn start_relay_link(
    inner: &Arc<NodeInner>,
    _me: &str,
    peer: &str,
    relay_url: &str,
    relay_tx: &mpsc::UnboundedSender<String>,
    peer_ins: &Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
) -> mpsc::UnboundedSender<String> {
    use aspen_wire::relay::RelayFrame;

    // The link about to come up is a relay one; run_link records the kind
    // it finds pending here when the hello completes.
    inner.mesh().map(|m| {
        m.link_kind
            .lock()
            .unwrap()
            .insert(format!("pending:{peer}"), format!("relay:{relay_url}"))
    });
    let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    peer_ins
        .lock()
        .unwrap()
        .insert(peer.to_owned(), in_tx.clone());

    // Bridge this link's outbound frames into relay Route envelopes.
    let relay_tx2 = relay_tx.clone();
    let peer2 = peer.to_owned();
    tokio::spawn(async move {
        while let Some(data) = out_rx.recv().await {
            let framed = serde_json::to_string(&RelayFrame::Route {
                to: Some(peer2.clone()),
                from: None,
                data,
            })
            .unwrap();
            if relay_tx2.send(framed).is_err() {
                break;
            }
        }
    });

    let inner2 = inner.clone();
    let peer3 = peer.to_owned();
    let peer_ins2 = peer_ins.clone();
    tracing::info!(peer = %peer, relay = %relay_url, "relay link starting");
    tokio::spawn(async move {
        if let Err(e) = run_link(inner2, out_tx, in_rx).await {
            tracing::info!(peer = %peer3, error = %e, "relay link ended");
        }
        peer_ins2.lock().unwrap().remove(&peer3);
    });
    in_tx
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn ops_are_classed() {
        assert_eq!(op_capability("transcript"), Capability::Observe);
        assert_eq!(op_capability("http"), Capability::Observe);
        assert_eq!(op_capability("message"), Capability::Control);
        assert_eq!(op_capability("spawn"), Capability::Spawn);
        assert_eq!(op_capability("adoption"), Capability::Trust);
        assert_eq!(op_capability("something_new"), Capability::Control);
    }

    #[test]
    fn policy_and_console_grants() {
        let root = aspen_wire::identity::MeshRoot::create("home");
        let mut me = NodeIdentity::create("me");
        me.install_cert(root.certify(&me.join_request()).unwrap()).unwrap();
        let work_root = aspen_wire::identity::MeshRoot::create("work");
        let peer = NodeIdentity::create("w1");
        let peer_cert = work_root.certify(&peer.join_request()).unwrap();
        let cfg = MeshConfig { mesh: "home".into(), root_public: root.root_public.clone(), peers: vec![], relay: None, relays: vec![], policy: None };
        let st = MeshState::new(me, cfg);
        st.extra.write().unwrap().push(MeshConfig {
            mesh: "work".into(),
            root_public: work_root.root_public.clone(),
            peers: vec![crate::mesh::PeerConfig { cert: peer_cert, url: None }],
            relay: None,
            relays: vec![],
            policy: None,
        });
        assert_eq!(st.mesh_of_peer("w1").as_deref(), Some("work"));
        assert_eq!(st.policy_of("work"), "observe");
        assert!(st.allows("w1", Capability::Observe));
        assert!(!st.allows("w1", Capability::Control));
        st.extra.write().unwrap()[0].policy = Some("full".into());
        assert!(st.allows("w1", Capability::Spawn));
        assert!(st.allows("console-abc", Capability::Control));
        assert!(!st.allows("console-abc", Capability::Spawn));
    }
}

#[cfg(test)]
mod hello_tests {
    #[test]
    fn sealed_envelope_is_not_a_hello() {
        let env = serde_json::json!({
            "v": 1, "from": "a", "to": "b", "nonce": "AAAA", "ciphertext": "AAAA", "sig": "AAAA"
        })
        .to_string();
        assert!(serde_json::from_str::<super::Hello>(&env).is_err());
    }
}
