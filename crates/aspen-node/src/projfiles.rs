//! Project files (PROPOSALS-2026-11-A §A-5, as designed in §5).
//!
//! A small shared space per project. The **manifest** (`project_files`)
//! maps each path to an entry: the content's sha256, who wrote it, where,
//! when, the revisions it descends from and the last few versions. It is
//! synced by digest like boards. The content lives in **blobs**
//! (`<data>/blobs/<sha>`), fetched in pieces from a peer that has them.
//!
//! On a node with a member of the project, the files are also a real
//! **folder**, `<data>/projects/<id>/`, linked into each local member's repo
//! as `.aspen/projects/<slug>` (kept out of git by `.git/info/exclude`).
//! Agents use their own file tools there, so the folder is scanned every
//! few seconds: a change made there becomes a version, and a version made
//! elsewhere is written there.
//!
//! Two writes to one path made apart are both kept: the later takes the
//! name, the other becomes `name (from @x).ext`. Every node decides the
//! same way, so they converge.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::node::{NodeInner, TurnState};
use crate::store::Board;

/// The most a project's files may hold (current versions).
pub const MAX_PROJECT_BYTES: u64 = 64 * 1024 * 1024;
/// Versions of a path kept for restore.
const HISTORY: usize = 10;
/// Revisions an entry remembers descending from.
const ANCESTORS: usize = 32;
/// One piece of a blob over the mesh.
pub const PIECE: u64 = 1024 * 1024;
const TICK_SECS: u64 = 3;
/// Lines one waiting files notice lists before "and more".
const NOTICE_LINES: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileVersion {
    pub sha: String,
    pub size: u64,
    pub by: String,
    pub at: f64,
}

/// One path's entry in a project's manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub project: String,
    pub path: String,
    /// This revision's id (a write makes a new one).
    pub rev: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub size: u64,
    /// An agent's address, `operator`, or `(folder on <node>)`.
    #[serde(default)]
    pub by: String,
    /// The node it was written on (where its blob is sure to be).
    #[serde(default)]
    pub node: String,
    pub at: f64,
    /// The revisions this one descends from, newest first.
    #[serde(default)]
    pub ancestors: Vec<String>,
    /// Earlier contents of the path, newest first.
    #[serde(default)]
    pub history: Vec<FileVersion>,
    /// Kept beside another write made apart from it (*both kept*).
    #[serde(default)]
    pub conflict: bool,
    #[serde(default)]
    pub deleted: bool,
}

// ------------------------------------------------------------------ merge

/// Where the losing side of a write made apart is kept:
/// `specs/api.md` by `arch@nl` → `specs/api (from @arch).md`.
pub fn conflict_path(path: &str, by: &str, node: &str) -> String {
    let who = if by.starts_with('(') || by.is_empty() {
        node.to_owned()
    } else {
        format!("@{}", crate::addr::bare(by))
    };
    let (dir, file) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    match file.rfind('.') {
        Some(i) if i > 0 => format!("{dir}{} (from {who}){}", &file[..i], &file[i..]),
        _ => format!("{dir}{file} (from {who})"),
    }
}

fn conflict_copy(e: &FileEntry) -> FileEntry {
    FileEntry {
        path: conflict_path(&e.path, &e.by, &e.node),
        rev: format!("{}~both", e.rev),
        ancestors: Vec::new(),
        history: Vec::new(),
        conflict: true,
        ..e.clone()
    }
}

/// Merge an entry from a peer over the one held: the entries to write
/// (none, the incoming one, and/or a both-kept copy). Pure, and the same
/// on every node.
pub fn merge_entry(local: Option<&FileEntry>, inc: &FileEntry) -> Vec<FileEntry> {
    let Some(l) = local else {
        return vec![inc.clone()];
    };
    if l.rev == inc.rev || l.ancestors.contains(&inc.rev) {
        return Vec::new();
    }
    if inc.ancestors.contains(&l.rev) {
        return vec![inc.clone()];
    }
    // Written apart: the later (at, rev) takes the name.
    let inc_wins = match inc.at.partial_cmp(&l.at) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Less) => false,
        _ => inc.rev > l.rev,
    };
    let (win, lose) = if inc_wins { (inc, l) } else { (l, inc) };
    let mut out = Vec::new();
    if inc_wins {
        out.push(inc.clone());
    }
    // A deletion that lost needs no copy; nor does the same content.
    if !lose.deleted && !(!win.deleted && win.sha == lose.sha) {
        out.push(conflict_copy(lose));
    }
    out
}

// ------------------------------------------------------------------ paths

/// A project-relative path, cleaned: forward slashes, no empty, `.` or
/// `..` parts, no drive or root.
pub fn clean_path(p: &str) -> Result<String> {
    let p = p.trim().replace('\\', "/");
    let mut parts = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => continue,
            ".." => bail!("a project path cannot climb out (`..`)"),
            x if x.contains(':') => bail!("a project path is relative (no drive)"),
            x => parts.push(x),
        }
    }
    if parts.is_empty() {
        bail!("an empty path");
    }
    if parts.contains(&".git") {
        bail!("`.git` is not a project path");
    }
    Ok(parts.join("/"))
}

/// Files a folder scan leaves alone: editors' droppings and Aspen's own
/// temporary files.
fn ignored(name: &str) -> bool {
    name.starts_with(".#")
        || name.starts_with("~$")
        || name.starts_with(".aspen-tmp-")
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name == "4913"
        || name == ".DS_Store"
        || name == "Thumbs.db"
        || name == "desktop.ini"
        || name == ".git"
}

// ------------------------------------------------------------------ blobs

fn blobs_dir(inner: &NodeInner) -> Result<PathBuf> {
    Ok(inner
        .data_dir
        .as_deref()
        .ok_or_else(|| anyhow!("this node keeps no data directory"))?
        .join("blobs"))
}

fn blob_path(inner: &NodeInner, sha: &str) -> Result<PathBuf> {
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("not a blob id");
    }
    Ok(blobs_dir(inner)?.join(&sha[..2]).join(sha))
}

pub fn has_blob(inner: &NodeInner, sha: &str) -> bool {
    blob_path(inner, sha).map(|p| p.is_file()).unwrap_or(false)
}

pub fn read_blob(inner: &NodeInner, sha: &str) -> Result<Vec<u8>> {
    Ok(std::fs::read(blob_path(inner, sha)?)?)
}

pub fn sha_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| anyhow!("no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".aspen-tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

pub fn store_blob(inner: &NodeInner, bytes: &[u8]) -> Result<String> {
    let sha = sha_of(bytes);
    let p = blob_path(inner, &sha)?;
    if !p.is_file() {
        write_atomic(&p, bytes)?;
    }
    Ok(sha)
}

/// One piece of a blob, for a peer (`project_blob`).
pub fn blob_piece(inner: &NodeInner, sha: &str, offset: u64) -> Result<Value> {
    use std::io::{Read, Seek, SeekFrom};
    let p = blob_path(inner, sha)?;
    let mut f = std::fs::File::open(&p).map_err(|_| anyhow!("no blob {sha} here"))?;
    let size = f.metadata()?.len();
    f.seek(SeekFrom::Start(offset.min(size)))?;
    let mut buf = vec![0u8; PIECE.min(size.saturating_sub(offset)) as usize];
    f.read_exact(&mut buf)?;
    Ok(json!({
        "size": size,
        "data": base64::engine::general_purpose::STANDARD.encode(&buf),
    }))
}

fn fetching() -> &'static Mutex<HashSet<String>> {
    static F: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Make sure a blob is here, pulling it from a peer if not: the writer's
/// node first (`hint`), then every reachable peer.
pub async fn fetch_blob(inner: &Arc<NodeInner>, sha: &str, hint: Option<&str>) -> Result<()> {
    if has_blob(inner, sha) {
        return Ok(());
    }
    let mesh = inner
        .mesh()
        .ok_or_else(|| anyhow!("not here, and this node is in no mesh"))?;
    if !fetching().lock().unwrap().insert(sha.to_owned()) {
        bail!("already being fetched");
    }
    struct Done<'a>(&'a str);
    impl Drop for Done<'_> {
        fn drop(&mut self) {
            fetching().lock().unwrap().remove(self.0);
        }
    }
    let _done = Done(sha);
    let me = mesh.identity.node.clone();
    let mut peers: Vec<String> = Vec::new();
    if let Some(h) = hint.filter(|h| *h != me && !h.is_empty()) {
        peers.push(h.to_owned());
    }
    for p in mesh.peers() {
        if !peers.contains(&p.cert.node) {
            peers.push(p.cert.node.clone());
        }
    }
    let mut last = anyhow!("no reachable peer");
    for peer in peers.into_iter().filter(|p| mesh.link_up(p)) {
        let mut buf: Vec<u8> = Vec::new();
        let ok = loop {
            let r = mesh
                .api_call(
                    &peer,
                    "project_blob",
                    "",
                    json!({ "sha": sha, "offset": buf.len() }),
                    std::time::Duration::from_secs(60),
                )
                .await;
            let v = match r {
                Ok(v) => v,
                Err(e) => {
                    last = e;
                    break false;
                }
            };
            let size = v.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            let data = v
                .get("data")
                .and_then(|d| d.as_str())
                .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                .unwrap_or_default();
            if data.is_empty() && (buf.len() as u64) < size {
                last = anyhow!("an empty piece from {peer}");
                break false;
            }
            buf.extend_from_slice(&data);
            if buf.len() as u64 >= size {
                break true;
            }
        };
        if ok {
            if sha_of(&buf) != sha {
                last = anyhow!("{peer} sent a blob that does not match");
                continue;
            }
            store_blob(inner, &buf)?;
            return Ok(());
        }
    }
    Err(last)
}

// ------------------------------------------------------------- projects

fn self_node(inner: &NodeInner) -> String {
    inner
        .mesh()
        .map(|m| m.identity.node.clone())
        .unwrap_or_else(|| "this node".into())
}

fn board(inner: &NodeInner, id: &str) -> Option<Board> {
    inner
        .store
        .boards(false)
        .ok()?
        .into_iter()
        .find(|b| b.id == id)
}

fn status(b: &Board) -> &str {
    b.status
        .as_ref()
        .and_then(|s| s.as_str())
        .unwrap_or("active")
}

/// A project's folder name in repos: its name made path-safe, the id's
/// head added when two projects would share one.
pub fn slugs(boards: &[Board]) -> HashMap<String, String> {
    let slug = |name: &str, id: &str| {
        let mut s = String::new();
        for c in name.to_lowercase().chars() {
            if c.is_alphanumeric() {
                s.push(c);
            } else if !s.ends_with('-') && !s.is_empty() {
                s.push('-');
            }
        }
        let s = s.trim_end_matches('-').to_owned();
        if s.is_empty() {
            id.chars().take(8).collect()
        } else {
            s
        }
    };
    let mut live: Vec<&Board> = boards.iter().filter(|b| !b.deleted).collect();
    live.sort_by(|a, b| a.id.cmp(&b.id));
    let mut taken: HashMap<String, usize> = HashMap::new();
    for b in &live {
        *taken.entry(slug(&b.name, &b.id)).or_default() += 1;
    }
    live.iter()
        .map(|b| {
            let s = slug(&b.name, &b.id);
            let s = if taken.get(&s).copied().unwrap_or(0) > 1 {
                format!("{s}-{}", b.id.chars().take(6).collect::<String>())
            } else {
                s
            };
            (b.id.clone(), s)
        })
        .collect()
}

/// The project's members on this node (local keys).
fn local_members(inner: &Arc<NodeInner>, b: &Board, local: &BTreeSet<String>) -> Vec<String> {
    if b.deleted {
        return Vec::new();
    }
    crate::topology::board_panes(inner, b)
        .into_iter()
        .filter(|m| local.contains(m))
        .collect()
}

fn local_agents(inner: &NodeInner) -> BTreeMap<String, PathBuf> {
    inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.moved_to.is_none())
        .map(|a| (a.name, a.repo))
        .collect()
}

pub fn folder(inner: &NodeInner, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    Some(inner.data_dir.as_deref()?.join("projects").join(id))
}

/// Where an agent finds a project's files: the link in its repo when the
/// node made one, else the node's folder.
pub fn folder_for(inner: &Arc<NodeInner>, agent: &str, b: &Board) -> Option<String> {
    let repo = local_agents(inner).get(agent).cloned()?;
    let slug = slugs(&inner.store.boards(false).unwrap_or_default())
        .get(&b.id)
        .cloned()?;
    let link = repo.join(".aspen").join("projects").join(&slug);
    let dir = folder(inner, &b.id)?;
    // A member told where its files are before the loop came round: make
    // its link now, so what it is told is where they are.
    if std::fs::symlink_metadata(&link).is_err()
        && crate::topology::board_panes(inner, b)
            .iter()
            .any(|m| m == agent)
        && std::fs::create_dir_all(&dir).is_ok()
        && std::fs::create_dir_all(link.parent()?).is_ok()
    {
        exclude_aspen(&repo);
        if let Err(e) = make_link(&dir, &link) {
            tracing::debug!(link = %link.display(), err = %e, "project link not made yet");
        }
    }
    if std::fs::symlink_metadata(&link).is_ok() {
        return Some(link.to_string_lossy().into_owned());
    }
    Some(dir.to_string_lossy().into_owned())
}

fn current_bytes(entries: &[FileEntry]) -> u64 {
    entries.iter().filter(|e| !e.deleted).map(|e| e.size).sum()
}

#[allow(clippy::too_many_arguments)]
fn next_entry(
    inner: &NodeInner,
    project: &str,
    path: &str,
    cur: Option<&FileEntry>,
    sha: String,
    size: u64,
    by: &str,
    deleted: bool,
) -> FileEntry {
    let mut ancestors = Vec::new();
    let mut history = Vec::new();
    if let Some(c) = cur {
        ancestors.push(c.rev.clone());
        ancestors.extend(c.ancestors.iter().cloned());
        ancestors.truncate(ANCESTORS);
        history = c.history.clone();
        if !c.deleted {
            history.insert(
                0,
                FileVersion {
                    sha: c.sha.clone(),
                    size: c.size,
                    by: c.by.clone(),
                    at: c.at,
                },
            );
        }
        history.truncate(HISTORY);
    }
    FileEntry {
        project: project.to_owned(),
        path: path.to_owned(),
        rev: uuid::Uuid::new_v4().to_string(),
        sha,
        size,
        by: by.to_owned(),
        node: self_node(inner),
        at: inner.store.hlc_now(),
        ancestors,
        history,
        conflict: false,
        deleted,
    }
}

/// Write a file into a project, as `by`. Returns the new entry (or the
/// held one when the content is the same).
pub fn write(
    inner: &Arc<NodeInner>,
    project: &str,
    path: &str,
    bytes: &[u8],
    by: &str,
) -> Result<FileEntry> {
    let path = clean_path(path)?;
    let b = board(inner, project).ok_or_else(|| anyhow!("no project {project:?}"))?;
    if status(&b) == "archived" {
        bail!("the project is archived (read-only)");
    }
    let entries = inner.store.file_entries(Some(project))?;
    let cur = entries.iter().find(|e| e.path == path).cloned();
    let held = cur
        .as_ref()
        .filter(|c| !c.deleted)
        .map(|c| c.size)
        .unwrap_or(0);
    if current_bytes(&entries) - held + bytes.len() as u64 > MAX_PROJECT_BYTES {
        bail!(
            "the project's files would pass {} MB",
            MAX_PROJECT_BYTES / (1024 * 1024)
        );
    }
    let sha = store_blob(inner, bytes)?;
    if let Some(c) = cur.as_ref().filter(|c| !c.deleted && c.sha == sha) {
        return Ok(c.clone());
    }
    let e = next_entry(
        inner,
        project,
        &path,
        cur.as_ref(),
        sha,
        bytes.len() as u64,
        by,
        false,
    );
    inner.store.put_file_entry(&e)?;
    changed(inner, std::slice::from_ref(&e));
    Ok(e)
}

/// Delete a path (a tombstone, so the delete travels).
pub fn delete(inner: &Arc<NodeInner>, project: &str, path: &str, by: &str) -> Result<()> {
    let path = clean_path(path)?;
    let cur = inner
        .store
        .file_entry(project, &path)?
        .filter(|c| !c.deleted)
        .ok_or_else(|| anyhow!("no file {path:?} in the project"))?;
    let e = next_entry(
        inner,
        project,
        &path,
        Some(&cur),
        String::new(),
        0,
        by,
        true,
    );
    inner.store.put_file_entry(&e)?;
    changed(inner, &[e]);
    Ok(())
}

/// Read a path's content (or an earlier version's, by sha), fetching the
/// blob from a peer if this node lacks it.
pub async fn read(
    inner: &Arc<NodeInner>,
    project: &str,
    path: &str,
    version: Option<&str>,
) -> Result<(FileEntry, Vec<u8>)> {
    let path = clean_path(path)?;
    let e = inner
        .store
        .file_entry(project, &path)?
        .ok_or_else(|| anyhow!("no file {path:?} in the project"))?;
    let sha = match version {
        Some(v) => {
            if v != e.sha && !e.history.iter().any(|h| h.sha == v) {
                bail!("{path:?} has no version {v}");
            }
            v.to_owned()
        }
        None => {
            if e.deleted {
                bail!("{path:?} was deleted");
            }
            e.sha.clone()
        }
    };
    if !has_blob(inner, &sha) {
        fetch_blob(inner, &sha, Some(&e.node)).await?;
    }
    let bytes = read_blob(inner, &sha)?;
    Ok((e, bytes))
}

/// Merge an entry from a peer; returns whether the manifest changed.
pub fn merge_incoming(inner: &Arc<NodeInner>, inc: &FileEntry) -> bool {
    if clean_path(&inc.path).ok().as_deref() != Some(inc.path.as_str()) {
        return false;
    }
    inner.store.hlc_observe(inc.at);
    let cur = inner
        .store
        .file_entry(&inc.project, &inc.path)
        .ok()
        .flatten();
    let mut wrote = Vec::new();
    for o in merge_entry(cur.as_ref(), inc) {
        let held = inner.store.file_entry(&o.project, &o.path).ok().flatten();
        if held.as_ref() == Some(&o) {
            continue;
        }
        // A both-kept copy lands on a path of its own, unless something
        // else already sits there: then it merges like any write.
        let outs = if o.conflict && held.is_some() {
            merge_entry(held.as_ref(), &o)
        } else {
            vec![o]
        };
        for o in outs {
            if inner.store.put_file_entry(&o).is_ok() {
                wrote.push(o);
            }
        }
    }
    if wrote.is_empty() {
        return false;
    }
    changed(inner, &wrote);
    true
}

// ----------------------------------------------------------- the notice

fn who(by: &str) -> String {
    if by.starts_with('(') || by == "operator" {
        by.to_owned()
    } else {
        format!("@{}", crate::addr::bare(by))
    }
}

/// After the manifest changed: wake the loop (to write the folder), and
/// tell each local member except the writer, in one ambient digest that
/// grows while it waits.
fn changed(inner: &Arc<NodeInner>, entries: &[FileEntry]) {
    wake();
    let local: BTreeSet<String> = local_agents(inner).into_keys().collect();
    let mut by_project: BTreeMap<&str, Vec<&FileEntry>> = BTreeMap::new();
    for e in entries {
        by_project.entry(e.project.as_str()).or_default().push(e);
    }
    for (pid, es) in by_project {
        let Some(b) = board(inner, pid) else { continue };
        if status(&b) != "active" {
            continue;
        }
        let thread = format!("project:{pid}");
        let prefix = format!("[project files] \u{201c}{}\u{201d}", b.name);
        for m in local_members(inner, &b, &local) {
            let lines: Vec<(String, String)> = es
                .iter()
                .filter(|e| e.by != m)
                .map(|e| {
                    let what = if e.deleted {
                        " deleted"
                    } else if e.conflict {
                        " (both kept: written apart from the version at its name)"
                    } else {
                        ""
                    };
                    let w = who(&e.by);
                    let line = if w.starts_with('(') {
                        format!("- {}{what} {w}", e.path)
                    } else {
                        format!("- {}{what} ({w})", e.path)
                    };
                    (format!("- {} ", e.path), line)
                })
                .collect();
            if lines.is_empty() {
                continue;
            }
            match inner.store.pending_ambient(&m, &thread, &prefix) {
                Ok(Some((id, body))) => {
                    let mut body = body;
                    let have = body.lines().filter(|l| l.starts_with("- ")).count();
                    let mut n = have;
                    for (key, l) in lines {
                        // One line per path: a later change replaces it.
                        if body.lines().any(|x| x == l) {
                            continue;
                        }
                        if let Some(old) = body
                            .lines()
                            .find(|x| x.starts_with(&key))
                            .map(str::to_owned)
                        {
                            body = body.replacen(&old, &l, 1);
                            continue;
                        }
                        if n >= NOTICE_LINES {
                            if !body.ends_with("- and more") {
                                body.push_str("\n- and more");
                            }
                            break;
                        }
                        body.push('\n');
                        body.push_str(&l);
                        n += 1;
                    }
                    let _ = inner.store.set_message_body(id, &body);
                }
                _ => {
                    let at = folder_for(inner, &m, &b)
                        .map(|f| format!(" (in {f})"))
                        .unwrap_or_default();
                    let mut body = format!("{prefix} files changed{at}:");
                    for (_, l) in lines.into_iter().take(NOTICE_LINES) {
                        body.push('\n');
                        body.push_str(&l);
                    }
                    let display = format!("@{}", crate::addr::bare(&m));
                    if inner
                        .store
                        .insert_message(
                            "operator",
                            &m,
                            &display,
                            "ambient",
                            &body,
                            Some(&thread),
                            None,
                            None,
                        )
                        .is_ok()
                    {
                        inner.tick_delivery(&m);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------- the loop

fn waker() -> &'static tokio::sync::Notify {
    static W: OnceLock<tokio::sync::Notify> = OnceLock::new();
    W.get_or_init(tokio::sync::Notify::new)
}

fn wake() {
    waker().notify_one();
}

/// Every few seconds (or at once after a write): fetch the blobs member
/// projects lack, scan and write their folders, keep the repo links.
pub fn spawn_loop(inner: Arc<NodeInner>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let mut n: u64 = 0;
        loop {
            let i = inner.clone();
            let wants = tokio::task::spawn_blocking(move || missing_blobs(&i))
                .await
                .unwrap_or_default();
            for (sha, hint) in wants.into_iter().take(8) {
                if let Err(e) = fetch_blob(&inner, &sha, Some(&hint)).await {
                    tracing::debug!(sha, err = %e, "project blob not fetched yet");
                }
            }
            let i = inner.clone();
            let changed = tokio::task::spawn_blocking(move || reconcile(&i))
                .await
                .unwrap_or(false);
            if changed {
                crate::federation::broadcast_roster(&inner);
            }
            n += 1;
            if n % 600 == 1 {
                let i = inner.clone();
                let _ = tokio::task::spawn_blocking(move || gc(&i)).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(TICK_SECS)) => {}
                _ = waker().notified() => {
                    // Let a burst of writes settle into one pass.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        }
    });
}

/// Blobs the projects with a member here need and lack: (sha, the
/// writer's node).
fn missing_blobs(inner: &Arc<NodeInner>) -> Vec<(String, String)> {
    let local: BTreeSet<String> = local_agents(inner).into_keys().collect();
    let mut out = Vec::new();
    for b in inner.store.boards(false).unwrap_or_default() {
        if local_members(inner, &b, &local).is_empty() {
            continue;
        }
        for e in inner.store.file_entries(Some(&b.id)).unwrap_or_default() {
            if !e.deleted && !has_blob(inner, &e.sha) {
                out.push((e.sha, e.node));
            }
        }
    }
    out
}

/// One pass: links, then each member project's folder. Returns whether
/// the manifest changed (a change found in a folder).
fn reconcile(inner: &Arc<NodeInner>) -> bool {
    let agents = local_agents(inner);
    let local: BTreeSet<String> = agents.keys().cloned().collect();
    let boards = inner.store.boards(true).unwrap_or_default();
    let slugs = slugs(&boards);
    let mut changed = false;
    // repo → the links it should have (slug → project id).
    let mut want: BTreeMap<PathBuf, BTreeMap<String, String>> = BTreeMap::new();
    for b in boards.iter().filter(|b| !b.deleted) {
        let members = local_members(inner, b, &local);
        if members.is_empty() {
            continue;
        }
        let Some(dir) = folder(inner, &b.id) else {
            continue;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        if let Some(slug) = slugs.get(&b.id) {
            for m in &members {
                if let Some(repo) = agents.get(m) {
                    want.entry(repo.clone())
                        .or_default()
                        .insert(slug.clone(), b.id.clone());
                }
            }
        }
        match sync_folder(inner, b, &dir, &members) {
            Ok(c) => changed |= c,
            Err(e) => tracing::warn!(project = %b.id, err = %e, "project folder not synced"),
        }
    }
    let Some(root) = inner.data_dir.as_deref().map(|d| d.join("projects")) else {
        return changed;
    };
    let repos: BTreeSet<PathBuf> = agents.values().cloned().collect();
    for repo in repos {
        let links = want.remove(&repo).unwrap_or_default();
        keep_links(&repo, &links, &root);
    }
    changed
}

fn mtime(md: &std::fs::Metadata) -> f64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf, std::fs::Metadata)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if ignored(&name) {
            continue;
        }
        let Ok(md) = std::fs::symlink_metadata(ent.path()) else {
            continue;
        };
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if md.file_type().is_symlink() {
            continue;
        } else if md.is_dir() {
            walk(&ent.path(), &r, out);
        } else if md.is_file() {
            out.push((r, ent.path(), md));
        }
    }
}

/// Who made a change found in a folder: the only local member, or the
/// one local member mid-turn if exactly one is, else the folder itself.
fn folder_author(inner: &NodeInner, members: &[String]) -> String {
    if let [one] = members {
        return one.clone();
    }
    let busy: Vec<&String> = members
        .iter()
        .filter(|m| {
            inner
                .live(m)
                .is_some_and(|s| s.turn_state() == TurnState::Busy)
        })
        .collect();
    match busy.as_slice() {
        [one] => (*one).clone(),
        _ => format!("(folder on {})", self_node(inner)),
    }
}

fn sync_folder(inner: &Arc<NodeInner>, b: &Board, dir: &Path, members: &[String]) -> Result<bool> {
    let store = &inner.store;
    let mut disk = store.disk_state(&b.id)?;
    let entries: HashMap<String, FileEntry> = store
        .file_entries(Some(&b.id))?
        .into_iter()
        .map(|e| (e.path.clone(), e))
        .collect();
    let mut files = Vec::new();
    walk(dir, "", &mut files);
    let mut seen = HashSet::new();
    let mut changed = false;
    let writable = status(b) != "archived";
    // What changed in the folder since this node last looked.
    for (rel, full, md) in &files {
        seen.insert(rel.clone());
        let (size, mt) = (md.len(), mtime(md));
        if let Some((_, s, m)) = disk.get(rel) {
            if *s == size && *m == mt {
                continue;
            }
        }
        if size > MAX_PROJECT_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(full) else {
            continue;
        };
        let sha = sha_of(&bytes);
        let base = disk.get(rel).map(|(s, _, _)| s.clone());
        let cur = entries.get(rel);
        if cur.is_some_and(|e| !e.deleted && e.sha == sha) {
            store.set_disk_state(&b.id, rel, Some((&sha, size, mt)))?;
            disk.insert(rel.clone(), (sha, size, mt));
            continue;
        }
        if !writable {
            continue;
        }
        let by = folder_author(inner, members);
        // Written on top of what the manifest holds: a version. Written on
        // top of something older (the manifest moved on meanwhile): kept
        // beside it.
        let on_top = match cur {
            None => true,
            Some(e) if e.deleted => true,
            Some(e) => base.as_deref() == Some(e.sha.as_str()),
        };
        let r = if on_top {
            write(inner, &b.id, rel, &bytes, &by).map(|_| ())
        } else {
            let p = conflict_path(rel, &by, &self_node(inner));
            write(inner, &b.id, &p, &bytes, &by).map(|e| {
                let mut e = e;
                e.conflict = true;
                let _ = store.put_file_entry(&e);
            })
        };
        match r {
            Ok(()) => {
                changed = true;
                if on_top {
                    store.set_disk_state(&b.id, rel, Some((&sha, size, mt)))?;
                    disk.insert(rel.clone(), (sha, size, mt));
                }
            }
            Err(e) => {
                tracing::warn!(project = %b.id, path = %rel, err = %e, "a folder change was not taken")
            }
        }
    }
    // Removed from the folder.
    for (rel, (sha, _, _)) in disk.clone() {
        if seen.contains(&rel) {
            continue;
        }
        let cur = entries.get(&rel);
        if writable && cur.is_some_and(|e| !e.deleted && e.sha == sha) {
            let by = folder_author(inner, members);
            if delete(inner, &b.id, &rel, &by).is_ok() {
                changed = true;
            }
        }
        store.set_disk_state(&b.id, &rel, None)?;
        disk.remove(&rel);
    }
    // The manifest, written out.
    let entries = store.file_entries(Some(&b.id))?;
    for e in &entries {
        let on_disk = disk.get(&e.path).map(|(s, _, _)| s.as_str());
        let full = dir.join(&e.path);
        if e.deleted {
            if on_disk.is_some() {
                let _ = std::fs::remove_file(&full);
                store.set_disk_state(&b.id, &e.path, None)?;
                prune_dirs(dir, full.parent());
            }
            continue;
        }
        if on_disk == Some(e.sha.as_str()) || !has_blob(inner, &e.sha) {
            continue;
        }
        // A file there that this node never wrote nor saw (on_disk None):
        // the scan above already took it as a change, so it is safe.
        let bytes = read_blob(inner, &e.sha)?;
        write_atomic(&full, &bytes)?;
        if let Ok(md) = std::fs::metadata(&full) {
            store.set_disk_state(&b.id, &e.path, Some((&e.sha, md.len(), mtime(&md))))?;
        }
    }
    Ok(changed)
}

fn prune_dirs(root: &Path, mut dir: Option<&Path>) {
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

fn gc(inner: &Arc<NodeInner>) {
    let Ok(dir) = blobs_dir(inner) else { return };
    let mut keep: HashSet<String> = HashSet::new();
    for e in inner.store.file_entries(None).unwrap_or_default() {
        keep.insert(e.sha.clone());
        for h in e.history {
            keep.insert(h.sha);
        }
    }
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    for sub in rd.flatten() {
        let Ok(rd2) = std::fs::read_dir(sub.path()) else {
            continue;
        };
        for f in rd2.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let old = f
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t < hour_ago)
                .unwrap_or(false);
            if old && !keep.contains(&name) {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
}

// ------------------------------------------------------------ repo links

fn strip_verbatim(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").map(str::to_owned).unwrap_or(s)
}

/// Make a repo's `.aspen/projects/` hold exactly the links it should
/// (slug → project id). Only links into this node's project folders are
/// ever removed; anything else there is left alone.
fn keep_links(repo: &Path, want: &BTreeMap<String, String>, root: &Path) {
    let dir = repo.join(".aspen").join("projects");
    let root_s = strip_verbatim(root);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            let Ok(target) = std::fs::read_link(ent.path()) else {
                continue;
            };
            let t = strip_verbatim(&target);
            if !t.starts_with(&root_s) {
                continue;
            }
            let right = want
                .get(&name)
                .is_some_and(|id| strip_verbatim(&root.join(id)) == t);
            if !right {
                remove_link(&ent.path());
            }
        }
    }
    if want.is_empty() {
        // Leave no empty folders behind for a repo with no projects.
        let _ = std::fs::remove_dir(&dir);
        let _ = std::fs::remove_dir(repo.join(".aspen"));
        return;
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    exclude_aspen(repo);
    for (slug, id) in want {
        let link = dir.join(slug);
        if std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        if let Err(e) = make_link(&root.join(id), &link) {
            tracing::warn!(link = %link.display(), err = %e, "project link not made");
        }
    }
}

fn remove_link(p: &Path) {
    #[cfg(windows)]
    {
        if std::fs::remove_dir(p).is_err() {
            let _ = std::fs::remove_file(p);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(unix)]
fn make_link(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(windows)]
fn make_link(target: &Path, link: &Path) -> Result<()> {
    // A directory junction: no admin rights or developer mode needed.
    let out = aspen_core::quiet_command("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()?;
    if !out.status.success() {
        bail!("mklink /J: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn make_link(_target: &Path, _link: &Path) -> Result<()> {
    bail!("no links on this platform")
}

/// The repo's git directory, for `info/exclude` (a worktree's `.git` is a
/// file naming its own; exclude lives in the common one).
fn git_common_dir(repo: &Path) -> Option<PathBuf> {
    let dotgit = repo.join(".git");
    let md = std::fs::metadata(&dotgit).ok()?;
    if md.is_dir() {
        return Some(dotgit);
    }
    let text = std::fs::read_to_string(&dotgit).ok()?;
    let gd = text.trim().strip_prefix("gitdir:")?.trim();
    let gd = if Path::new(gd).is_absolute() {
        PathBuf::from(gd)
    } else {
        repo.join(gd)
    };
    match std::fs::read_to_string(gd.join("commondir")) {
        Ok(c) => {
            let c = c.trim();
            Some(if Path::new(c).is_absolute() {
                PathBuf::from(c)
            } else {
                gd.join(c)
            })
        }
        Err(_) => Some(gd),
    }
}

/// Keep `.aspen/` out of git, locally (no tracked file is touched).
fn exclude_aspen(repo: &Path) {
    let Some(gd) = git_common_dir(repo) else {
        return;
    };
    let path = gd.join("info").join("exclude");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if text
        .lines()
        .any(|l| matches!(l.trim(), "/.aspen/" | ".aspen/" | "/.aspen" | ".aspen"))
    {
        return;
    }
    let mut text = text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# Aspen's project folders (linked, not part of the repo)\n/.aspen/\n");
    let _ = std::fs::create_dir_all(gd.join("info"));
    let _ = std::fs::write(&path, text);
}

// ------------------------------------------------------------- listings

/// A project's files for a reader: current entries (deleted left out).
pub fn listing(inner: &NodeInner, project: &str) -> Vec<FileEntry> {
    inner
        .store
        .file_entries(Some(project))
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !e.deleted)
        .collect()
}

pub fn decode_b64(s: &str) -> std::result::Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim().as_bytes())
        .map_err(|e| e.to_string())
}

/// Text a reader can be shown: UTF-8 with no NULs.
pub fn as_text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|s| !s.contains('\0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, rev: &str, sha: &str, at: f64, anc: &[&str]) -> FileEntry {
        FileEntry {
            project: "p".into(),
            path: path.into(),
            rev: rev.into(),
            sha: sha.into(),
            size: 1,
            by: "arch@nl".into(),
            node: "a".into(),
            at,
            ancestors: anc.iter().map(|s| s.to_string()).collect(),
            history: Vec::new(),
            conflict: false,
            deleted: false,
        }
    }

    #[test]
    fn a_later_version_replaces_and_an_older_one_does_not() {
        let v1 = e("a.md", "r1", "s1", 1.0, &[]);
        let v2 = e("a.md", "r2", "s2", 2.0, &["r1"]);
        assert_eq!(merge_entry(Some(&v1), &v2), vec![v2.clone()]);
        assert!(merge_entry(Some(&v2), &v1).is_empty());
        assert!(merge_entry(Some(&v2), &v2).is_empty());
        assert_eq!(merge_entry(None, &v1), vec![v1]);
    }

    #[test]
    fn writes_made_apart_are_both_kept_the_same_way_on_each_side() {
        let a = e("specs/api.md", "ra", "sa", 5.0, &["r0"]);
        let mut b = e("specs/api.md", "rb", "sb", 6.0, &["r0"]);
        b.by = "far@q".into();
        // On a's node, b arrives: b takes the name, a is kept beside it.
        let on_a = merge_entry(Some(&a), &b);
        assert_eq!(on_a.len(), 2);
        assert_eq!(on_a[0], b);
        assert_eq!(on_a[1].path, "specs/api (from @arch).md");
        assert!(on_a[1].conflict);
        // On b's node, a arrives: only the copy.
        let on_b = merge_entry(Some(&b), &a);
        assert_eq!(on_b, vec![on_a[1].clone()]);
    }

    #[test]
    fn a_lost_delete_needs_no_copy() {
        let mut d = e("x.txt", "rd", "", 5.0, &["r0"]);
        d.deleted = true;
        let w = e("x.txt", "rw", "sw", 6.0, &["r0"]);
        assert_eq!(merge_entry(Some(&d), &w), vec![w.clone()]);
        assert!(merge_entry(Some(&w), &d).is_empty());
    }

    #[test]
    fn paths_stay_inside() {
        assert_eq!(clean_path("./specs//api.md").unwrap(), "specs/api.md");
        assert_eq!(clean_path("a\\b.txt").unwrap(), "a/b.txt");
        assert!(clean_path("../x").is_err());
        assert!(clean_path("C:/x").is_err());
        assert!(clean_path("").is_err());
        assert!(clean_path(".git/config").is_err());
        assert_eq!(
            conflict_path("notes", "(folder on b)", "b"),
            "notes (from b)"
        );
    }
}
