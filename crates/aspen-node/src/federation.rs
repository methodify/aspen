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
}

pub struct MeshState {
    pub identity: NodeIdentity,
    /// Interior-mutable so `aspen mesh peers-add` / `relay` take effect in
    /// the running daemon without a restart (see Node::reload_mesh).
    pub config: std::sync::RwLock<MeshConfig>,
    /// Peers with a dialer task already running (idempotent ensure_dialers).
    pub dialing: Mutex<std::collections::HashSet<String>>,
    /// Relay URL a client task is already maintaining, if any.
    pub relay_running: Mutex<Option<String>>,
    /// The roster ticker has been started.
    pub ticker_started: std::sync::atomic::AtomicBool,
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
    /// Epoch seconds when the relay session came up; None when down/unused.
    pub relay_connected_at: Mutex<Option<f64>>,
    /// Per-peer diagnostics for the console: why isn't X linked?
    pub health: Mutex<HashMap<String, PeerHealth>>,
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
    pub fn note(&self, peer: &str, f: impl FnOnce(&mut PeerHealth)) {
        let mut h = self.health.lock().unwrap();
        f(h.entry(peer.to_owned()).or_default());
    }
}

impl MeshState {
    pub fn new(identity: NodeIdentity, config: MeshConfig) -> Self {
        Self {
            identity,
            config: std::sync::RwLock::new(config),
            dialing: Mutex::new(std::collections::HashSet::new()),
            relay_running: Mutex::new(None),
            ticker_started: std::sync::atomic::AtomicBool::new(false),
            links: Mutex::new(HashMap::new()),
            remote: Mutex::new(HashMap::new()),
            pending_api: Mutex::new(HashMap::new()),
            served_subs: Mutex::new(HashMap::new()),
            remote_subs: Mutex::new(HashMap::new()),
            relay_connected_at: Mutex::new(None),
            health: Mutex::new(HashMap::new()),
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
    pub fn peers(&self) -> Vec<crate::mesh::PeerConfig> {
        self.config.read().unwrap().peers.clone()
    }
    pub fn relay_url(&self) -> Option<String> {
        self.config.read().unwrap().relay.clone()
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
        self.config
            .read()
            .unwrap()
            .peers
            .iter()
            .find(|p| p.cert.node == node)
            .map(|p| p.cert.clone())
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

pub fn roster_payload(inner: &Arc<NodeInner>) -> Value {
    let agents = inner.store.agents().unwrap_or_default();
    let list: Vec<Value> = agents
        .iter()
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
    json!({
        "t": "roster", "agents": list, "version": version, "sha": sha,
        "servicing": inner.servicing.roster_json(mode),
    })
}

/// Push the current roster to every connected peer (spawns/exits/timer).
pub fn broadcast_roster(inner: &Arc<NodeInner>) {
    let Some(mesh) = inner.mesh() else { return };
    let payload = roster_payload(inner);
    let peers: Vec<String> = mesh.links.lock().unwrap().keys().cloned().collect();
    for p in peers {
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
        })?)
        .map_err(|_| anyhow!("link closed before hello"))?;

    // 2. Hello in: verify the peer's cert against OUR trusted root.
    let first = in_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("link closed before peer hello"))?;
    let peer_hello: Hello = serde_json::from_str(&first)?;
    let peer_cert = peer_hello.hello;
    if let Err(e) = peer_cert.verify_against(&mesh.root_public()) {
        mesh.note(&peer_cert.node, |h| {
            h.last_error = Some(format!(
                "cert from '{}' does not verify against this mesh's root: {e}",
                peer_cert.node
            ));
            h.last_error_at = Some(crate::store::now_epoch());
        });
        return Err(e);
    }
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
        // Not fatal by design: a valid root-signed cert we don't have on
        // file yet gets recorded for the session (certs are public facts).
        tracing::info!(peer = %peer_cert.node, "peer cert not in mesh.json; trusting root signature for this session");
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

    let auth_frame = in_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("link closed before peer auth"))?;
    let env: SealedEnvelope = serde_json::from_str(&auth_frame)?;
    let payload: Value = serde_json::from_slice(&env.open(&mesh.identity, &peer_cert)?)?;
    if payload.get("t").and_then(|t| t.as_str()) != Some("auth")
        || payload.get("nonce").and_then(|n| n.as_str())
            != Some(aspen_wire::b64::encode(&my_nonce)).as_deref()
    {
        bail!("peer failed nonce proof");
    }

    // 4. Link up.
    let peer = peer_cert.node.clone();
    mesh.links
        .lock()
        .unwrap()
        .insert(peer.clone(), out_tx.clone());
    tracing::info!(peer = %peer, "federation link up");
    mesh.note(&peer, |h| {
        h.last_up = Some(crate::store::now_epoch());
        h.last_error = None;
        h.last_error_at = None;
    });
    let _ = mesh.send_to(&peer, &roster_payload(&inner));
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
    if links.get(&peer).is_some_and(|tx| tx.same_channel(&out_tx)) {
        links.remove(&peer);
    }
    drop(links);
    mesh.remote.lock().unwrap().remove(&peer);
    // Consumers of subscriptions served over this link learn immediately
    // (their channel closes) rather than waiting on silence.
    mesh.remote_subs
        .lock()
        .unwrap()
        .retain(|_, (served_by, _)| served_by != &peer);
    tracing::info!(peer = %peer, "federation link down");
    mesh.note(&peer, |h| h.last_down = Some(crate::store::now_epoch()));
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
                    mesh.note(peer, |h| {
                        h.last_roster = Some(crate::store::now_epoch());
                        if v.is_some() {
                            h.version = v;
                        }
                        if s.is_some() {
                            h.sha = s;
                        }
                        if let Some(svc) = &svc {
                            let g = |k: &str| svc.get(k).and_then(|x| x.as_str()).map(str::to_owned);
                            h.update_available = g("available");
                            h.service_state = g("state");
                            h.service_detail = g("state_detail");
                            h.policy = g("policy");
                            h.inventory = svc.get("inventory").cloned().filter(|v| !v.is_null());
                            h.last_outcome = svc.get("last_outcome").cloned().filter(|v| !v.is_null());
                        }
                    });
                }
                // Channel members from before scoped names (`main@node`)
                // can only be resolved once we see that node's roster.
                let _ = inner.store.heal_legacy_remote_members(peer, &names);
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
                tokio::spawn(async move {
                    let res = serve_api_req(&inner, &op, &agent, body).await;
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
async fn serve_api_req(
    inner: &Arc<NodeInner>,
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
            let uuid = node.send_operator_message(agent, text.to_owned()).await?;
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
            node.revive_agent(agent, true).await?;
            Ok(json!({}))
        }
        "branch" => {
            node.branch_agent(
                agent,
                body.get("label").and_then(|l| l.as_str()),
                body.get("at").and_then(|a| a.as_str()),
            )
            .await?;
            Ok(json!({}))
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
            node.resume_bookmark(agent, id).await?;
            Ok(json!({}))
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
            Ok(json!({ "prompts": prompts, "inbox": inbox }))
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
            let items = aspen_claude::transcript::rehydrate(&row.repo, sid).unwrap_or_default();
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
                    let sessions = aspen_claude::transcript::enumerate_sessions(&r.path)
                        .map_or(0, |v| v.iter().filter(|si| si.user_messages > 0).count());
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
            let rows = aspen_claude::transcript::enumerate_sessions(path)?;
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
            let sess = node
                .spawn_agent(&name, std::path::PathBuf::from(repo), opts)
                .await?;
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
            Ok(json!({ "ok": true, "latest": r.version, "behind": inner.servicing.newer().is_some() }))
        }
        "node_update_status" => Ok(crate::servicing::status_json(inner)),
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
        let Some(url) = peer.url.clone() else {
            continue;
        };
        let name = peer.cert.node.clone();
        if !mesh.dialing.lock().unwrap().insert(name.clone()) {
            continue; // already dialing this peer
        }
        let inner = inner.clone();
        tokio::spawn(async move {
            loop {
                // Only one live link per peer; if an inbound one exists, wait.
                let already = inner.mesh().is_some_and(|m| m.link_up(&name));
                if !already {
                    match tokio_tungstenite::connect_async(&url).await {
                        Ok((ws, _)) => {
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
                            tracing::debug!(peer = %name, error = %e, "dial failed");
                            if let Some(m) = inner.mesh() {
                                m.note(&name, |h| {
                                    h.last_error = Some(format!("dial {url} failed: {e}"));
                                    h.last_error_at = Some(crate::store::now_epoch());
                                });
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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

    // If a rendezvous relay is configured, keep a connection to it — the
    // universal fallback for peers with no direct path.
    if let Some(relay_url) = mesh.relay_url() {
        let mut running = mesh.relay_running.lock().unwrap();
        if running.as_deref() != Some(relay_url.as_str()) {
            *running = Some(relay_url.clone());
            spawn_relay_client(inner.clone(), relay_url);
        }
    }
}

/// Maintain one relay connection, muxing per-peer federation links over it.
fn spawn_relay_client(inner: Arc<NodeInner>, relay_url: String) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = relay_session(&inner, &relay_url).await {
                tracing::debug!(error = %e, "relay session ended");
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

async fn relay_session(inner: &Arc<NodeInner>, relay_url: &str) -> Result<()> {
    use aspen_wire::relay::{Challenge, Register};

    let mesh = inner.mesh().ok_or_else(|| anyhow!("no mesh"))?.clone();
    let (ws, _) = tokio_tungstenite::connect_async(relay_url).await?;
    let (mut sink, mut stream) = ws.split();

    // Challenge → Register.
    let first = stream
        .next()
        .await
        .ok_or_else(|| anyhow!("relay closed before challenge"))??;
    let challenge: Challenge = serde_json::from_str(first.to_text()?)?;
    let cert = mesh
        .identity
        .cert
        .clone()
        .ok_or_else(|| anyhow!("node not certified"))?;
    let reg = Register {
        mesh: mesh.mesh_name(),
        node: mesh.identity.node.clone(),
        challenge_sig: mesh
            .identity
            .sign_relay_challenge(&mesh.mesh_name(), &challenge.nonce)?,
        cert,
    };
    sink.send(tokio_tungstenite::tungstenite::Message::text(
        serde_json::to_string(&reg)?,
    ))
    .await?;

    *mesh.relay_connected_at.lock().unwrap() = Some(crate::store::now_epoch());

    // Mux: outbound relay frames + per-peer inbound channels.
    let (relay_tx, mut relay_rx) = mpsc::unbounded_channel::<String>();
    let peer_ins: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));

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

    let result = relay_read_loop(inner, &mesh, &me, &relay_tx, &peer_ins, &mut stream).await;
    *mesh.relay_connected_at.lock().unwrap() = None;
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

    while let Some(msg) = stream.next().await {
        let msg = msg?;
        let text = match msg.to_text() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let frame: RelayFrame = match serde_json::from_str(text) {
            Ok(f) => f,
            Err(_) => continue,
        };
        match frame {
            RelayFrame::Welcome { peers } => {
                for p in peers {
                    if me < p.as_str() && !mesh.link_up(&p) {
                        start_relay_link(inner, me, &p, relay_tx, peer_ins);
                    }
                }
            }
            RelayFrame::Presence { node, online } => {
                if online && me < node.as_str() && !mesh.link_up(&node) {
                    start_relay_link(inner, me, &node, relay_tx, peer_ins);
                }
            }
            RelayFrame::Route {
                from: Some(from),
                data,
                ..
            } => {
                let sender = peer_ins.lock().unwrap().get(&from).cloned();
                match sender {
                    Some(tx) => {
                        let _ = tx.send(data);
                    }
                    None => {
                        // First contact from a peer that dials us: accept.
                        let tx = start_relay_link(inner, me, &from, relay_tx, peer_ins);
                        let _ = tx.send(data);
                    }
                }
            }
            RelayFrame::Undeliverable { to } => {
                peer_ins.lock().unwrap().remove(&to);
            }
            RelayFrame::Rejected { reason } => {
                return Err(anyhow!("relay rejected this node: {reason}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Start one federation link that rides the relay to `peer`: wrap outbound
/// frames as Route{to:peer}, feed inbound Route data in. Returns the inbound
/// sender registered for the peer.
fn start_relay_link(
    inner: &Arc<NodeInner>,
    _me: &str,
    peer: &str,
    relay_tx: &mpsc::UnboundedSender<String>,
    peer_ins: &Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
) -> mpsc::UnboundedSender<String> {
    use aspen_wire::relay::RelayFrame;

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
    tokio::spawn(async move {
        let _ = run_link(inner2, out_tx, in_rx).await;
        peer_ins2.lock().unwrap().remove(&peer3);
    });
    in_tx
}
