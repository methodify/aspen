//! Notices (docs/NOTIFICATIONS.md): moments the operator may want told
//! about — a turn ending, a question, a permission prompt, a background
//! activity settling, an exit, mail for the operator. Raised by the
//! session pump and the bus, kept in the store for the console to poll,
//! and optionally pushed out through a webhook or a command.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::node::NodeInner;
use crate::store::Notice;

/// Raise a notice: store it, then fire the outbound hook if this kind is
/// configured to. Never blocks the caller on the hook.
pub fn raise(
    inner: &Arc<NodeInner>,
    agent: &str,
    kind: &str,
    title: &str,
    body: Option<&str>,
    link: Option<&str>,
) {
    let notice = match inner.store.add_notice(agent, kind, title, body, link) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(%e, "storing notice");
            return;
        }
    };
    let Some(data_dir) = inner.data_dir.clone() else {
        return;
    };
    let settings = crate::settings::load(&data_dir).notify;
    if !settings.configured() || !settings.fires(kind) {
        return;
    }
    let node = inner.mesh().map(|m| m.identity.node.clone());
    let payload = notice_json(&notice, node.as_deref());
    tokio::spawn(async move {
        if let Some(url) = settings.webhook.as_deref().filter(|u| !u.trim().is_empty()) {
            let url = url.trim().to_owned();
            let body = payload.to_string();
            let r = tokio::task::spawn_blocking(move || {
                ureq::post(&url)
                    .timeout(std::time::Duration::from_secs(5))
                    .set("content-type", "application/json")
                    .send_string(&body)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .await;
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(%e, "notify webhook"),
                Err(e) => tracing::warn!(%e, "notify webhook task"),
            }
        }
        if let Some(cmd) = settings.command.as_deref().filter(|c| !c.trim().is_empty()) {
            let cmd = cmd.trim().to_owned();
            let body = payload.to_string();
            let r = tokio::task::spawn_blocking(move || run_command(&cmd, &body)).await;
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(%e, "notify command"),
                Err(e) => tracing::warn!(%e, "notify command task"),
            }
        }
    });
}

pub fn notice_json(n: &Notice, node: Option<&str>) -> Value {
    json!({
        "id": n.id,
        "ts": n.ts,
        "node": node,
        "agent": n.agent,
        "kind": n.kind,
        "title": n.title,
        "body": n.body,
        "link": n.link,
    })
}

fn run_command(cmd: &str, stdin: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    // quiet_command: a hook must not flash a console on Windows.
    let mut child = if cfg!(windows) {
        aspen_core::quiet_command("cmd").args(["/C", cmd]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
    } else {
        aspen_core::quiet_command("sh").args(["-c", cmd]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
    }
    .map_err(|e| e.to_string())?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(stdin.as_bytes());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                if st.success() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut se) = child.stderr.take() {
                    use std::io::Read;
                    let _ = se.read_to_string(&mut err);
                }
                return Err(format!("exit {st}: {}", err.trim()));
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                return Err("timed out after 5s".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Which activity ids are running for a session right now — the pump keeps
/// the previous set to raise `activity_settled` when one disappears.
pub fn running_ids(inner: &crate::node::NodeInner, harness: aspen_core::Harness, repo: &std::path::Path, session_id: Option<&str>, since: Option<f64>) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Some(sid) = session_id {
        for a in inner.store_for(harness).activities(repo, sid, since) {
            if a.get("status").and_then(|s| s.as_str()) == Some("running") {
                let kind = a.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                let id = a.get("id").and_then(|k| k.as_str()).unwrap_or("");
                let label = a.get("label").and_then(|k| k.as_str()).unwrap_or("").to_owned();
                m.insert(format!("{kind}:{id}"), label);
            }
        }
    }
    m
}
