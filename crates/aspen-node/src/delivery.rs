//! The delivery engine.
//!
//! Owning the pipe makes delivery physics simple and exact:
//!
//! - recipient live + idle  → write now (wakes them; one batch, one turn)
//! - recipient live + busy  → `normal`: write now — the CLI queues and
//!   coalesces into the *next* turn, which IS boundary delivery;
//!   `gating`: interrupt first (~110 ms), then write
//! - recipient not running  → rows stay pending; delivered at next spawn
//! - `notice`               → never delivered alone; rides along with any
//!   other delivery or operator send
//!
//! Everything pending delivers together, in send order — class never
//! reorders (a gating message may depend on the normal ones before it).

use std::sync::Arc;

use aspen_core::SessionHandle;
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
    // Notices never deliver alone — they must not wake or interrupt anyone.
    if !pending.iter().any(|m| m.urgency != "notice") {
        return Ok(());
    }
    let busy = sess.turn_state() == TurnState::Busy;
    let has_gating = pending.iter().any(|m| m.urgency == "gating");
    if busy && has_gating {
        // The interrupt ends the in-flight turn with an error-flavored
        // result; our queued write then forms the next turn.
        if let Err(e) = sess.handle.interrupt().await {
            tracing::warn!(error = %e, "interrupt for gating delivery failed; delivering at boundary instead");
        }
    }
    let via = if busy {
        if has_gating {
            "interrupt"
        } else {
            "boundary" // CLI-queued; reaches the model when this turn ends
        }
    } else {
        "wake"
    };
    let text = compose(&pending);
    sess.mark_busy();
    let _ = inner.store.record_event(
        &sess.name,
        "ask",
        serde_json::json!({
            "from": "bus",
            "senders": pending.iter().map(|m| m.sender.clone()).collect::<std::collections::BTreeSet<_>>(),
            "count": pending.len(),
            "gating": has_gating,
        }),
    );
    let ingest_uuid = sess.handle.send_user(text).await?;
    let ids: Vec<i64> = pending.iter().map(|m| m.id).collect();
    inner.store.mark_delivered(&ids, via, Some(&ingest_uuid))?;
    Ok(())
}

/// Flush pending notices for a session about to receive an operator message
/// — the only lane besides a delivery that a notice may ride.
pub async fn flush_notices(inner: &Arc<NodeInner>, sess: &Arc<ManagedSession>) {
    let Ok(pending) = inner.store.pending_for(&sess.name) else {
        return;
    };
    if pending.is_empty() || pending.iter().any(|m| m.urgency != "notice") {
        // Nothing to flush, or a real delivery is due anyway — let the
        // delivery engine handle the full batch in order.
        if !pending.is_empty() {
            inner.tick_delivery(&sess.name);
        }
        return;
    }
    let text = compose(&pending);
    if let Ok(ingest_uuid) = sess.handle.send_user(text).await {
        let ids: Vec<i64> = pending.iter().map(|m| m.id).collect();
        let _ = inner
            .store
            .mark_delivered(&ids, "rode-along", Some(&ingest_uuid));
    }
}

/// The injection envelope: unmistakably bus traffic, never mistaken for the
/// operator. One write carries everything pending, in send order.
pub fn compose(messages: &[StoredMessage]) -> String {
    let mut out = String::new();
    if messages.len() > 1 {
        out.push_str(&format!(
            "[aspen bus] {} messages, in send order:\n",
            messages.len()
        ));
    }
    for m in messages {
        out.push_str(&format!("\n[aspen bus] {} from @{}", m.urgency, m.sender));
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
        out.push_str(&m.body);
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
        assert!(one.contains("[aspen bus] normal from @arch"));
        assert!(one.contains("hello"));
        let two = compose(&[
            msg("normal", "arch", "first"),
            msg("gating", "op", "second"),
        ]);
        assert!(two.starts_with("[aspen bus] 2 messages"));
        // Send order preserved in the rendered text.
        assert!(two.find("first").unwrap() < two.find("second").unwrap());
    }
}
