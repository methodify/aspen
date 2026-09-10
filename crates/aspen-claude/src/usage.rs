//! Usage (docs/USAGE.md, PROPOSALS-B §4): what a session consumed and
//! cost, from its transcript. Tokens are folded from every assistant
//! line's `message.usage` per `message.model`, over the main transcript
//! and each subagent file. Money is the harness's own figure: the last
//! `cost-state` line carries `totalCostUSD` and per-model `costUSD`, so
//! no price table is needed for a session the harness has priced.
//! Cached by the transcript's (size, mtime) like the activity ledger.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
    /// Assistant messages (API calls) on this model.
    pub calls: u64,
    /// The harness's priced figure for this model, when it wrote one.
    pub cost_usd: Option<f64>,
}

impl ModelUsage {
    fn add(&mut self, u: &Value) {
        let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        self.input += n("input_tokens");
        self.output += n("output_tokens");
        self.cache_read += n("cache_read_input_tokens");
        self.cache_create += n("cache_creation_input_tokens");
        self.calls += 1;
    }
    fn fold(&mut self, o: &ModelUsage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_create += o.cache_create;
        self.calls += o.calls;
        if let Some(c) = o.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionUsage {
    pub session_id: String,
    pub models: BTreeMap<String, ModelUsage>,
    pub total: ModelUsage,
    /// Operator (or bus) prompts in the main transcript.
    pub turns: u64,
    /// The harness's session total, from its last `cost-state` line.
    pub cost_usd: Option<f64>,
    /// How many subagent transcripts were folded in, and their tokens.
    pub subagents: u64,
    pub subagent_tokens: u64,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    /// Harness-reported lines added/removed, when it wrote them.
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
}

fn fold_file(path: &Path, into: &mut SessionUsage, main: bool) -> u64 {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let mut tokens_here = 0u64;
    let mut cost_state: Option<Value> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                let Some(msg) = v.get("message") else {
                    continue;
                };
                let model = msg
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                if let Some(u) = msg.get("usage") {
                    let mu = into.models.entry(model).or_default();
                    let before = mu.input + mu.output + mu.cache_read + mu.cache_create;
                    mu.add(u);
                    tokens_here +=
                        (mu.input + mu.output + mu.cache_read + mu.cache_create) - before;
                }
                if main {
                    if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                        if into.first_ts.is_none() {
                            into.first_ts = Some(ts.to_owned());
                        }
                        into.last_ts = Some(ts.to_owned());
                    }
                }
            }
            Some("user") if main => {
                if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true)
                    || v.get("isMeta").and_then(|b| b.as_bool()) == Some(true)
                {
                    continue;
                }
                // Count real prompts: string content or a text block, not
                // tool results.
                let real = match v.get("message").and_then(|m| m.get("content")) {
                    Some(Value::String(_)) => true,
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("text")),
                    _ => false,
                };
                if real {
                    into.turns += 1;
                }
            }
            Some("cost-state") if main => cost_state = Some(v),
            _ => {}
        }
    }
    if let Some(cs) = cost_state {
        into.cost_usd = cs.get("totalCostUSD").and_then(|c| c.as_f64());
        into.lines_added = cs.get("totalLinesAdded").and_then(|c| c.as_u64());
        into.lines_removed = cs.get("totalLinesRemoved").and_then(|c| c.as_u64());
        if let Some(mu) = cs.get("modelUsage").and_then(|m| m.as_object()) {
            for (model, u) in mu {
                let e = into.models.entry(model.clone()).or_default();
                e.cost_usd = u.get("costUSD").and_then(|c| c.as_f64());
            }
        }
    }
    tokens_here
}

/// Fold the main transcript and every subagent file of a session.
pub fn compute(project_path: &Path, session_id: &str) -> SessionUsage {
    compute_files(
        session_id,
        &crate::transcript::transcript_path(project_path, session_id),
        &crate::activity::subagents_dir(project_path, session_id),
    )
}

pub fn compute_files(session_id: &str, main: &Path, subagents: &Path) -> SessionUsage {
    let mut su = SessionUsage {
        session_id: session_id.to_owned(),
        ..Default::default()
    };
    fold_file(main, &mut su, true);
    if let Ok(rd) = std::fs::read_dir(subagents) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                su.subagents += 1;
                su.subagent_tokens += fold_file(&p, &mut su, false);
            }
        }
    }
    let mut total = ModelUsage::default();
    for m in su.models.values() {
        total.fold(m);
    }
    su.total = total;
    su
}

struct Cached {
    len: u64,
    mtime: f64,
    usage: Arc<SessionUsage>,
}

fn cache() -> &'static Mutex<std::collections::HashMap<PathBuf, Cached>> {
    static C: OnceLock<Mutex<std::collections::HashMap<PathBuf, Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Cached by the main transcript's (size, mtime); subagent files change
/// together with it in practice (a task notification lands in the main
/// file when a subagent finishes).
pub fn session_usage(project_path: &Path, session_id: &str) -> Arc<SessionUsage> {
    let path = crate::transcript::transcript_path(project_path, session_id);
    let Ok(meta) = std::fs::metadata(&path) else {
        return Arc::new(SessionUsage {
            session_id: session_id.to_owned(),
            ..Default::default()
        });
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
            return c.usage.clone();
        }
    }
    let usage = Arc::new(compute(project_path, session_id));
    let mut c = cache().lock().unwrap();
    c.insert(
        path,
        Cached {
            len,
            mtime,
            usage: usage.clone(),
        },
    );
    if c.len() > 128 {
        if let Some(k) = c.keys().next().cloned() {
            c.remove(&k);
        }
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_usage_and_cost_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc.jsonl");
        let subs = dir.path().join("abc").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(
            subs.join("agent-x.jsonl"),
            r#"{"type":"assistant","message":{"model":"m2","usage":{"input_tokens":5,"output_tokens":7},"content":[]}}"#,
        )
        .unwrap();
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"hi"},"timestamp":"2026-09-07T00:00:00Z"}"#,
            r#"{"type":"assistant","timestamp":"2026-09-07T00:00:01Z","message":{"model":"m1","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40},"content":[{"type":"text","text":"x"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-07T00:00:02Z","message":{"model":"m1","usage":{"input_tokens":1,"output_tokens":2},"content":[]}}"#,
            r#"{"type":"cost-state","totalCostUSD":1.5,"totalLinesAdded":3,"modelUsage":{"m1":{"costUSD":1.5}}}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let u = compute_files("abc", &path, &subs);
        assert_eq!(u.turns, 1);
        assert_eq!(u.subagents, 1);
        assert_eq!(u.subagent_tokens, 12);
        assert_eq!(u.total.input, 16);
        assert_eq!(u.total.output, 29);
        assert_eq!(u.total.cache_read, 30);
        assert_eq!(u.total.calls, 3);
        assert_eq!(u.cost_usd, Some(1.5));
        assert_eq!(u.models["m1"].cost_usd, Some(1.5));
        assert_eq!(u.lines_added, Some(3));
        assert_eq!(u.first_ts.as_deref(), Some("2026-09-07T00:00:01Z"));
    }
}
