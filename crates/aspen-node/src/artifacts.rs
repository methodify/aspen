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
use serde::{Deserialize, Serialize};
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

/// Files a session named, by its harness's own record: Claude's transcript
/// tool calls, Codex's `FileChange` items in the rollout (and the rollouts
/// it chains to).
pub fn touched_paths_for(harness: aspen_core::Harness, main_path: &Path) -> Vec<Artifact> {
    match harness {
        aspen_core::Harness::Claude => touched_paths(main_path),
        aspen_core::Harness::Codex => touched_paths_codex(main_path),
    }
}

fn touched_paths_codex(path: &Path) -> Vec<Artifact> {
    let store = aspen_codex::CodexStore::new();
    let Ok(lines) = aspen_codex::store::read_lines_with_history(&store, path, 0) else {
        return Vec::new();
    };
    let mut out: Vec<Artifact> = Vec::new();
    for line in lines.iter().rev() {
        if line.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let p = &line["payload"];
        if p.get("type").and_then(|t| t.as_str()) != Some("item_completed") {
            continue;
        }
        let item = &p["item"];
        let at = line
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(str::to_owned);
        match item.get("type").and_then(|t| t.as_str()) {
            Some("FileChange") => {
                if item.get("status").and_then(|s| s.as_str()) != Some("completed") {
                    continue;
                }
                if let Some(changes) = item.get("changes").and_then(|c| c.as_object()) {
                    for (path, ch) in changes {
                        let kind = match ch.get("type").and_then(|t| t.as_str()) {
                            Some("add") => "wrote",
                            Some("delete") => continue,
                            _ => "edited",
                        };
                        if let Some(existing) = out.iter_mut().find(|a| &a.path == path) {
                            if existing.kind == "edited" && kind == "wrote" {
                                existing.kind = kind.into();
                            }
                        } else {
                            out.push(Artifact {
                                path: path.clone(),
                                kind: kind.into(),
                                at: at.clone(),
                                tool: "fileChange".into(),
                            });
                        }
                    }
                }
            }
            Some("CommandExecution") => {
                // Reads the app-server parsed (`cat`, `head`, …).
                if let Some(actions) = item.get("parsed_cmd").and_then(|a| a.as_array()) {
                    for a in actions {
                        if a.get("type").and_then(|t| t.as_str()) == Some("read") {
                            if let Some(path) = a
                                .get("path")
                                .or_else(|| a.get("name"))
                                .and_then(|x| x.as_str())
                            {
                                if !out.iter().any(|x| x.path == path) {
                                    out.push(Artifact {
                                        path: path.to_owned(),
                                        kind: "read".into(),
                                        at: at.clone(),
                                        tool: "commandExecution".into(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Files named by a session's tool calls, newest first, deduplicated to
/// the strongest kind seen (wrote > edited > read).
pub fn touched_paths(path: &Path) -> Vec<Artifact> {
    let Ok(text) = std::fs::read_to_string(path) else {
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
/// The directories an agent's files may come from, labelled: the repo, the
/// session's own data, plans, temp and its attachments. One list for
/// reading (`resolve`) and listing (`list_dir`), so they cannot disagree.
pub fn roots(
    data_dir: Option<&Path>,
    repo: &Path,
    session_id: Option<&str>,
) -> Vec<(&'static str, PathBuf)> {
    let claude = aspen_claude::transcript::claude_home();
    let mut v = vec![
        ("repo", canon(repo)),
        (
            "session data",
            canon(
                &claude
                    .join("projects")
                    .join(aspen_claude::transcript::project_slug(repo)),
            ),
        ),
        ("image cache", canon(&claude.join("image-cache"))),
        ("plans", canon(&claude.join("plans"))),
        (
            "codex sessions",
            canon(&aspen_codex::codex_home().join("sessions")),
        ),
        ("temp", canon(&std::env::temp_dir())),
    ];
    if let (Some(dd), Some(sid)) = (data_dir, session_id) {
        v.push(("attachments", canon(&attachments_dir(dd, sid))));
    }
    v
}

/// At most this many entries in one listing.
pub const LIST_MAX: usize = 2000;

/// One directory of the agent's file space (PROPOSALS-2026-10-R.md R-1):
/// gated like reading (the directory must lie in a root); an entry whose
/// resolved target leaves the roots (a symlink out) is listed but not
/// served. `.git` is left out; dotfiles and gitignored names are marked
/// `hidden`. In temp, only entries changed since `since` (temp is shared
/// by everything on the machine).
pub fn list_dir(
    data_dir: Option<&Path>,
    repo: &Path,
    session_id: Option<&str>,
    dir: &str,
    since: Option<f64>,
) -> Result<Value> {
    let raw = expand_home(dir.trim());
    let target = canon(&if raw.is_absolute() {
        raw
    } else {
        repo.join(raw)
    });
    let roots = roots(data_dir, repo, session_id);
    let Some((label, root)) = roots
        .iter()
        .filter(|(_, r)| target.starts_with(r))
        .max_by_key(|(_, r)| r.as_os_str().len())
        .cloned()
    else {
        return Err(anyhow!(
            "not served: {} is outside the agent's repo, the session's own data, temp and its attachments",
            target.display()
        ));
    };
    let in_temp = label == "temp";
    let rd = std::fs::read_dir(&target).map_err(|e| anyhow!("{}: {e}", target.display()))?;
    let mut entries: Vec<Value> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut truncated = false;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let Ok(lm) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        let meta = std::fs::metadata(e.path()).ok();
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        if in_temp && since.is_some_and(|s| mtime.unwrap_or(0.0) < s) {
            continue;
        }
        if entries.len() >= LIST_MAX {
            truncated = true;
            break;
        }
        let resolved = canon(&e.path());
        let served = roots.iter().any(|(_, r)| resolved.starts_with(r));
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        entries.push(serde_json::json!({
            "name": name,
            "kind": if lm.file_type().is_symlink() { "link" } else if is_dir { "dir" } else { "file" },
            "is_dir": is_dir,
            "size": meta.as_ref().map(|m| m.len()),
            "mtime": mtime,
            "served": served,
            "hidden": name.starts_with('.'),
        }));
        names.push(name);
    }
    // Gitignored names, in one `git check-ignore` for the whole listing.
    if label == "repo" && !names.is_empty() {
        let ignored = git_ignored(&target, &names);
        for e in entries.iter_mut() {
            if e["name"].as_str().is_some_and(|n| ignored.contains(n)) {
                e["hidden"] = Value::Bool(true);
            }
        }
    }
    entries.sort_by(|a, b| {
        let (ad, bd) = (
            a["is_dir"].as_bool().unwrap_or(false),
            b["is_dir"].as_bool().unwrap_or(false),
        );
        bd.cmp(&ad).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    let parent = (target != root)
        .then(|| target.parent().map(|p| p.to_string_lossy().into_owned()))
        .flatten();
    Ok(serde_json::json!({
        "dir": target.to_string_lossy(),
        "root": root.to_string_lossy(),
        "root_label": label,
        "parent": parent,
        "entries": entries,
        "truncated": truncated,
    }))
}

/// Which of `names` (in `dir`) git ignores; empty when not a git tree.
fn git_ignored(dir: &Path, names: &[String]) -> std::collections::HashSet<String> {
    use std::io::Write;
    let mut cmd = crate::gitstate::quiet_command("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["check-ignore", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return Default::default();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(names.join("\n").as_bytes());
    }
    child
        .wait_with_output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The newest-modified files in the repo (R-1 "recent"): what an agent made
/// however it wrote it. Skips `.git`, dependency and build trees; walks at
/// most 50,000 entries.
pub fn recent_files(repo: &Path, limit: usize) -> Vec<Value> {
    const SKIP: [&str; 6] = [
        ".git",
        "node_modules",
        "target",
        "dist",
        ".venv",
        "__pycache__",
    ];
    let mut found: Vec<(f64, PathBuf, u64)> = Vec::new();
    let mut stack = vec![canon(repo)];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > 50_000 {
                stack.clear();
                break;
            }
            let name = e.file_name();
            let n = name.to_string_lossy();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if !SKIP.contains(&n.as_ref()) {
                    stack.push(e.path());
                }
                continue;
            }
            if let Ok(m) = e.metadata() {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                found.push((t, e.path(), m.len()));
            }
        }
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    found
        .into_iter()
        .take(limit)
        .map(|(t, p, size)| {
            serde_json::json!({
                "path": p.to_string_lossy(),
                "name": p.file_name().map(|n| n.to_string_lossy().into_owned()),
                "size": size,
                "mtime": t,
            })
        })
        .collect()
}

pub fn resolve(
    data_dir: Option<&Path>,
    repo: &Path,
    session_id: Option<&str>,
    path: &str,
    harness: aspen_core::Harness,
) -> Result<PathBuf> {
    let raw = expand_home(path.trim());
    let target = if raw.is_absolute() {
        raw
    } else {
        // Repo-relative, as the agent writes them in prose.
        repo.join(raw)
    };
    let target = canon(&target);

    let roots: Vec<PathBuf> = roots(data_dir, repo, session_id)
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    if roots.iter().any(|r| target.starts_with(r)) {
        return Ok(target);
    }
    if let Some(sid) = session_id {
        let main = match harness {
            aspen_core::Harness::Claude => aspen_claude::transcript::transcript_path(repo, sid),
            aspen_core::Harness::Codex => {
                aspen_core::SessionStore::main_path(&aspen_codex::CodexStore::new(), repo, sid)
            }
        };
        let touched = touched_paths_for(harness, &main);
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

/// One pasted attachment on an operator message: `n` matches the marker
/// `[attachment n: name]` in the text; `data` is base64.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub n: u32,
    pub name: String,
    pub media_type: String,
    pub data: String,
}

/// Per-attachment and per-message caps (raw bytes).
pub const ATTACHMENT_MAX: usize = 8 * 1024 * 1024;
pub const ATTACHMENTS_MAX_TOTAL: usize = 24 * 1024 * 1024;

fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "attachment".into()
    } else {
        cleaned
    }
}

/// Save attachments under `dir` and build the runtime content array:
/// text split at markers, raster images inserted as image blocks where
/// their marker was, other files' markers rewritten to the saved path.
/// Returns the content and a plain-text rendering for summaries.
pub fn compose_with_attachments(
    dir: &Path,
    text: &str,
    attachments: &[Attachment],
) -> Result<(Value, String)> {
    std::fs::create_dir_all(dir)?;
    let mut total = 0usize;
    let mut saved: std::collections::HashMap<u32, (PathBuf, &Attachment, Vec<u8>)> =
        std::collections::HashMap::new();
    for a in attachments {
        let bytes = aspen_wire::b64::decode(&a.data)
            .map_err(|e| anyhow!("attachment {}: bad base64: {e}", a.n))?;
        if bytes.len() > ATTACHMENT_MAX {
            return Err(anyhow!(
                "attachment {} ({}) is {} bytes; the cap is {}",
                a.n,
                a.name,
                bytes.len(),
                ATTACHMENT_MAX
            ));
        }
        total += bytes.len();
        if total > ATTACHMENTS_MAX_TOTAL {
            return Err(anyhow!(
                "attachments total more than {} bytes",
                ATTACHMENTS_MAX_TOTAL
            ));
        }
        let path = dir.join(format!("{}-{}", a.n, safe_name(&a.name)));
        std::fs::write(&path, &bytes)?;
        saved.insert(a.n, (path, a, bytes));
    }
    let is_raster = |m: &str| matches!(m, "image/png" | "image/jpeg" | "image/gif" | "image/webp");

    // Walk the text; at each marker emit what came before, then the block.
    let re = regex_lite::Regex::new(r"\[attachment (\d+): [^\]]*\]").expect("marker regex");
    let mut blocks: Vec<Value> = Vec::new();
    let mut plain = String::new();
    let mut buf = String::new();
    let mut last = 0usize;
    for m in re.find_iter(text) {
        buf.push_str(&text[last..m.start()]);
        let n: u32 = m.as_str()[12..]
            .split(':')
            .next()
            .and_then(|x| x.trim().parse().ok())
            .unwrap_or(0);
        match saved.get(&n) {
            Some((path, a, _)) if is_raster(&a.media_type) => {
                let note = format!(
                    "[attachment {}: {} — image below, also saved at {}]",
                    n,
                    a.name,
                    path.display()
                );
                buf.push_str(&note);
                plain.push_str(&buf);
                blocks
                    .push(serde_json::json!({ "type": "text", "text": std::mem::take(&mut buf) }));
                blocks.push(serde_json::json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": a.media_type, "data": a.data },
                }));
            }
            Some((path, a, _)) => {
                let note = format!(
                    "[attachment {}: {} — saved at {}; read it from there]",
                    n,
                    a.name,
                    path.display()
                );
                buf.push_str(&note);
            }
            None => buf.push_str(m.as_str()),
        }
        last = m.end();
    }
    buf.push_str(&text[last..]);
    // Attachments never referenced by a marker still ride along, at the end.
    let mut unreferenced: Vec<&(PathBuf, &Attachment, Vec<u8>)> = saved
        .values()
        .filter(|(_, a, _)| {
            !re.find_iter(text)
                .any(|m| m.as_str().starts_with(&format!("[attachment {}:", a.n)))
        })
        .collect();
    unreferenced.sort_by_key(|(_, a, _)| a.n);
    for (path, a, _) in unreferenced {
        if is_raster(&a.media_type) {
            buf.push_str(&format!(
                "\n[attachment {}: {} — image below, also saved at {}]",
                a.n,
                a.name,
                path.display()
            ));
            plain.push_str(&buf);
            blocks.push(serde_json::json!({ "type": "text", "text": std::mem::take(&mut buf) }));
            blocks.push(serde_json::json!({
                "type": "image",
                "source": { "type": "base64", "media_type": a.media_type, "data": a.data },
            }));
        } else {
            buf.push_str(&format!(
                "\n[attachment {}: {} — saved at {}; read it from there]",
                a.n,
                a.name,
                path.display()
            ));
        }
    }
    if !buf.is_empty() || blocks.is_empty() {
        plain.push_str(&buf);
        blocks.push(serde_json::json!({ "type": "text", "text": buf }));
    }
    Ok((Value::Array(blocks), plain))
}

#[cfg(test)]
mod list_tests {
    use super::*;

    #[test]
    fn a_directory_lists_inside_its_root_and_nothing_escapes() {
        // Not under the system temp dir: that is a root itself.
        let t = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let repo = t.path().join("repo");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("docs/report.md"), "x").unwrap();
        std::fs::write(repo.join(".env"), "SECRET=1").unwrap();
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, repo.join("escape")).unwrap();

        let v = list_dir(None, &repo, None, ".", None).unwrap();
        let names: Vec<&str> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"docs"));
        assert!(!names.contains(&".git"), "{names:?}");
        let env = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == ".env")
            .unwrap();
        assert_eq!(env["hidden"], true);
        #[cfg(unix)]
        {
            let esc = v["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == "escape")
                .unwrap();
            assert_eq!(esc["served"], false);
            // Listing through the link is refused: it resolves outside.
            assert!(list_dir(None, &repo, None, "escape", None).is_err());
        }
        assert!(list_dir(None, &repo, None, &outside.to_string_lossy(), None).is_err());
        let sub = list_dir(None, &repo, None, "docs", None).unwrap();
        assert_eq!(sub["entries"][0]["name"], "report.md");
        assert!(sub["parent"].as_str().unwrap().ends_with("repo"));
    }

    #[test]
    fn recent_lists_newest_files_first_and_skips_build_trees() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path();
        std::fs::create_dir_all(repo.join("node_modules/x")).unwrap();
        std::fs::write(repo.join("node_modules/x/big.js"), "x").unwrap();
        std::fs::write(repo.join("old.md"), "x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(repo.join("new.md"), "x").unwrap();
        let r = recent_files(repo, 10);
        let names: Vec<&str> = r.iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["new.md", "old.md"]);
    }
}
