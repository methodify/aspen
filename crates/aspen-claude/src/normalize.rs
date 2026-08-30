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
    vec![SessionEvent::Raw {
        raw: frame.clone(),
    }]
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
                out.push(SessionEvent::ToolUse {
                    tool_use_id: s(b, "id").unwrap_or_default(),
                    tool_name: s(b, "name").unwrap_or_default(),
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                    parent_tool_use_id: parent.clone(),
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
        raw: frame,
    }
}

fn normalize_system(frame: Value) -> Vec<SessionEvent> {
    match frame.get("subtype").and_then(|t| t.as_str()).unwrap_or("") {
        "init" => vec![SessionEvent::RuntimeInit {
            session_id: s(&frame, "session_id").unwrap_or_default(),
            model: s(&frame, "model"),
            raw: frame,
        }],
        _ => vec![SessionEvent::Status { raw: frame }],
    }
}
