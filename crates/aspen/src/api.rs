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
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use aspen_core::SessionEvent;
use aspen_node::{Node, SpawnOpts, TurnState};

pub struct AppState {
    pub node: Node,
    pub node_name: String,
    pub token: Option<String>,
    /// Fired by POST /api/shutdown — the platform-independent way to ask
    /// for the graceful ladder (Windows has no SIGTERM, and a detached
    /// process has no window for taskkill to close).
    pub shutdown: Arc<tokio::sync::Notify>,
    /// The embedded rendezvous relay (relayhost.rs).
    pub relay: Arc<crate::relayhost::RelayHost>,
}

type S = State<Arc<AppState>>;

pub async fn serve(
    node: Node,
    listen: SocketAddr,
    ui_dir: Option<PathBuf>,
    headless: bool,
    data_dir: &std::path::Path,
) -> Result<()> {
    let shutdown_node = node.clone();
    // Loopback listeners trust the local user; anything wider requires the
    // node token on every /api call (the federation WS is exempt — it has
    // its own cryptographic auth and carries only sealed frames).
    let token = if listen.ip().is_loopback() {
        None
    } else {
        Some(load_or_create_token(data_dir)?)
    };
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let state = Arc::new(AppState {
        node,
        node_name: hostname(),
        token: token.clone(),
        shutdown: shutdown_notify.clone(),
        relay: Arc::new(crate::relayhost::RelayHost::default()),
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
        .route("/agents/{name}/branch", post(post_branch))
        .route("/agents/{name}/bookmarks", get(get_bookmarks))
        .route(
            "/agents/{name}/bookmarks/{id}/resume",
            post(post_bookmark_resume),
        )
        .route("/agents/{name}/bookmarks/{id}", delete(delete_bookmark))
        .route("/agents/{name}/events", get(ws_events))
        .route("/agents/{name}/transcript", get(get_transcript))
        .route("/sessions", get(get_sessions))
        .route("/bus/log", get(get_bus_log))
        .route("/bus/send", post(post_bus_send))
        .route("/operator/inbox", get(get_inbox))
        .route("/operator/inbox/read", post(post_inbox_read))
        .route("/channels", get(get_channels).post(post_channel))
        .route("/channels/{name}", delete(delete_channel_route))
        .route("/channels/{name}/log", get(get_channel_log))
        .route("/channels/{name}/members", post(post_channel_member))
        .route(
            "/channels/{name}/members/remove",
            post(remove_channel_member_route),
        )
        .route("/activity", get(get_activity))
        .route("/repos", get(get_repos).post(post_repo))
        .route("/repos/skip", post(post_repo_skip))
        .route("/repos/forget", post(post_repo_forget))
        .route("/repo/autorun", get(get_autorun))
        .route("/repo/skills", get(get_skills))
        .route(
            "/repo/skill",
            get(get_skill).put(put_skill).delete(delete_skill),
        )
        .route("/agents/{name}/reload", post(post_reload))
        .route("/agents/{name}/runtime", get(get_runtime))
        .route("/agents/{name}/context", get(get_context))
        .route("/agents/{name}/model", post(post_model))
        .route("/agents/{name}/mode", post(post_mode))
        .route("/agents/{name}/title", post(post_title))
        .route("/agents/{name}/charter", post(post_charter))
        .route("/needs", get(get_needs))
        .route("/needs/read", post(post_needs_read))
        .route("/dms", get(get_dms))
        .route("/dm", get(get_dm))
        .route("/bus/post/{post}", get(get_post_receipts))
        .route("/mesh", get(get_mesh))
        .route("/repos/discover", post(post_repos_discover))
        .route("/repos/rename", post(post_repo_rename))
        .route("/mesh/repos", get(get_mesh_repos))
        .route("/mesh/reload", post(post_mesh_reload))
        .route("/mesh/inspect", post(post_mesh_inspect))
        .route(
            "/mesh/pending",
            get(get_mesh_pending).post(post_mesh_pending),
        )
        .route("/mesh/pending/{id}", delete(delete_mesh_pending))
        .route("/history", get(get_history))
        .route("/links", get(get_links).post(post_link))
        .route("/links/{id}", delete(delete_link))
        .route("/shutdown", post(post_shutdown))
        .route("/settings", get(get_settings).put(put_settings))
        .route(
            "/update",
            get(get_update).post(post_update).delete(delete_update),
        )
        .route("/update/check", post(post_update_check))
        .route("/update/policy", put(put_update_policy))
        .route(
            "/update/fleet",
            post(post_update_fleet).delete(delete_update_fleet),
        )
        .route("/logs", get(get_logs))
        .route("/adoptions", get(get_adoptions))
        .route("/adoptions/scan", post(post_adoptions_scan))
        .route("/adoptions/{id}", post(post_adoption))
        .route("/hooks/session", post(post_hook_session))
        .route("/repos/trust", post(post_repo_trust))
        .route("/repos/untrust", post(post_repo_untrust))
        .route("/federation/ws", get(ws_federation))
        .route("/federation/relay", get(ws_relay))
        .with_state(state.clone());

    let api = api.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth_middleware,
    ));
    let mut app = Router::new().nest("/api", api);
    let ui_for_state = ui_dir.clone();
    if headless {
        // API-only node: another node's console (or the CLI) drives it.
        app = app.fallback(headless_root);
    } else if let Some(dir) = ui_dir {
        // Explicit --ui override: serve a dist directory from disk.
        let index = dir.join("index.html");
        app = app.fallback_service(
            tower_http::services::ServeDir::new(&dir)
                .fallback(tower_http::services::ServeFile::new(index)),
        );
    } else {
        // The console ships inside the binary (rust-embed): release builds
        // embed ui/dist at compile time; debug builds read it from disk
        // live, so UI iteration needs no rebuild.
        app = app.fallback(embedded_ui);
    }

    let listener = tokio::net::TcpListener::bind(listen).await?;
    // Port 0 = ephemeral: the OS just chose, so report what it chose.
    let actual = listener.local_addr().unwrap_or(listen);
    // Bound successfully — now this process owns the daemon state file.
    crate::write_daemon_state(data_dir, actual, listen, ui_for_state.as_deref(), headless);
    tracing::info!("aspen node API listening on http://{actual}");
    match &token {
        Some(tk) => eprintln!("[aspen] node up: http://{actual}/?token={tk}"),
        None => eprintln!("[aspen] node up: http://{actual}"),
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal(shutdown_notify).await;
            eprintln!("\n[aspen] shutting down sessions…");
            // Going down: exits during the ladder must keep each agent's
            // `live` mark — that mark IS the resume ledger, and because it
            // is maintained continuously it survives a crash too.
            shutdown_node
                .inner
                .shutting_down
                .store(true, std::sync::atomic::Ordering::SeqCst);
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
                    shutdown_node.shutdown_for_restart(&name),
                )
                .await;
            }
            eprintln!("[aspen] bye");
        })
        .await?;
    Ok(())
}

/// Resolve on Ctrl-C (SIGINT) or SIGTERM — the latter is what `aspen down`
/// sends to a detached node, so both take the clean shutdown ladder.
async fn shutdown_signal(api_request: Arc<tokio::sync::Notify>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = api_request.notified() => {}
                }
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = api_request.notified() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = api_request.notified() => {}
        }
    }
}

/// Graceful stop over the API: sessions get the clean ladder, agents keep
/// their live marks for revive, daemon.json is removed on exit — the
/// signal `aspen down` waits for.
async fn post_shutdown(State(s): S) -> impl IntoResponse {
    s.shutdown.notify_one();
    Json(json!({ "ok": true, "stopping": true }))
}

/// The console, embedded. Release builds carry ui/dist inside the binary;
/// debug builds read the folder live from disk (rust-embed's behavior).
#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]
struct UiAssets;

async fn embedded_ui(req: axum::extract::Request) -> axum::response::Response {
    let path = req.uri().path().trim_start_matches('/');
    // SPA routing: unknown paths (and "/") fall back to index.html.
    let (name, spa_fallback) = if path.is_empty() || UiAssets::get(path).is_none() {
        ("index.html", true)
    } else {
        (path, false)
    };
    let Some(file) = UiAssets::get(name) else {
        return err(
            StatusCode::NOT_FOUND,
            "console not built into this binary (build ui/dist, or pass --ui)",
        )
        .into_response();
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    // Hashed assets are immutable; the entry point must always revalidate.
    let cache = if !spa_fallback && name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, mime.as_ref().to_owned()),
            (axum::http::header::CACHE_CONTROL, cache.to_owned()),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

async fn headless_root() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        "aspen node running headless (--headless): API only, no console. \
         Reach this node from another node's console over the mesh.",
    )
}

#[derive(Deserialize)]
struct RepoRenameBody {
    path: String,
    handle: String,
    node: Option<String>,
}

/// Rename a repo's handle — its address segment and channel name. Refused
/// while sessions in it are running (their addresses would change).
async fn post_repo_rename(State(s): S, Json(b): Json<RepoRenameBody>) -> impl IntoResponse {
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(
            &s,
            node,
            "node_repo_rename",
            "",
            json!({ "path": b.path, "handle": b.handle }),
        )
        .await;
    }
    let path = aspen_node::node::normalize_repo(std::path::Path::new(&b.path));
    let live: Vec<String> = s
        .node
        .inner
        .sessions
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    match s.node.inner.store.rename_handle(&path, &b.handle, &live) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

/// Recover repos from Claude Code's session store on this machine and
/// register the new ones.
#[derive(Deserialize, Default)]
struct DiscoverBody {
    /// Node to run discovery on; absent or this node's name = local.
    node: Option<String>,
}

async fn post_repos_discover(State(s): S, body: Option<Json<DiscoverBody>>) -> impl IntoResponse {
    let node = body.and_then(|b| b.0.node);
    if let Some(node) = node.as_deref().filter(|n| !is_self_node(&s, n)) {
        // Run discovery on the peer; its repos register there, not here.
        return proxy(&s, node, "node_discover", "", json!({})).await;
    }
    match s.node.discover_repos() {
        Ok(found) => Json(json!({
            "found": found
                .iter()
                .map(|(path, sessions, added)| json!({
                    "path": path, "sessions": sessions, "added": added,
                }))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

async fn get_settings(State(s): S) -> impl IntoResponse {
    let settings = s
        .node
        .inner
        .data_dir
        .as_deref()
        .map(aspen_node::settings::load)
        .unwrap_or_default();
    Json(serde_json::to_value(settings).unwrap_or_default())
}

/// Merge semantics: only the top-level keys present in the body change
/// (so a console saving harness args doesn't wipe the update policy).
async fn put_settings(State(s): S, Json(body): Json<Value>) -> impl IntoResponse {
    let Some(dir) = s.node.inner.data_dir.as_deref() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "node has no data dir").into_response();
    };
    let mut merged = serde_json::to_value(aspen_node::settings::load(dir)).unwrap_or_default();
    let Some(obj) = body.as_object() else {
        return err(StatusCode::BAD_REQUEST, "settings must be an object").into_response();
    };
    if let Some(m) = merged.as_object_mut() {
        for (k, v) in obj {
            m.insert(k.clone(), v.clone());
        }
    }
    let settings: aspen_node::settings::Settings = match serde_json::from_value(merged) {
        Ok(v) => v,
        Err(e) => {
            return err(StatusCode::BAD_REQUEST, format!("bad settings shape: {e}")).into_response()
        }
    };
    // Validate now so a typo surfaces here, not at next spawn / next check.
    for (harness, h) in &settings.harness {
        if let Err(e) = aspen_node::settings::split_args(&h.args, None) {
            return err(StatusCode::BAD_REQUEST, format!("{harness}: {e:#}")).into_response();
        }
    }
    if let Err(e) = settings.update.validate() {
        return err(StatusCode::BAD_REQUEST, format!("{e:#}")).into_response();
    }
    match aspen_node::settings::save(dir, &settings) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

fn load_or_create_token(data_dir: &std::path::Path) -> Result<String> {
    let path = data_dir.join("api-token");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_owned();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    use rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::create_dir_all(data_dir).ok();
    std::fs::write(&path, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

async fn auth_middleware(
    State(s): S,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let Some(expected) = &s.token else {
        return next.run(req).await;
    };
    // Federation carries sealed frames and authenticates cryptographically;
    // the relay admits by cert + challenge and reads nothing it routes.
    if req.uri().path().ends_with("/federation/ws")
        || req.uri().path().ends_with("/federation/relay")
    {
        return next.run(req).await;
    }
    let presented = req
        .headers()
        .get("x-aspen-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            req.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("token=").map(str::to_owned))
            })
        });
    if presented.as_deref() == Some(expected.as_str()) {
        next.run(req).await
    } else {
        err(
            StatusCode::UNAUTHORIZED,
            "missing or invalid node token — this node listens beyond loopback; open the console from the URL `aspen status` prints (it carries the token)",
        )
        .into_response()
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            aspen_node::gitstate::quiet_command("hostname")
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

/// A fully qualified `name@repo@node` address targeting a DIFFERENT node
/// resolves to (local key, node) for mesh proxying; one naming this node
/// collapses to local. `name@repo` is always local.
fn remote_parts(s: &AppState, name: &str) -> Option<(String, String)> {
    let node = aspen_node::addr::node_of(name)?;
    let mesh = s.node.inner.mesh()?;
    if node == mesh.identity.node {
        return None;
    }
    let key = aspen_node::addr::strip_node(name, node).to_owned();
    Some((key, node.to_owned()))
}

const REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// For mesh-wide listings (repos), where a peer's slowness shouldn't stall
/// the page: short, and calls run concurrently.
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Proxy one op to the agent's home node; map errors to a response.
async fn proxy(
    s: &AppState,
    node: &str,
    op: &str,
    agent: &str,
    body: Value,
) -> axum::response::Response {
    let Some(mesh) = s.node.inner.mesh() else {
        return err(StatusCode::NOT_FOUND, "this node is not in a mesh").into_response();
    };
    match mesh.api_call(node, op, agent, body, REMOTE_TIMEOUT).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("via node '{node}': {e}")).into_response(),
    }
}

// ------------------------------------------------------------------- handlers

async fn get_node(State(s): S) -> Json<Value> {
    let svc = &s.node.inner.servicing;
    let st = svc.state();
    let policy = s
        .node
        .inner
        .data_dir
        .as_deref()
        .map(aspen_node::settings::load)
        .unwrap_or_default()
        .update;
    let newer = svc.newer();
    let listen = s
        .node
        .inner
        .data_dir
        .as_deref()
        .and_then(crate::read_daemon_state)
        .and_then(|st| st["listen"].as_str().map(str::to_owned));
    let loopback_only = listen
        .as_deref()
        .and_then(|l| l.parse::<std::net::SocketAddr>().ok())
        .is_none_or(|a| a.ip().is_loopback());
    Json(json!({
        "node": s.node_name,
        "version": env!("CARGO_PKG_VERSION"),
        "sha": env!("ASPEN_GIT_SHA"),
        "built": env!("ASPEN_BUILD_DATE"),
        "listen": listen,
        "loopback_only": loopback_only,
        "hostname": hostname(),
        // Servicing summary for the badge; GET /api/update has the rest.
        "update_available": newer.as_ref().map(|r| r.version.clone()),
        "update_skipped": newer.as_ref().is_some_and(|r| policy.skip.as_deref() == Some(r.version.as_str())),
        "withdrawn": svc.withdrawn(),
        "service_state": st.name(),
        "service_detail": st.detail(),
        "started_at": svc.inventory.started_at,
    }))
}

// ---------------------------------------------------------------- servicing

#[derive(Deserialize, Default)]
struct UpdateBody {
    /// quiet (default) | now
    when: Option<String>,
    /// Act on a peer instead of this node.
    node: Option<String>,
}

/// Full servicing status: release seen, policy, state, inventory, last
/// outcome, rollout progress. `?node=` asks a peer for its own.
async fn get_update(State(s): S, Query(q): Query<NodeQuery>) -> impl IntoResponse {
    if let Some(node) = q.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(&s, node, "node_update_status", "", json!({})).await;
    }
    Json(aspen_node::servicing::status_json(&s.node.inner)).into_response()
}

#[derive(Deserialize, Default)]
struct NodeQuery {
    node: Option<String>,
    lines: Option<usize>,
}

/// Ask a node to update: drain (refuse spawns, wait for quiet), then run
/// the updater. `when: now` skips the quiet gate — sessions mid-turn lose
/// that turn (they are revived).
async fn post_update(State(s): S, body: Option<Json<UpdateBody>>) -> impl IntoResponse {
    let b = body.map(|b| b.0).unwrap_or_default();
    let when = b.when.clone().unwrap_or_else(|| "quiet".into());
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(
            &s,
            node,
            "node_update",
            "",
            json!({ "when": when, "by": format!("operator@{}", s.node_name) }),
        )
        .await;
    }
    match aspen_node::servicing::request(&s.node.inner, &when, "operator") {
        Ok(st) => Json(serde_json::to_value(st).unwrap_or_default()).into_response(),
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

async fn delete_update(State(s): S, body: Option<Json<UpdateBody>>) -> impl IntoResponse {
    let b = body.map(|b| b.0).unwrap_or_default();
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(
            &s,
            node,
            "node_update_cancel",
            "",
            json!({ "by": format!("operator@{}", s.node_name) }),
        )
        .await;
    }
    match aspen_node::servicing::cancel(&s.node.inner, "operator") {
        Ok(c) => Json(json!({ "ok": true, "cancelled": c })).into_response(),
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

async fn post_update_check(State(s): S, body: Option<Json<UpdateBody>>) -> impl IntoResponse {
    let b = body.map(|b| b.0).unwrap_or_default();
    if b.node.as_deref() == Some("*") {
        // The whole mesh: this node plus every linked peer, concurrently.
        let mut results = serde_json::Map::new();
        let peers: Vec<String> = s
            .node
            .inner
            .mesh()
            .map(|m| m.links.lock().unwrap().keys().cloned().collect())
            .unwrap_or_default();
        let local = aspen_node::servicing::check_async(s.node.inner.clone());
        let remote = futures_util::future::join_all(peers.iter().map(|p| {
            let s = s.clone();
            let p = p.clone();
            async move {
                let r = match s.node.inner.mesh() {
                    Some(m) => {
                        m.api_call(&p, "node_update_check", "", json!({}), LIST_TIMEOUT)
                            .await
                    }
                    None => Err(anyhow::anyhow!("not in a mesh")),
                };
                (p, r)
            }
        }));
        let (local, remote) = tokio::join!(local, remote);
        results.insert(
            self_node_name(&s),
            match local {
                Ok(r) => json!({ "ok": true, "latest": r.version, "behind": s.node.inner.servicing.newer().is_some() }),
                Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
            },
        );
        for (p, r) in remote {
            results.insert(
                p,
                match r {
                    Ok(v) => v,
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                },
            );
        }
        let behind = results
            .values()
            .filter(|v| v["behind"].as_bool() == Some(true))
            .count();
        return Json(json!({ "ok": true, "results": results, "behind": behind })).into_response();
    }
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(&s, node, "node_update_check", "", json!({})).await;
    }
    match aspen_node::servicing::check_async(s.node.inner.clone()).await {
        Ok(r) => Json(json!({
            "ok": true, "latest": r.version, "tag": r.tag,
            "behind": s.node.inner.servicing.newer().is_some(),
        }))
        .into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

#[derive(Deserialize)]
struct PolicyBody {
    /// A node name, or "*" for this node and every linked peer.
    node: Option<String>,
    policy: aspen_node::settings::UpdateSettings,
}

/// Set the update policy on this node, a peer, or the whole mesh (`*`).
async fn put_update_policy(State(s): S, Json(b): Json<PolicyBody>) -> impl IntoResponse {
    if let Err(e) = b.policy.validate() {
        return err(StatusCode::BAD_REQUEST, format!("{e:#}")).into_response();
    }
    let set_local = |s: &AppState| -> Result<()> {
        let dir = s
            .node
            .inner
            .data_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("node has no data dir"))?;
        let mut cur = aspen_node::settings::load(dir);
        cur.update = b.policy.clone();
        aspen_node::settings::save(dir, &cur)
    };
    match b.node.as_deref() {
        Some("*") => {
            let mut results = serde_json::Map::new();
            results.insert(
                self_node_name(&s),
                match set_local(&s) {
                    Ok(()) => json!({ "ok": true }),
                    Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
                },
            );
            if let Some(mesh) = s.node.inner.mesh() {
                let peers: Vec<String> = mesh.links.lock().unwrap().keys().cloned().collect();
                for p in peers {
                    let r = mesh
                        .api_call(
                            &p,
                            "node_update_policy",
                            "",
                            json!({ "policy": b.policy }),
                            LIST_TIMEOUT,
                        )
                        .await;
                    results.insert(
                        p,
                        match r {
                            Ok(_) => json!({ "ok": true }),
                            Err(e) => json!({ "ok": false, "error": e.to_string() }),
                        },
                    );
                }
            }
            Json(json!({ "ok": true, "results": results })).into_response()
        }
        Some(node) if !is_self_node(&s, node) => {
            proxy(
                &s,
                node,
                "node_update_policy",
                "",
                json!({ "policy": b.policy }),
            )
            .await
        }
        _ => match set_local(&s) {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        },
    }
}

/// Rolling update of every linked peer, then this node (docs/SERVICING §7).
async fn post_update_fleet(State(s): S, body: Option<Json<UpdateBody>>) -> impl IntoResponse {
    let when = body
        .and_then(|b| b.0.when)
        .unwrap_or_else(|| "quiet".into());
    match aspen_node::servicing::start_rollout(&s.node.inner, &when) {
        Ok(r) => Json(serde_json::to_value(r).unwrap_or_default()).into_response(),
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

async fn delete_update_fleet(State(s): S) -> impl IntoResponse {
    let stopped = aspen_node::servicing::stop_rollout(&s.node.inner);
    Json(json!({ "ok": true, "stopping": stopped }))
}

/// Tail this node's (or a peer's) daemon log.
async fn get_logs(State(s): S, Query(q): Query<NodeQuery>) -> impl IntoResponse {
    let lines = q.lines.unwrap_or(200);
    if let Some(node) = q.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(&s, node, "node_logs", "", json!({ "lines": lines })).await;
    }
    let Some(dir) = s.node.inner.data_dir.as_deref() else {
        return err(StatusCode::NOT_FOUND, "node has no data dir").into_response();
    };
    Json(json!({ "lines": aspen_node::servicing::tail_log(dir, lines) })).into_response()
}

fn agent_json(s: &AppState, a: &aspen_node::store::AgentRow) -> Value {
    let live = s.node.inner.live(&a.name);
    let (busy_since, last_tool) = live
        .as_ref()
        .map(|m| m.presence_detail())
        .unwrap_or((None, None));
    json!({
        "name": a.name,
        "bare": aspen_node::addr::bare(&a.name),
        "title": a.title,
        "busy_since": busy_since,
        "last_tool": last_tool,
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
        "summary": live.as_ref().map(|m| aspen_node::node::summary_json(m)),
        "git": aspen_node::gitstate::get(&a.repo),
        "last_exit_code": a.last_exit_code,
        "last_exit_at": a.last_exit_at,
    })
}

async fn get_agents(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.agents() {
        Ok(rows) => {
            let mut out: Vec<Value> = rows.iter().map(|a| agent_json(&s, a)).collect();
            for v in out.iter_mut() {
                v["node"] = json!(s.node.inner.mesh().map(|m| m.identity.node.clone()));
            }
            if let Some(mesh) = s.node.inner.mesh() {
                let remote = mesh.remote.lock().unwrap();
                for (node, agents) in remote.iter() {
                    let reachable = mesh.link_up(node);
                    for a in agents {
                        out.push(json!({
                            "name": format!("{}@{}", a.name, node),
                            "bare": aspen_node::addr::bare(&a.name),
                            "title": a.title,
                            "summary": if reachable { a.summary.clone() } else { None },
                            "repo": null,
                            "channel": a.channel,
                            "session_id": null,
                            "charter": null,
                            "live": a.live && reachable,
                            "turn_state": if reachable { json!(a.turn_state) } else { Value::Null },
                            "pending": s.node.inner.store.pending_count(&format!("{}@{}", a.name, node)).unwrap_or(0),
                            "node": node,
                            "remote": true,
                        }));
                    }
                }
            }
            Json(out).into_response()
        }
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
    /// Skip permission prompts (bypassPermissions). Omit to use the repo's
    /// stored default.
    skip_permissions: Option<bool>,
    /// The operator reviewed this repo's autorun surface and trusts it.
    #[serde(default)]
    acknowledge_trust: bool,
    /// Display title to set on the new agent (e.g. carried over from an
    /// mcc session name).
    title: Option<String>,
    /// Per-session harness CLI args (raw string), appended after the
    /// harness defaults from settings.
    extra_args: Option<String>,
    /// Target node for the session. Absent or this node's own name = local;
    /// a peer name spawns on that node over the mesh.
    node: Option<String>,
}

async fn post_agent(State(s): S, Json(body): Json<SpawnBody>) -> impl IntoResponse {
    // The operator gives the bare name; the address is bare@<repo handle>,
    // composed by the node (names are per repo).
    let name = body.name.trim().trim_start_matches('@').to_owned();
    if name.is_empty()
        || name == "operator"
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return err(
            StatusCode::BAD_REQUEST,
            "agent names are [A-Za-z0-9_-]+ (no '@' — the repo is added for you) and not 'operator'",
        )
        .into_response();
    }

    // Remote spawn: the repo lives on a peer, so start the session there.
    if let Some(node) = body.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        let Some(mesh) = s.node.inner.mesh() else {
            return err(StatusCode::NOT_FOUND, "this node is not in a mesh").into_response();
        };
        let req = json!({
            "name": name, "repo": body.repo, "charter": body.charter,
            "model": body.model, "resume": body.resume, "allow_all": body.allow_all,
            "skip_permissions": body.skip_permissions,
            "acknowledge_trust": body.acknowledge_trust,
            "title": body.title, "extra_args": body.extra_args,
        });
        return match mesh.api_call(node, "spawn", "", req, REMOTE_TIMEOUT).await {
            // The remote signals an untrusted repo the same way local does:
            // a 428 the console turns into the trust-review dialog.
            Ok(v) if v.get("trust_required").and_then(|b| b.as_bool()) == Some(true) => (
                StatusCode::PRECONDITION_REQUIRED,
                Json(json!({
                    "error": "untrusted repo: review what it auto-runs, then retry with acknowledge_trust",
                    "autorun": v.get("autorun").cloned().unwrap_or(Value::Null),
                })),
            )
                .into_response(),
            Ok(v) => {
                // The peer answers with the registered key (bare@repo);
                // qualify it with the node, matching the roster.
                let key = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or(&name)
                    .to_owned();
                Json(json!({
                    "name": format!("{key}@{node}"),
                    "bare": aspen_node::addr::bare(&key),
                    "node": node,
                    "remote": true,
                    "repo": body.repo,
                    "live": true,
                }))
                .into_response()
            }
            Err(e) => err(StatusCode::BAD_GATEWAY, format!("via node '{node}': {e}")).into_response(),
        };
    }
    // The trust gate (reference §7.7): headless sessions never show the
    // workspace-trust dialog, so the console owns it. A repo that would
    // auto-run anything requires explicit consent once.
    let repo_path = aspen_node::node::normalize_repo(std::path::Path::new(&body.repo));
    let (autorun, trusted) = s.node.trust_state(&repo_path);
    if body.acknowledge_trust {
        let _ = s.node.record_trust(&repo_path);
    } else if !trusted && autorun.has_autorun {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({
                "error": "untrusted repo: review what it auto-runs, then retry with acknowledge_trust",
                "autorun": autorun,
            })),
        )
            .into_response();
    }
    let opts = SpawnOpts {
        charter: body.charter,
        model: body.model,
        resume: body.resume,
        allow_all: body.allow_all,
        interactive: true,
        skip_permissions: body.skip_permissions,
        extra_args: body.extra_args.filter(|a| !a.trim().is_empty()),
        ..Default::default()
    };
    match s
        .node
        .spawn_agent(&name, PathBuf::from(body.repo), opts)
        .await
    {
        Ok(sess) => {
            // The registered key is bare@<repo handle>.
            let key = sess.name.clone();
            if let Some(title) = body.title.as_deref().filter(|t| !t.trim().is_empty()) {
                let _ = s.node.inner.store.set_agent_title(&key, Some(title));
            }
            let rows = s.node.inner.store.agents().unwrap_or_default();
            match rows.iter().find(|a| a.name == key) {
                Some(a) => Json(agent_json(&s, a)).into_response(),
                None => err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "spawned but not registered",
                )
                .into_response(),
            }
        }
        Err(e) if e.to_string().contains("already running") => {
            err(StatusCode::CONFLICT, format!("{e:#}")).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
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
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "message", &bare, json!({ "text": body.text })).await;
    }
    match s.node.send_operator_message(&name, body.text).await {
        Ok(uuid) => Json(json!({ "uuid": uuid })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

async fn post_interrupt(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "interrupt", &bare, json!({})).await;
    }
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
    /// "Always allow" rules — echo the prompt's `suggestions` back verbatim.
    updated_permissions: Option<Value>,
}

async fn post_permission(
    State(s): S,
    Path((name, request_id)): Path<(String, String)>,
    Json(body): Json<PermissionBody>,
) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(
            &s,
            &node,
            "permission",
            &bare,
            json!({
                "request_id": request_id, "allow": body.allow,
                "message": body.message, "updated_input": body.updated_input,
            }),
        )
        .await;
    }
    match s.node.answer_permission(
        &name,
        &request_id,
        body.allow,
        body.message,
        body.updated_input,
        body.updated_permissions,
    ) {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::GONE, e).into_response(),
    }
}

async fn post_revive(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "revive", &bare, json!({})).await;
    }
    match s.node.revive_agent(&name, true).await {
        Ok(_) => {
            let rows = s.node.inner.store.agents().unwrap_or_default();
            match rows.iter().find(|a| a.name == name) {
                Some(a) => Json(agent_json(&s, a)).into_response(),
                None => err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "revived but not registered",
                )
                .into_response(),
            }
        }
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

#[derive(Deserialize, Default)]
struct BranchBody {
    /// Label for the bookmark left on the tip being departed.
    label: Option<String>,
    /// Branch from this message (assistant message uuid) instead of the tip.
    at: Option<String>,
    /// Continue the fork as a NEW agent with this name; this agent keeps
    /// its session (split). Absent: this name follows the fork (carry).
    r#as: Option<String>,
}

/// Branch here: bookmark the current tip, fork, move the head.
async fn post_branch(
    State(s): S,
    Path(name): Path<String>,
    body: Option<Json<BranchBody>>,
) -> impl IntoResponse {
    let b = body.map(|j| j.0).unwrap_or_default();
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(
            &s,
            &node,
            "branch",
            &bare,
            json!({ "label": b.label, "at": b.at, "as": b.r#as }),
        )
        .await;
    }
    match s
        .node
        .branch_agent(
            &name,
            b.label.as_deref(),
            b.at.as_deref(),
            b.r#as.as_deref(),
        )
        .await
    {
        // A split answers with the NEW agent; a carry with this one.
        Ok(sess) => agent_response(&s, &sess.name),
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

/// The agent's bookmarks (tips left behind by branch/swap, plus manual
/// ones) and the lineage of its current head.
async fn get_bookmarks(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "bookmarks", &bare, json!({})).await;
    }
    Json(bookmarks_json(&s.node, &name)).into_response()
}

#[derive(Deserialize, Default)]
struct ResumeBookmarkBody {
    /// Continue the bookmark as a NEW agent with this name (split).
    r#as: Option<String>,
}

async fn post_bookmark_resume(
    State(s): S,
    Path((name, id)): Path<(String, i64)>,
    body: Option<Json<ResumeBookmarkBody>>,
) -> impl IntoResponse {
    let b = body.map(|j| j.0).unwrap_or_default();
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(
            &s,
            &node,
            "resume_bookmark",
            &bare,
            json!({ "id": id, "as": b.r#as }),
        )
        .await;
    }
    match s.node.resume_bookmark(&name, id, b.r#as.as_deref()).await {
        Ok(sess) => agent_response(&s, &sess.name),
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

async fn delete_bookmark(State(s): S, Path((name, id)): Path<(String, i64)>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "delete_bookmark", &bare, json!({ "id": id })).await;
    }
    match s.node.inner.store.delete_bookmark(&name, id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

fn agent_response(s: &AppState, name: &str) -> axum::response::Response {
    let rows = s.node.inner.store.agents().unwrap_or_default();
    match rows.iter().find(|a| a.name == name) {
        Some(a) => Json(agent_json(s, a)).into_response(),
        None => err(StatusCode::INTERNAL_SERVER_ERROR, "not registered").into_response(),
    }
}

/// Shared by the local handler and the federation op.
pub fn bookmarks_json(node: &Node, name: &str) -> Value {
    let head = node
        .inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.name == name)
        .and_then(|a| a.session_id);
    let lineage = head
        .as_deref()
        .and_then(|h| node.inner.store.lineage_of(h).ok())
        .unwrap_or_default();
    json!({
        "head": head,
        "lineage": lineage.iter().map(|(p, at)| json!({ "session_id": p, "fork_message": at })).collect::<Vec<_>>(),
        "bookmarks": node.inner.store.bookmarks(name).unwrap_or_default(),
    })
}

async fn delete_agent(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "shutdown", &bare, json!({})).await;
    }
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
    if let Some((bare, node)) = remote_parts(&s, &name) {
        let Some(mesh) = s.node.inner.mesh() else {
            return err(StatusCode::NOT_FOUND, "not in a mesh").into_response();
        };
        return ws
            .on_upgrade(move |socket| pump_remote_events(socket, mesh, node, bare))
            .into_response();
    }
    let Some(rx) = s.node.subscribe(&name) else {
        return err(StatusCode::NOT_FOUND, format!("no running agent @{name}")).into_response();
    };
    ws.on_upgrade(move |socket| pump_events(socket, rx))
        .into_response()
}

/// Bridge a console WS to a subscription served by the agent's home node.
async fn pump_remote_events(
    mut socket: WebSocket,
    mesh: Arc<aspen_node::federation::MeshState>,
    node: String,
    agent: String,
) {
    let sub_id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    mesh.remote_subs
        .lock()
        .unwrap()
        .insert(sub_id.clone(), (node.clone(), tx));
    if mesh
        .send_to(&node, &json!({ "t": "sub", "id": sub_id, "agent": agent }))
        .is_err()
    {
        mesh.remote_subs.lock().unwrap().remove(&sub_id);
        let _ = socket
            .send(Message::Text(
                json!({ "kind": "status", "raw": { "error": format!("node '{node}' unreachable") } })
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    }
    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Some(f) => {
                    if f.get("t").and_then(|t| t.as_str()) == Some("sub_end") {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    let Some(ev) = f.get("ev") else { continue };
                    if socket.send(Message::Text(ev.to_string().into())).await.is_err() {
                        break;
                    }
                }
                None => { // link died
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            },
            msg = socket.recv() => match msg {
                Some(Ok(_)) => {}
                _ => break, // console gone: tell the serving node to stop
            },
        }
    }
    mesh.remote_subs.lock().unwrap().remove(&sub_id);
    let _ = mesh.send_to(&node, &json!({ "t": "unsub", "id": sub_id }));
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
    /// Owning node; absent or this node's name = local.
    node: Option<String>,
}

/// Enumerate a repo's sessions from disk (the filesystem is the registry).
async fn get_sessions(State(s): S, Query(q): Query<SessionsQuery>) -> impl IntoResponse {
    // Remote repo: enumerate on its owning node.
    if let Some(node) = q.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(&s, node, "node_sessions", "", json!({ "repo": q.repo })).await;
    }
    let repo = std::path::Path::new(&q.repo);
    // mcc's register, when the repo has one: names win over derived titles,
    // and configured args ride along for the resume flow.
    let mcc = aspen_node::mcc::read(repo);
    match aspen_claude::transcript::enumerate_sessions(repo) {
        Ok(rows) => Json(
            rows.iter()
                .map(|s| {
                    let m = mcc.get(&s.session_id);
                    json!({
                        "session_id": s.session_id,
                        "title": s.title,
                        "entrypoint": s.entrypoint,
                        "modified": s.modified_epoch,
                        "user_messages": s.user_messages,
                        "mcc_name": m.map(|m| m.name.clone()),
                        "mcc_args": m.and_then(|m| m.args.clone()),
                        "mcc_skip": m.map(|m| m.skip_permissions),
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
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "transcript", &bare, json!({})).await;
    }
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

/// Inbound federation link: bridge the axum socket to text-frame channels
/// and hand it to the transport-blind link runner. Every frame after hello
/// is a sealed envelope, so an unauthenticated caller can hold a socket
/// open but can neither read nor forge mesh traffic.
/// The embedded relay: any member may rendezvous here (docs: relayhost.rs).
async fn ws_relay(State(s): S, ws: WebSocketUpgrade) -> impl IntoResponse {
    let Some(mesh) = s.node.inner.mesh() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "this node has not joined a mesh",
        )
        .into_response();
    };
    let host = s.relay.clone();
    let name = mesh.mesh_name();
    let root = mesh.root_public();
    let me = mesh.identity.node.clone();
    ws.on_upgrade(move |socket| crate::relayhost::serve(host, name, root, me, socket))
        .into_response()
}

async fn ws_federation(State(s): S, ws: WebSocketUpgrade) -> impl IntoResponse {
    if s.node.inner.mesh().is_none() {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "this node has not joined a mesh",
        )
        .into_response();
    }
    let inner = s.node.inner.clone();
    ws.on_upgrade(move |socket| async move {
        let (mut sink, mut stream) = {
            use futures_util::StreamExt as _;
            socket.split()
        };
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(async move {
            use futures_util::SinkExt as _;
            while let Some(f) = out_rx.recv().await {
                if sink.send(Message::Text(f.into())).await.is_err() {
                    break;
                }
            }
        });
        let reader = tokio::spawn(async move {
            use futures_util::StreamExt as _;
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(t) = msg {
                    if in_tx.send(t.to_string()).is_err() {
                        break;
                    }
                }
            }
        });
        if let Err(e) = aspen_node::federation::run_link(inner, out_tx, in_rx).await {
            tracing::debug!(error = %e, "inbound federation link ended");
        }
        writer.abort();
        reader.abort();
    })
    .into_response()
}

// --------------------------------------------------------------------- skills

#[derive(Deserialize)]
struct RepoQuery {
    repo: String,
}

// -------------------------------------------------- session runtime controls

/// The runtime's own view: handshake (commands/models/output style) plus the
/// system/init inventory. This is the source for slash autocomplete and the
/// loaded-skill/MCP panel — never parsed from disk.
async fn get_runtime(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "runtime", &bare, json!({})).await;
    }
    match s.node.runtime_info(&name) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

async fn get_context(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "context", &bare, json!({})).await;
    }
    match s.node.context_usage(&name).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct ModelBody {
    model: Option<String>,
}

async fn post_model(
    State(s): S,
    Path(name): Path<String>,
    Json(b): Json<ModelBody>,
) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "set_model", &bare, json!({ "model": b.model })).await;
    }
    match s.node.set_model(&name, b.model.as_deref()).await {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct ModeBody {
    mode: String,
}

async fn post_mode(
    State(s): S,
    Path(name): Path<String>,
    Json(b): Json<ModeBody>,
) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "set_mode", &bare, json!({ "mode": b.mode })).await;
    }
    match s.node.set_permission_mode(&name, &b.mode).await {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct TitleBody {
    title: Option<String>,
}

async fn post_title(
    State(s): S,
    Path(name): Path<String>,
    Json(b): Json<TitleBody>,
) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "title", &bare, json!({ "title": b.title })).await;
    }
    match s.node.set_title(&name, b.title.as_deref()) {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct CharterBody {
    charter: Option<String>,
}

async fn post_charter(
    State(s): S,
    Path(name): Path<String>,
    Json(b): Json<CharterBody>,
) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "charter", &bare, json!({ "charter": b.charter })).await;
    }
    match s.node.set_charter(&name, b.charter.as_deref()) {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

// ------------------------------------------------------ the mesh-wide inbox

/// Everything in the WHOLE MESH that needs the operator: open permission
/// prompts and questions (local + every connected peer) and @operator mail
/// from every node's store. The console's Command surface is built on this.
async fn get_needs(State(s): S) -> impl IntoResponse {
    let mut prompts: Vec<Value> = s
        .node
        .open_prompts()
        .into_iter()
        .map(|(agent, p)| {
            json!({
                "agent": agent, "node": Value::Null, "request_id": p.request_id,
                "tool_name": p.tool_name, "input": p.input,
                "suggestions": p.suggestions, "asked_at": p.asked_at,
                "is_question": p.is_question,
            })
        })
        .collect();
    let mut inbox: Vec<Value> = s
        .node
        .inner
        .store
        .pending_for("operator")
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let mut v = message_json(m);
            v["node"] = Value::Null;
            v
        })
        .collect();
    let mut adoptions: Vec<Value> = s
        .node
        .inner
        .store
        .adoptions(true)
        .unwrap_or_default()
        .iter()
        .map(|a| {
            let mut v = aspen_node::adoption::adoption_json(a);
            v["node"] = Value::Null;
            v
        })
        .collect();

    if let Some(mesh) = s.node.inner.mesh() {
        let peers: Vec<String> = mesh.links.lock().unwrap().keys().cloned().collect();
        for peer in peers {
            match mesh
                .api_call(
                    &peer,
                    "needs",
                    "",
                    json!({}),
                    std::time::Duration::from_secs(5),
                )
                .await
            {
                Ok(v) => {
                    if let Some(ps) = v.get("prompts").and_then(|p| p.as_array()) {
                        for p in ps {
                            let mut p = p.clone();
                            p["node"] = json!(peer);
                            // Remote prompts are answered via name@node.
                            if let Some(a) = p.get("agent").and_then(|a| a.as_str()) {
                                p["agent"] = json!(format!("{a}@{peer}"));
                            }
                            prompts.push(p);
                        }
                    }
                    if let Some(ms) = v.get("inbox").and_then(|m| m.as_array()) {
                        for m in ms {
                            let mut m = m.clone();
                            m["node"] = json!(peer);
                            inbox.push(m);
                        }
                    }
                    if let Some(ads) = v.get("adoptions").and_then(|m| m.as_array()) {
                        for a in ads {
                            let mut a = a.clone();
                            a["node"] = json!(peer);
                            if let Some(ag) = a.get("of_agent").and_then(|x| x.as_str()) {
                                a["of_agent"] = json!(format!("{ag}@{peer}"));
                            }
                            adoptions.push(a);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(peer, error = %e, "needs aggregation: peer unreachable");
                }
            }
        }
    }
    Json(json!({ "prompts": prompts, "inbox": inbox, "adoptions": adoptions })).into_response()
}

// ----------------------------------------------------------------- adoption

#[derive(Deserialize)]
struct AdoptionBody {
    /// carry | split | ignore | revive
    action: String,
    /// For split: the new agent's bare name.
    name: Option<String>,
    /// The node the adoption belongs to (absent/self = local).
    node: Option<String>,
}

/// All adoptions this node has recorded (answered ones included, newest
/// first) — the mesh-wide open ones ride /api/needs.
async fn get_adoptions(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.adoptions(false) {
        Ok(v) => Json(
            v.iter()
                .map(aspen_node::adoption::adoption_json)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// Answer an adoption: which identity follows the session.
async fn post_adoption(
    State(s): S,
    Path(id): Path<i64>,
    Json(b): Json<AdoptionBody>,
) -> impl IntoResponse {
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(
            &s,
            node,
            "adoption",
            "",
            json!({ "id": id, "action": b.action, "name": b.name }),
        )
        .await;
    }
    match aspen_node::adoption::resolve(&s.node, id, &b.action, b.name.as_deref()).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    }
}

/// Run the adoption scan now (the console's "check" and tests).
async fn post_adoptions_scan(State(s): S) -> impl IntoResponse {
    let inner = s.node.inner.clone();
    match tokio::task::spawn_blocking(move || aspen_node::adoption::scan(&inner)).await {
        Ok(Ok(raised)) => Json(json!({ "ok": true, "raised": raised })).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

/// Claude Code's SessionStart/SessionEnd hook, relayed by `aspen hook`.
/// Body is the hook's own JSON (session_id, transcript_path, cwd, source /
/// reason, hook_event_name). We only use it as a nudge.
async fn post_hook_session(State(s): S, Json(b): Json<Value>) -> impl IntoResponse {
    let cwd = b
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::PathBuf::from);
    let _ = s.node.inner.store.record_event(
        "node",
        "hook_session",
        json!({
            "event": b.get("hook_event_name"), "session": b.get("session_id"),
            "cwd": b.get("cwd"), "source": b.get("source"), "reason": b.get("reason"),
        }),
    );
    aspen_node::adoption::on_hook(&s.node.inner, cwd.as_deref());
    Json(json!({ "ok": true }))
}

/// Mark the operator inbox read — locally and on every connected peer.
async fn post_needs_read(State(s): S) -> impl IntoResponse {
    let store = &s.node.inner.store;
    if let Ok(rows) = store.pending_for("operator") {
        let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
        let _ = store.mark_delivered(&ids, "operator-ui", None);
    }
    if let Some(mesh) = s.node.inner.mesh() {
        let peers: Vec<String> = mesh.links.lock().unwrap().keys().cloned().collect();
        for peer in peers {
            let _ = mesh
                .api_call(
                    &peer,
                    "inbox_read",
                    "",
                    json!({}),
                    std::time::Duration::from_secs(5),
                )
                .await;
        }
    }
    Json(json!({})).into_response()
}

// -------------------------------------------------------- direct messages

async fn get_dms(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.dm_pairs() {
        Ok(rows) => Json(
            rows.iter()
                .map(|(a, b, last, n)| json!({ "a": a, "b": b, "last_at": last, "messages": n }))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct DmQuery {
    a: String,
    b: String,
    n: Option<i64>,
}

async fn get_dm(State(s): S, Query(q): Query<DmQuery>) -> impl IntoResponse {
    match s.node.inner.store.dm_log(&q.a, &q.b, q.n.unwrap_or(200)) {
        Ok(rows) => Json(rows.iter().map(message_json).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// Per-recipient receipts for one logical post — watch a routed message land.
async fn get_post_receipts(State(s): S, Path(post): Path<String>) -> impl IntoResponse {
    match s.node.inner.store.post_receipts(&post) {
        Ok(rows) => Json(rows.iter().map(message_json).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

// ---------------------------------------------------------------- mesh info

/// Mesh legibility: identity, peers with live link state, relay health.
async fn get_mesh(State(s): S) -> impl IntoResponse {
    let data_dir = s.node.inner.data_dir.clone();
    let pending = data_dir
        .as_deref()
        .map(aspen_node::pending::load)
        .unwrap_or_default();
    let Some(mesh) = s.node.inner.mesh() else {
        // Not in a mesh — but the files may say otherwise (enrolled, not
        // joined), and the console can still author the ceremony.
        let files = data_dir.as_deref().map(aspen_node::mesh::MeshFiles::new);
        let identity = files
            .as_ref()
            .and_then(|f| f.load_identity().ok().flatten());
        return Json(json!({
            "in_mesh": false,
            "node": s.node_name,
            "identity": identity.map(|id| json!({
                "node": id.node,
                "fingerprint": aspen_node::federation::fingerprint(&id.ed_public),
                "certified": id.cert.is_some(),
                "enroll_blob": aspen_wire::identity::to_blob("enroll", &id.join_request()).ok(),
            })),
            "pending": pending,
        }))
        .into_response();
    };
    // Before the std guards below: an await while holding them would make
    // this future !Send.
    let hosted_present = s.relay.present().await;
    let hosted_waiting = s.relay.waiting().await;
    let relays: Vec<Value> = {
        let up = mesh.relay_up.lock().unwrap();
        let errs = mesh.relay_errors.lock().unwrap();
        mesh.relay_urls()
            .iter()
            .map(|u| {
                json!({
                    "url": u,
                    "connected_at": up.get(u).map(|(t, _)| *t),
                    "last_error": errs.get(u).map(|(e, _)| e.clone()),
                    "last_error_at": errs.get(u).map(|(_, t)| *t),
                })
            })
            .collect()
    };
    let mailed = mesh.mailed.lock().unwrap().len();
    let discovered: Vec<Value> = {
        let up = mesh.relay_up.lock().unwrap();
        mesh.discovered_relays
            .lock()
            .unwrap()
            .iter()
            .map(|(u, from)| json!({ "url": u, "from": from, "connected_at": up.get(u).map(|(t, _)| *t) }))
            .collect()
    };
    let my_adv = aspen_node::federation::advertised(&s.node.inner);
    let kinds = mesh.link_kind.lock().unwrap().clone();
    let links = mesh.links.lock().unwrap();
    let remote = mesh.remote.lock().unwrap();
    let health = mesh.health.lock().unwrap();
    let peers: Vec<Value> = mesh
        .peers()
        .iter()
        .map(|p| {
            let name = &p.cert.node;
            let h = health.get(name).cloned().unwrap_or_default();
            json!({
                "node": name,
                "url": p.url,
                // Where that node's console probably is: its dial URL minus
                // the federation path (a guess when it listens headless).
                "console_url": p.url.as_deref().and_then(console_url_of),
                "link_up": links.contains_key(name),
                // "direct" or "relay:<url>" while linked.
                "link_kind": kinds.get(name),
                "advertised": h.advertised,
                "agents": remote.get(name).map(|v| v.len()).unwrap_or(0),
                "fingerprint": aspen_node::federation::fingerprint(&p.cert.ed_public),
                "has_root": h.has_root,
                "health": h,
            })
        })
        .collect();
    let (version, sha) = aspen_node::federation::VERSION
        .get()
        .cloned()
        .unwrap_or_default();
    let cert_blob = mesh
        .identity
        .cert
        .as_ref()
        .and_then(|c| aspen_wire::identity::to_blob("cert", c).ok());
    let has_root = data_dir
        .as_deref()
        .map(aspen_node::mesh::MeshFiles::new)
        .and_then(|f| f.load_root().ok().flatten())
        .is_some();
    Json(json!({
        "in_mesh": true,
        "mesh": mesh.mesh_name(),
        "node": mesh.identity.node,
        "identity": {
            "node": mesh.identity.node,
            "fingerprint": aspen_node::federation::fingerprint(&mesh.identity.ed_public),
            "certified": true,
            "cert_blob": cert_blob,
            "version": version,
            "sha": sha,
            "has_root": has_root,
            "root_key_path": if has_root { data_dir.as_deref().map(|d| d.join("root.key").to_string_lossy().into_owned()) } else { None },
            // What this node tells peers about reaching it; empty = spoke.
            "advertised": my_adv,
        },
        "root_public": aspen_wire::b64::encode(&mesh.root_public()),
        "peers": peers,
        "relay": {
            // First relay, for older readers; `relays` has them all.
            "url": mesh.relay_url(),
            "connected_at": *mesh.relay_connected_at.lock().unwrap(),
            "relays": relays,
            // Relays peers host, learned from their rosters (not configured).
            "discovered": discovered,
            // Bus rows this node has handed to a relay mailbox, awaiting the
            // peer's ack.
            "mailed": mailed,
            // This node's own relay endpoint: who is rendezvousing here and
            // whose mail is waiting here.
            "hosted_path": "/api/federation/relay",
            "hosted_present": hosted_present,
            "hosted_waiting": hosted_waiting,
        },
        "pending": pending,
    }))
    .into_response()
}

/// ws://host:port/api/federation/ws → http://host:port
fn console_url_of(dial: &str) -> Option<String> {
    let (scheme, rest) = dial.split_once("://")?;
    let host = rest.split('/').next()?;
    let http = match scheme {
        "ws" => "http",
        "wss" => "https",
        other => other,
    };
    Some(format!("{http}://{host}"))
}

// ---------------------------------------------------------- mesh ceremony

#[derive(Deserialize)]
struct InspectBody {
    blob: String,
}

/// Read-only: parse an enroll / cert / bundle blob and say what it is and
/// what accepting it would do here, with warnings (duplicate names…).
async fn post_mesh_inspect(State(s): S, Json(b): Json<InspectBody>) -> impl IntoResponse {
    use aspen_node::mesh::JoinBundle;
    use aspen_wire::identity::{self, JoinRequest, NodeCert};
    let blob = b.blob.trim();
    let files = s
        .node
        .inner
        .data_dir
        .as_deref()
        .map(aspen_node::mesh::MeshFiles::new);
    let my_name = files
        .as_ref()
        .and_then(|f| f.load_identity().ok().flatten())
        .map(|id| id.node);
    let peer_names: Vec<String> = files
        .as_ref()
        .and_then(|f| f.verified_peers().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.cert.node)
        .collect();
    let mut warnings: Vec<String> = Vec::new();
    let dup = |name: &str, warnings: &mut Vec<String>| {
        if my_name.as_deref() == Some(name) {
            warnings.push(format!(
                "'{name}' is THIS node's name — re-enroll the other node with a distinct name"
            ));
        } else if peer_names.iter().any(|p| p == name) {
            warnings.push(format!("a peer named '{name}' already exists in this mesh"));
        }
    };
    if let Ok(req) = identity::from_blob::<JoinRequest>("enroll", blob) {
        dup(&req.node, &mut warnings);
        return Json(json!({
            "kind": "enroll", "node": req.node,
            "fingerprint": aspen_node::federation::fingerprint(&req.ed_public),
            "warnings": warnings,
            "next": "certify here (needs the root key on this node)",
        }))
        .into_response();
    }
    if let Ok(bun) = identity::from_blob::<JoinBundle>("bundle", blob) {
        return Json(json!({
            "kind": "bundle", "node": bun.cert.node, "mesh": bun.cert.mesh,
            "fingerprint": aspen_node::federation::fingerprint(&bun.cert.ed_public),
            "certifier": bun.certifier.node,
            "certifier_fingerprint": aspen_node::federation::fingerprint(&bun.certifier.ed_public),
            "certifier_url": bun.certifier_url, "relay": bun.relay,
            "warnings": warnings,
            "next": "join here (installs this node's cert and registers the certifier as a peer)",
        }))
        .into_response();
    }
    if let Ok(cert) = identity::from_blob::<NodeCert>("cert", blob) {
        dup(&cert.node, &mut warnings);
        return Json(json!({
            "kind": "cert", "node": cert.node, "mesh": cert.mesh,
            "fingerprint": aspen_node::federation::fingerprint(&cert.ed_public),
            "warnings": warnings,
            "next": "peers-add here (registers this node as a peer, optionally with a dial URL)",
        }))
        .into_response();
    }
    err(
        StatusCode::BAD_REQUEST,
        "not an aspen enroll/cert/bundle blob",
    )
    .into_response()
}

#[derive(Deserialize)]
struct ProposeBody {
    kind: String,
    #[serde(default)]
    args: Value,
}

/// Queue a mesh change for `aspen mesh apply`. The daemon never executes
/// these — see pending.rs.
async fn post_mesh_pending(State(s): S, Json(b): Json<ProposeBody>) -> impl IntoResponse {
    if ![
        "enroll",
        "certify",
        "join",
        "peers_add",
        "relay",
        "init",
        "peers_remove",
        "leave",
    ]
    .contains(&b.kind.as_str())
    {
        return err(
            StatusCode::BAD_REQUEST,
            "kind must be enroll|certify|join|peers_add|relay|init|peers_remove|leave",
        )
        .into_response();
    }
    let Some(dir) = s.node.inner.data_dir.as_deref() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "node has no data dir").into_response();
    };
    match aspen_node::pending::propose(dir, &b.kind, b.args, "console") {
        Ok(p) => {
            Json(json!({ "ok": true, "proposal": p, "apply": "aspen mesh apply" })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn get_mesh_pending(State(s): S) -> impl IntoResponse {
    let q = s
        .node
        .inner
        .data_dir
        .as_deref()
        .map(aspen_node::pending::load)
        .unwrap_or_default();
    Json(q)
}

async fn delete_mesh_pending(State(s): S, Path(id): Path<String>) -> impl IntoResponse {
    let Some(dir) = s.node.inner.data_dir.as_deref() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "node has no data dir").into_response();
    };
    if id == "outcomes" {
        let _ = aspen_node::pending::clear_outcomes(dir);
        return Json(json!({ "ok": true })).into_response();
    }
    match aspen_node::pending::withdraw(dir, &id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such proposal").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

// -------------------------------------------------------------- trust gate// -------------------------------------------------------------- trust gate

async fn post_repo_trust(State(s): S, Json(b): Json<RepoPathBody>) -> impl IntoResponse {
    let path = aspen_node::node::normalize_repo(std::path::Path::new(&b.path));
    match s.node.record_trust(&path) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn post_repo_untrust(State(s): S, Json(b): Json<RepoPathBody>) -> impl IntoResponse {
    let path = aspen_node::node::normalize_repo(std::path::Path::new(&b.path));
    match s.node.revoke_trust(&path) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

// ------------------------------------------------------------------- channels

/// Every channel the operator can address: the auto per-repo channels
/// (derived from where agents live) and custom channels (explicit
/// membership, may span repos/nodes). Auto and custom are distinguished so
/// the UI can present them differently.
async fn get_channels(State(s): S) -> impl IntoResponse {
    let mut out: Vec<Value> = Vec::new();

    // Auto repo channels: group agents by their channel.
    let agents = s.node.inner.store.agents().unwrap_or_default();
    let mut by_channel: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for a in &agents {
        by_channel
            .entry(a.channel.clone())
            .or_default()
            .push(a.name.clone());
    }
    // Remote roster contributes auto members too.
    if let Some(mesh) = s.node.inner.mesh() {
        for ras in mesh.remote.lock().unwrap().values() {
            for ra in ras {
                by_channel
                    .entry(ra.channel.clone())
                    .or_default()
                    .push(ra.name.clone());
            }
        }
    }
    for (name, members) in by_channel {
        out.push(json!({
            "name": name, "kind": "repo", "topic": Value::Null,
            "members": members, "member_count": null,
        }));
    }

    // Custom channels.
    if let Ok(chans) = s.node.inner.store.channels() {
        for (name, topic, count) in chans {
            let members = s
                .node
                .inner
                .store
                .custom_channel_members(&name)
                .unwrap_or_default();
            out.push(json!({
                "name": name, "kind": "custom", "topic": topic,
                "members": members, "member_count": count,
            }));
        }
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct ChannelCreate {
    name: String,
    topic: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

async fn post_channel(State(s): S, Json(b): Json<ChannelCreate>) -> impl IntoResponse {
    let name = b.name.trim().trim_start_matches('#').to_owned();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return err(StatusCode::BAD_REQUEST, "channel names are [A-Za-z0-9_-]+").into_response();
    }
    let store = &s.node.inner.store;
    if let Err(e) = store.create_channel(&name, b.topic.as_deref()) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    for m in &b.members {
        let m = aspen_node::tools::canonical_member(m);
        let _ = store.add_channel_member(&name, &m);
    }
    Json(json!({ "ok": true, "name": name })).into_response()
}

async fn delete_channel_route(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    match s
        .node
        .inner
        .store
        .delete_channel(name.trim_start_matches('#'))
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct MemberBody {
    member: String,
}

async fn post_channel_member(
    State(s): S,
    Path(name): Path<String>,
    Json(b): Json<MemberBody>,
) -> impl IntoResponse {
    let name = name.trim_start_matches('#');
    let store = &s.node.inner.store;
    let _ = store.create_channel(name, None); // ensure it exists
    match store.add_channel_member(name, &aspen_node::tools::canonical_member(&b.member)) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn remove_channel_member_route(
    State(s): S,
    Path(name): Path<String>,
    Json(b): Json<MemberBody>,
) -> impl IntoResponse {
    match s.node.inner.store.remove_channel_member(
        name.trim_start_matches('#'),
        &aspen_node::tools::canonical_member(&b.member),
    ) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn get_channel_log(
    State(s): S,
    Path(name): Path<String>,
    Query(q): Query<LogQuery>,
) -> impl IntoResponse {
    let display = format!("#{}", name.trim_start_matches('#'));
    match s.node.inner.store.channel_log(&display, q.n.unwrap_or(100)) {
        Ok(posts) => Json(
            posts
                .iter()
                .map(|p| {
                    json!({
                        "post": p.post, "sender": p.sender, "urgency": p.urgency,
                        "body": p.body, "thread": p.thread, "record": p.record_ref,
                        "created_at": p.created_at, "recipients": p.recipients,
                        "delivered": p.delivered, "ingested": p.ingested,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

// ------------------------------------------------------------------- activity

/// A mesh-wide snapshot for the home/command surface: every session with its
/// live turn-state and pending count (local + remote), plus the recent trail
/// tail. Presence is DERIVED from turn-state, so it can't lie.
async fn get_activity(State(s): S) -> impl IntoResponse {
    let mut sessions: Vec<Value> = Vec::new();
    let node_name = s
        .node
        .inner
        .mesh()
        .map(|m| m.identity.node.clone())
        .unwrap_or_else(|| s.node_name.clone());
    for a in s.node.inner.store.agents().unwrap_or_default() {
        let live = s.node.inner.live(&a.name);
        let (busy_since, last_tool) = live
            .as_ref()
            .map(|m| m.presence_detail())
            .unwrap_or((None, None));
        sessions.push(json!({
            "name": a.name, "node": node_name, "channel": a.channel,
            "repo": a.repo.to_string_lossy(),
            "title": a.title,
            "live": live.is_some(),
            "turn_state": live.as_ref().map(|m| match m.turn_state() {
                TurnState::Idle => "idle", TurnState::Busy => "busy" }),
            "busy_since": busy_since,
            "last_tool": last_tool,
            "pending": s.node.inner.store.pending_count(&a.name).unwrap_or(0),
            "remote": false,
        }));
    }
    if let Some(mesh) = s.node.inner.mesh() {
        for (node, ras) in mesh.remote.lock().unwrap().iter() {
            let reachable = mesh.link_up(node);
            for ra in ras {
                sessions.push(json!({
                    "name": format!("{}@{}", ra.name, node), "node": node,
                    "channel": ra.channel, "repo": Value::Null,
                    "live": ra.live && reachable,
                    "turn_state": if reachable { json!(ra.turn_state) } else { Value::Null },
                    "pending": s.node.inner.store.pending_count(&format!("{}@{}", ra.name, node)).unwrap_or(0),
                    "remote": true,
                }));
            }
        }
    }
    let recent = s.node.inner.store.log(200).unwrap_or_default();
    let trail: Vec<Value> = recent
        .iter()
        .rev()
        .take(40)
        .rev()
        .map(message_json)
        .collect();

    // Waiting-on heuristic: an IDLE agent whose most recent outbound direct
    // message has seen no reply is *likely waiting* on that counterpart.
    // Honest labeling ("likely") — this is inference from the trail, not a
    // protocol fact.
    let mut waiting: Vec<Value> = Vec::new();
    for sv in &sessions {
        if sv["live"] != json!(true) || sv["turn_state"] != json!("idle") {
            continue;
        }
        let name = sv["name"].as_str().unwrap_or_default();
        let last_out = recent
            .iter()
            .rev()
            .find(|m| m.sender == name && !m.to_display.starts_with('#'));
        if let Some(out) = last_out {
            let replied = recent.iter().any(|m| {
                m.id > out.id
                    && (m.sender == out.recipient
                        || m.sender == format!("{}@{}", out.recipient, node_name))
                    && (m.recipient == name || m.recipient.starts_with(&format!("{name}@")))
            });
            if !replied {
                let snippet: String = out.body.chars().take(90).collect();
                waiting.push(json!({
                    "agent": name,
                    "on": out.recipient,
                    "since": out.created_at,
                    "snippet": snippet,
                }));
            }
        }
    }

    let inbox = s.node.inner.store.pending_count("operator").unwrap_or(0);
    Json(json!({ "sessions": sessions, "trail": trail, "inbox": inbox, "waiting": waiting }))
        .into_response()
}

// ---------------------------------------------------------------------- repos

// ---------------------------------------------------------------- history

#[derive(Deserialize)]
struct HistoryQuery {
    /// Window, epoch seconds. Defaults: the last 24h.
    from: Option<f64>,
    to: Option<f64>,
    agent: Option<String>,
    n: Option<i64>,
    /// Include peers' events (default true).
    mesh: Option<bool>,
}

/// The fleet's timeline: events (asks, turns, tools, prompts, exits,
/// spawns/branches) plus bus messages in a window, this node and every
/// reachable peer, each item tagged with its node.
async fn get_history(State(s): S, Query(q): Query<HistoryQuery>) -> impl IntoResponse {
    let now = aspen_node::store::now_epoch();
    let to = q.to.unwrap_or(now);
    let from = q.from.unwrap_or(to - 86400.0);
    let n = q.n.unwrap_or(2000).clamp(1, 20000);
    let me = self_node_name(&s);
    let mut events: Vec<Value> = s
        .node
        .inner
        .store
        .events(from, to, q.agent.as_deref(), n)
        .unwrap_or_default()
        .into_iter()
        .map(|e| json!({ "id": e.id, "ts": e.ts, "agent": e.agent, "kind": e.kind, "detail": e.detail, "node": me }))
        .collect();
    let mut messages: Vec<Value> = s
        .node
        .inner
        .store
        .messages_between(from, to, n)
        .unwrap_or_default()
        .iter()
        .filter(|m| {
            q.agent
                .as_deref()
                .is_none_or(|a| m.sender == a || m.recipient == a)
        })
        .map(|m| {
            let mut v = message_json(m);
            v["node"] = json!(me);
            v
        })
        .collect();

    if q.mesh.unwrap_or(true) {
        if let Some(mesh) = s.node.inner.mesh() {
            let calls = mesh
                .peers()
                .into_iter()
                .filter(|p| mesh.link_up(&p.cert.node))
                .map(|peer| {
                    let mesh = mesh.clone();
                    let agent = q.agent.clone();
                    async move {
                        let name = peer.cert.node.clone();
                        let r = mesh
                            .api_call(
                                &name,
                                "history",
                                "",
                                json!({ "from": from, "to": to, "agent": agent, "n": n }),
                                LIST_TIMEOUT,
                            )
                            .await
                            .ok();
                        (name, r)
                    }
                });
            for (name, r) in futures_util::future::join_all(calls).await {
                let Some(v) = r else { continue };
                for e in v
                    .get("events")
                    .and_then(|e| e.as_array())
                    .cloned()
                    .unwrap_or_default()
                {
                    let mut e = e;
                    e["node"] = json!(name);
                    // Remote agents are addressed here as key@node.
                    if let Some(a) = e.get("agent").and_then(|a| a.as_str()) {
                        e["agent"] = json!(format!("{a}@{name}"));
                    }
                    events.push(e);
                }
                for m in v
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .cloned()
                    .unwrap_or_default()
                {
                    let mut m = m;
                    m["node"] = json!(name);
                    messages.push(m);
                }
            }
        }
    }
    events.sort_by(|a, b| {
        a["ts"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["ts"].as_f64().unwrap_or(0.0))
    });
    messages.sort_by(|a, b| {
        a["created_at"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["created_at"].as_f64().unwrap_or(0.0))
    });
    Json(json!({ "from": from, "to": to, "self": me, "events": events, "messages": messages }))
        .into_response()
}

// ------------------------------------------------------------------ links

#[derive(Deserialize)]
struct LinkBody {
    /// Endpoints: `agent:name@repo[@node]`, `repo:handle[@node]`,
    /// `node:name`, `operator` (bare `@x`/`#x` accepted).
    from: String,
    to: String,
    #[serde(default)]
    two_way: bool,
    purpose: Option<String>,
    urgency: Option<String>,
}

fn endpoint_node(s: &AppState, e: &aspen_node::topology::Endpoint) -> Option<String> {
    use aspen_node::topology::Endpoint;
    let n = match e {
        Endpoint::Agent { node, .. } | Endpoint::Repo { node, .. } => node.clone(),
        Endpoint::Node(n) => Some(n.clone()),
        Endpoint::Operator => None,
    };
    n.filter(|n| !is_self_node(s, n))
}

/// Declare a link. Stored here, and mirrored to any peer node an endpoint
/// lives on, so agents on both ends are told about it.
async fn post_link(State(s): S, Json(b): Json<LinkBody>) -> impl IntoResponse {
    use aspen_node::topology::Endpoint;
    let (from, to) = match (Endpoint::parse(&b.from), Endpoint::parse(&b.to)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => return err(StatusCode::BAD_REQUEST, e).into_response(),
    };
    if from == to {
        return err(
            StatusCode::BAD_REQUEST,
            "a link needs two different endpoints",
        )
        .into_response();
    }
    if let Some(u) = b.urgency.as_deref() {
        if !["gating", "normal", "notice"].contains(&u) {
            return err(
                StatusCode::BAD_REQUEST,
                "urgency must be gating|normal|notice",
            )
            .into_response();
        }
    }
    let (src, dst) = (from.to_string(), to.to_string());
    let id = match s.node.inner.store.add_link(
        &src,
        &dst,
        b.two_way,
        b.purpose.as_deref(),
        b.urgency.as_deref(),
    ) {
        Ok(id) => id,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    // Mirror to peers that host an endpoint.
    let mut peers: Vec<String> = [endpoint_node(&s, &from), endpoint_node(&s, &to)]
        .into_iter()
        .flatten()
        .collect();
    peers.dedup();
    if let Some(mesh) = s.node.inner.mesh() {
        for peer in peers {
            let _ = mesh
                .api_call(
                    &peer,
                    "link_add",
                    "",
                    json!({ "src": src, "dst": dst, "two_way": b.two_way, "purpose": b.purpose, "urgency": b.urgency }),
                    REMOTE_TIMEOUT,
                )
                .await;
        }
    }
    Json(json!({ "ok": true, "id": id })).into_response()
}

async fn get_links(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.links() {
        Ok(ls) => Json(ls).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn delete_link(State(s): S, Path(id): Path<i64>) -> impl IntoResponse {
    use aspen_node::topology::Endpoint;
    let Some(link) = s
        .node
        .inner
        .store
        .links()
        .unwrap_or_default()
        .into_iter()
        .find(|l| l.id == id)
    else {
        return err(StatusCode::NOT_FOUND, "no such link").into_response();
    };
    let _ = s.node.inner.store.delete_link(id);
    let ends = [
        Endpoint::parse(&link.src).ok(),
        Endpoint::parse(&link.dst).ok(),
    ];
    let mut peers: Vec<String> = ends
        .iter()
        .flatten()
        .filter_map(|e| endpoint_node(&s, e))
        .collect();
    peers.dedup();
    if let Some(mesh) = s.node.inner.mesh() {
        for peer in peers {
            let _ = mesh
                .api_call(
                    &peer,
                    "link_del",
                    "",
                    json!({ "src": link.src, "dst": link.dst }),
                    REMOTE_TIMEOUT,
                )
                .await;
        }
    }
    Json(json!({ "ok": true })).into_response()
}

/// Apply mesh files to the running daemon (join live, pick up peers/relay).
/// The mesh CLI calls this after every mutation so no restart is needed.
async fn post_mesh_reload(State(s): S) -> impl IntoResponse {
    match s.node.reload_mesh() {
        Ok(summary) => Json(json!({ "ok": true, "summary": summary })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// True when `node` names this node (or the mesh isn't set up, so
/// everything is local).
fn is_self_node(s: &AppState, node: &str) -> bool {
    match s.node.inner.mesh() {
        Some(m) => m.identity.node == node,
        None => true,
    }
}

/// The self node's name, or "local" when not in a mesh.
fn self_node_name(s: &AppState) -> String {
    s.node
        .inner
        .mesh()
        .map(|m| m.identity.node.clone())
        .unwrap_or_else(|| "local".into())
}

/// Mesh-wide repo registry, grouped by node: this node plus every peer with
/// a live link. Peers that are down are listed as unreachable with no repos
/// (their content lives on them; a broken link hides it, by design).
async fn get_mesh_repos(State(s): S) -> impl IntoResponse {
    let mut nodes = Vec::new();
    let local: Vec<Value> = s
        .node
        .inner
        .store
        .repos()
        .unwrap_or_default()
        .iter()
        .map(|r| repo_json(&s, r))
        .collect();
    nodes.push(json!({
        "node": self_node_name(&s),
        "self": true,
        "reachable": true,
        "repos": local,
    }));

    if let Some(mesh) = s.node.inner.mesh() {
        // All peers at once, with a listing-appropriate timeout — one slow
        // or wedged peer must not hold the whole Library hostage.
        let calls = mesh.peers().into_iter().map(|peer| {
            let mesh = mesh.clone();
            async move {
                let name = peer.cert.node.clone();
                let reachable = mesh.link_up(&name);
                let repos = if reachable {
                    mesh.api_call(&name, "node_repos", "", json!({}), LIST_TIMEOUT)
                        .await
                        .ok()
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                json!({
                    "node": name,
                    "self": false,
                    "reachable": reachable,
                    "repos": repos,
                })
            }
        });
        nodes.extend(futures_util::future::join_all(calls).await);
    }
    Json(json!({ "nodes": nodes }))
}

fn repo_json(s: &AppState, r: &aspen_node::store::RepoRow) -> Value {
    // How many discovered sessions and how many live agents this repo has.
    let sessions = aspen_claude::transcript::enumerate_sessions(&r.path)
        .map_or(0, |v| v.iter().filter(|si| si.user_messages > 0).count());
    let live = s
        .node
        .inner
        .store
        .agents()
        .unwrap_or_default()
        .iter()
        .filter(|a| a.repo == r.path && s.node.inner.live(&a.name).is_some())
        .count();
    let (autorun, trusted) = s.node.trust_state(&r.path);
    json!({
        "path": r.path.to_string_lossy(),
        "handle": r.handle,
        "git": aspen_node::gitstate::get(&r.path),
        "skip_permissions": r.skip_permissions,
        "last_used_at": r.last_used_at,
        "sessions": sessions,
        "live_agents": live,
        "trusted": trusted,
        "has_autorun": autorun.has_autorun,
    })
}

async fn get_repos(State(s): S) -> impl IntoResponse {
    match s.node.inner.store.repos() {
        Ok(rows) => Json(rows.iter().map(|r| repo_json(&s, r)).collect::<Vec<_>>()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct RepoAddBody {
    path: String,
    skip_permissions: Option<bool>,
}

async fn post_repo(State(s): S, Json(b): Json<RepoAddBody>) -> impl IntoResponse {
    // Register only real directories, stored in the one normalized form
    // every other entry point uses (see aspen_node::node::normalize_repo).
    let path = match dunce::canonicalize(std::path::Path::new(&b.path)) {
        Ok(p) if p.is_dir() => p,
        Ok(_) => return err(StatusCode::BAD_REQUEST, "not a directory").into_response(),
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{}: {e}", b.path)).into_response(),
    };
    if let Err(e) = s.node.inner.store.add_repo(&path, b.skip_permissions) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    match s.node.inner.store.repo(&path) {
        Ok(Some(r)) => Json(repo_json(&s, &r)).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "added but not found").into_response(),
    }
}

#[derive(Deserialize)]
struct RepoSkipBody {
    path: String,
    skip_permissions: bool,
    /// Owning node; absent or this node's name = local.
    node: Option<String>,
}

async fn post_repo_skip(State(s): S, Json(b): Json<RepoSkipBody>) -> impl IntoResponse {
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(
            &s,
            node,
            "node_repo_skip",
            "",
            json!({ "path": b.path, "skip_permissions": b.skip_permissions }),
        )
        .await;
    }
    let path = aspen_node::node::normalize_repo(std::path::Path::new(&b.path));
    match s.node.inner.store.set_repo_skip(&path, b.skip_permissions) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct RepoPathBody {
    path: String,
    /// Owning node; absent or this node's name = local.
    node: Option<String>,
}

async fn post_repo_forget(State(s): S, Json(b): Json<RepoPathBody>) -> impl IntoResponse {
    if let Some(node) = b.node.as_deref().filter(|n| !is_self_node(&s, n)) {
        return proxy(&s, node, "node_repo_forget", "", json!({ "path": b.path })).await;
    }
    let path = aspen_node::node::normalize_repo(std::path::Path::new(&b.path));
    match s.node.inner.store.remove_repo(&path) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// The trust gate's inspection: exactly what this repo would auto-run on
/// spawn (hooks, MCP servers, skills). "Here is everything this repository
/// will run, before it runs" (reference §7.7).
async fn get_autorun(Query(q): Query<RepoQuery>) -> impl IntoResponse {
    Json(aspen_node::trust::inspect(std::path::Path::new(&q.repo))).into_response()
}

async fn get_skills(Query(q): Query<RepoQuery>) -> impl IntoResponse {
    match aspen_node::skills::list(std::path::Path::new(&q.repo)) {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct SkillQuery {
    repo: String,
    rel: String,
}

async fn get_skill(Query(q): Query<SkillQuery>) -> impl IntoResponse {
    match aspen_node::skills::read(std::path::Path::new(&q.repo), &q.rel) {
        Ok(content) => Json(json!({ "content": content })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[derive(Deserialize)]
struct SkillWrite {
    repo: String,
    rel: String,
    content: String,
    /// Reload live sessions in this repo after saving (default true).
    #[serde(default = "yes")]
    reload: bool,
}

fn yes() -> bool {
    true
}

async fn put_skill(State(s): S, Json(b): Json<SkillWrite>) -> impl IntoResponse {
    let repo = std::path::PathBuf::from(&b.repo);
    if let Err(e) = aspen_node::skills::write(&repo, &b.rel, &b.content) {
        return err(StatusCode::BAD_REQUEST, e).into_response();
    }
    let reloaded = if b.reload {
        s.node.reload_repo(&repo).await
    } else {
        0
    };
    Json(json!({ "ok": true, "reloaded_sessions": reloaded })).into_response()
}

async fn delete_skill(Query(q): Query<SkillQuery>) -> impl IntoResponse {
    match aspen_node::skills::delete(std::path::Path::new(&q.repo), &q.rel) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn post_reload(State(s): S, Path(name): Path<String>) -> impl IntoResponse {
    if let Some((bare, node)) = remote_parts(&s, &name) {
        return proxy(&s, &node, "reload", &bare, json!({})).await;
    }
    match s.node.reload_plugins(&name).await {
        Ok(inv) => Json(inv).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e).into_response(),
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
        "post": m.post,
    })
}

#[derive(Deserialize)]
struct LogQuery {
    n: Option<i64>,
    sender: Option<String>,
    recipient: Option<String>,
    thread: Option<String>,
    record: Option<String>,
    urgency: Option<String>,
    q: Option<String>,
    /// Only rows not yet delivered (stuck / queued).
    pending: Option<bool>,
}

async fn get_bus_log(State(s): S, Query(q): Query<LogQuery>) -> impl IntoResponse {
    let res = s.node.inner.store.log_filtered(
        q.sender.as_deref().filter(|v| !v.is_empty()),
        q.recipient.as_deref().filter(|v| !v.is_empty()),
        q.thread.as_deref().filter(|v| !v.is_empty()),
        q.record.as_deref().filter(|v| !v.is_empty()),
        q.urgency.as_deref().filter(|v| !v.is_empty()),
        q.pending.unwrap_or(false),
        q.q.as_deref().filter(|v| !v.is_empty()),
        q.n.unwrap_or(50),
    );
    match res {
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
