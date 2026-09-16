//! The delivery engine (PROPOSALS-2026-09-H.md, decision 2026-09-16).
//!
//! Owning the pipe makes delivery physics simple and exact:
//!
//! - recipient live + idle  → write now: wakes them, one batch, one turn
//! - recipient live + busy  → write now, exactly as an operator's message
//!   mid-turn: the harness appends it to its next model request, so the
//!   recipient sees it at the next sensible point in its own work and
//!   decides for itself whether to act now or park it. No interrupt —
//!   the sender's guess at urgency is not the recipient's judgement.
//! - recipient not running  → rows stay pending; delivered at next spawn
//!
//! Urgency is carried in the envelope for the recipient to read; it no
//! longer changes timing. Everything pending delivers together, in send
//! order. What a mid-turn write does *not* guarantee is a wake at the
//! boundary when the harness has no further model request in that turn:
//! node.rs watches the replay acks and nudges at the boundary for
//! anything still held (the record of every write is `inputs`).

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::node::{ManagedSession, NodeInner, TurnState};
use crate::store::StoredMessage;

pub async fn run(inner: Arc<NodeInner>, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(recipient) = rx.recv().await {
        if recipient == "operator" {
            continue; // operator mail is read from the store/UI, not injected
        }
        let Some(sess) = inner.live(&recipient) else {
            // Not running here. A node-qualified address (`name@node`) or a
            // bare name homed elsewhere forwards over the mesh when the
            // link is up; otherwise rows stay pending (next spawn here, or
            // next link-up).
            if let Some(mesh) = inner.mesh() {
                // `name@repo@node` names its home; `name@repo` may be homed
                // on a peer whose roster lists that key.
                let home = match crate::addr::node_of(&recipient) {
                    Some(node) => Some(node.to_owned()),
                    None => mesh.find_remote(&recipient).map(|(node, _)| node),
                };
                if let Some(node) = home {
                    if mesh.link_up(&node) {
                        crate::federation::forward_pending(&inner, &recipient, &node);
                    } else {
                        // No link: a relay mailbox carries it until the
                        // peer shows up (bounded; rows stay pending here
                        // until the peer's ack comes back).
                        crate::federation::mail_pending(&inner, &recipient, &node);
                    }
                }
            }
            continue;
        };
        if let Err(e) = attempt(&inner, &sess).await {
            tracing::warn!(recipient, error = %e, "bus delivery attempt failed; rows remain pending");
        }
    }
}

async fn attempt(inner: &Arc<NodeInner>, sess: &Arc<ManagedSession>) -> anyhow::Result<()> {
    let pending = inner.store.pending_for(&sess.name)?;
    if pending.is_empty() {
        return Ok(());
    }
    let busy = sess.turn_state() == TurnState::Busy;
    let via = if busy { "mid-turn" } else { "wake" };
    let text = compose(&pending);
    sess.mark_busy();
    let _ = inner.store.record_event(
        &sess.name,
        "ask",
        serde_json::json!({
            "from": "bus",
            "senders": pending.iter().map(|m| m.sender.clone()).collect::<std::collections::BTreeSet<_>>(),
            "count": pending.len(),
            "mid_turn": busy,
        }),
    );
    // A wedged child must not stall delivery for every other agent.
    let ingest_uuid = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        sess.handle.send_user(text.clone()),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "@{}: the session did not accept input within 30s",
            sess.name
        )
    })??;
    let ids: Vec<i64> = pending.iter().map(|m| m.id).collect();
    inner.store.mark_delivered(&ids, via, Some(&ingest_uuid))?;
    let _ = inner
        .store
        .record_input(&sess.name, &ingest_uuid, "bus", &text, busy);
    Ok(())
}

/// Before an operator message: anything pending for the session (rows that
/// waited while it was down) goes first, in send order — chronology, so
/// the operator's reply to a message lands after the message.
pub async fn flush_notices(inner: &Arc<NodeInner>, sess: &Arc<ManagedSession>) {
    let Ok(pending) = inner.store.pending_for(&sess.name) else {
        return;
    };
    if pending.is_empty() {
        return;
    }
    if let Err(e) = attempt(inner, sess).await {
        tracing::warn!(agent = %sess.name, error = %e, "pending mail before an operator message: attempt failed");
    }
}

/// The end-of-message line: a bus segment runs from its header to this,
/// so a transcript line the harness merged from several writes splits
/// back into the operator's words and each message (node.rs, the console).
pub const BUS_END: &str = "[aspen bus end]";

/// The injection envelope: unmistakably bus traffic, never mistaken for the
/// operator. One write carries everything pending, in send order, each
/// message self-contained between its header line and `[aspen bus end]`.
pub fn compose(messages: &[StoredMessage]) -> String {
    let mut out = String::new();
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("[aspen bus] {} from @{}", m.urgency, m.sender));
        if m.to_display.starts_with('#') {
            out.push_str(&format!(" · {}", m.to_display));
        }
        if let Some(t) = &m.thread {
            out.push_str(&format!(" · thread {t}"));
        }
        if let Some(r) = &m.record_ref {
            out.push_str(&format!(" · record {r}"));
        }
        out.push('\n');
        out.push_str(m.body.trim_end());
        out.push('\n');
        out.push_str(BUS_END);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(urgency: &str, sender: &str, body: &str) -> StoredMessage {
        StoredMessage {
            id: 1,
            uuid: "u".into(),
            thread: None,
            sender: sender.into(),
            recipient: "impl".into(),
            to_display: "@impl".into(),
            urgency: urgency.into(),
            body: body.into(),
            record_ref: None,
            created_at: 0.0,
            delivered_at: None,
            delivered_via: None,
            ingested_at: None,
            post: None,
        }
    }

    #[test]
    fn compose_single_and_batch() {
        let one = compose(&[msg("normal", "arch", "hello")]);
        assert!(one.starts_with("[aspen bus] normal from @arch"));
        assert!(one.trim_end().ends_with(BUS_END));
        assert!(one.contains("hello"));
        let two = compose(&[
            msg("normal", "arch", "first"),
            msg("gating", "op", "second"),
        ]);
        assert!(two.starts_with("[aspen bus] "));
        assert_eq!(two.matches(BUS_END).count(), 2);
        // Send order preserved in the rendered text.
        assert!(two.find("first").unwrap() < two.find("second").unwrap());
    }
}
