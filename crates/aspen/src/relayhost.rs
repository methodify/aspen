//! The rendezvous relay, embedded in every node at
//! `/api/federation/relay` — the same protocol as the standalone
//! `aspen-relay` (aspen-wire::relay), served by the daemon itself.
//!
//! Why: nodes that only dial out (a laptop's Windows and WSL sides, each on
//! a loopback listener) can both reach the root but not each other. The
//! root already talks to everyone; letting it route sealed frames between
//! them makes the mesh whole without a separate process. Any node can host
//! this — it needs only the mesh's root PUBLIC key to admit members, and
//! it reads nothing it routes. It is on whenever the node is in a mesh;
//! whether anyone uses it is a matter of what they set with
//! `aspen mesh relay`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use tokio::sync::{mpsc, Mutex};

use aspen_wire::relay::{Challenge, Mailbox, Register, RelayFrame};

#[derive(Default)]
pub struct RelayHost {
    /// node name → its socket writer.
    nodes: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    /// Bus envelopes waiting for nodes that aren't here (bounded, TTL'd).
    mailbox: Mutex<Mailbox>,
}

impl RelayHost {
    pub async fn present(&self) -> Vec<String> {
        let mut v: Vec<String> = self.nodes.lock().await.keys().cloned().collect();
        v.sort();
        v
    }
    /// recipient → items waiting.
    pub async fn waiting(&self) -> HashMap<String, usize> {
        self.mailbox.lock().await.waiting()
    }
}

/// Serve one relay connection: challenge → register (verified against the
/// mesh's root) → welcome → route until it closes.
pub async fn serve(
    host: Arc<RelayHost>,
    mesh: String,
    root_pubkey: Vec<u8>,
    mut socket: WebSocket,
) {
    // 1. Challenge.
    let mut nonce = [0u8; 32];
    {
        use rand_core::RngCore;
        rand_core::OsRng.fill_bytes(&mut nonce);
    }
    let challenge = serde_json::to_string(&Challenge {
        nonce: nonce.to_vec(),
    })
    .unwrap_or_default();
    if socket.send(Message::Text(challenge.into())).await.is_err() {
        return;
    }

    // 2. Register.
    let reg: Register = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str(&t) {
                Ok(r) => break r,
                Err(e) => {
                    reject(&mut socket, &format!("bad register: {e}")).await;
                    return;
                }
            },
            Some(Ok(_)) => continue,
            _ => return,
        }
    };
    if let Err(reason) = aspen_wire::relay::verify_register(&mesh, &root_pubkey, &reg, &nonce) {
        reject(&mut socket, &reason).await;
        return;
    }
    let name = reg.node.clone();
    tracing::info!(node = %name, "relay: node registered");

    // 3. Register the connection; announce presence.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let existing: Vec<String> = {
        let mut nodes = host.nodes.lock().await;
        let peers = nodes.keys().cloned().collect();
        nodes.insert(name.clone(), tx);
        peers
    };
    let welcome =
        serde_json::to_string(&RelayFrame::Welcome { peers: existing }).unwrap_or_default();
    if socket.send(Message::Text(welcome.into())).await.is_err() {
        host.nodes.lock().await.remove(&name);
        return;
    }
    broadcast_presence(&host, &name, true).await;
    // Mail that waited for this node, oldest first.
    for item in host.mailbox.lock().await.drain(&name) {
        let frame = serde_json::to_string(&RelayFrame::Mail {
            from: item.from,
            id: item.id,
            data: item.data,
        })
        .unwrap_or_default();
        if socket.send(Message::Text(frame.into())).await.is_err() {
            break;
        }
    }

    // 4. Pump both directions.
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(frame) => {
                    if socket.send(Message::Text(frame.into())).await.is_err() { break; }
                }
                None => break,
            },
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(t))) => route(&host, &name, &t).await,
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }

    // 5. Teardown.
    host.nodes.lock().await.remove(&name);
    broadcast_presence(&host, &name, false).await;
    tracing::info!(node = %name, "relay: node disconnected");
}

async fn route(host: &RelayHost, from: &str, text: &str) {
    let Ok(frame) = serde_json::from_str::<RelayFrame>(text) else {
        return;
    };
    match frame {
        RelayFrame::Route {
            to: Some(to), data, ..
        } => {
            let nodes = host.nodes.lock().await;
            if let Some(dest) = nodes.get(&to) {
                let fwd = serde_json::to_string(&RelayFrame::Route {
                    to: None,
                    from: Some(from.to_owned()),
                    data,
                })
                .unwrap_or_default();
                let _ = dest.send(fwd);
            } else if let Some(src) = nodes.get(from) {
                let note =
                    serde_json::to_string(&RelayFrame::Undeliverable { to }).unwrap_or_default();
                let _ = src.send(note);
            }
        }
        RelayFrame::Store { to, id, data } => {
            let nodes = host.nodes.lock().await;
            if let Some(dest) = nodes.get(&to) {
                // Present: hand it straight over.
                let m = serde_json::to_string(&RelayFrame::Mail {
                    from: from.to_owned(),
                    id,
                    data,
                })
                .unwrap_or_default();
                let _ = dest.send(m);
            } else if host
                .mailbox
                .lock()
                .await
                .store(&to, from, &id, data)
                .is_err()
            {
                if let Some(src) = nodes.get(from) {
                    let note =
                        serde_json::to_string(&RelayFrame::MailboxFull { to }).unwrap_or_default();
                    let _ = src.send(note);
                }
            }
        }
        _ => {}
    }
}

async fn broadcast_presence(host: &RelayHost, node: &str, online: bool) {
    let msg = serde_json::to_string(&RelayFrame::Presence {
        node: node.to_owned(),
        online,
    })
    .unwrap_or_default();
    let nodes = host.nodes.lock().await;
    for (name, tx) in nodes.iter() {
        if name != node {
            let _ = tx.send(msg.clone());
        }
    }
}

async fn reject(socket: &mut WebSocket, reason: &str) {
    if let Ok(msg) = serde_json::to_string(&RelayFrame::Rejected {
        reason: reason.to_owned(),
    }) {
        let _ = socket.send(Message::Text(msg.into())).await;
    }
    tracing::warn!(reason, "relay: registration rejected");
}
