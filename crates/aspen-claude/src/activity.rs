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

/// A JS/TS object key's value in a workflow script's `meta` (`name:
/// 'x'`, `description: "y"`): the quoted string after the key, or the bare
/// token when unquoted.
fn script_meta(script: &str, key: &str) -> Option<String> {
    let marker = format!("{key}:");
    let i = script.find(&marker)? + marker.len();
    let rest = script[i..].trim_start();
    let mut chars = rest.chars();
    let first = chars.next()?;
    if matches!(first, '\'' | '"' | '`') {
        let body: String = chars.take_while(|c| *c != first).collect();
        return (!body.is_empty()).then_some(body);
    }
    let tok: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!tok.is_empty()).then_some(tok)
}

/// The rest of the line after `marker` ("Summary: …").
fn line_after(text: &str, marker: &str) -> Option<String> {
    let i = text.find(marker)? + marker.len();
    let line = text[i..].lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

/// A workflow script's file name without the run suffix:
/// `pick-when-wave4-wf_1e0966ef-8da.js` → `pick-when-wave4`.
fn name_from_script_path(p: &str) -> Option<String> {
    let base = p.rsplit(['/', '\\']).next()?;
    let stem = base.strip_suffix(".js").unwrap_or(base);
    let stem = match stem.find("-wf_") {
        Some(i) => &stem[..i],
        None => stem,
    };
    (!stem.is_empty()).then(|| stem.to_owned())
}

/// A `<task-notification>` (any carrier line): end the activity it names.
fn apply_notification(acts: &mut [Activity], text: &str, ts: &Option<String>) {
    if !text.contains("<task-notification>") {
        return;
    }
    let Some(id) = tag(text, "task-id") else {
        return;
    };
    let status = tag(text, "status").unwrap_or_else(|| "completed".into());
    let summary = tag(text, "summary");
    // The first report ends it (the enqueue line, then the attachment,
    // then the remove line all carry the same notification).
    if let Some(a) = acts
        .iter_mut()
        .rev()
        .find(|a| a.id == id && a.status == "running")
    {
        a.status = status;
        a.ended_at = ts.clone();
        if let Some(s) = summary {
            a.detail["summary"] = Value::String(snippet(&s, 200));
            a.detail["output"] = Value::String(snippet(&s, 4000));
        }
        if let Some(f) = tag(text, "output-file") {
            a.detail["output_file"] = Value::String(f);
        }
    }
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
        // A task's end reaches the transcript in more than one shape (all
        // seen in real sessions, CLI 2.1.25x–2.1.27x): a `user` line whose
        // content is the notification as a plain string; a `user` line
        // with it as a text block; and — when the harness absorbed it
        // mid-turn — only an `attachment` line (`commandMode:
        // task-notification`) and the `queue-operation` pair, never a user
        // line at all. Missing the last two left tasks "running" forever.
        match ty {
            "attachment" => {
                if let Some(p) = v.pointer("/attachment/prompt").and_then(|p| p.as_str()) {
                    apply_notification(&mut acts, p, &ts);
                }
                continue;
            }
            "queue-operation" => {
                if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                    apply_notification(&mut acts, c, &ts);
                }
                continue;
            }
            "user"
                if v.pointer("/message/content")
                    .and_then(|c| c.as_str())
                    .is_some() =>
            {
                let text = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                apply_notification(&mut acts, text, &ts);
                continue;
            }
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
                            // A workflow's name is in its script's `meta`,
                            // or in the saved script's file name on a
                            // resume; the launch result's Summary line
                            // fills in what neither says.
                            let script = input.get("script").and_then(|s| s.as_str());
                            let nm = input
                                .get("name")
                                .and_then(|n| n.as_str())
                                .map(str::to_owned)
                                .or_else(|| script.and_then(|s| script_meta(s, "name")))
                                .or_else(|| {
                                    input
                                        .get("scriptPath")
                                        .and_then(|p| p.as_str())
                                        .and_then(name_from_script_path)
                                })
                                .unwrap_or_else(|| "workflow".into());
                            (
                                "workflow",
                                nm,
                                serde_json::json!({
                                    "description": script.and_then(|s| script_meta(s, "description")),
                                    "run_id": input.get("resumeFromRunId"),
                                    "script_path": input.get("scriptPath"),
                                }),
                            )
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
                                    // "Workflow launched in background. Task
                                    // ID: w…\nSummary: …\nTranscript dir:
                                    // …/wf_<run>". The notification that ends
                                    // it names the Task ID, so that is the id;
                                    // the run id keys the files on disk.
                                    if let Some(id) = find_after(&text, "Task ID:") {
                                        a.id = id;
                                    }
                                    if let Some(run) =
                                        find_after(&text, "wf_").map(|s| format!("wf_{s}"))
                                    {
                                        a.detail["run_id"] = Value::String(run);
                                    }
                                    if let Some(sum) = line_after(&text, "Summary:") {
                                        if a.detail.get("description").is_none_or(|d| d.is_null()) {
                                            a.detail["description"] = Value::String(sum.clone());
                                        }
                                        if a.label == "workflow" {
                                            a.label = snippet(&sum, 80);
                                        }
                                    }
                                    if is_err {
                                        a.status = "failed".into();
                                        a.ended_at = ts.clone();
                                        a.detail["output"] = Value::String(snippet(&text, 2000));
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
                        apply_notification(&mut acts, &text, &ts);
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
    fold_workflow_resumes(&mut acts);
    acts
}

/// A resumed workflow (`resumeFromRunId`) is the same run under a new task
/// id: one row per run, the newest launch, with how many attempts it took.
fn fold_workflow_resumes(acts: &mut Vec<Activity>) {
    let run_of = |a: &Activity| {
        a.detail
            .get("run_id")
            .and_then(|r| r.as_str())
            .map(str::to_owned)
    };
    let mut attempts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for a in acts.iter().filter(|a| a.kind == "workflow") {
        if let Some(r) = run_of(a) {
            *attempts.entry(r).or_default() += 1;
        }
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Keep the LAST launch of each run: walk backwards, drop earlier ones.
    let mut keep = vec![true; acts.len()];
    for (i, a) in acts.iter().enumerate().rev() {
        if a.kind != "workflow" {
            continue;
        }
        if let Some(r) = run_of(a) {
            if !seen.insert(r) {
                keep[i] = false;
            }
        }
    }
    let mut i = 0;
    acts.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
    for a in acts.iter_mut().filter(|a| a.kind == "workflow") {
        if let Some(n) = run_of(a).and_then(|r| attempts.get(&r).copied()) {
            if n > 1 {
                a.detail["attempts"] = serde_json::json!(n);
            }
        }
    }
}

/// `<project>/<session>/workflows` — the harness's per-run state files
/// (`wf_<run>.json`, written when a run ends) and saved scripts.
pub fn workflows_dir(project_path: &Path, session_id: &str) -> PathBuf {
    crate::transcript::transcript_path(project_path, session_id)
        .with_extension("")
        .join("workflows")
}

/// A run's state file, if the harness wrote one.
pub fn workflow_state(project_path: &Path, session_id: &str, run_id: &str) -> Option<Value> {
    if run_id.contains(['/', '\\', '.']) {
        return None;
    }
    let p = workflows_dir(project_path, session_id).join(format!("{run_id}.json"));
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

/// The second source for a workflow's end (PROPOSALS-2026-09-O §2.3): the
/// run's state file, when it was written after this launch started, says
/// how it ended — so a run that finished across a restart reads
/// "completed", not "unknown" — and carries its totals.
fn apply_workflow_states(acts: &mut [Activity], project_path: &Path, session_id: &str) {
    for a in acts.iter_mut().filter(|a| a.kind == "workflow") {
        let Some(run) = a
            .detail
            .get("run_id")
            .and_then(|r| r.as_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(st) = workflow_state(project_path, session_id, &run) else {
            continue;
        };
        let written = st
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_iso);
        let started = a.started_at.as_deref().and_then(parse_iso);
        if let (Some(w), Some(s)) = (written, started) {
            if w + 1.0 < s {
                continue; // an earlier attempt's file; this launch is newer
            }
        }
        if let Some(n) = st.get("workflowName").and_then(|n| n.as_str()) {
            if a.label == "workflow" || a.label.is_empty() {
                a.label = n.to_owned();
            }
        }
        for (k, from) in [
            ("agent_count", "agentCount"),
            ("total_tokens", "totalTokens"),
            ("total_tool_calls", "totalToolCalls"),
            ("duration_ms", "durationMs"),
        ] {
            if let Some(v) = st.get(from) {
                a.detail[k] = v.clone();
            }
        }
        let status = st.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if a.status == "running" || a.status == "unknown" {
            match status {
                "completed" | "failed" | "killed" | "stopped" => {
                    a.status = status.to_owned();
                    if a.ended_at.is_none() {
                        a.ended_at = st
                            .get("timestamp")
                            .and_then(|t| t.as_str())
                            .map(str::to_owned);
                    }
                }
                _ => {}
            }
        }
    }
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
    let dir = subagents_dir(project_path, session_id);
    let direct = dir.join(format!("agent-{agent_id}.jsonl"));
    if direct.is_file() {
        return direct;
    }
    // A workflow's agents live under `subagents/workflows/wf_<run>/`.
    if let Ok(rd) = std::fs::read_dir(dir.join("workflows")) {
        for e in rd.flatten() {
            let p = e.path().join(format!("agent-{agent_id}.jsonl"));
            if p.is_file() {
                return p;
            }
        }
    }
    direct
}

/// What an agent's transcript says about it, for a workflow run read from
/// disk: when it started, its model, tool calls, the last tool and what it
/// was doing, and its context size (the harness's "tokens" figure is the
/// agent's final context: input + cache of the last turn). Cached by the
/// file's size and mtime — a live run is re-polled every few seconds.
fn agent_file_summary(path: &Path) -> Option<Value> {
    type Entry = (u64, f64, Value);
    static C: OnceLock<Mutex<std::collections::HashMap<PathBuf, Entry>>> = OnceLock::new();
    let meta = std::fs::metadata(path).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let cache = C.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some((l, m, v)) = cache.lock().unwrap().get(path) {
        if *l == len && *m == mtime {
            return Some(v.clone());
        }
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut first: Option<String> = None;
    let mut model: Option<String> = None;
    let mut tools = 0u64;
    let mut last_tool: Option<String> = None;
    let mut last_summary: Option<String> = None;
    let mut ctx: Option<u64> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if first.is_none() {
            first = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(str::to_owned);
        }
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let m = v.get("message").cloned().unwrap_or(Value::Null);
        if let Some(md) = m.get("model").and_then(|x| x.as_str()) {
            model = Some(md.to_owned());
        }
        if let Some(u) = m.get("usage") {
            let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            ctx = Some(
                g("input_tokens") + g("cache_read_input_tokens") + g("cache_creation_input_tokens"),
            );
        }
        for b in m
            .get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                tools += 1;
                last_tool = b.get("name").and_then(|n| n.as_str()).map(str::to_owned);
                let inp = b.get("input").cloned().unwrap_or(Value::Null);
                last_summary = [
                    "command",
                    "description",
                    "file_path",
                    "pattern",
                    "url",
                    "prompt",
                ]
                .iter()
                .find_map(|k| inp.get(*k).and_then(|x| x.as_str()))
                .map(|x| snippet(x, 60));
            }
        }
    }
    let v = serde_json::json!({
        "started_at": first.as_deref().and_then(parse_iso).map(|t| (t * 1000.0) as u64),
        "model": model,
        "tool_calls": tools,
        "last_tool": last_tool,
        "last_tool_summary": last_summary,
        "tokens": ctx,
        "last_progress_at": (mtime * 1000.0) as u64,
    });
    let mut c = cache.lock().unwrap();
    if c.len() > 512 {
        c.clear();
    }
    c.insert(path.to_owned(), (len, mtime, v.clone()));
    Some(v)
}

/// One workflow run, read from disk (PROPOSALS-2026-09-O §2.3): the state
/// file when the run has ended, else the live journal plus each agent's
/// meta and transcript. `progress` is the latest `workflow_progress` array
/// the harness streamed, when the caller has one (a live run).
pub fn workflow_detail(
    project_path: &Path,
    session_id: &str,
    run_id: &str,
    progress: Option<&Value>,
) -> Option<Value> {
    if run_id.contains(['/', '\\', '.']) {
        return None;
    }
    let state = workflow_state(project_path, session_id, run_id);
    let agents_dir = subagents_dir(project_path, session_id)
        .join("workflows")
        .join(run_id);
    if state.is_none() && progress.is_none() && !agents_dir.is_dir() {
        return None;
    }
    let st = state.clone().unwrap_or(Value::Null);
    let prog: Vec<Value> = progress
        .and_then(|p| p.as_array().cloned())
        .or_else(|| {
            st.get("workflowProgress")
                .and_then(|p| p.as_array().cloned())
        })
        .unwrap_or_default();
    // Phases: from progress entries, else the state's `phases`.
    let mut phases: Vec<Value> = Vec::new();
    for p in prog
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("workflow_phase"))
    {
        phases.push(serde_json::json!({
            "index": p.get("index"), "title": p.get("title"), "detail": null, "agents": [],
        }));
    }
    if phases.is_empty() {
        if let Some(ps) = st.get("phases").and_then(|p| p.as_array()) {
            for (i, p) in ps.iter().enumerate() {
                phases.push(serde_json::json!({
                    "index": i + 1, "title": p.get("title"), "detail": p.get("detail"), "agents": [],
                }));
            }
        }
    }
    if let Some(ps) = st.get("phases").and_then(|p| p.as_array()) {
        for ph in phases.iter_mut() {
            let t = ph.get("title").cloned();
            if let Some(src) = ps.iter().find(|p| p.get("title") == t.as_ref()) {
                ph["detail"] = src.get("detail").cloned().unwrap_or(Value::Null);
            }
        }
    }
    let has_file = |id: &str| agents_dir.join(format!("agent-{id}.jsonl")).is_file();
    let mut agents: Vec<Value> = Vec::new();
    for p in prog
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("workflow_agent"))
    {
        let id = p.get("agentId").and_then(|a| a.as_str()).unwrap_or("");
        agents.push(serde_json::json!({
            "agent_id": id,
            "label": p.get("label"),
            "phase": p.get("phaseTitle"),
            "phase_index": p.get("phaseIndex"),
            "model": p.get("model"),
            "state": p.get("state"),
            "attempt": p.get("attempt"),
            "retry_reason": p.get("lastAttemptReason"),
            "queued_at": p.get("queuedAt"),
            "started_at": p.get("startedAt"),
            "last_progress_at": p.get("lastProgressAt"),
            "duration_ms": p.get("durationMs"),
            "tokens": p.get("tokens"),
            "tool_calls": p.get("toolCalls"),
            "last_tool": p.get("lastToolName"),
            "last_tool_summary": p.get("lastToolSummary"),
            "prompt_preview": p.get("promptPreview"),
            "result_preview": p.get("resultPreview"),
            "has_transcript": !id.is_empty() && has_file(id),
        }));
    }
    // No progress yet (a live run the node has no frame for): the journal
    // and the agents' meta files say who started, who finished.
    let mut journal_results: Vec<Value> = Vec::new();
    if agents.is_empty() {
        let mut order: Vec<String> = Vec::new();
        let mut rows: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        if let Ok(text) = std::fs::read_to_string(agents_dir.join("journal.jsonl")) {
            for line in text.lines() {
                let Ok(j) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let id = j
                    .get("agentId")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_owned();
                match j.get("type").and_then(|t| t.as_str()) {
                    Some("started") if !id.is_empty() => {
                        if !rows.contains_key(&id) {
                            order.push(id.clone());
                        }
                        let mut r = serde_json::json!({
                            "agent_id": id, "label": j.get("label"), "phase": j.get("phase"),
                            "state": "running", "has_transcript": has_file(&id),
                        });
                        if let Some(sum) =
                            agent_file_summary(&agents_dir.join(format!("agent-{id}.jsonl")))
                        {
                            if let (Some(o), Some(m)) = (r.as_object_mut(), sum.as_object()) {
                                for (k, v) in m {
                                    o.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        rows.insert(id, r);
                    }
                    Some("result") if !id.is_empty() => {
                        let res = j.get("result").and_then(|r| r.as_str()).unwrap_or("");
                        if let Some(r) = rows.get_mut(&id) {
                            r["state"] = Value::String("done".into());
                            r["result_preview"] = Value::String(snippet(res, 400));
                            if let (Some(a), Some(b)) = (
                                r.get("started_at").and_then(|x| x.as_u64()),
                                r.get("last_progress_at").and_then(|x| x.as_u64()),
                            ) {
                                r["duration_ms"] = serde_json::json!(b.saturating_sub(a));
                            }
                        }
                        journal_results.push(serde_json::json!({ "key": id, "report": res }));
                    }
                    _ => {}
                }
            }
        }
        for id in order {
            if let Some(r) = rows.remove(&id) {
                agents.push(r);
            }
        }
        if phases.is_empty() {
            let mut titles: Vec<String> = Vec::new();
            for a in &agents {
                if let Some(t) = a.get("phase").and_then(|p| p.as_str()) {
                    if !titles.iter().any(|x| x == t) {
                        titles.push(t.to_owned());
                    }
                }
            }
            for (i, t) in titles.iter().enumerate() {
                phases.push(
                    serde_json::json!({ "index": i + 1, "title": t, "detail": null, "agents": [] }),
                );
            }
        }
    }
    // Agents into their phases (by index, else by title; else the first).
    for a in agents {
        let idx = a.get("phase_index").and_then(|i| i.as_u64());
        let title = a.get("phase").and_then(|p| p.as_str()).map(str::to_owned);
        let slot = phases
            .iter_mut()
            .position(|ph| {
                idx.is_some_and(|i| ph.get("index").and_then(|x| x.as_u64()) == Some(i))
                    || title
                        .as_deref()
                        .is_some_and(|t| ph.get("title").and_then(|x| x.as_str()) == Some(t))
            })
            .unwrap_or(0);
        if phases.is_empty() {
            phases.push(
                serde_json::json!({ "index": 1, "title": title, "detail": null, "agents": [] }),
            );
        }
        if let Some(arr) = phases[slot]
            .get_mut("agents")
            .and_then(|x| x.as_array_mut())
        {
            arr.push(a);
        }
    }
    let script_path = st.get("scriptPath").cloned().or_else(|| {
        std::fs::read_dir(workflows_dir(project_path, session_id).join("scripts"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(run_id))
            })
            .map(|p| Value::String(p.to_string_lossy().into_owned()))
    });
    let results = st
        .get("result")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or(journal_results);
    Some(serde_json::json!({
        "run_id": run_id,
        "task_id": st.get("taskId"),
        "name": st.get("workflowName"),
        "description": st.get("summary"),
        "status": st.get("status"),
        "ended_at": st.get("timestamp"),
        "started_at": st.get("startTime"),
        "duration_ms": st.get("durationMs"),
        "total_tokens": st.get("totalTokens"),
        "total_tool_calls": st.get("totalToolCalls"),
        "agent_count": st.get("agentCount"),
        "default_model": st.get("defaultModel"),
        "logs": st.get("logs").cloned().unwrap_or_else(|| serde_json::json!([])),
        "phases": phases,
        "results": results,
        "script_path": script_path,
        "source": if progress.is_some() { "live" } else if state.is_some() { "state" } else { "journal" },
    }))
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
    let mut derived = derive(
        text.lines().map(str::to_owned),
        Some(&subagents_dir(project_path, session_id)),
    );
    apply_workflow_states(&mut derived, project_path, session_id);
    let acts = Arc::new(derived);
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
    fn notification_absorbed_mid_turn_ends_the_task() {
        // The harness took the notification mid-turn: only an attachment
        // line (and the queue-operation pair) record it — no user line.
        let lines = vec![
            r#"{"type":"assistant","timestamp":"t1","message":{"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"sleep 5","run_in_background":true,"description":"wait"}}]}}"#,
            r#"{"type":"user","timestamp":"t2","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"Command running in background with ID: bg9. Output is being written to: x"}]}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"t3","content":"<task-notification>\n<task-id>bg9</task-id>\n<status>completed</status>\n<summary>Background command \"wait\" completed (exit code 0)</summary>\n</task-notification>"}"#,
            r#"{"type":"attachment","timestamp":"t3","attachment":{"type":"queued_command","commandMode":"task-notification","prompt":"<task-notification>\n<task-id>bg9</task-id>\n<status>completed</status>\n<summary>Background command \"wait\" completed (exit code 0)</summary>\n</task-notification>"}}"#,
            r#"{"type":"queue-operation","operation":"remove","timestamp":"t4","content":"<task-notification>\n<task-id>bg9</task-id>\n<status>completed</status>\n<summary>Background command \"wait\" completed (exit code 0)</summary>\n</task-notification>","reason":"absorbed_mid_turn"}"#,
        ];
        let acts = derive(lines.into_iter().map(str::to_owned), None);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].status, "completed");
        assert_eq!(acts[0].ended_at.as_deref(), Some("t3"));
    }

    #[test]
    fn notification_as_string_content_ends_the_task() {
        let lines = vec![
            r#"{"type":"assistant","timestamp":"t1","message":{"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"sleep 5","run_in_background":true}}]}}"#,
            r#"{"type":"user","timestamp":"t2","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"Command running in background with ID: bg9. Output is being written to: x"}]}}"#,
            r#"{"type":"user","timestamp":"t3","message":{"role":"user","content":"<task-notification>\n<task-id>bg9</task-id>\n<status>failed</status>\n<summary>exit 1</summary>\n</task-notification>"}}"#,
        ];
        let acts = derive(lines.into_iter().map(str::to_owned), None);
        assert_eq!(acts[0].status, "failed");
        assert_eq!(acts[0].ended_at.as_deref(), Some("t3"));
    }

    #[test]
    fn workflow_ends_by_task_id_named_from_meta() {
        // The launch result names the Task ID and the run; the notification
        // names the Task ID. The name is quoted in the script's meta.
        let lines = vec![
            r#"{"type":"assistant","timestamp":"2026-09-28T10:00:00Z","message":{"content":[{"type":"tool_use","id":"tu1","name":"Workflow","input":{"script":"export const meta = {\n  name: 'wave-4',\n  description: 'nine builders',\n}"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-09-28T10:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"Workflow launched in background. Task ID: wt1\nSummary: Wave 4 builders\nTranscript dir: /p/s/subagents/workflows/wf_1e0966ef-8da\nScript: x"}]}}"#,
            r#"{"type":"user","timestamp":"2026-09-28T11:00:00Z","message":{"role":"user","content":"<task-notification>\n<task-id>wt1</task-id>\n<status>completed</status>\n<summary>Workflow wave-4 completed</summary>\n</task-notification>"}}"#,
        ];
        let acts = derive(lines.into_iter().map(str::to_owned), None);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].id, "wt1");
        assert_eq!(acts[0].label, "wave-4");
        assert_eq!(acts[0].detail["run_id"], "wf_1e0966ef-8da");
        assert_eq!(acts[0].detail["description"], "nine builders");
        assert_eq!(acts[0].status, "completed");
    }

    #[test]
    fn workflow_resumes_fold_into_one_row() {
        let launch = |tu: &str, task: &str, ts: &str, input: &str| {
            vec![
                format!(
                    r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{tu}","name":"Workflow","input":{input}}}]}}}}"#
                ),
                format!(
                    r#"{{"type":"user","timestamp":"{ts}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{tu}","content":"Workflow launched in background. Task ID: {task}\nSummary: Wave 3\nTranscript dir: /p/s/subagents/workflows/wf_63b0b35f-1dd"}}]}}}}"#
                ),
            ]
        };
        let mut lines = launch(
            "tu1",
            "wa",
            "t1",
            r#"{"script":"meta = { name: 'wave-3' }"}"#,
        );
        lines.extend(launch("tu2", "wb", "t2", r#"{"scriptPath":"/p/s/workflows/scripts/wave-3-wf_63b0b35f-1dd.js","resumeFromRunId":"wf_63b0b35f-1dd"}"#));
        let acts = derive(lines.into_iter(), None);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].id, "wb");
        assert_eq!(acts[0].label, "wave-3");
        assert_eq!(acts[0].detail["attempts"], 2);
    }

    #[test]
    fn script_meta_reads_quoted_and_bare() {
        assert_eq!(
            script_meta("meta = { name: 'a-b' }", "name").as_deref(),
            Some("a-b")
        );
        assert_eq!(
            script_meta("{ name: \"x y\" }", "name").as_deref(),
            Some("x y")
        );
        assert_eq!(
            script_meta("{ name: plain }", "name").as_deref(),
            Some("plain")
        );
        assert_eq!(
            name_from_script_path("/a/b/wave-4-wf_1e0966ef-8da.js").as_deref(),
            Some("wave-4")
        );
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
