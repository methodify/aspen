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
on the aspen bus. `to` is '@name', '#channel', or '@operator'. `urgency` is delivery timing, \
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
        server.register(Tool {
            name: "bus_status",
            description: "Who is on this bus: every agent, their repo channel, whether their \
session is running, whether they are mid-turn or idle, and how many messages are pending for \
them. Check here when a peer seems unresponsive before concluding a message was lost — \
silence usually means mid-turn or not running; it never means the bus dropped something."
                .into(),
            input_schema: json!({ "type": "object", "properties": {} }),
            handler: Box::new(move |_args| bus_status(&inner)),
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
        vec![name.to_owned()]
    } else if let Some(channel) = to.strip_prefix('#') {
        let mut members = inner
            .store
            .channel_members(channel)
            .map_err(|e| e.to_string())?;
        // Cross-node channel members join the fan-out under their bare
        // names; the delivery engine forwards to their home nodes.
        if let Some(mesh) = &inner.mesh {
            for (name, _node) in mesh.remote_channel_members(channel) {
                if !members.contains(&name) {
                    members.push(name);
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

    let mut out = Vec::new();
    for recipient in &recipients {
        inner
            .store
            .insert_message(from, recipient, to, urgency, body, thread, record)
            .map_err(|e| e.to_string())?;
        inner.tick_delivery(recipient);
        out.push(delivery_note(inner, recipient, urgency));
    }
    Ok(out)
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
            if let Some(mesh) = &inner.mesh {
                let found = match recipient.split_once('@') {
                    Some((bare, node)) => Some((
                        node.to_owned(),
                        crate::federation::RemoteAgent {
                            name: bare.to_owned(),
                            channel: String::new(),
                            live: mesh
                                .remote
                                .lock()
                                .unwrap()
                                .get(node)
                                .map(|v| v.iter().any(|a| a.name == bare && a.live))
                                .unwrap_or(false),
                            turn_state: None,
                        },
                    )),
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

fn bus_status(inner: &Arc<NodeInner>) -> Result<String, String> {
    let agents = inner.store.agents().map_err(|e| e.to_string())?;
    if agents.is_empty() {
        return Ok("No agents registered on this bus yet.".into());
    }
    let mut lines = vec!["Bus roster:".to_owned()];
    for a in agents {
        let state = match inner.live(&a.name) {
            None => "not running".to_owned(),
            Some(s) => match s.turn_state() {
                TurnState::Idle => "running, idle".into(),
                TurnState::Busy => "running, mid-turn".into(),
            },
        };
        let pending = inner.store.pending_count(&a.name).unwrap_or(0);
        let mut line = format!("  @{} — #{} — {}", a.name, a.channel, state);
        if pending > 0 {
            line.push_str(&format!(", {pending} pending"));
        }
        lines.push(line);
    }
    if let Some(mesh) = &inner.mesh {
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
                let pending = inner.store.pending_count(&a.name).unwrap_or(0);
                let mut line = format!(
                    "  @{} — #{} — on node '{}' — {}",
                    a.name, a.channel, node, state
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
