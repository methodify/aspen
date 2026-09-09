//! Activity: what a session runs beside its main turn — background shell
//! tasks, subagents, workflows, monitors (PROPOSALS-2026-09 §8; reference
//! docs/ACTIVITY.md).
//!
//! Every fact is in the transcript: a start is a `tool_use` (`Bash` with
//! `run_in_background`, `Agent`, `Workflow`, a scheduling tool); its
//! `tool_result` names the id the harness gave it ("agentId: …",
//! "Command running in background with ID: …", "wf_…"); the end is the
//! `<task-notification>` user line the harness injects when the work
//! stops, with `<task-id>` and `<status>`. A synchronous `Agent` (result
//! returned in the same call) starts and ends at once.
//!
//! The ledger is derived from the file — for a live session too — and
//! cached by the file's size and mtime, so the roster can carry counts
//! every tick without re-parsing a 50 MB transcript.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    /// The harness's id (agent id, task id, run id) when known, else the
    /// tool_use id.
    pub id: String,
    /// task | agent | workflow | monitor
    pub kind: String,
    pub label: String,
    pub tool_use_id: String,
    /// ISO timestamps from the transcript.
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// running | completed | stopped | failed | done
    pub status: String,
    /// Kind-specific: command, agent type, prompt snippet, summary, result
    /// snippet, output file.
    pub detail: Value,
    /// For agents: the sidechain transcript exists under the session's
    /// subagents dir.
    #[serde(default)]
    pub has_transcript: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityCounts {
    pub running: usize,
    pub agents: usize,
    pub tasks: usize,
    pub workflows: usize,
    pub monitors: usize,
}

pub fn counts(acts: &[Activity]) -> ActivityCounts {
    let mut c = ActivityCounts::default();
    for a in acts.iter().filter(|a| a.status == "running") {
        c.running += 1;
        match a.kind.as_str() {
            "agent" => c.agents += 1,
            "task" => c.tasks += 1,
            "workflow" => c.workflows += 1,
            "monitor" => c.monitors += 1,
            _ => {}
        }
    }
    c
}

fn snippet(s: &str, n: usize) -> String {
    let t = s.trim().replace('\n', " ");
    if t.chars().count() <= n {
        t
    } else {
        let cut: String = t.chars().take(n).collect();
        format!("{cut}…")
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    b.get("text").and_then(|t| t.as_str()).map(str::to_owned)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn tag(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let i = text.find(&open)? + open.len();
    let j = text[i..].find(&close)? + i;
    Some(text[i..j].trim().to_owned())
}

fn find_after(text: &str, marker: &str) -> Option<String> {
    let i = text.find(marker)? + marker.len();
    let rest = &text[i..];
    let id: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!id.is_empty()).then_some(id)
}

/// Replay a transcript's lines into a ledger.
pub fn derive(lines: impl Iterator<Item = String>, subagents_dir: Option<&Path>) -> Vec<Activity> {
    let mut acts: Vec<Activity> = Vec::new();
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(str::to_owned);
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let blocks = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        match ty {
            "assistant" => {
                for b in &blocks {
                    if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = b.get("input").cloned().unwrap_or(Value::Null);
                    let tuid = b
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_owned();
                    let (kind, label, detail) = match name {
                        "Bash"
                            if input.get("run_in_background").and_then(|x| x.as_bool())
                                == Some(true) =>
                        {
                            let cmd = input.get("command").and_then(|c| c.as_str()).unwrap_or("");
                            let desc = input
                                .get("description")
                                .and_then(|c| c.as_str())
                                .unwrap_or("");
                            (
                                "task",
                                if desc.is_empty() {
                                    snippet(cmd, 80)
                                } else {
                                    desc.to_owned()
                                },
                                serde_json::json!({ "command": snippet(cmd, 400) }),
                            )
                        }
                        "Agent" | "Task" => {
                            let desc = input
                                .get("description")
                                .and_then(|c| c.as_str())
                                .unwrap_or("agent");
                            (
                                "agent",
                                desc.to_owned(),
                                serde_json::json!({
                                    "agent_type": input.get("subagent_type"),
                                    "model": input.get("model"),
                                    "prompt": input.get("prompt").and_then(|p| p.as_str()).map(|p| snippet(p, 300)),
                                }),
                            )
                        }
                        "Workflow" => {
                            let nm = input
                                .get("name")
                                .and_then(|n| n.as_str())
                                .map(str::to_owned)
                                .or_else(|| {
                                    input
                                        .get("script")
                                        .and_then(|s| s.as_str())
                                        .and_then(|s| find_after(s, "name:"))
                                        .map(|n| n.trim_matches(['\'', '"', ' ']).to_owned())
                                })
                                .unwrap_or_else(|| "workflow".into());
                            ("workflow", nm, serde_json::json!({}))
                        }
                        "ScheduleWakeup" | "CronCreate" | "Monitor" => {
                            let lbl = input
                                .get("reason")
                                .or_else(|| input.get("prompt"))
                                .or_else(|| input.get("command"))
                                .and_then(|r| r.as_str())
                                .map(|r| snippet(r, 80))
                                .unwrap_or_else(|| name.to_lowercase());
                            (
                                "monitor",
                                lbl,
                                serde_json::json!({
                                    "tool": name,
                                    "delay_seconds": input.get("delaySeconds"),
                                    "cron": input.get("cron"),
                                    // The script, whole: the details view and the
                                    // process match (PROPOSALS-MCP.md §6.4) need it.
                                    "command": input.get("command"),
                                    "description": input.get("description"),
                                    "timeout": input.get("timeout"),
                                }),
                            )
                        }
                        _ => continue,
                    };
                    acts.push(Activity {
                        id: tuid.clone(),
                        kind: kind.into(),
                        label,
                        tool_use_id: tuid,
                        started_at: ts.clone(),
                        ended_at: None,
                        status: "running".into(),
                        detail,
                        has_transcript: false,
                    });
                }
            }
            "user" => {
                for b in &blocks {
                    let bt = b.get("type").and_then(|t| t.as_str());
                    if bt == Some("tool_result") {
                        let tuid = b.get("tool_use_id").and_then(|i| i.as_str()).unwrap_or("");
                        let text = text_of(b.get("content").unwrap_or(&Value::Null));
                        let is_err = b.get("is_error").and_then(|e| e.as_bool()) == Some(true);
                        if let Some(a) = acts.iter_mut().find(|a| a.tool_use_id == tuid) {
                            match a.kind.as_str() {
                                "agent" => {
                                    if let Some(id) = find_after(&text, "agentId:") {
                                        a.id = id;
                                    } else {
                                        // Synchronous: the result is the work.
                                        a.status = if is_err {
                                            "failed".into()
                                        } else {
                                            "done".into()
                                        };
                                        a.ended_at = ts.clone();
                                        a.detail["result"] = Value::String(snippet(&text, 300));
                                    }
                                }
                                "task" => {
                                    if let Some(id) = find_after(&text, "background with ID:") {
                                        a.id = id;
                                    } else {
                                        a.status = if is_err {
                                            "failed".into()
                                        } else {
                                            "done".into()
                                        };
                                        a.ended_at = ts.clone();
                                        a.detail["result"] = Value::String(snippet(&text, 300));
                                    }
                                }
                                "workflow" => {
                                    if let Some(id) =
                                        find_after(&text, "wf_").map(|s| format!("wf_{s}"))
                                    {
                                        a.id = id;
                                    }
                                    if is_err {
                                        a.status = "failed".into();
                                        a.ended_at = ts.clone();
                                    }
                                }
                                "monitor" => {
                                    if is_err {
                                        a.status = "failed".into();
                                        a.ended_at = ts.clone();
                                        a.detail["output"] = Value::String(snippet(&text, 2000));
                                    } else if a.detail.get("tool").and_then(|t| t.as_str())
                                        == Some("ScheduleWakeup")
                                    {
                                        // A wakeup fires once; the tool result is its scheduling ack.
                                    } else {
                                        // A background monitor answers with its id
                                        // and later with output; keep what it said.
                                        if let Some(id) = find_after(&text, "background with ID:")
                                            .or_else(|| find_after(&text, "(task "))
                                            .or_else(|| find_after(&text, "monitor ID:"))
                                            .or_else(|| find_after(&text, "ID:"))
                                        {
                                            a.id = id;
                                        }
                                        a.detail["output"] = Value::String(snippet(&text, 2000));
                                    }
                                }
                                _ => {}
                            }
                        }
                    } else if bt == Some("text") || b.is_string() {
                        let text = text_of(&Value::Array(vec![b.clone()]));
                        if text.contains("<task-notification>") {
                            if let Some(id) = tag(&text, "task-id") {
                                let status =
                                    tag(&text, "status").unwrap_or_else(|| "completed".into());
                                let summary = tag(&text, "summary");
                                if let Some(a) = acts.iter_mut().rev().find(|a| a.id == id) {
                                    a.status = status;
                                    a.ended_at = ts.clone();
                                    if let Some(s) = summary {
                                        a.detail["summary"] = Value::String(snippet(&s, 200));
                                        a.detail["output"] = Value::String(snippet(&s, 4000));
                                    }
                                    if let Some(f) = tag(&text, "output-file") {
                                        a.detail["output_file"] = Value::String(f);
                                    }
                                }
                            }
                        }
                    }
                }
                // Plain-string user content (older lines) carrying a notification.
                if let Some(s) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if s.contains("<task-notification>") {
                        if let (Some(id), Some(status)) = (tag(s, "task-id"), tag(s, "status")) {
                            if let Some(a) = acts.iter_mut().rev().find(|a| a.id == id) {
                                a.status = status;
                                a.ended_at = ts.clone();
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(dir) = subagents_dir {
        // meta.json files map a tool_use id to the agent id the harness
        // chose — the only way to find a synchronous agent's transcript.
        let mut by_tool: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if let Some(stem) = n
                    .strip_prefix("agent-")
                    .and_then(|r| r.strip_suffix(".meta.json"))
                {
                    if let Ok(b) = std::fs::read(e.path()) {
                        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
                            if let Some(t) = v.get("toolUseId").and_then(|t| t.as_str()) {
                                by_tool.insert(t.to_owned(), stem.to_owned());
                            }
                        }
                    }
                }
            }
        }
        for a in acts.iter_mut().filter(|a| a.kind == "agent") {
            if let Some(id) = by_tool.get(&a.tool_use_id) {
                a.id = id.clone();
            }
            a.has_transcript = dir.join(format!("agent-{}.jsonl", a.id)).is_file();
        }
    }
    acts
}

/// An activity that was still "running" when its process ended is not
/// running: anything started before `process_started` (epoch seconds)
/// and never ended is marked `unknown`. Notifications for it will never
/// come — the harness that owned it is gone.
pub fn settle_before(acts: &mut [Activity], process_started: Option<f64>) {
    let Some(ps) = process_started else { return };
    for a in acts.iter_mut().filter(|a| a.status == "running") {
        let started = a.started_at.as_deref().and_then(parse_iso).unwrap_or(0.0);
        if started < ps {
            a.status = "unknown".into();
        }
    }
}

/// ISO-8601 `YYYY-MM-DDTHH:MM:SS(.fff)Z` → epoch seconds (UTC only).
pub fn parse_iso(s: &str) -> Option<f64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let mut t = time.split(':');
    let (h, mi): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: f64 = t.next()?.parse().ok()?;
    // days from civil (Howard Hinnant)
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + sec)
}

/// The session's subagents directory: `<project>/<session>/subagents`.
pub fn subagents_dir(project_path: &Path, session_id: &str) -> PathBuf {
    crate::transcript::transcript_path(project_path, session_id)
        .with_extension("")
        .join("subagents")
}

pub fn subagent_transcript(project_path: &Path, session_id: &str, agent_id: &str) -> PathBuf {
    subagents_dir(project_path, session_id).join(format!("agent-{agent_id}.jsonl"))
}

struct Cached {
    len: u64,
    mtime: f64,
    acts: Arc<Vec<Activity>>,
}

fn cache() -> &'static Mutex<std::collections::HashMap<PathBuf, Cached>> {
    static C: OnceLock<Mutex<std::collections::HashMap<PathBuf, Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// The ledger for a session, re-derived only when the transcript changed.
/// The ledger with the stale rule applied for a process started at
/// `process_started`.
pub fn activities_for(
    project_path: &Path,
    session_id: &str,
    process_started: Option<f64>,
) -> Vec<Activity> {
    let mut v: Vec<Activity> = (*activities(project_path, session_id)).clone();
    settle_before(&mut v, process_started);
    v
}

pub fn activities(project_path: &Path, session_id: &str) -> Arc<Vec<Activity>> {
    let path = crate::transcript::transcript_path(project_path, session_id);
    let Ok(meta) = std::fs::metadata(&path) else {
        return Arc::new(Vec::new());
    };
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    if let Some(c) = cache().lock().unwrap().get(&path) {
        if c.len == len && c.mtime == mtime {
            return c.acts.clone();
        }
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let acts = Arc::new(derive(
        text.lines().map(str::to_owned),
        Some(&subagents_dir(project_path, session_id)),
    ));
    let mut c = cache().lock().unwrap();
    c.insert(
        path,
        Cached {
            len,
            mtime,
            acts: acts.clone(),
        },
    );
    if c.len() > 64 {
        let victim = c.keys().next().cloned();
        if let Some(k) = victim {
            c.remove(&k);
        }
    }
    acts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_lifecycle() {
        let lines = vec![
            r#"{"type":"assistant","timestamp":"t1","message":{"content":[{"type":"tool_use","id":"tu1","name":"Agent","input":{"description":"Summarize repo","subagent_type":"Explore","prompt":"read it"}}]}}"#,
            r#"{"type":"user","timestamp":"t2","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"Async agent launched successfully. agentId: abc123 (internal)"}]}}"#,
            r#"{"type":"assistant","timestamp":"t3","message":{"content":[{"type":"tool_use","id":"tu2","name":"Bash","input":{"command":"sleep 5","run_in_background":true,"description":"wait"}}]}}"#,
            r#"{"type":"user","timestamp":"t4","message":{"content":[{"type":"tool_result","tool_use_id":"tu2","content":"Command running in background with ID: bg9. Output is being written to: x"}]}}"#,
            r#"{"type":"user","timestamp":"t5","message":{"content":[{"type":"text","text":"<task-notification>\n<task-id>abc123</task-id>\n<status>completed</status>\n<summary>Agent \"Summarize repo\" finished</summary>\n</task-notification>"}]}}"#,
        ];
        let acts = derive(lines.into_iter().map(str::to_owned), None);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0].kind, "agent");
        assert_eq!(acts[0].id, "abc123");
        assert_eq!(acts[0].status, "completed");
        assert_eq!(acts[0].ended_at.as_deref(), Some("t5"));
        assert_eq!(acts[1].kind, "task");
        assert_eq!(acts[1].id, "bg9");
        assert_eq!(acts[1].status, "running");
        let c = counts(&acts);
        assert_eq!((c.running, c.tasks, c.agents), (1, 1, 0));
    }

    #[test]
    fn sync_agent_is_done_at_result() {
        let lines = vec![
            r#"{"type":"assistant","timestamp":"t1","message":{"content":[{"type":"tool_use","id":"tu1","name":"Agent","input":{"description":"quick"}}]}}"#,
            r#"{"type":"user","timestamp":"t2","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"here is the answer"}]}}"#,
        ];
        let acts = derive(lines.into_iter().map(str::to_owned), None);
        assert_eq!(acts[0].status, "done");
        assert_eq!(acts[0].detail["result"], "here is the answer");
    }
}

#[cfg(test)]
mod iso_tests {
    #[test]
    fn iso_epoch() {
        assert_eq!(super::parse_iso("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(
            super::parse_iso("2026-09-07T00:00:00.000Z"),
            Some(1788739200.0)
        );
    }
}
