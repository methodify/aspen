//! Boards on the node: the write path that also tells the agents on a
//! board when their team changed (PROPOSALS-2026-09-K.md K-3).
//!
//! A board is a team (BOARDS.md §9): its session panes are each other's
//! neighborhood. The charter says so at spawn and `bus_status` says so on
//! demand; this is the push — when a board's membership changes, every
//! *local* agent whose set of board-mates changed gets one `notice` on
//! the bus, from the operator (who edited the board). Layout-only edits
//! (splits, sizes, viewer panes) send nothing. Under always-lands delivery
//! (v0.34) a notice wakes an idle agent, so this is one turn per affected
//! agent per membership change — the operator's call, taken 2026-09-22.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;

use crate::node::NodeInner;
use crate::store::Board;

/// Write the board (last writer wins) and notify. Returns whether the
/// stored row changed, as `Store::upsert_board` does.
pub fn upsert_and_notify(inner: &Arc<NodeInner>, b: &Board) -> Result<bool> {
    let before = inner
        .store
        .boards(false)
        .unwrap_or_default()
        .into_iter()
        .find(|x| x.id == b.id);
    let changed = inner.store.upsert_board(b)?;
    if changed {
        notify_membership(inner, before.as_ref(), b);
    }
    Ok(changed)
}

/// Tombstone the board and tell its members it is gone.
pub fn delete_and_notify(inner: &Arc<NodeInner>, id: &str) -> Result<()> {
    let before = inner
        .store
        .boards(false)
        .unwrap_or_default()
        .into_iter()
        .find(|x| x.id == id);
    inner.store.delete_board(id)?;
    if let Some(b) = before {
        let mut gone = b.clone();
        gone.deleted = true;
        gone.layout = serde_json::json!({ "kind": "empty", "id": "x" });
        notify_membership(inner, Some(&b), &gone);
    }
    Ok(())
}

fn notify_membership(inner: &Arc<NodeInner>, before: Option<&Board>, after: &Board) {
    let old: BTreeSet<String> = before
        .map(|b| crate::topology::board_panes(inner, b).into_iter().collect())
        .unwrap_or_default();
    let new: BTreeSet<String> = if after.deleted {
        BTreeSet::new()
    } else {
        crate::topology::board_panes(inner, after)
            .into_iter()
            .collect()
    };
    if old == new {
        return;
    }
    let local: BTreeSet<String> = inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.name)
        .collect();
    let name = &after.name;
    for agent in old.union(&new) {
        if !local.contains(agent) {
            continue;
        }
        let was = old.contains(agent);
        let is = new.contains(agent);
        let mates_old: Vec<&String> = old.iter().filter(|m| *m != agent).collect();
        let mates_new: Vec<&String> = new.iter().filter(|m| *m != agent).collect();
        // Nothing to say when this agent's own team is unchanged.
        if was && is && mates_old == mates_new {
            continue;
        }
        let list = |v: &[&String]| {
            v.iter()
                .map(|m| format!("@{m}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = if !is {
            format!("[board] You are no longer on the operator's board \u{201c}{name}\u{201d}.")
        } else if !was {
            if mates_new.is_empty() {
                format!("[board] The operator put you on the board \u{201c}{name}\u{201d} (alone there so far).")
            } else {
                format!(
                    "[board] The operator put you on the board \u{201c}{name}\u{201d} with {} \u{2014} they are your team for it; bus_status lists them.",
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
                "[board] On the operator's board \u{201c}{name}\u{201d}: {}. With you now: {}.",
                parts.join("; "),
                if mates_new.is_empty() {
                    "nobody".to_owned()
                } else {
                    list(&mates_new)
                }
            )
        };
        let display = format!("@{}", crate::addr::bare(agent));
        match inner.store.insert_message(
            "operator",
            agent,
            &display,
            "notice",
            &body,
            Some(&format!("board:{}", after.id)),
            None,
            None,
        ) {
            Ok(_) => inner.tick_delivery(agent),
            Err(e) => tracing::warn!(agent, err = %e, "board notice not queued"),
        }
    }
}
