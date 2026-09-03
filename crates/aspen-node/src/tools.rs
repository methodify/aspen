//! The bus tools served to agents over the in-process MCP path.
//!
//! Tool descriptions ARE the contract — guidance delivered at the point of
//! use, traveling with the session (plumb's lesson). They also have to earn
//! their load: on current builds SDK-served tools arrive deferred and the
//! model reads the description to decide whether to load them.

use std::sync::Arc;

use serde_json::{json, Value};

use aspen_claude::mcp::{McpServer, Tool};

use crate::node::{NodeInner, TurnState};

pub fn build_mcp(inner: Arc<NodeInner>, me: String) -> McpServer {
    let mut server = McpServer::new();

    // ------------------------------------------------------------- bus_send
    {
        let inner = inner.clone();
        let me = me.clone();
        server.register(Tool {
            name: "bus_send",
            description: "Send a message to a peer agent, a repo channel, or the human operator \
on the aspen bus. `to` is '@name' (a peer in your repo), '@name@repo' (a peer in another repo; \
add '@node' only if that repo exists on several nodes), '#channel', or '@operator'. `urgency` is delivery timing, \
nothing else: 'gating' interrupts the recipient mid-turn; 'normal' (default) is delivered now \
if they are idle (waking them) or at their turn boundary if they are busy; 'notice' is an \
ambient fact that never wakes or interrupts anyone. A peer who is not running receives at \
their next session start. Delivery always carries everything pending in send order. There is \
no subject line; put everything in body. Silence from a peer never means your message was \
lost — check bus_status before concluding anything from silence."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "'@agent', '#channel', or '@operator'. bus_status lists who exists." },
                    "body": { "type": "string", "description": "The whole message." },
                    "urgency": { "type": "string", "enum": ["gating", "normal", "notice"], "default": "normal" },
                    "thread": { "type": "string", "description": "Optional thread id grouping an exchange; no effect on delivery." },
                    "record": { "type": "string", "description": "Durable record this is about (issue id, doc path), if any — the message is the notification; the record is where it lives." }
                },
                "required": ["to", "body"]
            }),
            handler: Box::new(move |args| bus_send(&inner, &me, args)),
        });
    }

    // ----------------------------------------------------------- bus_status
    {
        let inner = inner.clone();
        let me = me.clone();
        server.register(Tool {
            name: "bus_status",
            description: "Who is on this bus: every agent, their repo channel, whether their \
session is running, whether they are mid-turn or idle, and how many messages are pending for \
them. Check here when a peer seems unresponsive before concluding a message was lost — \
silence usually means mid-turn or not running; it never means the bus dropped something."
                .into(),
            input_schema: json!({ "type": "object", "properties": {} }),
            handler: Box::new(move |_args| bus_status(&inner, &me)),
        });
    }

    // ------------------------------------------------------------ bus_inbox
    {
        let inner = inner.clone();
        let me = me.clone();
        server.register(Tool {
            name: "bus_inbox",
            description: "Read messages addressed to you that have not been delivered yet. You \
rarely need this: gating messages interrupt you and everything else arrives at your turn \
boundaries. Reach for it to drain deliberately — e.g. before reporting status, so you are not \
reporting against a ruling you have not read."
                .into(),
            input_schema: json!({ "type": "object", "properties": {} }),
            handler: Box::new(move |_args| bus_inbox(&inner, &me)),
        });
    }

    server
}

fn bus_send(inner: &Arc<NodeInner>, me: &str, args: Value) -> Result<String, String> {
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or("missing 'to'")?
        .trim()
        .to_owned();
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .ok_or("missing 'body'")?;
    let urgency = args
        .get("urgency")
        .and_then(|v| v.as_str())
        .unwrap_or("normal");
    let thread = args.get("thread").and_then(|v| v.as_str());
    let record = args.get("record").and_then(|v| v.as_str());
    let notes = send_message(inner, me, &to, body, urgency, thread, record)?;
    Ok(format!("Sent ({urgency}) to {}.\n{}", to, notes.join("\n")))
}

/// The one send path, shared by the agent-facing MCP tool and the operator
/// API. Returns per-recipient delivery notes.
pub fn send_message(
    inner: &Arc<NodeInner>,
    from: &str,
    to: &str,
    body: &str,
    urgency: &str,
    thread: Option<&str>,
    record: Option<&str>,
) -> Result<Vec<String>, String> {
    if body.trim().is_empty() {
        return Err("refusing to send an empty message".into());
    }
    if !["gating", "normal", "notice"].contains(&urgency) {
        return Err(format!(
            "urgency must be gating|normal|notice, not {urgency:?}"
        ));
    }

    // Resolve the address to concrete recipients.
    let recipients: Vec<String> = if to == "@operator" {
        vec!["operator".into()]
    } else if let Some(name) = to.strip_prefix('@') {
        vec![resolve_agent(inner, from, name)?]
    } else if let Some(channel) = to.strip_prefix('#') {
        let mut members: Vec<String> = Vec::new();
        // A custom channel carries explicit members (which may span repos
        // and nodes, and may include @operator); a bare repo name falls
        // back to that repo's auto-membership.
        if inner.store.channel_exists(channel).unwrap_or(false) {
            for m in inner
                .store
                .custom_channel_members(channel)
                .map_err(|e| e.to_string())?
            {
                members.push(canonical_member(&m));
            }
        } else {
            members = inner
                .store
                .channel_members(channel)
                .map_err(|e| e.to_string())?;
            if let Some(mesh) = inner.mesh() {
                for (name, _node) in mesh.remote_channel_members(channel) {
                    if !members.contains(&name) {
                        members.push(name);
                    }
                }
            }
        }
        let others: Vec<String> = members.into_iter().filter(|m| m != from).collect();
        if others.is_empty() {
            return Err(format!(
                "channel #{channel} has no members besides you — bus_status lists who exists"
            ));
        }
        others
    } else {
        return Err(format!(
            "'to' must be '@agent', '#channel', or '@operator', not {to:?}"
        ));
    };

    // One post id groups a fan-out (channel → N recipients) into a single
    // logical message for conversation views.
    let post = uuid::Uuid::new_v4().to_string();
    let mut out = Vec::new();
    for recipient in &recipients {
        inner
            .store
            .insert_message(
                from,
                recipient,
                to,
                urgency,
                body,
                thread,
                record,
                Some(&post),
            )
            .map_err(|e| e.to_string())?;
        inner.tick_delivery(recipient);
        out.push(delivery_note(inner, recipient, urgency));
    }
    Ok(out)
}

/// The self node's mesh name, if in a mesh.
fn self_node(inner: &Arc<NodeInner>) -> Option<String> {
    inner.mesh().map(|m| m.identity.node.clone())
}

/// Every agent this node knows about, as (key, node) — local agents with
/// node = None, roster-known remote agents with their node.
fn known_agents(inner: &Arc<NodeInner>) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = inner
        .store
        .agents()
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.name, None))
        .collect();
    if let Some(mesh) = inner.mesh() {
        for (node, agents) in mesh.remote.lock().unwrap().iter() {
            for a in agents {
                out.push((a.name.clone(), Some(node.clone())));
            }
        }
    }
    out
}

/// Resolve an agent address from the sender's context to a recipient key:
/// a local key (`arch@nl`) or a fully qualified remote one (`arch@nl@beta`).
///
/// - `name@repo@node`: taken literally (local if node is us).
/// - `name@repo`: local if such an agent (or at least the repo) is here;
///   else the one node whose roster has it; ambiguous → error.
/// - `name`: the sender's own repo first; else the single agent of that
///   name anywhere; else the single one sharing a custom channel with the
///   sender; else refused with the candidates — never a silent far guess.
pub fn resolve_agent(inner: &Arc<NodeInner>, from: &str, to: &str) -> Result<String, String> {
    let a = crate::addr::Addr::parse(to)?;
    if a.is_operator() {
        return Ok("operator".into());
    }
    let me = self_node(inner);
    if let (Some(repo), Some(node)) = (&a.repo, &a.node) {
        return Ok(if me.as_deref() == Some(node.as_str()) {
            crate::addr::local_key(&a.name, repo)
        } else {
            format!("{}@{}@{}", a.name, repo, node)
        });
    }
    let known = known_agents(inner);
    if let Some(repo) = &a.repo {
        let key = crate::addr::local_key(&a.name, repo);
        if known.iter().any(|(k, n)| k == &key && n.is_none()) {
            return Ok(key);
        }
        let remote: Vec<&String> = known
            .iter()
            .filter(|(k, n)| k == &key && n.is_some())
            .map(|(_, n)| n.as_ref().unwrap())
            .collect();
        return match remote.as_slice() {
            [node] => Ok(format!("{key}@{node}")),
            [] => {
                // The repo is here but the agent isn't registered yet — let
                // the message wait for it, as bare names always could.
                let repo_here = inner
                    .store
                    .repos()
                    .unwrap_or_default()
                    .iter()
                    .any(|r| r.handle == *repo);
                if repo_here {
                    Ok(key)
                } else {
                    Err(format!(
                        "no agent {key} on this node or any linked node — bus_status lists who exists"
                    ))
                }
            }
            many => Err(format!(
                "{key} exists on several nodes ({}) — say which: {key}@<node>",
                many.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    // Bare name: the sender's own repo first.
    if let Some(my_repo) = crate::addr::repo_of(from) {
        let key = crate::addr::local_key(&a.name, my_repo);
        if known.iter().any(|(k, n)| k == &key && n.is_none()) {
            return Ok(key);
        }
    }
    let matches: Vec<&(String, Option<String>)> = known
        .iter()
        .filter(|(k, _)| crate::addr::bare(k) == a.name)
        .collect();
    let qualify = |(k, n): &(String, Option<String>)| match n {
        Some(node) => format!("{k}@{node}"),
        None => k.clone(),
    };
    match matches.as_slice() {
        [] => Err(format!(
            "no agent named '{}' anywhere on the bus — bus_status lists who exists",
            a.name
        )),
        [one] => Ok(qualify(one)),
        many => {
            // Disambiguate through shared custom channels with the sender.
            let mine: std::collections::HashSet<String> = inner
                .store
                .channels_of(from)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let shared: Vec<String> = many
                .iter()
                .filter(|(k, n)| {
                    let full = match n {
                        Some(node) => format!("{k}@{node}"),
                        None => k.clone(),
                    };
                    inner
                        .store
                        .channels_of(&full)
                        .unwrap_or_default()
                        .iter()
                        .any(|c| mine.contains(c))
                })
                .map(|m| qualify(m))
                .collect();
            if let [one] = shared.as_slice() {
                return Ok(one.clone());
            }
            Err(format!(
                "'{}' is ambiguous — choose one of: {}",
                a.name,
                many.iter()
                    .map(|m| qualify(m))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

/// Canonicalize a channel-member address to a recipient key: `@operator`
/// and `operator` → "operator"; `@name` → "name"; `name@node` kept as-is
/// (the delivery engine forwards it).
pub fn canonical_member(addr: &str) -> String {
    let a = addr.trim();
    if a == "@operator" || a == "operator" {
        return "operator".into();
    }
    a.strip_prefix('@').unwrap_or(a).to_owned()
}

/// What the sender is told about how their message will actually land — the
/// signal no discipline ever gave them, at the moment they can act on it.
fn delivery_note(inner: &Arc<NodeInner>, recipient: &str, urgency: &str) -> String {
    if recipient == "operator" {
        return "→ @operator: lands in the operator inbox; the operator reads it on their own schedule.".into();
    }
    match inner.live(recipient) {
        None => {
            // Homed on a peer node? Say what will actually happen. Both
            // node-qualified (`name@node`) and roster-known bare names.
            if let Some(mesh) = inner.mesh() {
                // `key@node` names its home outright; a bare key may be
                // homed on a peer whose roster lists it.
                let found = match crate::addr::node_of(recipient) {
                    Some(node) => {
                        let key = crate::addr::strip_node(recipient, node);
                        let ra = mesh
                            .remote
                            .lock()
                            .unwrap()
                            .get(node)
                            .and_then(|v| v.iter().find(|a| a.name == key).cloned());
                        Some((
                            node.to_owned(),
                            ra.unwrap_or(crate::federation::RemoteAgent {
                                name: key.to_owned(),
                                channel: String::new(),
                                live: false,
                                turn_state: None,
                                summary: None,
                                title: None,
                            }),
                        ))
                    }
                    None => mesh.find_remote(recipient),
                };
                if let Some((node, ra)) = found {
                    return if mesh.link_up(&node) {
                        let state = if ra.live {
                            ra.turn_state.as_deref().unwrap_or("running").to_owned()
                        } else {
                            "not running there either".to_owned()
                        };
                        format!(
                            "→ @{recipient}: homed on node '{node}' ({state}) — forwarding \
                             over the mesh now; their node takes it from there."
                        )
                    } else {
                        format!(
                            "→ @{recipient}: homed on node '{node}', but that node is \
                             UNREACHABLE right now — held here and forwarded when the link \
                             returns."
                        )
                    };
                }
            }
            format!(
                "→ @{recipient}: NOT RUNNING — nothing can wake them; this is held and arrives at \
                 their next session start. If it needs them now, tell the operator."
            )
        }
        Some(sess) => match (sess.turn_state(), urgency) {
            (_, "notice") => format!(
                "→ @{recipient}: notice recorded; it rides along with their next delivery or turn."
            ),
            (TurnState::Idle, _) => format!("→ @{recipient}: idle — delivered now (this wakes them)."),
            (TurnState::Busy, "gating") => {
                format!("→ @{recipient}: mid-turn — interrupting them now.")
            }
            (TurnState::Busy, _) => format!(
                "→ @{recipient}: mid-turn — arrives when their current turn ends, which can take a while."
            ),
        },
    }
}

fn bus_status(inner: &Arc<NodeInner>, me: &str) -> Result<String, String> {
    let agents = inner.store.agents().map_err(|e| e.to_string())?;
    if agents.is_empty() {
        return Ok("No agents registered on this bus yet.".into());
    }
    let my_repo = crate::addr::repo_of(me);
    let self_node = self_node(inner).unwrap_or_else(|| "local".into());
    let mut lines = vec![format!(
        "You are {me}. Bus roster (addresses as you should write them):"
    )];
    for a in agents {
        let state = match inner.live(&a.name) {
            None => "not running".to_owned(),
            Some(s) => match s.turn_state() {
                TurnState::Idle => "running, idle".into(),
                TurnState::Busy => "running, mid-turn".into(),
            },
        };
        let pending = inner.store.pending_count(&a.name).unwrap_or(0);
        let shown = crate::addr::display_for(&a.name, my_repo, &self_node, false);
        let mut line = if a.name == me {
            format!("  {shown} (you) — #{}", a.channel)
        } else {
            format!("  {shown} — #{} — {}", a.channel, state)
        };
        if pending > 0 && a.name != me {
            line.push_str(&format!(", {pending} pending"));
        }
        lines.push(line);
    }
    if let Some(mesh) = inner.mesh() {
        let remote = mesh.remote.lock().unwrap();
        for (node, agents) in remote.iter() {
            let reachable = mesh.link_up(node);
            for a in agents {
                let state = if !reachable {
                    "node unreachable".to_owned()
                } else if a.live {
                    format!(
                        "running, {}",
                        a.turn_state.as_deref().unwrap_or("state unknown")
                    )
                } else {
                    "not running".to_owned()
                };
                let pending = inner
                    .store
                    .pending_count(&format!("{}@{}", a.name, node))
                    .unwrap_or(0);
                let mut line = format!(
                    "  {}@{} — #{} — on node '{}' — {}",
                    a.name, node, a.channel, node, state
                );
                if pending > 0 {
                    line.push_str(&format!(", {pending} pending here"));
                }
                lines.push(line);
            }
        }
    }
    let op_pending = inner.store.pending_count("operator").unwrap_or(0);
    lines.push(format!(
        "  @operator — the human — {op_pending} unread in their inbox"
    ));
    // The channels this agent belongs to, with co-members — a channel
    // someone put you in is only useful if you know you're in it.
    let mine = inner.store.channels_of(me).unwrap_or_default();
    if !mine.is_empty() {
        lines.push("Your custom channels (post with '#name'):".into());
        for c in mine {
            let members: Vec<String> = inner
                .store
                .custom_channel_members(&c)
                .unwrap_or_default()
                .into_iter()
                .map(|m| canonical_member(&m))
                .filter(|m| m != me)
                .map(|m| crate::addr::display_for(&m, my_repo, &self_node, false))
                .collect();
            lines.push(format!("  #{c} — with {}", members.join(", ")));
        }
    }
    Ok(lines.join("\n"))
}

fn bus_inbox(inner: &Arc<NodeInner>, me: &str) -> Result<String, String> {
    let pending = inner.store.pending_for(me).map_err(|e| e.to_string())?;
    if pending.is_empty() {
        return Ok("No undelivered messages.".into());
    }
    let text = crate::delivery::compose(&pending);
    let ids: Vec<i64> = pending.iter().map(|m| m.id).collect();
    inner
        .store
        .mark_delivered(&ids, "inbox", None)
        .map_err(|e| e.to_string())?;
    Ok(text)
}
