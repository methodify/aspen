//! The node's localhost API: REST + WS per docs/API.md, plus SPA static
//! serving. v0 is localhost-only and unauthenticated; the mesh security
//! model arrives with federation.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use aspen_core::SessionEvent;
use aspen_node::{Node, SpawnOpts, TurnState};

pub struct AppState {
    pub node: Node,
    pub node_name: String,
}

type S = State<Arc<AppState>>;

pub async fn serve(node: Node, listen: SocketAddr, ui_dir: Option<PathBuf>) -> Result<()> {
    let shutdown_node = node.clone();
    let state = Arc::new(AppState {
        node,
        node_name: hostname(),
    });

    let api = Router::new()
        .route("/node", get(get_node))
        .route("/agents", get(get_agents).post(post_agent))
        .route("/agents/{name}/message", post(post_message))
        .route("/agents/{name}/interrupt", post(post_interrupt))
        .route(
            "/agents/{name}/permission/{request_id}",
            post(post_permission),
        )
        .route("/agents/{name}", delete(delete_agent))
        .route("/agents/{name}/revive", post(post_revive))
        .route("/agents/{name}/events", get(ws_events))
        .route("/agents/{name}/transcript", get(get_transcript))
        .route("/sessions", get(get_sessions))
        .route("/bus/log", get(get_bus_log))
        .route("/bus/send", post(post_bus_send))
        .route("/operator/inbox", get(get_inbox))
        .route("/operator/inbox/read", post(post_inbox_read))
        .with_state(state);

    let mut app = Router::new().nest("/api", api);
    if let Some(dir) = ui_dir {
        let index = dir.join("index.html");
        app = app.fallback_service(
            tower_http::services::ServeDir::new(&dir)
                .fallback(tower_http::services::ServeFile::new(index)),
        );
    }

    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("aspen node API listening on http://{listen}");
    eprintln!("[aspen] node up: http://{listen}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("\n[aspen] shutting down sessions…");
            // Clean ladder for every live session; transcripts persist and
            // revive brings each one back with context intact.
            let names: Vec<String> = shutdown_node
                .inner
                .sessions
                .lock()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            for name in names {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(8),
                    shutdown_node.shutdown_agent(&name),
                )
                .await;
            }
            eprintln!("[aspen] bye");
        })
        .await?;
    Ok(())
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_owned())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "node".into())
}

fn err(status: StatusCode, e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": e.to_string() })))
}

// ------------------------------------------------------------------- handlers

async fn get_node(State(s): S) -> Json<Value> {
    Json(json!({ "node": s.node_name, "version": env!("CARGO_PKG_VERSION") }))
}

fn agent_json(s: &AppState, a: &aspen_node::store::AgentRow) -> Value {
    let live = s.node.inner.live(&a.name);
    json!({
        "name": a.name,
        "repo": a.repo.to_string_lossy(),
        "channel": a.channel,
        "session_id": a.session_id,
        "charter": a.charter,
        "live": live.is_some(),
        "turn_state": live.as_ref().map(|m| match m.turn_state() {
            TurnState::Idle => "idle",
            TurnState::Busy => "busy",
        }),
        "pending": s.node.inner.store.pending_count(&a.name).unwrap_or(0),
    })
}

async fn get_agents(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.agents() {
        Ok(rows) => Json(rows.iter().map(|a| agent_json(&s, a)).collect::<Vec<_>>())
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct SpawnBody {
    name: String,
    repo: String,
    charter: Option<String>,
    model: Option<String>,
    resume: Option<String>,
    #[serde(default)]
    allow_all: bool,
}

async fn post_agent(State(s): S, Json(body): Json<SpawnBody>) -> impl IntoResponse {
    let name = body.name.trim().trim_start_matches('@').to_owned();
    if name.is_empty()
        || name == "operator"
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return err(
            StatusCode::BAD_REQUEST,
            "agent names are [A-Za-z0-9_-]+ and not 'operator'",
        )
        .into_response();
    }
    let opts = SpawnOpts {
        charter: body.charter,
        model: body.model,
        resume: body.resume,
        allow_all: body.allow_all,
        interactive: true,
        ..Default::default()
    };
    match s.node.spawn_agent(&name, PathBuf::from(body.repo), opts).await {
        Ok(_) => {
            let rows = s.node.inner.store.agents().unwrap_or_default();
            match rows.iter().find(|a| a.name == name) {
                Some(a) => Json(agent_json(&s, a)).into_response(),
                None => err(StatusCode::INTERNAL_SERVER_ERROR, "spawned but not registered")
                    .into_response(),
            }
        }
        Err(e) if e.to_string().contains("already running") => {
            err(StatusCode::CONFLICT, e).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn post_message(
    State(s): S,
    Path(name): Path<String>,
    Json(body): Json<MessageBody>,
) -> impl IntoResponse {
    match s.node.send_operator_message(&name, body.text).await {
        Ok(uuid) => Json(json!({ "uuid": uuid })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

async fn post_interrupt(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    match s.node.interrupt(&name).await {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct PermissionBody {
    allow: bool,
    message: Option<String>,
    updated_input: Option<Value>,
}

async fn post_permission(
    State(s): S,
    Path((name, request_id)): Path<(String, String)>,
    Json(body): Json<PermissionBody>,
) -> impl IntoResponse {
    match s.node.answer_permission(
        &name,
        &request_id,
        body.allow,
        body.message,
        body.updated_input,
    ) {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::GONE, e).into_response(),
    }
}

async fn post_revive(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    match s.node.revive_agent(&name, true).await {
        Ok(_) => {
            let rows = s.node.inner.store.agents().unwrap_or_default();
            match rows.iter().find(|a| a.name == name) {
                Some(a) => Json(agent_json(&s, a)).into_response(),
                None => err(StatusCode::INTERNAL_SERVER_ERROR, "revived but not registered")
                    .into_response(),
            }
        }
        Err(e) => err(StatusCode::CONFLICT, e).into_response(),
    }
}

async fn delete_agent(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    match s.node.shutdown_agent(&name).await {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

// --------------------------------------------------------------------- events

async fn ws_events(
    State(s): S,
    Path(name): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(rx) = s.node.subscribe(&name) else {
        return err(StatusCode::NOT_FOUND, format!("no running agent @{name}")).into_response();
    };
    ws.on_upgrade(move |socket| pump_events(socket, rx))
        .into_response()
}

async fn pump_events(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<SessionEvent>,
) {
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return; // client gone
                    }
                    if matches!(ev, SessionEvent::Exited { .. }) {
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let note = json!({ "kind": "status", "raw": { "lagged": n } });
                    if socket.send(Message::Text(note.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Err(_) => { return; }
            },
            msg = socket.recv() => match msg {
                Some(Ok(_)) => {} // inbound frames ignored today
                _ => return,      // closed
            },
        }
    }
}

#[derive(Deserialize)]
struct SessionsQuery {
    repo: String,
}

/// Enumerate a repo's sessions from disk (the filesystem is the registry).
async fn get_sessions(Query(q): Query<SessionsQuery>) -> impl IntoResponse {
    match aspen_claude::transcript::enumerate_sessions(std::path::Path::new(&q.repo)) {
        Ok(rows) => Json(
            rows.iter()
                .map(|s| {
                    json!({
                        "session_id": s.session_id,
                        "title": s.title,
                        "entrypoint": s.entrypoint,
                        "modified": s.modified_epoch,
                        "user_messages": s.user_messages,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// Rehydrated history for an agent's session — what the console renders
/// above the live stream.
async fn get_transcript(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    let rows = match s.node.inner.store.agents() {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let Some(agent) = rows.iter().find(|a| a.name == name) else {
        return err(StatusCode::NOT_FOUND, format!("no agent named @{name}")).into_response();
    };
    let Some(sid) = &agent.session_id else {
        return Json(Vec::<Value>::new()).into_response();
    };
    match aspen_claude::transcript::rehydrate(&agent.repo, sid) {
        Ok(items) => Json(items).into_response(),
        Err(_) => Json(Vec::<Value>::new()).into_response(), // no transcript yet
    }
}

// ------------------------------------------------------------------------ bus

fn message_json(m: &aspen_node::StoredMessage) -> Value {
    json!({
        "id": m.id,
        "sender": m.sender,
        "recipient": m.recipient,
        "to_display": m.to_display,
        "urgency": m.urgency,
        "body": m.body,
        "thread": m.thread,
        "record": m.record_ref,
        "created_at": m.created_at,
        "delivered_at": m.delivered_at,
        "delivered_via": m.delivered_via,
        "ingested_at": m.ingested_at,
    })
}

#[derive(Deserialize)]
struct LogQuery {
    n: Option<i64>,
}

async fn get_bus_log(State(s): S, Query(q): Query<LogQuery>) -> impl IntoResponse {
    match s.node.inner.store.log(q.n.unwrap_or(50)) {
        Ok(rows) => Json(rows.iter().map(message_json).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct BusSendBody {
    to: String,
    body: String,
    urgency: Option<String>,
    thread: Option<String>,
    record: Option<String>,
}

async fn post_bus_send(State(s): S, Json(b): Json<BusSendBody>) -> impl IntoResponse {
    match aspen_node::tools::send_message(
        &s.node.inner,
        "operator",
        b.to.trim(),
        &b.body,
        b.urgency.as_deref().unwrap_or("normal"),
        b.thread.as_deref(),
        b.record.as_deref(),
    ) {
        Ok(notes) => Json(json!({ "notes": notes })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn get_inbox(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.pending_for("operator") {
        Ok(rows) => Json(rows.iter().map(message_json).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn post_inbox_read(State(s): S) -> impl IntoResponse {
    let store = &s.node.inner.store;
    match store.pending_for("operator") {
        Ok(rows) => {
            let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
            match store.mark_delivered(&ids, "operator-ui", None) {
                Ok(()) => Json(json!({})).into_response(),
                Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
            }
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}
