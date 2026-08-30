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
}

pub struct MeshState {
    pub identity: NodeIdentity,
    pub config: MeshConfig,
    /// node name → sender of already-serialized wire frames.
    pub links: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    /// node name → last roster it sent us.
    pub remote: Mutex<HashMap<String, Vec<RemoteAgent>>>,
}

impl MeshState {
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
        let env = SealedEnvelope::seal(
            &self.identity,
            &cert,
            payload.to_string().as_bytes(),
        )?;
        let frame = serde_json::to_string(&env)?;
        let links = self.links.lock().unwrap();
        let tx = links
            .get(node)
            .ok_or_else(|| anyhow!("no live link to node {node:?}"))?;
        tx.send(frame).map_err(|_| anyhow!("link to {node:?} closed"))
    }
}

// ------------------------------------------------------------------ payloads

fn bus_payload(m: &StoredMessage, dest_node: &str) -> Value {
    // A recipient qualified with the destination node travels bare — on
    // that node it IS the local name.
    let recipient = m
        .recipient
        .strip_suffix(&format!("@{dest_node}"))
        .unwrap_or(&m.recipient);
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
                "live": live.is_some(),
                "turn_state": live.map(|s| match s.turn_state() {
                    TurnState::Idle => "idle",
                    TurnState::Busy => "busy",
                }),
            })
        })
        .collect();
    json!({ "t": "roster", "agents": list })
}

/// Push the current roster to every connected peer (spawns/exits/timer).
pub fn broadcast_roster(inner: &Arc<NodeInner>) {
    let Some(mesh) = &inner.mesh else { return };
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
    let Some(mesh) = &inner.mesh else { return };
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
        .mesh
        .as_ref()
        .ok_or_else(|| anyhow!("this node has not joined a mesh"))?
        .clone();

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
        })?)
        .map_err(|_| anyhow!("link closed before hello"))?;

    // 2. Hello in: verify the peer's cert against OUR trusted root.
    let first = in_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("link closed before peer hello"))?;
    let peer_hello: Hello = serde_json::from_str(&first)?;
    let peer_cert = peer_hello.hello;
    peer_cert.verify_against(&mesh.config.root_public)?;
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
    if links
        .get(&peer)
        .is_some_and(|tx| tx.same_channel(&out_tx))
    {
        links.remove(&peer);
    }
    drop(links);
    mesh.remote.lock().unwrap().remove(&peer);
    tracing::info!(peer = %peer, "federation link down");
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
                let sender = payload.get("sender").and_then(|s| s.as_str()).unwrap_or("?");
                // Cross-node sender identity gains its home suffix unless it
                // already carries one.
                let sender = if sender.contains('@') {
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
                    payload.get("to_display").and_then(|s| s.as_str()).unwrap_or(""),
                    payload.get("urgency").and_then(|s| s.as_str()).unwrap_or("normal"),
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
                for name in names {
                    inner.tick_delivery(&name);
                }
            }
            other => tracing::debug!(peer, other, "unknown federation payload ignored"),
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- dialing

/// Dial every configured peer with a URL, forever, with backoff.
pub fn spawn_dialers(inner: Arc<NodeInner>) {
    let Some(mesh) = &inner.mesh else { return };
    for peer in &mesh.config.peers {
        let Some(url) = peer.url.clone() else { continue };
        let name = peer.cert.node.clone();
        let inner = inner.clone();
        tokio::spawn(async move {
            loop {
                // Only one live link per peer; if an inbound one exists, wait.
                let already = inner
                    .mesh
                    .as_ref()
                    .is_some_and(|m| m.link_up(&name));
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
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }
    // Periodic roster refresh so turn states stay roughly current.
    let inner2 = inner.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            broadcast_roster(&inner2);
        }
    });
}
