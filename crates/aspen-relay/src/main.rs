//! aspen-relay — the rendezvous. The minimal cloud piece: authenticate
//! nodes to one mesh, route sealed frames between them by name, report
//! presence. Holds only the mesh ROOT PUBLIC key; reads nothing it routes.
//!
//! This is the self-hostable reference (a ~one-file container). A Cloudflare
//! Workers + Durable Objects port lives in `rendezvous/cloudflare/` and
//! speaks the identical protocol (aspen-wire::relay).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::sync::{mpsc, Mutex};

use aspen_wire::relay::{Challenge, Register, RelayFrame};

use clap::Parser;

#[derive(Parser)]
#[command(name = "aspen-relay", about = "Aspen rendezvous relay")]
struct Cli {
    /// Listen address.
    #[arg(long, default_value = "0.0.0.0:7440")]
    listen: SocketAddr,
    /// Mesh name this relay serves.
    #[arg(long)]
    mesh: String,
    /// Mesh root PUBLIC key, base64 (from `aspen mesh root-pubkey`). Public
    /// and safe to hand a relay; it can verify membership, never forge it.
    #[arg(long)]
    root_pubkey: String,
}

struct Node {
    tx: mpsc::UnboundedSender<String>,
}

#[derive(Clone)]
struct Relay {
    mesh: String,
    root_pubkey: Vec<u8>,
    nodes: Arc<Mutex<HashMap<String, Node>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aspen_relay=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let root_pubkey =
        base64_decode(&cli.root_pubkey).context("--root-pubkey is not valid base64")?;

    let relay = Relay {
        mesh: cli.mesh.clone(),
        root_pubkey,
        nodes: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/relay", get(ws_handler))
        .with_state(relay);

    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    tracing::info!(
        "aspen-relay for mesh '{}' listening on {}",
        cli.mesh,
        cli.listen
    );
    axum::serve(listener, app).await?;
    Ok(())
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    aspen_wire::b64::decode(s)
}

async fn ws_handler(State(relay): State<Relay>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(relay, socket))
}

async fn handle(relay: Relay, mut socket: WebSocket) {
    // 1. Challenge.
    let mut nonce = [0u8; 32];
    {
        use rand_core::RngCore;
        rand_core::OsRng.fill_bytes(&mut nonce);
    }
    let challenge = serde_json::to_string(&Challenge {
        nonce: nonce.to_vec(),
    })
    .unwrap();
    if socket.send(Message::Text(challenge.into())).await.is_err() {
        return;
    }

    // 2. Register.
    let reg: Register = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str(&t) {
                Ok(r) => break r,
                Err(e) => {
                    let _ = reject(&mut socket, &format!("bad register: {e}")).await;
                    return;
                }
            },
            Some(Ok(_)) => continue,
            _ => return,
        }
    };

    if let Err(reason) = verify_register(&relay, &reg, &nonce) {
        let _ = reject(&mut socket, &reason).await;
        return;
    }
    let node_name = reg.node.clone();
    tracing::info!(node = %node_name, "node registered");

    // 3. Register the connection; announce presence.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let existing: Vec<String> = {
        let mut nodes = relay.nodes.lock().await;
        let peers = nodes.keys().cloned().collect();
        nodes.insert(node_name.clone(), Node { tx });
        peers
    };
    let welcome = serde_json::to_string(&RelayFrame::Welcome { peers: existing }).unwrap();
    if socket.send(Message::Text(welcome.into())).await.is_err() {
        relay.nodes.lock().await.remove(&node_name);
        return;
    }
    broadcast_presence(&relay, &node_name, true).await;

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
                Some(Ok(Message::Text(t))) => {
                    route(&relay, &node_name, &t).await;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }

    // 5. Teardown.
    relay.nodes.lock().await.remove(&node_name);
    broadcast_presence(&relay, &node_name, false).await;
    tracing::info!(node = %node_name, "node disconnected");
}

fn verify_register(relay: &Relay, reg: &Register, nonce: &[u8]) -> Result<(), String> {
    aspen_wire::relay::verify_register(&relay.mesh, &relay.root_pubkey, reg, nonce)
}

async fn route(relay: &Relay, from: &str, text: &str) {
    let Ok(frame) = serde_json::from_str::<RelayFrame>(text) else {
        return;
    };
    if let RelayFrame::Route {
        to: Some(to), data, ..
    } = frame
    {
        let nodes = relay.nodes.lock().await;
        if let Some(dest) = nodes.get(&to) {
            let fwd = serde_json::to_string(&RelayFrame::Route {
                to: None,
                from: Some(from.to_owned()),
                data,
            })
            .unwrap();
            let _ = dest.tx.send(fwd);
        } else if let Some(src) = nodes.get(from) {
            let note = serde_json::to_string(&RelayFrame::Undeliverable { to }).unwrap();
            let _ = src.tx.send(note);
        }
    }
}

async fn broadcast_presence(relay: &Relay, node: &str, online: bool) {
    let msg = serde_json::to_string(&RelayFrame::Presence {
        node: node.to_owned(),
        online,
    })
    .unwrap();
    let nodes = relay.nodes.lock().await;
    for (name, n) in nodes.iter() {
        if name != node {
            let _ = n.tx.send(msg.clone());
        }
    }
}

async fn reject(socket: &mut WebSocket, reason: &str) -> Result<()> {
    let msg = serde_json::to_string(&RelayFrame::Rejected {
        reason: reason.to_owned(),
    })?;
    let _ = socket.send(Message::Text(msg.into())).await;
    tracing::warn!(reason, "registration rejected");
    Ok(())
}
