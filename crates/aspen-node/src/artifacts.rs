//! What a session touched, and which of its files the operator may open
//! (PROPOSALS-2026-09 §3).
//!
//! Two things live here. The **provenance index**: every file a session's
//! tool calls named — written, edited, read — scanned from its transcript,
//! so "what did it produce?" is answerable and so a path the agent points
//! at can be served. And the **serving rule**: a path is viewable for an
//! agent when it is inside the agent's repo, inside the harness's own data
//! for that project, inside the system temp dir, inside the session's
//! attachment dir, or named by a tool call in the transcript. Not the
//! whole disk.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;

/// One file a session named in a tool call.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub path: String,
    /// "wrote" | "edited" | "read"
    pub kind: String,
    /// ISO timestamp of the call, when the transcript had one.
    pub at: Option<String>,
    /// The tool that named it.
    pub tool: String,
}

/// Files named by a session's tool calls, newest first, deduplicated to
/// the strongest kind seen (wrote > edited > read).
pub fn touched_paths(repo: &Path, session_id: &str) -> Vec<Artifact> {
    let path = aspen_claude::transcript::transcript_path(repo, session_id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    // Calls the harness refused or that failed named nothing real.
    let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        if let Some(blocks) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            for b in blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    && b.get("is_error").and_then(|e| e.as_bool()) == Some(true)
                {
                    if let Some(id) = b.get("tool_use_id").and_then(|i| i.as_str()) {
                        failed.insert(id.to_owned());
                    }
                }
            }
        }
    }
    let mut seen: std::collections::HashMap<String, Artifact> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let at = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(str::to_owned);
        let Some(blocks) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            if b.get("id")
                .and_then(|i| i.as_str())
                .is_some_and(|id| failed.contains(id))
            {
                continue;
            }
            let tool = b.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let input = b.get("input");
            let (kind, key) = match tool {
                "Write" => ("wrote", "file_path"),
                "Edit" | "MultiEdit" => ("edited", "file_path"),
                "NotebookEdit" => ("edited", "notebook_path"),
                "Read" => ("read", "file_path"),
                _ => continue,
            };
            let Some(p) = input
                .and_then(|i| i.get(key))
                .and_then(|p| p.as_str())
                .filter(|p| !p.is_empty())
            else {
                continue;
            };
            let p = p.to_owned();
            let rank = |k: &str| match k {
                "wrote" => 3,
                "edited" => 2,
                _ => 1,
            };
            match seen.get_mut(&p) {
                Some(a) => {
                    if rank(kind) >= rank(&a.kind) {
                        a.kind = kind.to_owned();
                        a.tool = tool.to_owned();
                        a.at = at.clone();
                    }
                    // Move to the front of recency.
                    order.retain(|x| x != &p);
                    order.push(p);
                }
                None => {
                    seen.insert(
                        p.clone(),
                        Artifact {
                            path: p.clone(),
                            kind: kind.to_owned(),
                            at: at.clone(),
                            tool: tool.to_owned(),
                        },
                    );
                    order.push(p);
                }
            }
        }
    }
    order
        .into_iter()
        .rev()
        .filter_map(|p| seen.remove(&p))
        .collect()
}

/// Where a session's pasted attachments land on its home node.
pub fn attachments_dir(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir.join("attachments").join(session_id)
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    if p == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    PathBuf::from(p)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Resolve `path` for `agent` under the serving rule. `Ok(path)` is a
/// canonical path the caller may stat and read (it may not exist);
/// `Err` names the rule that would have admitted it.
pub fn resolve(
    data_dir: Option<&Path>,
    repo: &Path,
    session_id: Option<&str>,
    path: &str,
) -> Result<PathBuf> {
    let raw = expand_home(path.trim());
    let target = if raw.is_absolute() {
        raw
    } else {
        // Repo-relative, as the agent writes them in prose.
        repo.join(raw)
    };
    let target = canon(&target);

    let mut roots: Vec<PathBuf> = vec![canon(repo)];
    let claude = aspen_claude::transcript::claude_home();
    roots.push(canon(
        &claude
            .join("projects")
            .join(aspen_claude::transcript::project_slug(repo)),
    ));
    roots.push(canon(&claude.join("image-cache")));
    roots.push(canon(&claude.join("plans")));
    roots.push(canon(&std::env::temp_dir()));
    if let (Some(dd), Some(sid)) = (data_dir, session_id) {
        roots.push(canon(&attachments_dir(dd, sid)));
    }
    if roots.iter().any(|r| target.starts_with(r)) {
        return Ok(target);
    }
    if let Some(sid) = session_id {
        let touched = touched_paths(repo, sid);
        if touched
            .iter()
            .any(|a| canon(&expand_home(&a.path)) == target)
        {
            return Ok(target);
        }
    }
    Err(anyhow!(
        "not served: {} is outside the agent's repo, the session's own data, temp, its attachments, and the files its tool calls named",
        target.display()
    ))
}

/// Stat for the viewer: existence, size, mtime, a media type guess.
pub fn stat(path: &Path) -> Value {
    match std::fs::metadata(path) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            serde_json::json!({
                "exists": true,
                "is_dir": m.is_dir(),
                "size": m.len(),
                "mtime": mtime,
                "media_type": media_type(path),
                "name": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "path": path.to_string_lossy(),
            })
        }
        Err(_) => serde_json::json!({ "exists": false, "path": path.to_string_lossy() }),
    }
}

/// A media type from the extension; text-like unknowns read as text.
pub fn media_type(path: &Path) -> String {
    let guess = mime_guess::from_path(path).first();
    match guess {
        Some(m) => m.essence_str().to_owned(),
        None => {
            // Common agent outputs without a registered type.
            match path.extension().and_then(|e| e.to_str()) {
                Some("jsonl") => "application/x-ndjson".into(),
                Some("toml") | Some("yml") | Some("yaml") | Some("rs") | Some("ts")
                | Some("tsx") | Some("py") | Some("sh") | Some("log") | Some("cfg")
                | Some("ini") | Some("env") | None => "text/plain".into(),
                _ => "application/octet-stream".into(),
            }
        }
    }
}

/// Largest chunk one `file_read` returns (raw bytes; base64 on the wire).
pub const READ_CHUNK: u64 = 256 * 1024;
/// Largest file the viewer serves whole; beyond it, download only.
pub const VIEW_CAP: u64 = 32 * 1024 * 1024;

/// `len` bytes from `offset`, capped at `READ_CHUNK`.
pub fn read_chunk(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len.min(READ_CHUNK) as usize];
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
