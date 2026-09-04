//! Adoption: noticing sessions that happen to an agent *outside* Aspen and
//! asking which identity follows them.
//!
//! Branching is not a wire operation in the runtime's protocol — a fork is
//! a relaunch (`-r <id> --fork-session`), a rewind is TUI-only — so there
//! is nothing to intercept in the stream. What there is: the transcript on
//! disk (a fork copies the parent's prefix and stamps `forkedFrom`), and
//! the SessionStart hook (`aspen hook`), which gets us there in a second
//! instead of a scan interval. Two kinds are raised, both as needs:
//!
//! - `fork`: a new session in a registered repo forked from an agent's
//!   session. Verbs: carry (the name moves to it), split (a new agent
//!   takes it), ignore.
//! - `resumed`: an agent's head session grew while the agent was down —
//!   someone opened it in a terminal. Verbs: revive (later), ignore.
//!
//! Sessions merely *started* in a registered repo are not raised: they are
//! visible in the Mesh list already and would be noise. The default when
//! nobody answers is ignore: a name never moves on its own.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::node::{Node, NodeInner};
use crate::store::now_epoch;

/// Lines written by Aspen's own processes carry this entrypoint.
const OUR_ENTRYPOINT: &str = "aspen";
/// Leave a fresh file alone this long so its first turn (and our own
/// lineage record, if it is ours) can land.
const GRACE_SECS: f64 = 20.0;

/// One pass over every registered repo. Cheap when nothing changed: the
/// session enumeration is (mtime,size)-cached and the origin read touches
/// only the head and tail of new files.
pub fn scan(inner: &Arc<NodeInner>) -> Result<Vec<i64>> {
    let mut raised = Vec::new();
    let repos = inner.store.repos()?;
    let known = inner.store.known_sessions()?;
    let agents = inner.store.agents()?;
    // Our own forks in flight: a live session launched as a fork of P whose
    // head is still recorded as P (the runtime hasn't announced the child
    // id yet — that lands with the first turn). A new file forked from one
    // of these is ours; look again later rather than announce it.
    let pending_forks: Vec<String> = inner
        .sessions
        .lock()
        .unwrap()
        .values()
        .filter_map(|s| {
            let (p, _) = s.fork_from.as_ref()?;
            let head = agents
                .iter()
                .find(|a| a.name == s.name)?
                .session_id
                .as_ref()?;
            (head == p).then(|| p.clone())
        })
        .collect();
    let now = now_epoch();

    for repo in &repos {
        let path = repo.path.as_path();
        let sessions = aspen_claude::transcript::enumerate_sessions(path).unwrap_or_default();
        let seen = inner.store.seen_sessions(path)?;
        if seen.is_empty() {
            // First look at this repo: baseline, don't announce history.
            let all: Vec<String> = sessions.iter().map(|s| s.session_id.clone()).collect();
            inner.store.mark_seen(path, &all)?;
            continue;
        }
        let seen: std::collections::HashSet<String> = seen.into_iter().collect();
        let mut newly = Vec::new();
        for si in &sessions {
            if seen.contains(&si.session_id) || known.contains(&si.session_id) {
                continue;
            }
            if now - si.modified_epoch < GRACE_SECS {
                continue; // look again next pass
            }
            let origin = aspen_claude::transcript::session_origin(path, &si.session_id);
            if origin.last_entrypoint.as_deref() == Some(OUR_ENTRYPOINT) {
                // Ours (spawned here; its id is recorded at the first turn).
                // Not marked seen: a copied prefix still says "aspen" even
                // when someone else forked it, so re-check once it has
                // lines of its own.
                continue;
            }
            newly.push(si.session_id.clone());
            // Parent: stamped, or the agent head in this repo sharing the
            // longest uuid prefix with the new file (a fork copies its
            // parent's lines, uuids intact). Siblings share prefixes too, so
            // a candidate wholly contained in the new file wins outright —
            // it is the line that was actually forked.
            let parent = origin.forked_from.clone().or_else(|| {
                origin.first_uuid.as_ref()?;
                let child = aspen_claude::transcript::conversation_uuids(path, &si.session_id);
                let mut best: Option<(String, usize, bool)> = None;
                for sid in agents
                    .iter()
                    .filter(|a| a.repo == repo.path)
                    .filter_map(|a| a.session_id.clone())
                    .filter(|sid| sid != &si.session_id)
                {
                    let cand = aspen_claude::transcript::conversation_uuids(path, &sid);
                    let shared = aspen_claude::transcript::shared_prefix(&child, &cand);
                    if shared == 0 {
                        continue;
                    }
                    let whole = shared == cand.len();
                    let better = match &best {
                        None => true,
                        Some((_, s, w)) => (whole && !w) || (whole == *w && shared > *s),
                    };
                    if better {
                        best = Some((sid, shared, whole));
                    }
                }
                let (sid, shared, _) = best?;
                Some((sid, child.get(shared.saturating_sub(1)).cloned()))
            });
            let Some((parent, at)) = parent else {
                continue; // started fresh outside Aspen: not raised
            };
            if pending_forks.contains(&parent) {
                newly.retain(|x| x != &si.session_id);
                continue; // our own branch, mid-launch: look again
            }
            let Some(agent) = inner.store.agent_for_session(&parent)? else {
                continue; // forked from a session Aspen never named
            };
            if let Some(id) = inner.store.upsert_adoption(
                path,
                &si.session_id,
                "fork",
                Some(&agent),
                Some(&parent),
                at.as_deref(),
                si.title.as_deref(),
                origin.last_entrypoint.as_deref(),
            )? {
                let _ = inner.store.record_event(
                    &agent,
                    "fork_seen",
                    json!({ "session": si.session_id, "parent": parent, "entrypoint": origin.last_entrypoint }),
                );
                raised.push(id);
            }
        }
        inner.store.mark_seen(path, &newly)?;

        // Heads driven from elsewhere: the file grew after the agent went
        // down (and after the last time we asked), by something not us.
        for a in agents.iter().filter(|a| a.repo == repo.path) {
            let Some(head) = &a.session_id else { continue };
            if inner.live(&a.name).is_some() {
                continue;
            }
            let Some(si) = sessions.iter().find(|s| &s.session_id == head) else {
                continue;
            };
            let since = a
                .last_exit_at
                .unwrap_or(0.0)
                .max(inner.store.adoption_resolved_at(head)?.unwrap_or(0.0));
            if since == 0.0
                || si.modified_epoch <= since + 5.0
                || now - si.modified_epoch < GRACE_SECS
            {
                continue;
            }
            let origin = aspen_claude::transcript::session_origin(path, head);
            if origin.last_entrypoint.as_deref() == Some(OUR_ENTRYPOINT) {
                continue;
            }
            if let Some(id) = inner.store.upsert_adoption(
                path,
                head,
                "resumed",
                Some(&a.name),
                None,
                None,
                si.title.as_deref(),
                origin.last_entrypoint.as_deref(),
            )? {
                let _ = inner.store.record_event(
                    &a.name,
                    "resumed_elsewhere",
                    json!({ "session": head, "entrypoint": origin.last_entrypoint }),
                );
                raised.push(id);
            }
        }
    }
    Ok(raised)
}

/// The SessionStart/SessionEnd hook told us about a session. Aspen's own
/// processes are filtered by the hook itself (their environment carries our
/// entrypoint); anything else is worth a prompt scan of that repo.
pub fn on_hook(inner: &Arc<NodeInner>, cwd: Option<&Path>) {
    let _ = cwd;
    let inner = inner.clone();
    tokio::task::spawn_blocking(move || {
        // A fresh file needs its grace period; scan now (for resumes, which
        // grow an existing file) and again after the grace.
        let _ = scan(&inner);
        std::thread::sleep(std::time::Duration::from_secs_f64(GRACE_SECS + 2.0));
        let _ = scan(&inner);
    });
}

pub fn spawn_scanner(inner: Arc<NodeInner>, every_secs: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        loop {
            let i = inner.clone();
            let _ = tokio::task::spawn_blocking(move || scan(&i)).await;
            tokio::time::sleep(std::time::Duration::from_secs(every_secs)).await;
        }
    });
}

/// Answer an adoption. `carry`: the agent's name moves to the session (its
/// old tip is bookmarked; a live process is restarted onto the new head).
/// `split`: a new agent named `name` resumes the session in place; the
/// original keeps its own. `revive`: bring the agent back on its head.
/// `ignore`: leave it.
pub async fn resolve(
    node: &Node,
    id: i64,
    how: &str,
    name: Option<&str>,
) -> Result<serde_json::Value> {
    let inner = &node.inner;
    let ad = inner
        .store
        .adoption(id)?
        .ok_or_else(|| anyhow!("no adoption {id}"))?;
    if ad.resolved.is_some() {
        return Err(anyhow!(
            "adoption {id} was already answered ({})",
            ad.resolved.unwrap_or_default()
        ));
    }
    let agent = ad.of_agent.clone().unwrap_or_default();
    match how {
        "ignore" => {
            inner.store.resolve_adoption(id, "ignore", None)?;
            Ok(json!({ "ok": true }))
        }
        "carry" => {
            if ad.kind != "fork" {
                return Err(anyhow!("carry applies to forks"));
            }
            let rows = inner.store.agents()?;
            let row = rows
                .iter()
                .find(|a| a.name == agent)
                .ok_or_else(|| anyhow!("no agent named {agent}"))?;
            let was_live = inner.live(&agent).is_some();
            if let Some(head) = &row.session_id {
                let _ = inner
                    .store
                    .add_bookmark(&agent, head, None, row.title.as_deref(), "carry");
            }
            inner.store.record_lineage(
                &agent,
                &ad.session_id,
                ad.parent_session.as_deref().unwrap_or(""),
                ad.fork_message.as_deref(),
            )?;
            inner.store.set_agent_session(&agent, &ad.session_id)?;
            inner.store.resolve_adoption(id, "carry", Some(&agent))?;
            let _ = inner.store.record_event(
                &agent,
                "carry",
                json!({ "session": ad.session_id, "from": row.session_id }),
            );
            if was_live {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    node.shutdown_for_restart(&agent),
                )
                .await;
                for _ in 0..50 {
                    if inner.live(&agent).is_none() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                node.revive_agent(&agent, true).await?;
            }
            Ok(json!({ "ok": true, "agent": agent, "revived": was_live }))
        }
        "split" => {
            let name = name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .ok_or_else(|| anyhow!("split needs a name"))?;
            let rows = inner.store.agents()?;
            let row = rows.iter().find(|a| a.name == agent);
            let opts = crate::node::SpawnOpts {
                charter: row.and_then(|r| r.charter.clone()),
                resume: Some(ad.session_id.clone()),
                interactive: true,
                extra_args: row.and_then(|r| r.extra_args.clone()),
                ..Default::default()
            };
            let sess = node.spawn_agent(name, ad.repo.clone(), opts).await?;
            if let Some(p) = &ad.parent_session {
                let _ = inner.store.record_lineage(
                    &sess.name,
                    &ad.session_id,
                    p,
                    ad.fork_message.as_deref(),
                );
            }
            inner
                .store
                .resolve_adoption(id, "split", Some(&sess.name))?;
            let _ = inner.store.record_event(
                &sess.name,
                "split",
                json!({ "from": agent, "session": ad.session_id, "adopted": true }),
            );
            Ok(json!({ "ok": true, "agent": sess.name }))
        }
        "revive" => {
            inner.store.resolve_adoption(id, "revive", Some(&agent))?;
            node.revive_agent(&agent, true).await?;
            Ok(json!({ "ok": true, "agent": agent }))
        }
        other => Err(anyhow!(
            "unknown action {other:?} (carry | split | ignore | revive)"
        )),
    }
}

pub fn adoption_json(a: &crate::store::Adoption) -> serde_json::Value {
    json!({
        "id": a.id,
        "repo": a.repo.to_string_lossy(),
        "session_id": a.session_id,
        "kind": a.kind,
        "of_agent": a.of_agent,
        "parent_session": a.parent_session,
        "fork_message": a.fork_message,
        "title": a.title,
        "entrypoint": a.entrypoint,
        "first_seen": a.first_seen,
        "resolved": a.resolved,
        "resolved_at": a.resolved_at,
        "resolved_as": a.resolved_as,
    })
}
