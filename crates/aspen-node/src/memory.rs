//! Memory convergence (docs/MEMORY.md, PROPOSALS-B §5), opt-in.
//!
//! Project memory — `<claude home>/projects/<slug>/memory/` — is text the
//! harness writes and reads per repo. When two nodes hold counterparts of
//! a repo (same git origin, else same basename) and both have
//! `memory.sync` on, their memory directories converge:
//!
//! - the roster carries `memory: {repo key: digest}` (a hash over every
//!   file's rel path and content hash, tombstones included);
//! - a peer whose digest for a key differs pulls `memory_files {key}` —
//!   every text file, canonicalized with the sender's `PathCtx` — and
//!   merges per file against a stored **base** (the last content both
//!   sides agreed on): remote == base → keep local; local == base → take
//!   remote; else a line-level 3-way merge (`diffy`); a conflict keeps the
//!   local file, writes the incoming one beside it as
//!   `<name>.from-<node><ext>`, and raises a `memory_conflict` notice
//!   that Now lists with keep-mine / take-theirs.
//!
//! Deletions are tombstones in the base table: a tracked file that
//! vanishes locally is recorded deleted and propagates (the peer deletes
//! its copy only if that copy still equals the base — an edit beats a
//! delete).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::migrate::{canonicalize_str, localize_str, PathCtx};
use crate::node::NodeInner;

const MAX_FILE: u64 = 512 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemorySettings {
    /// Converge project memory with peers that also have this on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<bool>,
}

impl MemorySettings {
    pub fn on(&self) -> bool {
        self.sync.unwrap_or(false)
    }
}

/// How a repo is matched with its counterpart on a peer.
pub fn repo_key(repo: &Path) -> String {
    match crate::migrate::git_origin(repo) {
        Some(o) => format!("origin:{o}"),
        None => format!(
            "name:{}",
            repo.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
    }
}

pub fn memory_dir(repo: &Path) -> PathBuf {
    PathBuf::from(PathCtx::local(repo).project_dir).join("memory")
}

fn fnv(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn is_text_name(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    !lower.contains(".from-")
        && [
            ".md", ".txt", ".json", ".yaml", ".yml", ".toml", ".csv", ".jsonl",
        ]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// Every text file in a memory dir: rel → (hash, content). Files that
/// are conflict copies (`.from-<node>`) are never synced.
pub fn read_dir_files(dir: &Path) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if p.is_dir() {
                stack.push((p, rel));
            } else if is_text_name(&rel) {
                let Ok(meta) = p.metadata() else { continue };
                if meta.len() > MAX_FILE {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&p) {
                    out.insert(rel, (fnv(text.as_bytes()), text));
                }
            }
        }
    }
    out
}

/// The digest a peer compares against: over (rel, hash) of live files and
/// (rel, "deleted") of tombstones. Also records tombstones for tracked
/// files that vanished.
pub fn digest(inner: &Arc<NodeInner>, repo: &Path) -> String {
    let dir = memory_dir(repo);
    let files = read_dir_files(&dir);
    let repo_s = repo.to_string_lossy().to_string();
    let base = inner.store.memory_base(&repo_s).unwrap_or_default();
    let now = crate::store::now_epoch();
    let mut entries: Vec<(String, String)> = Vec::new();
    for (rel, (hash, _)) in &files {
        entries.push((rel.clone(), hash.clone()));
    }
    for (rel, b) in &base {
        if !files.contains_key(rel) {
            if !b.deleted {
                // Tracked, now gone: tombstone it.
                let _ = inner.store.set_memory_base(&repo_s, rel, None, true, now);
            }
            entries.push((rel.clone(), "deleted".into()));
        }
    }
    entries.sort();
    let mut buf = Vec::new();
    for (rel, h) in entries {
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(h.as_bytes());
        buf.push(0);
    }
    fnv(&buf)
}

/// The roster field: key → digest for every registered repo, when the
/// setting is on.
pub fn roster_digests(inner: &Arc<NodeInner>) -> Option<Value> {
    let dd = inner.data_dir.clone()?;
    if !crate::settings::load(&dd).memory.on() {
        return None;
    }
    let mut m = serde_json::Map::new();
    for r in inner.store.repos().ok()? {
        m.insert(repo_key(&r.path), json!(digest(inner, &r.path)));
    }
    Some(Value::Object(m))
}

/// `memory_files {key}`: every file of the repo matching `key`,
/// canonicalized, plus tombstones.
pub fn files_for_key(inner: &Arc<NodeInner>, key: &str) -> Result<Value> {
    let dd = inner
        .data_dir
        .clone()
        .ok_or_else(|| anyhow!("no data dir"))?;
    if !crate::settings::load(&dd).memory.on() {
        return Err(anyhow!("memory sync is off on this node"));
    }
    let repo = inner
        .store
        .repos()?
        .into_iter()
        .map(|r| r.path)
        .find(|p| repo_key(p) == key)
        .ok_or_else(|| anyhow!("no repo here matches {key}"))?;
    let ctx = PathCtx::local(&repo);
    let files = read_dir_files(&memory_dir(&repo));
    let repo_s = repo.to_string_lossy().to_string();
    let base = inner.store.memory_base(&repo_s).unwrap_or_default();
    let mut out = Vec::new();
    for (rel, (hash, content)) in &files {
        out.push(json!({
            "rel": rel,
            "hash": hash,
            "content": canonicalize_str(content, &ctx),
        }));
    }
    for (rel, b) in &base {
        if b.deleted && !files.contains_key(rel) {
            out.push(json!({ "rel": rel, "deleted": true }));
        }
    }
    Ok(json!({ "key": key, "files": out, "node": inner.mesh().map(|m| m.identity.node.clone()) }))
}

/// Merge what a peer sent for one repo. Returns (written, conflicts).
pub fn merge_from(
    inner: &Arc<NodeInner>,
    peer: &str,
    repo: &Path,
    files: &[Value],
) -> (usize, Vec<String>) {
    let dir = memory_dir(repo);
    let ctx = PathCtx::local(repo);
    let repo_s = repo.to_string_lossy().to_string();
    let local = read_dir_files(&dir);
    let base = inner.store.memory_base(&repo_s).unwrap_or_default();
    let now = crate::store::now_epoch();
    let mut written = 0usize;
    let mut conflicts = Vec::new();
    for f in files {
        let Some(rel) = f.get("rel").and_then(|r| r.as_str()) else {
            continue;
        };
        if rel
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
            || rel.contains('\\')
        {
            continue;
        }
        let path = dir.join(rel);
        let remote_deleted = f.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false);
        let remote: Option<String> = f
            .get("content")
            .and_then(|c| c.as_str())
            .map(|c| localize_str(c, &ctx));
        let l = local.get(rel).map(|(_, c)| c.clone());
        let b = base.get(rel);
        let b_content = b.and_then(|x| if x.deleted { None } else { x.content.clone() });
        match (remote_deleted, remote, l) {
            (true, _, None) => {
                if b.map(|x| !x.deleted).unwrap_or(false) {
                    let _ = inner.store.set_memory_base(&repo_s, rel, None, true, now);
                }
            }
            (true, _, Some(lc)) => {
                // Delete only what we have not edited since the base.
                if b_content.as_deref() == Some(lc.as_str()) {
                    let _ = std::fs::remove_file(&path);
                    let _ = inner.store.set_memory_base(&repo_s, rel, None, true, now);
                    written += 1;
                }
            }
            (false, Some(rc), None) => {
                // We deleted it (tombstone matching what they hold): stay deleted.
                if b.map(|x| x.deleted).unwrap_or(false)
                    && b.and_then(|x| x.content.as_deref()) == Some(rc.as_str())
                {
                    continue;
                }
                if write(&path, &rc).is_ok() {
                    let _ = inner
                        .store
                        .set_memory_base(&repo_s, rel, Some(&rc), false, now);
                    written += 1;
                }
            }
            (false, Some(rc), Some(lc)) => {
                if rc == lc {
                    if b_content.as_deref() != Some(lc.as_str()) {
                        let _ = inner
                            .store
                            .set_memory_base(&repo_s, rel, Some(&lc), false, now);
                    }
                    // Converged: a conflict copy from this peer is stale now.
                    let stale = from_path(&dir, rel, peer);
                    if stale.is_file() {
                        let _ = std::fs::remove_file(&stale);
                    }
                    continue;
                }
                match b_content.as_deref() {
                    Some(bc) if bc == rc => {} // they have not moved; keep ours
                    Some(bc) if bc == lc => {
                        if write(&path, &rc).is_ok() {
                            let _ =
                                inner
                                    .store
                                    .set_memory_base(&repo_s, rel, Some(&rc), false, now);
                            written += 1;
                        }
                    }
                    Some(bc) => match diffy::merge(bc, &lc, &rc) {
                        Ok(merged) => {
                            if write(&path, &merged).is_ok() {
                                let _ = inner.store.set_memory_base(
                                    &repo_s,
                                    rel,
                                    Some(&merged),
                                    false,
                                    now,
                                );
                                written += 1;
                            }
                        }
                        Err(_) => {
                            keep_both(&dir, rel, peer, &rc);
                            conflicts.push(rel.to_owned());
                        }
                    },
                    None => {
                        keep_both(&dir, rel, peer, &rc);
                        conflicts.push(rel.to_owned());
                    }
                }
            }
            (false, None, _) => {}
        }
    }
    (written, conflicts)
}

fn write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, content)
}

fn from_path(dir: &Path, rel: &str, peer: &str) -> PathBuf {
    let p = Path::new(rel);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_owned());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = p.parent().map(|x| x.to_path_buf()).unwrap_or_default();
    dir.join(parent).join(format!("{stem}.from-{peer}{ext}"))
}

fn keep_both(dir: &Path, rel: &str, peer: &str, remote: &str) {
    let _ = write(&from_path(dir, rel, peer), remote);
}

/// Pull one repo's memory from a peer and merge it; raise a notice per
/// conflict.
pub async fn sync_from(inner: &Arc<NodeInner>, peer: &str, key: &str) {
    let Some(mesh) = inner.mesh() else { return };
    let Ok(repos) = inner.store.repos() else {
        return;
    };
    let Some(repo) = repos
        .into_iter()
        .map(|r| r.path)
        .find(|p| repo_key(p) == key)
    else {
        return;
    };
    let v = match mesh
        .api_call(
            peer,
            "memory_files",
            "",
            json!({ "key": key }),
            std::time::Duration::from_secs(30),
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(%e, peer, key, "memory pull");
            return;
        }
    };
    let files = v
        .get("files")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let inner2 = inner.clone();
    let peer2 = peer.to_owned();
    let repo2 = repo.clone();
    let (written, conflicts) =
        tokio::task::spawn_blocking(move || merge_from(&inner2, &peer2, &repo2, &files))
            .await
            .unwrap_or((0, Vec::new()));
    if written > 0 {
        tracing::info!(peer, repo = %repo.display(), written, "memory converged");
        crate::federation::broadcast_roster(inner);
    }
    for rel in conflicts {
        let repo_handle = inner
            .store
            .repos()
            .ok()
            .and_then(|rs| rs.into_iter().find(|r| r.path == repo).map(|r| r.handle))
            .unwrap_or_default();
        crate::notify::raise(
            inner,
            &format!("memory@{repo_handle}"),
            "memory_conflict",
            &format!("memory conflict in #{repo_handle}: {rel}"),
            Some(&format!(
                "both nodes edited it since they last agreed; {peer}'s copy is beside it as {}",
                from_path(Path::new(""), &rel, peer).display()
            )),
            Some("/"),
        );
    }
}

/// Conflicts on this node: every `.from-<node>` file in a memory dir.
pub fn conflicts(inner: &Arc<NodeInner>) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(repos) = inner.store.repos() else {
        return out;
    };
    for r in repos {
        let dir = memory_dir(&r.path);
        let mut stack = vec![(dir.clone(), String::new())];
        while let Some((d, prefix)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                if p.is_dir() {
                    stack.push((p, rel));
                    continue;
                }
                let Some(idx) = name.find(".from-") else {
                    continue;
                };
                let stem = &name[..idx];
                let rest = &name[idx + 6..];
                let (peer, ext) = match rest.find('.') {
                    Some(i) => (&rest[..i], &rest[i..]),
                    None => (rest, ""),
                };
                let original = if prefix.is_empty() {
                    format!("{stem}{ext}")
                } else {
                    format!("{prefix}/{stem}{ext}")
                };
                // A copy identical to the file is a conflict already
                // resolved (the other side took ours, or ours theirs).
                if let (Ok(a), Ok(b)) = (
                    std::fs::read_to_string(&p),
                    std::fs::read_to_string(dir.join(&original)),
                ) {
                    if a == b {
                        let _ = std::fs::remove_file(&p);
                        continue;
                    }
                }
                let mtime = p
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                out.push(json!({
                    "repo": r.path.to_string_lossy(),
                    "handle": r.handle,
                    "rel": original,
                    "from": peer,
                    "copy": rel,
                    "at": mtime,
                }));
            }
        }
    }
    out
}

/// Resolve a conflict: `mine` drops the incoming copy; `theirs` replaces
/// the file with it. Either way the result becomes the new base.
pub fn resolve(
    inner: &Arc<NodeInner>,
    repo: &Path,
    rel: &str,
    copy: &str,
    choice: &str,
) -> Result<()> {
    let dir = memory_dir(repo);
    let target = dir.join(rel);
    let from = dir.join(copy);
    if !from.is_file() {
        return Err(anyhow!("no conflict copy at {copy}"));
    }
    match choice {
        "mine" => {
            std::fs::remove_file(&from)?;
        }
        "theirs" => {
            let content = std::fs::read_to_string(&from)?;
            write(&target, &content)?;
            std::fs::remove_file(&from)?;
        }
        other => return Err(anyhow!("choice must be mine|theirs, not {other:?}")),
    }
    let content = std::fs::read_to_string(&target).ok();
    let _ = inner.store.set_memory_base(
        &repo.to_string_lossy(),
        rel,
        content.as_deref(),
        content.is_none(),
        crate::store::now_epoch(),
    );
    crate::federation::broadcast_roster(inner);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_way_merge_and_conflict() {
        let base = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let ours = "A\nb\nc\nd\ne\nf\ng\nh\n";
        let theirs = "a\nb\nc\nd\ne\nf\ng\nH\n";
        assert_eq!(
            diffy::merge(base, ours, theirs).unwrap(),
            "A\nb\nc\nd\ne\nf\ng\nH\n"
        );
        assert!(
            diffy::merge(base, "X\nb\nc\nd\ne\nf\ng\nh\n", "Y\nb\nc\nd\ne\nf\ng\nh\n").is_err()
        );
    }

    #[test]
    fn conflict_copy_name() {
        assert_eq!(
            from_path(Path::new("/m"), "notes/plan.md", "j2"),
            PathBuf::from("/m/notes/plan.from-j2.md")
        );
        assert_eq!(
            from_path(Path::new("/m"), "README", "j2"),
            PathBuf::from("/m/README.from-j2")
        );
        assert!(!is_text_name("plan.from-j2.md"));
        assert!(is_text_name("plan.md"));
    }
}
