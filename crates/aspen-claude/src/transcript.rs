//! Sessions on disk (reference §10): the runtime writes the same JSONL
//! transcripts headless as interactive, at
//! `~/.claude/projects/<slug>/<session-id>.jsonl`. The filesystem is the
//! source of truth for what sessions exist — never an app registry — and
//! rehydration parses the JSONL into displayable turns: full fidelity for
//! text, honest degradation for tool traffic.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Slug derivation (reference §10.1): the project path exactly as the CLI
/// saw its cwd, every non-alphanumeric character → `-`. (The >200-char
/// wyhash suffix and NFC edge cases are unimplemented; fall back to prefix
/// matching if they ever bite.)
pub fn project_slug(project_path: &Path) -> String {
    project_path
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn claude_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".claude")
}

pub fn transcript_path(project_path: &Path, session_id: &str) -> PathBuf {
    claude_home()
        .join("projects")
        .join(project_slug(project_path))
        .join(format!("{session_id}.jsonl"))
}

/// One session discovered on disk.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub title: Option<String>,
    pub entrypoint: Option<String>,
    pub modified_epoch: f64,
    pub user_messages: usize,
}

/// Enumerate a project's sessions from disk, newest first. A session with
/// zero real user messages is a warm spawn — callers may hide it.
pub fn enumerate_sessions(project_path: &Path) -> Result<Vec<SessionInfo>> {
    let dir = claude_home().join("projects").join(project_slug(project_path));
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(out), // no sessions yet
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if uuid::Uuid::parse_str(stem).is_err() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let (title, entrypoint, user_messages) = scan_transcript(&path);
        out.push(SessionInfo {
            session_id: stem.to_owned(),
            title,
            entrypoint,
            modified_epoch: modified,
            user_messages,
        });
    }
    out.sort_by(|a, b| b.modified_epoch.total_cmp(&a.modified_epoch));
    Ok(out)
}

/// Title chain (custom-title → ai-title → first real user message), the
/// entrypoint stamp, and a count of real user messages.
fn scan_transcript(path: &Path) -> (Option<String>, Option<String>, usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None, 0);
    };
    let mut custom = None;
    let mut ai = None;
    let mut first_user: Option<String> = None;
    let mut entrypoint = None;
    let mut user_count = 0usize;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => custom = str_field(&v, "customTitle").or(custom),
            Some("ai-title") => ai = str_field(&v, "aiTitle").or(ai),
            Some("user") => {
                if entrypoint.is_none() {
                    entrypoint = str_field(&v, "entrypoint");
                }
                if is_real_user_line(&v) {
                    user_count += 1;
                    if first_user.is_none() {
                        first_user = extract_text(&v).map(|t| {
                            let t: String = t.chars().take(80).collect();
                            t
                        });
                    }
                }
            }
            _ => {}
        }
    }
    (custom.or(ai).or(first_user), entrypoint, user_count)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

fn is_real_user_line(v: &Value) -> bool {
    if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true)
        || v.get("isCompactSummary").and_then(|b| b.as_bool()) == Some(true)
        || v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true)
    {
        return false;
    }
    match extract_text(v) {
        None => false,
        Some(t) => {
            let t = t.trim_start();
            // Harness-injected wrappers are not things the user typed.
            !(t.starts_with("<command-name>")
                || t.starts_with("<local-command")
                || t.starts_with("<system-reminder>")
                || t.starts_with("[Request interrupted"))
        }
    }
}

fn extract_text(v: &Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_owned());
    }
    let blocks = content.as_array()?;
    let mut out = String::new();
    for b in blocks {
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(t);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Rehydrate a transcript into displayable items, filtered to what the user
/// experienced (reference §10.2): skip meta/sidechain/compact lines and
/// harness wrappers; merge consecutive same-id assistant lines; text in
/// full, tool traffic as name-level cards.
pub fn rehydrate(project_path: &Path, session_id: &str) -> Result<Vec<Value>> {
    let path = transcript_path(project_path, session_id);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading transcript {}", path.display()))?;
    let mut items: Vec<Value> = Vec::new();
    let mut last_assistant_id: Option<String> = None;

    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true)
            || v.get("isMeta").and_then(|b| b.as_bool()) == Some(true)
            || v.get("isCompactSummary").and_then(|b| b.as_bool()) == Some(true)
        {
            continue;
        }
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = str_field(&v, "timestamp");
        let uuid = str_field(&v, "uuid");
        match ty {
            "user" => {
                if !is_real_user_line(&v) {
                    continue;
                }
                let Some(text) = extract_text(&v) else { continue };
                let is_bus = text.trim_start().starts_with("[aspen bus]");
                items.push(json!({
                    "role": "user", "bus": is_bus, "text": text,
                    "uuid": uuid, "timestamp": timestamp,
                }));
                last_assistant_id = None;
            }
            "assistant" => {
                let msg_id = v
                    .get("message")
                    .and_then(|m| m.get("id"))
                    .and_then(|i| i.as_str())
                    .map(str::to_owned);
                let text = extract_text(&v);
                let mut tools: Vec<Value> = Vec::new();
                if let Some(blocks) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for b in blocks {
                        if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            tools.push(json!({
                                "id": b.get("id"),
                                "name": b.get("name"),
                            }));
                        }
                    }
                }
                // Merge consecutive lines sharing message.id.
                let same = msg_id.is_some() && msg_id == last_assistant_id;
                if same {
                    if let Some(last) = items.last_mut() {
                        if let Some(t) = &text {
                            let prev = last["text"].as_str().unwrap_or("").to_owned();
                            last["text"] = json!(if prev.is_empty() {
                                t.clone()
                            } else {
                                format!("{prev}\n\n{t}")
                            });
                        }
                        if let Some(arr) = last["tools"].as_array_mut() {
                            arr.extend(tools);
                        }
                        continue;
                    }
                }
                last_assistant_id = msg_id.clone();
                items.push(json!({
                    "role": "assistant", "text": text.unwrap_or_default(),
                    "tools": tools, "uuid": uuid, "timestamp": timestamp,
                }));
            }
            _ => {}
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_reference_examples() {
        assert_eq!(
            project_slug(Path::new("/home/u/src/my proj")),
            "-home-u-src-my-proj"
        );
        assert_eq!(project_slug(Path::new("C:\\Data\\x")), "C--Data-x");
    }

    #[test]
    fn rehydrate_filters_and_merges() {
        let dir = std::env::temp_dir().join(format!("aspen-tr-{}", std::process::id()));
        let proj = Path::new("/tmp/fake-proj");
        let slug_dir = dir.join("projects").join(project_slug(proj));
        std::fs::create_dir_all(&slug_dir).unwrap();
        let sid = "11111111-1111-1111-1111-111111111111";
        let lines = [
            json!({"type":"user","uuid":"u1","message":{"role":"user","content":"hello"}}),
            json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}),
            json!({"type":"assistant","uuid":"a1","message":{"id":"m1","content":[{"type":"text","text":"part one"}]}}),
            json!({"type":"assistant","uuid":"a2","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Read"}]}}),
            json!({"type":"assistant","uuid":"a3","message":{"id":"m2","content":[{"type":"text","text":"part two"}]}}),
        ];
        let body: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        std::fs::write(slug_dir.join(format!("{sid}.jsonl")), body).unwrap();

        // Point claude_home at our fixture via env.
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        let items = rehydrate(proj, sid).unwrap();
        std::env::remove_var("CLAUDE_CONFIG_DIR");

        assert_eq!(items.len(), 3); // user + merged m1 + m2
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["text"], "part one");
        assert_eq!(items[1]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(items[2]["text"], "part two");
    }
}
