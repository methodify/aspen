//! Projects on the node (PROPOSALS-2026-11-A; boards grown up): the write
//! paths, and what the members are told.
//!
//! A project is a team (BOARDS.md §9): its desk's session panes and its
//! bench are each other's neighborhood; the charter says so at spawn and
//! `bus_status` on demand. This is the push. When a project's membership,
//! charter or status changes, every *local* member it concerns gets one
//! **ambient** notice (A-3): it never wakes the agent, and is read with the
//! next thing written to it. Layout-only edits send nothing; a paused
//! project sends nothing at all. Each change is also an event on
//! `project:<id>`, for the project's activity.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use crate::node::NodeInner;
use crate::store::Board;

/// The most of a charter a notice carries.
const CHARTER_NOTICE_MAX: usize = 8 * 1024;

fn find(inner: &Arc<NodeInner>, id: &str) -> Option<Board> {
    inner
        .store
        .boards(true)
        .unwrap_or_default()
        .into_iter()
        .find(|x| x.id == id)
}

/// Merge a board from a peer (field by field) and notify. Returns whether
/// the stored row changed.
pub fn upsert_and_notify(inner: &Arc<NodeInner>, b: &Board) -> Result<bool> {
    let before = find(inner, &b.id).filter(|x| !x.deleted);
    let changed = inner.store.upsert_board(b)?;
    if changed {
        if let Some(after) = find(inner, &b.id) {
            notify(inner, before.as_ref(), &after);
        }
    }
    Ok(changed)
}

/// Merge the console's patch (the fields the operator changed) and notify.
pub fn patch_and_notify(inner: &Arc<NodeInner>, patch: &Value) -> Result<bool> {
    let id = patch
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_owned();
    let before = find(inner, &id).filter(|x| !x.deleted);
    let changed = inner.store.upsert_board_from_console(patch)?;
    if changed {
        if let Some(after) = find(inner, &id) {
            notify(inner, before.as_ref(), &after);
        }
    }
    Ok(changed)
}

/// Tombstone the project and tell its members it is gone.
pub fn delete_and_notify(inner: &Arc<NodeInner>, id: &str) -> Result<()> {
    let before = find(inner, id).filter(|x| !x.deleted);
    inner.store.delete_board(id)?;
    if let (Some(b), Some(after)) = (before, find(inner, id)) {
        notify(inner, Some(&b), &after);
    }
    Ok(())
}

fn status(b: &Board) -> &str {
    b.status
        .as_ref()
        .and_then(|s| s.as_str())
        .unwrap_or("active")
}

fn charter(b: &Board) -> Option<&str> {
    b.charter
        .as_ref()
        .and_then(|c| c.as_str())
        .filter(|c| !c.trim().is_empty())
}

fn notify(inner: &Arc<NodeInner>, before: Option<&Board>, after: &Board) {
    let id = &after.id;
    let name = &after.name;
    let members = |b: &Board| -> BTreeSet<String> {
        if b.deleted {
            BTreeSet::new()
        } else {
            crate::topology::board_panes(inner, b).into_iter().collect()
        }
    };
    let old = before.map(members).unwrap_or_default();
    let new = members(after);
    let (st_old, st_new) = (before.map(status).unwrap_or("active"), status(after));
    let (ch_old, ch_new) = (before.and_then(charter), charter(after));
    // The activity's record, whatever the status.
    let record = |kind: &str, detail: Value| {
        let _ = inner
            .store
            .record_event(&format!("project:{id}"), kind, detail);
    };
    if let Some(b) = before {
        if b.name != after.name {
            record("project_renamed", json!({ "from": b.name, "to": name }));
        }
    }
    if st_old != st_new {
        record("project_status", json!({ "from": st_old, "to": st_new }));
    }
    if ch_old != ch_new {
        record("project_charter", json!({ "charter": ch_new }));
    }
    for m in new.difference(&old) {
        record("project_joined", json!({ "agent": m }));
    }
    for m in old.difference(&new) {
        record("project_left", json!({ "agent": m }));
    }
    if after.deleted {
        record("project_deleted", json!({}));
    }
    // Paused: the members stay, and hear nothing until it resumes.
    if st_new == "paused" && st_old == "paused" {
        return;
    }
    let local: BTreeSet<String> = inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.name)
        .collect();
    let list = |v: &[&String]| {
        v.iter()
            .map(|m| format!("@{m}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let say = |agent: &str, body: String| {
        let display = format!("@{}", crate::addr::bare(agent));
        match inner.store.insert_message(
            "operator",
            agent,
            &display,
            "ambient",
            &body,
            Some(&format!("project:{id}")),
            None,
            None,
        ) {
            Ok(_) => inner.tick_delivery(agent),
            Err(e) => tracing::warn!(agent, err = %e, "project notice not queued"),
        }
    };
    // Status: told once, to whoever was or is a member.
    if st_old != st_new && !after.deleted {
        let line = match st_new {
            "paused" => format!("[project] The operator paused the project \u{201c}{name}\u{201d}: you stay on its team; nothing is needed from you for it until it resumes."),
            "archived" => format!("[project] The operator archived the project \u{201c}{name}\u{201d}: it is closed, and you are no longer on its team."),
            _ => format!("[project] The operator resumed the project \u{201c}{name}\u{201d}."),
        };
        for agent in old.union(&new).filter(|a| local.contains(*a)) {
            say(agent, line.clone());
        }
        // An archive ends the team; the status line said so.
        if st_new == "archived" {
            return;
        }
    }
    if st_new == "paused" {
        return;
    }
    // The charter: to every member, in full (capped).
    if ch_old != ch_new && !after.deleted {
        let body = match ch_new {
            Some(c) => {
                let c = if c.len() > CHARTER_NOTICE_MAX {
                    let mut end = CHARTER_NOTICE_MAX;
                    while !c.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}…", &c[..end])
                } else {
                    c.to_owned()
                };
                format!(
                    "[project] The charter of \u{201c}{name}\u{201d} changed. It now reads:\n\n{c}"
                )
            }
            None => {
                format!("[project] The project \u{201c}{name}\u{201d} no longer has a charter.")
            }
        };
        for agent in new
            .iter()
            .filter(|a| local.contains(*a) && old.contains(*a))
        {
            say(agent, body.clone());
        }
    }
    // Membership.
    for agent in old.union(&new) {
        if !local.contains(agent) {
            continue;
        }
        let was = old.contains(agent);
        let is = new.contains(agent);
        let mates_old: Vec<&String> = old.iter().filter(|m| *m != agent).collect();
        let mates_new: Vec<&String> = new.iter().filter(|m| *m != agent).collect();
        if was && is && mates_old == mates_new {
            continue;
        }
        let body = if !is {
            if after.deleted {
                format!("[project] The operator deleted the project \u{201c}{name}\u{201d}; you are no longer on its team.")
            } else {
                format!(
                    "[project] You are no longer on the operator's project \u{201c}{name}\u{201d}."
                )
            }
        } else if !was {
            let charter = charter(after)
                .map(|c| format!("\n\nIts charter:\n{c}"))
                .unwrap_or_default();
            if mates_new.is_empty() {
                format!("[project] The operator put you on the project \u{201c}{name}\u{201d} (alone there so far).{charter}")
            } else {
                format!(
                    "[project] The operator put you on the project \u{201c}{name}\u{201d} with {}: they are your team for it; bus_status lists them.{charter}",
                    list(&mates_new)
                )
            }
        } else {
            let joined: Vec<&String> = mates_new
                .iter()
                .filter(|m| !mates_old.contains(m))
                .cloned()
                .collect();
            let left: Vec<&String> = mates_old
                .iter()
                .filter(|m| !mates_new.contains(m))
                .cloned()
                .collect();
            let mut parts = Vec::new();
            if !joined.is_empty() {
                parts.push(format!("{} joined", list(&joined)));
            }
            if !left.is_empty() {
                parts.push(format!("{} left", list(&left)));
            }
            format!(
                "[project] On the operator's project \u{201c}{name}\u{201d}: {}. With you now: {}.",
                parts.join("; "),
                if mates_new.is_empty() {
                    "nobody".to_owned()
                } else {
                    list(&mates_new)
                }
            )
        };
        say(agent, body);
    }
}
