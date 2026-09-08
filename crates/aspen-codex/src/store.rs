//! The Codex session store: rollouts on disk (PROPOSALS-HARNESSES.md §3.4,
//! CODEX_RUNTIME_REFERENCE.md §3). One JSONL per thread under
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread>.jsonl`; every line
//! is `{timestamp, ordinal, type, payload}`. The first line is
//! `session_meta` (cwd, originator, forked_from_id); `event_msg` lines with
//! `item_completed` payloads carry the conversation in the app-server's
//! own item shapes (PascalCase types, snake_case fields); `token_count`
//! lines carry usage. Everything Aspen reads from a Codex session that is
//! not running comes from here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use aspen_core::{Harness, ProjectDirs, SessionInfo, SessionOrigin, SessionStore};

/// `$CODEX_HOME`, else `~/.codex`.
pub fn codex_home() -> PathBuf {
    if let Ok(h) = std::env::var("CODEX_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
        .unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".codex")
}

fn sessions_dir() -> PathBuf {
    codex_home().join("sessions")
}

/// What the first line of a rollout says.
#[derive(Debug, Clone)]
pub struct RolloutMeta {
    pub thread_id: String,
    pub cwd: PathBuf,
    pub originator: Option<String>,
    pub forked_from: Option<String>,
    pub timestamp: Option<String>,
    pub cli_version: Option<String>,
    /// A fork's inherited prefix: (parent thread id, exclusive end ordinal).
    pub history_base: Option<(String, u64)>,
}

/// Every rollout file under the sessions tree, newest directory first.
pub fn rollout_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let root = sessions_dir();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
            {
                out.push(p);
            }
        }
    }
    out
}

/// The thread id a rollout file name ends with.
pub fn thread_id_of(path: &Path) -> Option<String> {
    let name = path.file_stem()?.to_str()?;
    // rollout-YYYY-MM-DDThh-mm-ss-<uuid>; the uuid is the last 36 chars.
    if name.len() > 36 {
        let id = &name[name.len() - 36..];
        if id.chars().filter(|c| *c == '-').count() == 4 {
            return Some(id.to_owned());
        }
    }
    None
}

pub fn read_meta(path: &Path) -> Option<RolloutMeta> {
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(file), &mut first).ok()?;
    let v: Value = serde_json::from_str(first.trim()).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let p = v.get("payload")?;
    let s = |k: &str| p.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    Some(RolloutMeta {
        thread_id: s("id").or_else(|| thread_id_of(path))?,
        cwd: PathBuf::from(s("cwd").unwrap_or_default()),
        originator: s("originator"),
        forked_from: s("forked_from_id"),
        timestamp: s("timestamp"),
        cli_version: s("cli_version"),
        history_base: p.get("history_base").and_then(|h| {
            Some((h.get("thread_id")?.as_str()?.to_owned(), h.get("end_ordinal_exclusive")?.as_u64()?))
        }),
    })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn modified_epoch(path: &Path) -> Option<f64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
}

fn iso_to_epoch(ts: &str) -> Option<f64> {
    // 2026-09-07T21:50:34.330Z — no chrono; parse the fixed layout.
    let b = ts.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let y = num(&ts[0..4])?;
    let mo = num(&ts[5..7])?;
    let d = num(&ts[8..10])?;
    let h = num(&ts[11..13])?;
    let mi = num(&ts[14..16])?;
    let s = num(&ts[17..19])?;
    let frac: f64 = if b.len() > 20 && b[19] == b'.' {
        let end = ts[20..].find(|c: char| !c.is_ascii_digit()).map(|i| 20 + i).unwrap_or(ts.len());
        format!("0.{}", &ts[20..end]).parse().unwrap_or(0.0)
    } else {
        0.0
    };
    // days from civil (Howard Hinnant)
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + (h * 3600 + mi * 60 + s) as f64 + frac)
}

/// The rollout store, with a small (path, mtime) → meta cache so repeated
/// enumeration does not reread every first line.
#[derive(Default)]
pub struct CodexStore {
    meta_cache: Mutex<HashMap<PathBuf, (f64, RolloutMeta)>>,
    /// thread id → rollout path, rebuilt at most every few seconds (the
    /// tree walk is a directory scan per day; every store call needs it).
    index: Mutex<Option<(std::time::Instant, HashMap<String, PathBuf>)>>,
}

impl CodexStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn meta_cached(&self, path: &Path) -> Option<RolloutMeta> {
        let mtime = modified_epoch(path).unwrap_or(0.0);
        if let Some((t, m)) = self.meta_cache.lock().unwrap().get(path) {
            if *t == mtime {
                return Some(m.clone());
            }
        }
        let m = read_meta(path)?;
        self.meta_cache.lock().unwrap().insert(path.to_path_buf(), (mtime, m.clone()));
        Some(m)
    }

    fn index(&self, force: bool) -> HashMap<String, PathBuf> {
        let mut guard = self.index.lock().unwrap();
        if !force {
            if let Some((at, m)) = guard.as_ref() {
                if at.elapsed() < std::time::Duration::from_secs(5) {
                    return m.clone();
                }
            }
        }
        let m: HashMap<String, PathBuf> = rollout_files().into_iter().filter_map(|p| thread_id_of(&p).map(|id| (id, p))).collect();
        *guard = Some((std::time::Instant::now(), m.clone()));
        m
    }

    /// The rollout file for a thread id, if any (indexed; a miss rescans
    /// once, so a thread started seconds ago is found).
    pub fn rollout_path(&self, thread_id: &str) -> Option<PathBuf> {
        if let Some(p) = self.index(false).get(thread_id) {
            return Some(p.clone());
        }
        self.index(true).get(thread_id).cloned()
    }

    fn rollouts_for(&self, repo: &Path) -> Vec<(PathBuf, RolloutMeta)> {
        rollout_files()
            .into_iter()
            .filter_map(|p| self.meta_cached(&p).map(|m| (p, m)))
            .filter(|(_, m)| same_dir(&m.cwd, repo))
            .collect()
    }
}

/// A rollout with its inherited history: a fork's rollout starts where
/// the parent's `history_base` ends, so the parent's lines up to that
/// ordinal come first (recursively — a fork of a fork).
pub fn read_lines_with_history(store: &CodexStore, path: &Path, depth: usize) -> Result<Vec<Value>> {
    let own = read_lines(path)?;
    if depth > 8 {
        return Ok(own);
    }
    let Some(meta) = store.meta_cached(path) else { return Ok(own) };
    let Some((parent, end)) = meta.history_base else { return Ok(own) };
    let Some(parent_path) = store.rollout_path(&parent) else { return Ok(own) };
    let mut lines: Vec<Value> = read_lines_with_history(store, &parent_path, depth + 1)?
        .into_iter()
        .filter(|l| {
            // Inherited lines keep their own ordinals; only the parent's
            // own lines are bounded by the fork point.
            l.get("ordinal").and_then(|o| o.as_u64()).is_none_or(|o| o < end)
                || l.get("__inherited").is_some()
        })
        .map(|mut l| {
            l["__inherited"] = json!(true);
            l
        })
        .collect();
    lines.extend(own);
    Ok(lines)
}

/// Read every line of a rollout as JSON, in order.
pub fn read_lines(path: &Path) -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading rollout {}", path.display()))?;
    Ok(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect())
}

fn item_text(item: &Value) -> String {
    item.get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn command_string(item: &Value) -> String {
    match item.get("command") {
        Some(Value::Array(parts)) => {
            // ["/bin/bash","-lc","echo x"] → the script; else joined.
            if parts.len() == 3 && parts[1].as_str() == Some("-lc") {
                parts[2].as_str().unwrap_or("").to_owned()
            } else {
                parts.iter().filter_map(|p| p.as_str()).collect::<Vec<_>>().join(" ")
            }
        }
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Fold a rollout into the console's rehydrated item shape (the same one
/// the Claude store produces: `{role, text, tools[], usage, timestamp, uuid}`).
pub fn rehydrate_lines(lines: &[Value]) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut model: Option<String> = None;
    for line in lines {
        let ts = line.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_owned();
        let ty = line.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = line.get("payload").cloned().unwrap_or(Value::Null);
        match ty {
            "turn_context" => {
                if let Some(m) = payload.get("model").and_then(|m| m.as_str()) {
                    model = Some(m.to_owned());
                }
            }
            "event_msg" => {
                let pt = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match pt {
                    "item_completed" => {
                        let item = payload.get("item").cloned().unwrap_or(Value::Null);
                        let it = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        let id = item.get("id").and_then(|i| i.as_str()).unwrap_or("").to_owned();
                        match it {
                            "UserMessage" => {
                                let text = item_text(&item);
                                let is_bus = text.trim_start().starts_with("[aspen bus]");
                                items.push(json!({
                                    "role": "user", "bus": is_bus, "text": text,
                                    "images": [], "uuid": id, "timestamp": ts,
                                }));
                            }
                            "AgentMessage" => {
                                let text = item_text(&item);
                                items.push(json!({
                                    "role": "assistant", "text": text, "tools": [],
                                    "uuid": id, "timestamp": ts, "usage": null, "model": model,
                                }));
                            }
                            "Reasoning" | "ContextCompaction" | "Extension" | "Sleep" => {}
                            _ => {
                                // A tool-shaped item: attach to the open assistant
                                // bubble (or open one for it).
                                let (name, input, result, is_error) = tool_from_item(it, &item);
                                let tool = json!({
                                    "id": id, "name": name, "input": input,
                                    "result": result, "is_error": is_error,
                                });
                                let attach = items
                                    .last()
                                    .is_some_and(|l| l.get("role").and_then(|r| r.as_str()) == Some("assistant"));
                                if attach {
                                    if let Some(arr) = items.last_mut().and_then(|l| l["tools"].as_array_mut()) {
                                        arr.push(tool);
                                    }
                                } else {
                                    items.push(json!({
                                        "role": "assistant", "text": "", "tools": [tool],
                                        "uuid": id, "timestamp": ts, "usage": null, "model": model,
                                    }));
                                }
                            }
                        }
                    }
                    "token_count" => {
                        if let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) {
                            let n = |k: &str| last.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                            let usage = json!({
                                "input": n("input_tokens"), "output": n("output_tokens"),
                                "cache_read": n("cached_input_tokens"), "cache_create": n("cache_write_input_tokens"),
                            });
                            if let Some(l) = items
                                .iter_mut()
                                .rev()
                                .find(|l| l.get("role").and_then(|r| r.as_str()) == Some("assistant"))
                            {
                                l["usage"] = usage;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    items
}

/// A rollout item as a tool card: (name, input, result, is_error).
fn tool_from_item(it: &str, item: &Value) -> (String, Value, Value, bool) {
    let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
    let is_error = matches!(status, "failed" | "declined");
    match it {
        "CommandExecution" => {
            let cmd = command_string(item);
            let out = item
                .get("formatted_output")
                .or_else(|| item.get("aggregated_output"))
                .and_then(|o| o.as_str())
                .unwrap_or("")
                .to_owned();
            let exit = item.get("exit_code").and_then(|e| e.as_i64());
            let result = match (status, exit) {
                ("declined", _) => "declined".to_owned(),
                (_, Some(c)) if c != 0 => format!("{out}\n(exit code {c})"),
                _ => out,
            };
            (
                "commandExecution".into(),
                json!({ "command": cmd, "cwd": item.get("cwd") }),
                Value::String(result),
                is_error || exit.is_some_and(|c| c != 0),
            )
        }
        "FileChange" => {
            let changes: Vec<Value> = item
                .get("changes")
                .and_then(|c| c.as_object())
                .map(|m| {
                    m.iter()
                        .map(|(path, ch)| {
                            json!({
                                "path": path,
                                "kind": ch.get("type"),
                                "diff": ch.get("unified_diff").or_else(|| ch.get("content")).cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let out = item.get("stdout").and_then(|o| o.as_str()).unwrap_or("").to_owned();
            ("fileChange".into(), json!({ "changes": changes }), Value::String(out), is_error)
        }
        "McpToolCall" => {
            let server = item.get("server").and_then(|s| s.as_str()).unwrap_or("mcp");
            let tool = item.get("tool").and_then(|s| s.as_str()).unwrap_or("tool");
            let result = item
                .get("result")
                .map(|r| serde_json::to_string_pretty(r).unwrap_or_default())
                .or_else(|| item.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).map(str::to_owned))
                .unwrap_or_default();
            (
                format!("mcp__{server}__{tool}"),
                item.get("arguments").cloned().unwrap_or(json!({})),
                Value::String(result),
                is_error || item.get("error").is_some_and(|e| !e.is_null()),
            )
        }
        "WebSearch" => (
            "webSearch".into(),
            json!({ "query": item.get("query") }),
            Value::Null,
            false,
        ),
        other => (
            lower_camel(other),
            item.clone(),
            Value::Null,
            is_error,
        ),
    }
}

fn lower_camel(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Usage folded from `token_count` lines, in the console's SessionUsage shape.
pub fn usage_of(lines: &[Value], thread_id: &str) -> Value {
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_read = 0u64;
    let mut cache_create = 0u64;
    let mut calls = 0u64;
    let mut turns = 0u64;
    let mut model = String::from("codex");
    let mut first_ts: Option<String> = None;
    let mut last_ts: Option<String> = None;
    for line in lines {
        let ts = line.get("timestamp").and_then(|t| t.as_str()).map(str::to_owned);
        if first_ts.is_none() {
            first_ts = ts.clone();
        }
        last_ts = ts.or(last_ts);
        let payload = line.get("payload").cloned().unwrap_or(Value::Null);
        match line.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "turn_context" => {
                if let Some(m) = payload.get("model").and_then(|m| m.as_str()) {
                    model = m.to_owned();
                }
            }
            "event_msg" => match payload.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "task_started" => turns += 1,
                "token_count" => {
                    if let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) {
                        let n = |k: &str| last.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                        input += n("input_tokens");
                        output += n("output_tokens");
                        cache_read += n("cached_input_tokens");
                        cache_create += n("cache_write_input_tokens");
                        calls += 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    let per = json!({
        "input": input, "output": output, "cache_read": cache_read, "cache_create": cache_create,
        "calls": calls, "cost_usd": null,
    });
    json!({
        "session_id": thread_id,
        "models": { model: per },
        "total": per,
        "turns": turns,
        "cost_usd": null,
        "subagents": 0,
        "subagent_tokens": 0,
        "first_ts": first_ts,
        "last_ts": last_ts,
        "lines_added": null,
        "lines_removed": null,
    })
}

impl SessionStore for CodexStore {
    fn harness(&self) -> Harness {
        Harness::Codex
    }
    fn exists(&self, _repo: &Path, sid: &str) -> bool {
        self.rollout_path(sid).is_some()
    }
    fn modified(&self, _repo: &Path, sid: &str) -> Option<f64> {
        self.rollout_path(sid).and_then(|p| modified_epoch(&p))
    }
    fn enumerate(&self, repo: &Path) -> Result<Vec<SessionInfo>> {
        let mut out = Vec::new();
        for (path, meta) in self.rollouts_for(repo) {
            let lines = read_lines(&path).unwrap_or_default();
            let mut user_messages = 0usize;
            let mut title: Option<String> = None;
            for l in &lines {
                if l.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
                    continue;
                }
                let p = &l["payload"];
                if p.get("type").and_then(|t| t.as_str()) == Some("item_completed")
                    && p["item"].get("type").and_then(|t| t.as_str()) == Some("UserMessage")
                {
                    user_messages += 1;
                    if title.is_none() {
                        let t = item_text(&p["item"]);
                        if !t.trim_start().starts_with("[aspen bus]") {
                            title = Some(t.lines().next().unwrap_or("").chars().take(80).collect());
                        }
                    }
                }
            }
            out.push(SessionInfo {
                session_id: meta.thread_id.clone(),
                title: title.filter(|t| !t.is_empty()),
                entrypoint: meta.originator.clone(),
                modified_epoch: modified_epoch(&path).unwrap_or(0.0),
                user_messages,
                harness: Harness::Codex,
            });
        }
        out.sort_by(|a, b| b.modified_epoch.total_cmp(&a.modified_epoch));
        Ok(out)
    }
    fn rehydrate(&self, _repo: &Path, sid: &str) -> Result<Vec<Value>> {
        let path = self.rollout_path(sid).with_context(|| format!("no rollout for thread {sid}"))?;
        Ok(rehydrate_lines(&read_lines_with_history(self, &path, 0)?))
    }
    fn rehydrate_after(&self, repo: &Path, sid: &str, after: &str) -> Result<(Vec<Value>, bool)> {
        let items = self.rehydrate(repo, sid)?;
        let idx = items
            .iter()
            .position(|i| i.get("role").and_then(|r| r.as_str()) == Some("user") && i.get("uuid").and_then(|u| u.as_str()) == Some(after));
        match idx {
            Some(i) => Ok((items[i + 1..].to_vec(), true)),
            None => Ok((items, false)),
        }
    }
    fn rehydrate_file(&self, path: &Path) -> Result<Vec<Value>> {
        Ok(rehydrate_lines(&read_lines(path)?))
    }
    fn origin(&self, _repo: &Path, sid: &str) -> Option<SessionOrigin> {
        let path = self.rollout_path(sid)?;
        let meta = self.meta_cached(&path)?;
        let lines = read_lines(&path).ok()?;
        let first_ref = lines.iter().find_map(|l| {
            let p = l.get("payload")?;
            if l.get("type")?.as_str()? == "event_msg"
                && p.get("type")?.as_str()? == "item_completed"
                && p["item"].get("type")?.as_str()? == "UserMessage"
            {
                p["item"].get("id")?.as_str().map(str::to_owned)
            } else {
                None
            }
        });
        let last_ts = lines.last().and_then(|l| l.get("timestamp")).and_then(|t| t.as_str()).and_then(iso_to_epoch);
        Some(SessionOrigin {
            forked_from: meta.forked_from.map(|p| (p, None)),
            first_ref,
            last_entrypoint: meta.originator,
            last_ts,
        })
    }
    fn files(&self, _repo: &Path, sid: &str) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let mut next = Some(sid.to_owned());
        let mut seen = 0;
        while let Some(id) = next.take() {
            let Some(path) = self.rollout_path(&id) else { break };
            let rel = path
                .strip_prefix(codex_home())
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            out.push((rel, path.clone()));
            // A fork carries its parent's rollout too (the history base).
            next = self.meta_cached(&path).and_then(|m| m.history_base).map(|(p, _)| p);
            seen += 1;
            if seen > 8 {
                break;
            }
        }
        out
    }
    fn main_path(&self, _repo: &Path, sid: &str) -> PathBuf {
        self.rollout_path(sid).unwrap_or_else(|| sessions_dir().join(format!("rollout-{sid}.jsonl")))
    }
    fn usage(&self, _repo: &Path, sid: &str) -> Value {
        match self.rollout_path(sid).and_then(|p| read_lines_with_history(self, &p, 0).ok()) {
            Some(lines) => usage_of(&lines, sid),
            None => Value::Null,
        }
    }
    fn activities(&self, _repo: &Path, _sid: &str, _since: Option<f64>) -> Vec<Value> {
        Vec::new()
    }
    fn activity_counts(&self, _repo: &Path, _sid: Option<&str>, _since: Option<f64>) -> Value {
        json!({ "running": 0, "agents": 0, "tasks": 0, "workflows": 0, "monitors": 0 })
    }
    fn subagent(&self, _repo: &Path, _sid: &str, agent_id: &str) -> Result<Vec<Value>> {
        anyhow::bail!("codex sessions have no subagent transcripts (agent {agent_id})")
    }
    fn project_dirs(&self, _repo: &Path) -> ProjectDirs {
        let home = codex_home();
        ProjectDirs {
            project_dir: None,
            memory_dir: None,
            artifact_roots: vec![home.join("sessions")],
            encoded: None,
            home: Some(home),
        }
    }
    fn discover_repos(&self) -> Vec<(PathBuf, usize)> {
        let mut by_cwd: HashMap<PathBuf, usize> = HashMap::new();
        for p in rollout_files() {
            if let Some(m) = self.meta_cached(&p) {
                if m.cwd.is_dir() {
                    *by_cwd.entry(m.cwd).or_default() += 1;
                }
            }
        }
        by_cwd.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_parse() {
        let e = iso_to_epoch("2026-09-07T21:50:34.330Z").unwrap();
        assert!((e - 1788817834.33).abs() < 0.01, "{e}");
    }

    #[test]
    fn thread_id_from_name() {
        let p = Path::new("/x/rollout-2026-09-07T14-50-31-01a07dd9-f5a1-7810-8a13-d723094e2739.jsonl");
        assert_eq!(thread_id_of(p).as_deref(), Some("01a07dd9-f5a1-7810-8a13-d723094e2739"));
    }

    #[test]
    fn rehydrates_items() {
        let lines: Vec<Value> = vec![
            json!({"timestamp":"t0","type":"turn_context","payload":{"model":"gpt-6"}}),
            json!({"timestamp":"t1","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u1","content":[{"type":"text","text":"hi"}]}}}),
            json!({"timestamp":"t2","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"a1","content":[{"type":"Text","text":"running"}]}}}),
            json!({"timestamp":"t3","type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"e1","command":["/bin/bash","-lc","echo x"],"status":"completed","aggregated_output":"x\n","exit_code":0}}}),
            json!({"timestamp":"t4","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":2,"cached_input_tokens":4,"cache_write_input_tokens":0}}}}),
        ];
        let items = rehydrate_lines(&lines);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["tools"][0]["input"]["command"], "echo x");
        assert_eq!(items[1]["usage"]["input"], 10);
        assert_eq!(items[1]["model"], "gpt-6");
        let u = usage_of(&lines, "th");
        assert_eq!(u["total"]["input"], 10);
    }
}
