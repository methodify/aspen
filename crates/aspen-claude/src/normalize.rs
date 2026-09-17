//! Stream-plane frames → the normalized `SessionEvent` vocabulary.
//!
//! Control-plane frames never reach this module; `session` peels them first.
//! Unknown frame types become `Raw`, never errors (forward-compat rule:
//! ignore unknown types/subtypes/fields — reference §11).

use aspen_core::SessionEvent;
use serde_json::Value;

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_owned)
}

/// One inbound stream frame may yield several normalized events (an
/// assistant envelope carrying two tool_use blocks yields two `ToolUse`s
/// plus the `AssistantMessage` snapshot).
pub fn normalize(frame: Value) -> Vec<SessionEvent> {
    let ty = frame.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // A subagent's traffic rides the same stream, marked with the parent
    // Agent call's `parent_tool_use_id` (reference §5.2). It is not the
    // session's own conversation: its text, deltas and tool results never
    // become transcript events (they painted a subagent's web searches
    // into the main transcript). Its tool calls do pass, still marked, so
    // a consumer can show "subagents working" without showing the work.
    if s(&frame, "parent_tool_use_id").is_some() {
        return match ty {
            "assistant" => normalize_assistant(frame)
                .into_iter()
                .filter(|e| matches!(e, SessionEvent::ToolUse { .. }))
                .collect(),
            _ => vec![],
        };
    }
    match ty {
        "stream_event" => normalize_stream_event(&frame),
        "assistant" => normalize_assistant(frame),
        "user" => normalize_user(frame),
        "result" => vec![normalize_result(frame)],
        "system" => normalize_system(frame),
        "tool_progress" | "tool_use_summary" | "rate_limit_event" => {
            vec![SessionEvent::Status { raw: frame }]
        }
        "keep_alive" => vec![],
        _ => vec![SessionEvent::Raw { raw: frame }],
    }
}

fn normalize_stream_event(frame: &Value) -> Vec<SessionEvent> {
    let event = frame.get("event").cloned().unwrap_or(Value::Null);
    let ety = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ety == "content_block_delta" {
        if let Some(delta) = event.get("delta") {
            let dty = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let text_key = match dty {
                "text_delta" => Some(("text", false)),
                "thinking_delta" => Some(("thinking", true)),
                _ => None,
            };
            if let Some((key, thinking)) = text_key {
                if let Some(text) = delta.get(key).and_then(|t| t.as_str()) {
                    return vec![SessionEvent::TextDelta {
                        text: text.to_owned(),
                        thinking,
                    }];
                }
            }
        }
    }
    // Boundaries (message_start/stop, content_block_start/stop) matter to a
    // renderer; pass them through as Raw so the UI layer can use them.
    vec![SessionEvent::Raw { raw: frame.clone() }]
}

fn normalize_assistant(frame: Value) -> Vec<SessionEvent> {
    let mut out = Vec::new();
    let message_id = frame
        .get("message")
        .and_then(|m| m.get("id"))
        .and_then(|i| i.as_str())
        .map(str::to_owned);
    let parent = s(&frame, "parent_tool_use_id");
    if let Some(blocks) = frame
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                let tool_name = s(b, "name").unwrap_or_default();
                let input = b.get("input").cloned().unwrap_or(Value::Null);
                let tool_kind = crate::adapter::classify(&tool_name, &input);
                let (summary, path, command) = crate::adapter::describe(&tool_name, &input);
                out.push(SessionEvent::ToolUse {
                    tool_use_id: s(b, "id").unwrap_or_default(),
                    tool_name,
                    input,
                    parent_tool_use_id: parent.clone(),
                    tool_kind,
                    summary,
                    path,
                    command,
                });
            }
        }
    }
    out.push(SessionEvent::AssistantMessage {
        message_id,
        raw: frame,
    });
    out
}

fn normalize_user(frame: Value) -> Vec<SessionEvent> {
    if frame.get("isReplay").and_then(|b| b.as_bool()) == Some(true) {
        return vec![SessionEvent::UserReplay {
            uuid: s(&frame, "uuid").unwrap_or_default(),
        }];
    }
    // Tool results arrive inside user-typed envelopes (reference §5.2).
    let mut out = Vec::new();
    if let Some(blocks) = frame
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                out.push(SessionEvent::ToolResult {
                    tool_use_id: s(b, "tool_use_id"),
                    raw: b.clone(),
                });
            }
        }
    }
    if out.is_empty() {
        out.push(SessionEvent::Raw { raw: frame });
    }
    out
}

fn normalize_result(frame: Value) -> SessionEvent {
    SessionEvent::TurnEnded {
        subtype: s(&frame, "subtype").unwrap_or_else(|| "unknown".into()),
        duration_ms: frame.get("duration_ms").and_then(|d| d.as_u64()),
        // Session-cumulative, not per-turn — label it that way in any UI
        // (the $0.58-for-92-tokens museum entry).
        total_cost_usd: frame.get("total_cost_usd").and_then(|d| d.as_f64()),
        result_text: s(&frame, "result"),
        usage: crate::adapter::turn_usage(&frame),
        raw: frame,
    }
}

fn normalize_system(frame: Value) -> Vec<SessionEvent> {
    match frame.get("subtype").and_then(|t| t.as_str()).unwrap_or("") {
        "init" => {
            // The first MCP picture (name + status only; `mcp_status`
            // fills the rest when the node asks).
            let servers: Vec<aspen_core::McpServerState> = frame
                .get("mcp_servers")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().map(crate::adapter::mcp_state_from).collect())
                .unwrap_or_default();
            let mut out = vec![SessionEvent::RuntimeInit {
                session_id: s(&frame, "session_id").unwrap_or_default(),
                model: s(&frame, "model"),
                raw: frame,
            }];
            if !servers.is_empty() {
                out.push(SessionEvent::McpChanged { servers });
            }
            out
        }
        _ => vec![SessionEvent::Status { raw: frame }],
    }
}

#[cfg(test)]
mod subagent_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subagent_frames_yield_only_marked_tool_uses() {
        let assistant = json!({
            "type": "assistant", "parent_tool_use_id": "toolu_parent",
            "message": { "id": "m1", "content": [
                { "type": "text", "text": "searching…" },
                { "type": "tool_use", "id": "tu_sub", "name": "WebFetch", "input": { "url": "https://x" } }
            ] }
        });
        let evs = normalize(assistant);
        assert_eq!(evs.len(), 1);
        assert!(
            matches!(&evs[0], SessionEvent::ToolUse { parent_tool_use_id: Some(p), .. } if p == "toolu_parent")
        );
        let result = json!({
            "type": "user", "parent_tool_use_id": "toolu_parent",
            "message": { "content": [ { "type": "tool_result", "tool_use_id": "tu_sub", "content": "ok" } ] }
        });
        assert!(normalize(result).is_empty());
        let delta = json!({
            "type": "stream_event", "parent_tool_use_id": "toolu_parent",
            "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": "hi" } }
        });
        assert!(normalize(delta).is_empty());
        // The session's own frames are untouched.
        let own = json!({
            "type": "assistant", "parent_tool_use_id": null,
            "message": { "id": "m2", "content": [ { "type": "text", "text": "mine" } ] }
        });
        assert!(normalize(own)
            .iter()
            .any(|e| matches!(e, SessionEvent::AssistantMessage { .. })));
    }
}
