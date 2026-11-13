//! A project as its members see it (PROPOSALS-2026-11-A §A-4, §5): which
//! projects an agent is on, the team (role, node, state), the team thread
//! and the decisions. Shared by the agent tools and the operator's API.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::node::{NodeInner, TurnState};
use crate::store::Board;

pub fn status(b: &Board) -> &str {
    b.status
        .as_ref()
        .and_then(|s| s.as_str())
        .unwrap_or("active")
}

pub fn charter(b: &Board) -> Option<&str> {
    b.charter
        .as_ref()
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
}

/// The projects an agent is on (archived ones are no one's).
pub fn of_agent(inner: &Arc<NodeInner>, me: &str) -> Vec<Board> {
    inner
        .store
        .boards(false)
        .unwrap_or_default()
        .into_iter()
        .filter(|b| {
            crate::topology::board_panes(inner, b)
                .iter()
                .any(|p| p == me)
        })
        .collect()
}

/// One of the agent's projects, by id or name (any case); none needed
/// when it is on exactly one.
pub fn pick(inner: &Arc<NodeInner>, me: &str, which: Option<&str>) -> Result<Board, String> {
    let mine = of_agent(inner, me);
    let which = which.map(str::trim).filter(|w| !w.is_empty());
    match which {
        None => match mine.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("you are on no project".into()),
            many => Err(format!(
                "you are on several projects; say which: {}",
                many.iter()
                    .map(|b| format!("\u{201c}{}\u{201d}", b.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
        Some(w) => {
            let wl = w.to_lowercase();
            mine.iter()
                .find(|b| b.id == w)
                .or_else(|| mine.iter().find(|b| b.name.to_lowercase() == wl))
                .cloned()
                .ok_or_else(|| {
                    format!("you are not on a project called {w:?}; project_list lists yours")
                })
        }
    }
}

/// The first line of a charter: a member's role.
pub fn role_line(charter: &str) -> String {
    let line = charter
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches('#')
        .trim();
    line.chars().take(140).collect()
}

/// The team: `{agent, role, node, state, benched}` per member.
pub fn team(inner: &Arc<NodeInner>, b: &Board) -> Vec<Value> {
    let benched = crate::topology::bench_agents(b);
    let local = inner.store.agents().unwrap_or_default();
    let me_node = inner
        .mesh()
        .map(|m| m.identity.node.clone())
        .unwrap_or_else(|| "this node".into());
    let remote = inner
        .mesh()
        .map(|m| m.remote_snapshot())
        .unwrap_or_default();
    crate::topology::board_panes(inner, b)
        .into_iter()
        .map(|m| {
            let is_benched = benched.iter().any(|x| {
                x == &m || x.strip_suffix(&format!("@{me_node}")) == Some(m.as_str())
            });
            if let Some(a) = local.iter().find(|a| a.name == m) {
                let state = match inner.live(&a.name) {
                    None => "stopped",
                    Some(s) => match s.turn_state() {
                        TurnState::Busy => "running",
                        TurnState::Idle => "idle",
                    },
                };
                let role = a
                    .charter
                    .as_deref()
                    .map(role_line)
                    .filter(|r| !r.is_empty())
                    .or_else(|| a.title.clone());
                return json!({ "agent": m, "role": role, "node": me_node, "state": state, "benched": is_benched });
            }
            let (node, key) = match m.rfind('@') {
                Some(i) if m[..i].contains('@') => (m[i + 1..].to_owned(), m[..i].to_owned()),
                _ => (String::new(), m.clone()),
            };
            let ra = remote
                .iter()
                .find(|(n, _)| *n == node)
                .and_then(|(_, v)| v.iter().find(|a| a.name == key));
            let state = match ra {
                None => "unknown",
                Some(a) if !a.live => "stopped",
                Some(a) if a.turn_state.as_deref() == Some("busy") => "running",
                Some(_) => "idle",
            };
            json!({
                "agent": m,
                "role": ra.and_then(|a| a.role.clone().or_else(|| a.title.clone())),
                "node": node,
                "state": state,
                "benched": is_benched,
            })
        })
        .collect()
}

/// Pinned decisions, oldest first.
pub fn decisions(b: &Board) -> Vec<Value> {
    b.decisions
        .as_ref()
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .filter(|d| d.get("pinned").and_then(|p| p.as_bool()).unwrap_or(true))
        .cloned()
        .collect()
}

/// Add a decision (pinned) to the project, as `by`.
pub fn add_decision(inner: &Arc<NodeInner>, b: &Board, by: &str, text: &str) -> Result<(), String> {
    let at = inner.store.hlc_now();
    let d = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "at": at,
        "by": by,
        "text": text,
        "pinned": true,
    });
    crate::boards::patch_and_notify(inner, &json!({ "id": b.id, "decisions": [d] }))
        .map_err(|e| e.to_string())?;
    crate::federation::broadcast_roster(inner);
    Ok(())
}

/// Post to the whole team on the project's thread (never ambient). The
/// sender may be a member or the operator.
pub fn post(
    inner: &Arc<NodeInner>,
    from: &str,
    b: &Board,
    body: &str,
    urgency: &str,
    decision: bool,
) -> Result<Vec<String>, String> {
    if body.trim().is_empty() {
        return Err("refusing to post an empty message".into());
    }
    if !["normal", "notice", "gating"].contains(&urgency) {
        return Err(format!(
            "urgency must be normal|notice|gating, not {urgency:?}"
        ));
    }
    if status(b) == "archived" {
        return Err("the project is archived".into());
    }
    let members = crate::topology::board_panes(inner, b);
    if from != "operator" && !members.iter().any(|m| m == from) {
        return Err("you are not on that project".into());
    }
    let others: Vec<String> = members.into_iter().filter(|m| m != from).collect();
    let thread = format!("project:{}", b.id);
    let post = uuid::Uuid::new_v4().to_string();
    let display = format!("project \u{201c}{}\u{201d}", b.name);
    let body = if decision {
        format!("[decision] {body}")
    } else {
        body.to_owned()
    };
    let mut notes = Vec::new();
    for r in &others {
        inner
            .store
            .insert_message(
                from,
                r,
                &display,
                urgency,
                &body,
                Some(&thread),
                None,
                Some(&post),
            )
            .map_err(|e| e.to_string())?;
        inner.tick_delivery(r);
    }
    if others.is_empty() {
        // No one else on it yet: the post still lands in the project's
        // activity, addressed to the operator's record.
        inner
            .store
            .insert_message(
                from,
                "operator",
                &display,
                "notice",
                &body,
                Some(&thread),
                None,
                Some(&post),
            )
            .map_err(|e| e.to_string())?;
        notes.push("no one else is on the project yet; recorded in its activity".into());
    } else {
        notes.push(format!(
            "posted to {} on \u{201c}{}\u{201d}: {}",
            if others.len() == 1 {
                "1 member"
            } else {
                "the team"
            },
            b.name,
            others
                .iter()
                .map(|o| format!("@{}", crate::addr::bare(o)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if decision {
        let text = body.trim_start_matches("[decision] ");
        add_decision(inner, b, from, text)?;
        notes.push("pinned as a decision".into());
    }
    Ok(notes)
}

/// Put a local agent on a project's desk (PROPOSALS-2026-11-B §B-3): the
/// first empty pane, else a new pane to the right. Returns whether the
/// project changed (false: already on it).
pub fn place_member(inner: &Arc<NodeInner>, board_id: &str, key: &str) -> anyhow::Result<bool> {
    let b = inner
        .store
        .boards(false)?
        .into_iter()
        .find(|b| b.id == board_id)
        .ok_or_else(|| anyhow::anyhow!("no project {board_id:?}"))?;
    if b.query.is_some() {
        anyhow::bail!("a dynamic board has no members to add");
    }
    if status(&b) == "archived" {
        anyhow::bail!("the project is archived");
    }
    if crate::topology::board_panes(inner, &b)
        .iter()
        .any(|m| m == key)
    {
        return Ok(false);
    }
    let addr = match inner.mesh() {
        Some(m) => format!("{key}@{}", m.identity.node),
        None => key.to_owned(),
    };
    let short = uuid::Uuid::new_v4().simple().to_string();
    let pane = json!({ "kind": "session", "id": format!("p{}", &short[..8]), "agent": addr });
    fn fill(n: &mut Value, pane: &Value) -> bool {
        match n.get("kind").and_then(|k| k.as_str()) {
            Some("empty") => {
                let id = n.get("id").cloned().unwrap_or(Value::Null);
                *n = pane.clone();
                n["id"] = id;
                true
            }
            Some("split") => n
                .get_mut("children")
                .and_then(|c| c.as_array_mut())
                .is_some_and(|cs| cs.iter_mut().any(|c| fill(c, pane))),
            _ => false,
        }
    }
    let mut layout = b.layout.clone();
    if !fill(&mut layout, &pane) {
        layout = json!({
            "kind": "split",
            "id": format!("s{}", &short[8..16]),
            "dir": "row",
            "sizes": [50, 50],
            "children": [layout, pane],
        });
    }
    crate::boards::patch_and_notify(inner, &json!({ "id": b.id, "layout": layout }))?;
    Ok(true)
}
