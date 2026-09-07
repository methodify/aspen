//! Transcript replication (docs/REPLICATION.md, PROPOSALS-B §1), opt-in.
//!
//! A node with `replication.to = <peer>` streams the tail of every
//! session's transcript files — the main `<sid>.jsonl` and the session's
//! `<sid>/` folder (subagent transcripts, tool results) — to that peer
//! as they grow. The peer keeps them under `<data>/replicas/<node>/
//! <agent>/…` with a `replicas` row per file, so a session whose home
//! node is down can still be read from the console, and a move can start
//! from the copy instead of the (unreachable) original.
//!
//! Transport: two ops on the sealed link. `replica_offsets {agent}` asks
//! the peer how much of each file it holds (the source resumes from
//! there after a restart or a link outage); `replica_append {agent,
//! session_id, repo, title, ctx, rel, offset, data, truncate}` appends
//! one chunk. One chunk in flight per session, 256 KB each, a few per
//! tick, so a big backlog drains without starving the roster.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::node::NodeInner;

pub const CHUNK: u64 = 256 * 1024;
const CHUNKS_PER_TICK: usize = 6;
const TICK_SECS: u64 = 5;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplicationSettings {
    /// Peer node that receives this node's transcripts. None: off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Repo handles to replicate; None: every repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos: Option<Vec<String>>,
    /// Refuse replicas from peers when false (default: accept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<bool>,
    /// Prune replicas untouched this long (default 30 days).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_days: Option<u64>,
}

impl ReplicationSettings {
    pub fn accepts(&self) -> bool {
        self.accept.unwrap_or(true)
    }
    pub fn keep_secs(&self) -> f64 {
        self.keep_days.unwrap_or(30) as f64 * 86400.0
    }
    pub fn includes(&self, handle: &str) -> bool {
        match &self.repos {
            Some(r) => r.iter().any(|h| h == handle),
            None => true,
        }
    }
}

/// What the source has confirmed the target holds, per agent and file.
#[derive(Default)]
pub struct SourceState {
    /// agent → (rel → bytes acknowledged). An agent absent here has not
    /// been asked about since the daemon (or the link) came up.
    pub sent: HashMap<String, HashMap<String, u64>>,
    /// The target the offsets were learned from; a change resets them.
    pub target: Option<String>,
}

/// Files that make up a session on disk: `(rel, path)`; rel is the path
/// under the project dir (`<sid>.jsonl`, `<sid>/subagents/agent-x.jsonl`).
pub fn session_files(repo: &Path, sid: &str) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let main = aspen_claude::transcript::transcript_path(repo, sid);
    if main.is_file() {
        out.push((format!("{sid}.jsonl"), main.clone()));
    }
    let Some(project) = main.parent() else {
        return out;
    };
    let sub = project.join(sid);
    if sub.is_dir() {
        let mut stack = vec![(sub.clone(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
                if p.is_dir() {
                    stack.push((p, rel));
                } else if p.is_file() {
                    out.push((format!("{sid}/{rel}"), p));
                }
            }
        }
    }
    out
}

/// The replicator: every few seconds, push what has grown.
pub fn spawn_replicator(inner: Arc<NodeInner>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(TICK_SECS)).await;
            if inner.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if let Err(e) = tick(&inner).await {
                tracing::debug!(%e, "replication tick");
            }
            let _ = prune(&inner);
        }
    });
}

async fn tick(inner: &Arc<NodeInner>) -> Result<()> {
    let Some(dd) = inner.data_dir.clone() else {
        return Ok(());
    };
    let settings = crate::settings::load(&dd).replication;
    let Some(target) = settings.to.clone().filter(|t| !t.trim().is_empty()) else {
        inner.replication.lock().unwrap().sent.clear();
        return Ok(());
    };
    let Some(mesh) = inner.mesh() else {
        return Ok(());
    };
    if target == mesh.identity.node {
        return Ok(());
    }
    if !mesh.link_up(&target) {
        // Offsets learned before the outage may be stale (the target may
        // have pruned); ask again when the link returns.
        inner.replication.lock().unwrap().sent.clear();
        return Ok(());
    }
    {
        let mut st = inner.replication.lock().unwrap();
        if st.target.as_deref() != Some(&target) {
            st.sent.clear();
            st.target = Some(target.clone());
        }
    }
    let rows = inner.store.agents()?;
    let mut budget = CHUNKS_PER_TICK;
    for row in rows {
        if budget == 0 {
            break;
        }
        if row.moved_to.is_some() || !settings.includes(&row.channel) {
            continue;
        }
        let Some(sid) = row.session_id.clone() else {
            continue;
        };
        let files = session_files(&row.repo, &sid);
        if files.is_empty() {
            continue;
        }
        // Learn what the target holds, once per agent per link-life.
        let known = inner.replication.lock().unwrap().sent.get(&row.name).cloned();
        let mut offsets = match known {
            Some(k) => k,
            None => {
                let v = mesh
                    .api_call(
                        &target,
                        "replica_offsets",
                        &row.name,
                        json!({}),
                        std::time::Duration::from_secs(20),
                    )
                    .await?;
                let m: HashMap<String, u64> = v
                    .get("offsets")
                    .and_then(|o| serde_json::from_value(o.clone()).ok())
                    .unwrap_or_default();
                inner
                    .replication
                    .lock()
                    .unwrap()
                    .sent
                    .insert(row.name.clone(), m.clone());
                m
            }
        };
        let ctx = crate::migrate::PathCtx::local(&row.repo);
        for (rel, path) in files {
            if budget == 0 {
                break;
            }
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let size = meta.len();
            let have = offsets.get(&rel).copied().unwrap_or(0);
            let (mut offset, truncate) = if size < have { (0, true) } else { (have, false) };
            if size == have && !truncate {
                continue;
            }
            while (offset < size || truncate) && budget > 0 {
                let len = (size - offset).min(CHUNK);
                let data = read_range(&path, offset, len)?;
                let v = mesh
                    .api_call(
                        &target,
                        "replica_append",
                        &row.name,
                        json!({
                            "session_id": sid,
                            "repo": row.channel,
                            "title": row.title,
                            "ctx": ctx,
                            "rel": rel,
                            "offset": offset,
                            "truncate": truncate && offset == 0,
                            "data": aspen_wire::b64::encode(&data),
                        }),
                        std::time::Duration::from_secs(60),
                    )
                    .await?;
                budget -= 1;
                let acked = v.get("bytes").and_then(|b| b.as_u64()).unwrap_or(offset + len);
                offset = acked;
                offsets.insert(rel.clone(), acked);
                inner
                    .replication
                    .lock()
                    .unwrap()
                    .sent
                    .entry(row.name.clone())
                    .or_default()
                    .insert(rel.clone(), acked);
                if truncate {
                    break;
                }
                if len == 0 {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn read_range(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len as usize];
    let mut got = 0usize;
    while got < buf.len() {
        let n = f.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    buf.truncate(got);
    Ok(buf)
}

// ── target side ─────────────────────────────────────────────────────────

pub fn replicas_root(data_dir: &Path) -> PathBuf {
    data_dir.join("replicas")
}

pub fn replica_dir(data_dir: &Path, node: &str, agent: &str) -> PathBuf {
    replicas_root(data_dir).join(safe(node)).join(safe(agent))
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') { c } else { '_' })
        .collect()
}

fn safe_rel(rel: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return Err(anyhow!("bad replica path {rel:?}"));
        }
        out.push(part);
    }
    Ok(out)
}

/// `replica_offsets`: how much of each of this agent's files we hold.
pub fn offsets(inner: &Arc<NodeInner>, from: &str, agent: &str) -> Result<Value> {
    let dd = inner.data_dir.clone().ok_or_else(|| anyhow!("no data dir"))?;
    if !crate::settings::load(&dd).replication.accepts() {
        return Err(anyhow!("this node does not accept replicas"));
    }
    let rows = inner.store.replica_files(from, agent)?;
    let mut m = serde_json::Map::new();
    for (rel, bytes) in rows {
        // Trust the file, not the row, if they disagree (a crash mid-write).
        let p = replica_dir(&dd, from, agent).join(safe_rel(&rel)?);
        let actual = std::fs::metadata(&p).map(|x| x.len()).unwrap_or(0);
        m.insert(rel, json!(actual.min(bytes)));
    }
    Ok(json!({ "offsets": m }))
}

/// `replica_append`: append (or restart) one file.
pub fn append(inner: &Arc<NodeInner>, from: &str, agent: &str, body: &Value) -> Result<Value> {
    let dd = inner.data_dir.clone().ok_or_else(|| anyhow!("no data dir"))?;
    if !crate::settings::load(&dd).replication.accepts() {
        return Err(anyhow!("this node does not accept replicas"));
    }
    let rel = body.get("rel").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("rel required"))?;
    let offset = body.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
    let truncate = body.get("truncate").and_then(|v| v.as_bool()).unwrap_or(false);
    let data = aspen_wire::b64::decode(body.get("data").and_then(|v| v.as_str()).unwrap_or(""))?;
    let dir = replica_dir(&dd, from, agent);
    let path = dir.join(safe_rel(rel)?);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(truncate)
        .open(&path)?;
    let cur = f.metadata()?.len();
    if !truncate && offset > cur {
        // We are behind what the source thinks; tell it where we are.
        return Ok(json!({ "bytes": cur }));
    }
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(&data)?;
    f.flush()?;
    let bytes = offset + data.len() as u64;
    f.set_len(bytes)?;
    inner.store.upsert_replica(&crate::store::ReplicaRow {
        node: from.to_owned(),
        agent: agent.to_owned(),
        session_id: body.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
        repo: body.get("repo").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
        title: body.get("title").and_then(|v| v.as_str()).map(str::to_owned),
        ctx: body.get("ctx").cloned().unwrap_or(Value::Null),
        rel: rel.to_owned(),
        bytes,
        updated_at: crate::store::now_epoch(),
    })?;
    Ok(json!({ "bytes": bytes }))
}

/// A replica of one agent's session held here.
#[derive(Debug, Clone, Serialize)]
pub struct Replica {
    pub node: String,
    pub agent: String,
    pub session_id: String,
    pub repo: String,
    pub title: Option<String>,
    pub bytes: u64,
    pub files: usize,
    pub updated_at: f64,
    pub ctx: Value,
    /// The main transcript file, when held.
    #[serde(skip)]
    pub main: Option<PathBuf>,
    #[serde(skip)]
    pub dir: PathBuf,
}

pub fn list(inner: &Arc<NodeInner>) -> Vec<Replica> {
    let Some(dd) = inner.data_dir.clone() else {
        return Vec::new();
    };
    let rows = inner.store.replicas().unwrap_or_default();
    let mut by: std::collections::BTreeMap<(String, String), Replica> = Default::default();
    for r in rows {
        let key = (r.node.clone(), r.agent.clone());
        let dir = replica_dir(&dd, &r.node, &r.agent);
        let e = by.entry(key).or_insert_with(|| Replica {
            node: r.node.clone(),
            agent: r.agent.clone(),
            session_id: r.session_id.clone(),
            repo: r.repo.clone(),
            title: r.title.clone(),
            bytes: 0,
            files: 0,
            updated_at: 0.0,
            ctx: r.ctx.clone(),
            main: None,
            dir: dir.clone(),
        });
        e.bytes += r.bytes;
        e.files += 1;
        if r.updated_at > e.updated_at {
            e.updated_at = r.updated_at;
            e.session_id = r.session_id.clone();
            if r.title.is_some() {
                e.title = r.title.clone();
            }
            if !r.ctx.is_null() {
                e.ctx = r.ctx.clone();
            }
        }
        if r.rel == format!("{}.jsonl", r.session_id) {
            e.main = Some(dir.join(&r.rel));
        }
    }
    by.into_values().collect()
}

pub fn find(inner: &Arc<NodeInner>, node: &str, agent: &str) -> Option<Replica> {
    list(inner)
        .into_iter()
        .find(|r| r.node == node && r.agent == agent)
}

pub fn replica_json(r: &Replica, held_on: &str) -> Value {
    json!({
        "node": r.node,
        "agent": r.agent,
        "session_id": r.session_id,
        "repo": r.repo,
        "title": r.title,
        "bytes": r.bytes,
        "files": r.files,
        "as_of": r.updated_at,
        "held_on": held_on,
        "has_transcript": r.main.is_some(),
    })
}

/// Drop replicas nobody has written to in `keep_days`.
pub fn prune(inner: &Arc<NodeInner>) -> Result<()> {
    let Some(dd) = inner.data_dir.clone() else {
        return Ok(());
    };
    let keep = crate::settings::load(&dd).replication.keep_secs();
    let cutoff = crate::store::now_epoch() - keep;
    for r in list(inner) {
        if r.updated_at < cutoff {
            let _ = std::fs::remove_dir_all(&r.dir);
            let _ = inner.store.delete_replica(&r.node, &r.agent);
        }
    }
    Ok(())
}

/// Stage a bundle from a replica so `migrate::import` can install it as
/// a session here — tiers A and B, canonicalized with the source's
/// `PathCtx` so paths translate the same way a real export would.
pub fn stage_bundle(data_dir: &Path, r: &Replica) -> Result<PathBuf> {
    use crate::migrate::{BundleFile, Manifest, PathCtx};
    let ctx: PathCtx = serde_json::from_value(r.ctx.clone())
        .map_err(|_| anyhow!("replica of @{} carries no path context; it predates v0.16", r.agent))?;
    let bundle_id = uuid::Uuid::new_v4().to_string();
    let dir = crate::migrate::staging_root(data_dir).join(format!("replica-{bundle_id}"));
    std::fs::create_dir_all(&dir)?;
    let mut files: Vec<BundleFile> = Vec::new();
    let sid = r.session_id.clone();
    for (rel, bytes) in files_in(&r.dir)? {
        let src = r.dir.join(&rel);
        let (tier, dest, out_rel) = if rel == format!("{sid}.jsonl") {
            ("A", "transcript", format!("transcript/{sid}.jsonl"))
        } else if let Some(rest) = rel.strip_prefix(&format!("{sid}/")) {
            ("B", "sidechain", format!("sidechain/{rest}"))
        } else {
            continue;
        };
        let out = dir.join(&out_rel);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let texty = rel.ends_with(".jsonl") || rel.ends_with(".json") || rel.ends_with(".md") || rel.ends_with(".txt");
        if texty {
            let text = std::fs::read_to_string(&src).unwrap_or_default();
            let canon = crate::migrate::rewrite_jsonl(&text, &|s| crate::migrate::canonicalize_str(s, &ctx));
            std::fs::write(&out, canon)?;
        } else {
            std::fs::copy(&src, &out)?;
        }
        files.push(BundleFile {
            tier: tier.into(),
            rel: out_rel,
            kind: if rel.ends_with(".jsonl") { "jsonl".into() } else if texty { "text".into() } else { "binary".into() },
            dest: dest.into(),
            canon_path: None,
            size: bytes,
        });
    }
    let manifest = Manifest {
        version: 1,
        bundle_id: bundle_id.clone(),
        created_at: crate::store::now_epoch(),
        source_node: r.node.clone(),
        source: ctx,
        harness_version: None,
        mode: "copy".into(),
        agent: crate::migrate::AgentSpec {
            name: r.agent.clone(),
            channel: r.repo.clone(),
            session_id: sid,
            charter: None,
            title: r.title.clone(),
            extra_args: None,
            repo_basename: r.repo.clone(),
            repo_origin: None,
        },
        tiers: vec!["A".into(), "B".into()],
        files,
        lineage: Vec::new(),
        bookmarks: Vec::new(),
        notes: vec![format!(
            "started from the replica held on this node; {} may still run the original",
            r.node
        )],
    };
    std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    Ok(dir)
}

/// Every file under a replica dir as `(rel, bytes)`.
fn files_in(dir: &Path) -> Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            if p.is_dir() {
                stack.push((p, rel));
            } else if let Ok(m) = p.metadata() {
                out.push((rel, m.len()));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_paths_are_confined() {
        assert!(safe_rel("abc.jsonl").is_ok());
        assert!(safe_rel("abc/subagents/agent-1.jsonl").is_ok());
        assert!(safe_rel("../x").is_err());
        assert!(safe_rel("a/../../x").is_err());
        assert!(safe_rel("a\\b").is_err());
        assert!(safe_rel("").is_err());
    }

    #[test]
    fn walks_a_replica_dir() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("s.jsonl"), b"12345").unwrap();
        std::fs::create_dir_all(d.path().join("s/subagents")).unwrap();
        std::fs::write(d.path().join("s/subagents/agent-1.jsonl"), b"123").unwrap();
        let mut v = files_in(d.path()).unwrap();
        v.sort();
        assert_eq!(v, vec![("s.jsonl".to_owned(), 5), ("s/subagents/agent-1.jsonl".to_owned(), 3)]);
    }

    #[test]
    fn stages_a_bundle_with_canonical_paths() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        let rdir = d.path().join("replica");
        std::fs::create_dir_all(&rdir).unwrap();
        let ctx = crate::migrate::PathCtx {
            os: "unix".into(),
            home: "/home/alice".into(),
            repo: "/home/alice/src/hub".into(),
            project_dir: "/home/alice/.claude/projects/-home-alice-src-hub".into(),
            encoded: "-home-alice-src-hub".into(),
            tmp: "/tmp".into(),
        };
        std::fs::write(
            rdir.join("sid1.jsonl"),
            "{\"cwd\":\"/home/alice/src/hub\",\"type\":\"user\"}\n",
        )
        .unwrap();
        let r = Replica {
            node: "j1".into(),
            agent: "far@hub".into(),
            session_id: "sid1".into(),
            repo: "hub".into(),
            title: None,
            bytes: 10,
            files: 1,
            updated_at: 0.0,
            ctx: serde_json::to_value(&ctx).unwrap(),
            main: Some(rdir.join("sid1.jsonl")),
            dir: rdir.clone(),
        };
        let out = stage_bundle(&data, &r).unwrap();
        let manifest: crate::migrate::Manifest =
            serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.mode, "copy");
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].tier, "A");
        let staged = std::fs::read_to_string(out.join("transcript/sid1.jsonl")).unwrap();
        assert!(staged.contains("{{REPO}}"), "{staged}");
        assert!(!staged.contains("/home/alice"), "{staged}");
    }
}
