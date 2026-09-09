//! Text search over the sessions this node holds (PROPOSALS-2026-09-C.md
//! §2): every session of every registered repo, both harnesses, plus the
//! replicas held here. The main file is read once and pre-filtered by a
//! case-insensitive substring; only files that match are rehydrated, and
//! the hits are the items whose text carries the query. Ordinary text
//! search — no index, no ranking beyond recency — which is what "where did
//! we decide X" needs.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::node::NodeInner;

/// Hits per session, so one long session cannot crowd out the rest.
const PER_SESSION: usize = 5;
/// Files bigger than this are skipped: a runaway transcript should not
/// hold the search for everyone else.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct SearchResult {
    /// One row per session with hits: `{session_id, repo, repo_handle,
    /// agent, harness, title, modified, replica, home, hits: [...]}`.
    pub sessions: Vec<Value>,
    /// Session files looked at.
    pub scanned: usize,
}

pub fn search_local(inner: &Arc<NodeInner>, needle: &str, limit: usize, repo_filter: Option<&str>) -> SearchResult {
    let needle_lc = needle.to_lowercase();
    if needle_lc.trim().is_empty() {
        return SearchResult::default();
    }
    let me = inner.mesh().map(|m| m.identity.node.clone()).unwrap_or_default();
    let agents = inner.store.agents().unwrap_or_default();
    let repos = inner.store.repos().unwrap_or_default();
    let mut out = SearchResult::default();
    // Newest sessions first across every repo, so the cap keeps recency.
    let mut candidates: Vec<(std::path::PathBuf, String, aspen_core::SessionInfo)> = Vec::new();
    for r in &repos {
        if let Some(f) = repo_filter {
            if r.handle != f && r.path.to_string_lossy() != f {
                continue;
            }
        }
        for si in crate::node::enumerate_all(inner, &r.path) {
            candidates.push((r.path.clone(), r.handle.clone(), si));
        }
    }
    candidates.sort_by(|a, b| b.2.modified_epoch.total_cmp(&a.2.modified_epoch));
    for (repo, handle, si) in candidates {
        if out.sessions.len() >= limit {
            break;
        }
        let st = inner.store_for(si.harness);
        let main = st.main_path(&repo, &si.session_id);
        out.scanned += 1;
        if !file_mentions(&main, &needle_lc) {
            continue;
        }
        let items = st.rehydrate(&repo, &si.session_id).unwrap_or_default();
        let hits = hits_in(&items, &needle_lc);
        if hits.is_empty() {
            continue;
        }
        let agent = agents
            .iter()
            .find(|a| a.session_id.as_deref() == Some(si.session_id.as_str()) && a.moved_to.is_none())
            .map(|a| a.name.clone());
        let title = agents
            .iter()
            .find(|a| a.session_id.as_deref() == Some(si.session_id.as_str()))
            .and_then(|a| a.title.clone())
            .or_else(|| si.title.clone());
        out.sessions.push(json!({
            "session_id": si.session_id,
            "repo": repo.to_string_lossy(),
            "repo_handle": handle,
            "agent": agent,
            "harness": si.harness,
            "title": title,
            "modified": si.modified_epoch,
            "replica": false,
            "home": me,
            "hits": hits,
        }));
    }
    if repo_filter.is_none() {
        for r in crate::replicate::list(inner) {
            if out.sessions.len() >= limit {
                break;
            }
            let Some(main) = r.main.as_deref() else { continue };
            out.scanned += 1;
            if !file_mentions(main, &needle_lc) {
                continue;
            }
            let st = inner.store_for(r.harness);
            let items = st.rehydrate_file(main).unwrap_or_default();
            let hits = hits_in(&items, &needle_lc);
            if hits.is_empty() {
                continue;
            }
            out.sessions.push(json!({
                "session_id": r.session_id,
                "repo": r.repo,
                "repo_handle": Path::new(&r.repo).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| r.repo.clone()),
                "agent": r.agent,
                "harness": r.harness,
                "title": r.title,
                "modified": r.updated_at,
                "replica": true,
                "home": r.node,
                "hits": hits,
            }));
        }
    }
    out
}

/// The cheap gate: does the file contain the needle at all, case-folded?
fn file_mentions(path: &Path, needle_lc: &str) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if meta.len() > MAX_FILE_BYTES {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else { return false };
    // JSON escapes newlines and quotes, so a needle with either would miss
    // here; the item pass below sees the real text. Gate on the first
    // "word" then, which is never escaped.
    let probe = needle_lc.split_whitespace().next().unwrap_or(needle_lc);
    let text = String::from_utf8_lossy(&bytes).to_lowercase();
    text.contains(probe)
}

/// The items carrying the needle, newest first, capped. Text is the user
/// or assistant text plus a tool chip's visible fields (name, summary,
/// path, command).
fn hits_in(items: &[Value], needle_lc: &str) -> Vec<Value> {
    let mut hits = Vec::new();
    for it in items.iter().rev() {
        let mut fields: Vec<String> = Vec::new();
        if let Some(t) = it.get("text").and_then(|t| t.as_str()) {
            fields.push(t.to_owned());
        }
        if let Some(tools) = it.get("tools").and_then(|t| t.as_array()) {
            for t in tools {
                let parts: Vec<&str> = ["name", "summary", "path", "command"]
                    .iter()
                    .filter_map(|k| t.get(k).and_then(|v| v.as_str()))
                    .collect();
                if !parts.is_empty() {
                    fields.push(parts.join(" "));
                }
            }
        }
        let Some((field, pos)) = fields.iter().find_map(|f| f.to_lowercase().find(needle_lc).map(|p| (f, p))) else {
            continue;
        };
        hits.push(json!({
            "uuid": it.get("uuid"),
            "role": it.get("role"),
            "timestamp": it.get("timestamp"),
            "snippet": snippet_around(field, pos, needle_lc.len()),
        }));
        if hits.len() >= PER_SESSION {
            break;
        }
    }
    hits.reverse();
    hits
}

/// About 90 chars either side of the match, on char boundaries, with the
/// match's own offset so the console can highlight it.
fn snippet_around(text: &str, byte_pos: usize, needle_len: usize) -> Value {
    // `find` on the lowercased copy can drift from the original for
    // multi-byte case folds; clamp to boundaries rather than trust it.
    let mut start = byte_pos.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + needle_len).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let before: String = text[..start].chars().rev().take(90).collect::<Vec<_>>().into_iter().rev().collect();
    let after: String = text[end..].chars().take(90).collect();
    let cut_before = before.chars().count() < text[..start].chars().count();
    let cut_after = after.chars().count() < text[end..].chars().count();
    let matched = &text[start..end];
    json!({
        "before": format!("{}{}", if cut_before { "…" } else { "" }, before.replace('\n', " ")),
        "match": matched,
        "after": format!("{}{}", after.replace('\n', " "), if cut_after { "…" } else { "" }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_in_text_and_tools() {
        let items = vec![
            json!({ "role": "user", "text": "let's decide on Postgres", "uuid": "a" }),
            json!({ "role": "assistant", "text": "ok", "tools": [{ "name": "Edit", "path": "db/postgres.rs" }], "uuid": "b" }),
            json!({ "role": "assistant", "text": "unrelated", "uuid": "c" }),
        ];
        let hits = hits_in(&items, "postgres");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["uuid"], "a");
        assert_eq!(hits[0]["snippet"]["match"], "Postgres");
        assert_eq!(hits[1]["uuid"], "b");
    }

    #[test]
    fn snippet_is_bounded() {
        let long = format!("{}needle{}", "x".repeat(200), "y".repeat(200));
        let s = snippet_around(&long, 200, 6);
        assert!(s["before"].as_str().unwrap().starts_with('…'));
        assert!(s["after"].as_str().unwrap().ends_with('…'));
        assert_eq!(s["match"], "needle");
    }
}
