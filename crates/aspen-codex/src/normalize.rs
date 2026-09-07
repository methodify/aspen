//! App-server notifications → `SessionEvent` (CODEX_RUNTIME_REFERENCE.md
//! §4; PROPOSALS-HARNESSES.md §3.2). Every item type maps to a tool kind;
//! agent messages become the console's assistant snapshots in the same
//! envelope shape the Claude adapter emits (`{id, content:[{type:"text"}]}`),
//! so the transcript reducer stays one reducer.

use serde_json::{json, Value};

use aspen_core::{SessionEvent, ToolKind};

/// The bash script inside `/bin/bash -lc '<script>'`, else the command.
pub fn command_of(item: &Value) -> String {
    let full = item.get("command").and_then(|c| c.as_str()).unwrap_or("").to_owned();
    // The app-server renders the argv as one string; the actions carry the
    // bare command when parsed.
    if let Some(first) = item
        .get("commandActions")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|a| a.get("command"))
        .and_then(|c| c.as_str())
    {
        return first.to_owned();
    }
    full
}

/// What a command execution does, from the app-server's own parse.
pub fn shell_kind(item: &Value) -> ToolKind {
    let actions = item.get("commandActions").and_then(|a| a.as_array());
    let Some(actions) = actions else { return ToolKind::Shell };
    if actions.is_empty() {
        return ToolKind::Shell;
    }
    let all = |t: &[&str]| actions.iter().all(|a| a.get("type").and_then(|x| x.as_str()).is_some_and(|x| t.contains(&x)));
    if all(&["read"]) {
        ToolKind::FileRead
    } else if all(&["read", "listFiles", "search"]) {
        ToolKind::Search
    } else {
        ToolKind::Shell
    }
}

fn changes_of(item: &Value) -> (Vec<Value>, Option<String>) {
    let changes: Vec<Value> = item.get("changes").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    let first = changes.first().and_then(|c| c.get("path")).and_then(|p| p.as_str()).map(str::to_owned);
    (changes, first)
}

/// `item/started` → the tool-use event for tool-shaped items (None for
/// messages, reasoning, plans).
pub fn tool_use_of(item: &Value) -> Option<SessionEvent> {
    let ty = item.get("type").and_then(|t| t.as_str())?;
    let id = item.get("id").and_then(|i| i.as_str()).unwrap_or("").to_owned();
    match ty {
        "commandExecution" => {
            let cmd = command_of(item);
            Some(SessionEvent::ToolUse {
                tool_use_id: id,
                tool_name: "commandExecution".into(),
                input: json!({ "command": cmd, "cwd": item.get("cwd"), "actions": item.get("commandActions") }),
                parent_tool_use_id: None,
                tool_kind: shell_kind(item),
                summary: Some(cmd.clone()),
                path: None,
                command: Some(cmd),
            })
        }
        "fileChange" => {
            let (changes, first) = changes_of(item);
            let n = changes.len();
            let kind = if changes.iter().all(|c| c.get("kind").and_then(|k| k.get("type")).and_then(|t| t.as_str()) == Some("add")) {
                ToolKind::FileWrite
            } else {
                ToolKind::FileEdit
            };
            let summary = match (&first, n) {
                (Some(p), 1) => p.clone(),
                (Some(p), n) => format!("{p} (+{} more)", n - 1),
                _ => "file change".into(),
            };
            Some(SessionEvent::ToolUse {
                tool_use_id: id,
                tool_name: "fileChange".into(),
                input: json!({ "changes": changes }),
                parent_tool_use_id: None,
                tool_kind: kind,
                summary: Some(summary),
                path: first,
                command: None,
            })
        }
        "mcpToolCall" => {
            let server = item.get("server").and_then(|s| s.as_str()).unwrap_or("mcp");
            let tool = item.get("tool").and_then(|s| s.as_str()).unwrap_or("tool");
            Some(SessionEvent::ToolUse {
                tool_use_id: id,
                tool_name: format!("mcp__{server}__{tool}"),
                input: item.get("arguments").cloned().unwrap_or(json!({})),
                parent_tool_use_id: None,
                tool_kind: ToolKind::Mcp,
                summary: Some(format!("{server}.{tool}")),
                path: None,
                command: None,
            })
        }
        "dynamicToolCall" => Some(SessionEvent::ToolUse {
            tool_use_id: id,
            tool_name: item.get("tool").and_then(|s| s.as_str()).unwrap_or("tool").to_owned(),
            input: item.get("arguments").cloned().unwrap_or(json!({})),
            parent_tool_use_id: None,
            tool_kind: ToolKind::Other,
            summary: None,
            path: None,
            command: None,
        }),
        "webSearch" => Some(SessionEvent::ToolUse {
            tool_use_id: id,
            tool_name: "webSearch".into(),
            input: json!({ "query": item.get("query") }),
            parent_tool_use_id: None,
            tool_kind: ToolKind::Web,
            summary: item.get("query").and_then(|q| q.as_str()).map(str::to_owned),
            path: None,
            command: None,
        }),
        "collabAgentToolCall" => Some(SessionEvent::ToolUse {
            tool_use_id: id,
            tool_name: "collabAgentToolCall".into(),
            input: json!({ "tool": item.get("tool"), "prompt": item.get("prompt"), "receivers": item.get("receiverThreadIds") }),
            parent_tool_use_id: None,
            tool_kind: ToolKind::Agent,
            summary: item.get("prompt").and_then(|p| p.as_str()).map(|p| p.chars().take(120).collect()),
            path: None,
            command: None,
        }),
        "subAgentActivity" => Some(SessionEvent::ToolUse {
            tool_use_id: id,
            tool_name: "subAgentActivity".into(),
            input: json!({ "kind": item.get("kind"), "agent": item.get("agentPath") }),
            parent_tool_use_id: None,
            tool_kind: ToolKind::Agent,
            summary: item.get("agentPath").and_then(|p| p.as_str()).map(str::to_owned),
            path: None,
            command: None,
        }),
        "imageView" => Some(SessionEvent::ToolUse {
            tool_use_id: id,
            tool_name: "imageView".into(),
            input: json!({ "path": item.get("path") }),
            parent_tool_use_id: None,
            tool_kind: ToolKind::FileRead,
            summary: item.get("path").and_then(|p| p.as_str()).map(str::to_owned),
            path: item.get("path").and_then(|p| p.as_str()).map(str::to_owned),
            command: None,
        }),
        _ => None,
    }
}

/// `item/completed` → the tool result for tool-shaped items.
pub fn tool_result_of(item: &Value) -> Option<SessionEvent> {
    let ty = item.get("type").and_then(|t| t.as_str())?;
    let id = item.get("id").and_then(|i| i.as_str()).unwrap_or("").to_owned();
    let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
    let (text, is_error) = match ty {
        "commandExecution" => {
            let out = item.get("aggregatedOutput").and_then(|o| o.as_str()).unwrap_or("").to_owned();
            let exit = item.get("exitCode").and_then(|e| e.as_i64());
            match (status, exit) {
                ("declined", _) => ("declined by the operator".to_owned(), true),
                ("failed", _) => (if out.is_empty() { "failed".into() } else { out }, true),
                (_, Some(c)) if c != 0 => (format!("{out}\n(exit code {c})"), true),
                _ => (out, false),
            }
        }
        "fileChange" => {
            let (changes, _) = changes_of(item);
            let list: Vec<String> = changes
                .iter()
                .map(|c| {
                    let k = c.get("kind").and_then(|k| k.get("type")).and_then(|t| t.as_str()).unwrap_or("update");
                    format!("{k} {}", c.get("path").and_then(|p| p.as_str()).unwrap_or(""))
                })
                .collect();
            match status {
                "declined" => ("declined by the operator".to_owned(), true),
                "failed" => (format!("failed: {}", list.join(", ")), true),
                _ => (list.join("\n"), false),
            }
        }
        "mcpToolCall" => {
            if let Some(e) = item.get("error").filter(|e| !e.is_null()) {
                (e.get("message").and_then(|m| m.as_str()).unwrap_or("error").to_owned(), true)
            } else {
                let r = item.get("result").cloned().unwrap_or(Value::Null);
                let text = r
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| serde_json::to_string_pretty(&r).unwrap_or_default());
                (text, status == "failed")
            }
        }
        "dynamicToolCall" => {
            let text = item
                .get("contentItems")
                .and_then(|c| c.as_array())
                .map(|items| items.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            (text, item.get("success") == Some(&Value::Bool(false)))
        }
        "webSearch" | "collabAgentToolCall" | "subAgentActivity" | "imageView" => (
            item.get("status").and_then(|s| s.as_str()).unwrap_or("done").to_owned(),
            matches!(status, "failed" | "interrupted"),
        ),
        _ => return None,
    };
    Some(SessionEvent::ToolResult {
        tool_use_id: Some(id.clone()),
        raw: json!({ "tool_use_id": id, "content": text, "is_error": is_error }),
    })
}

/// The console's assistant snapshot for an agent message item.
pub fn assistant_message_of(item: &Value, model: Option<&str>) -> SessionEvent {
    let id = item.get("id").and_then(|i| i.as_str()).map(str::to_owned);
    let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
    SessionEvent::AssistantMessage {
        message_id: id.clone(),
        raw: json!({
            "id": id, "role": "assistant", "model": model,
            "content": [{ "type": "text", "text": text }],
            "phase": item.get("phase"),
        }),
    }
}

/// Neutral usage from a `thread/tokenUsage/updated` payload's `last`.
pub fn turn_usage(token_usage: &Value) -> Value {
    let last = token_usage.get("last").unwrap_or(&Value::Null);
    let n = |k: &str| last.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    if last.is_null() {
        return Value::Null;
    }
    json!({
        "input": n("inputTokens"),
        "output": n("outputTokens"),
        "cache_read": n("cachedInputTokens"),
        "cache_create": n("cacheWriteInputTokens"),
        "reasoning": n("reasoningOutputTokens"),
        "cumulative": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_kinds() {
        let read = json!({"type":"commandExecution","id":"e","command":"/bin/bash -lc 'cat x'","commandActions":[{"type":"read","command":"cat x","name":"x","path":"x"}]});
        assert_eq!(shell_kind(&read), ToolKind::FileRead);
        assert_eq!(command_of(&read), "cat x");
        let run = json!({"type":"commandExecution","id":"e","command":"/bin/bash -lc 'make'","commandActions":[{"type":"unknown","command":"make"}]});
        assert_eq!(shell_kind(&run), ToolKind::Shell);
        match tool_use_of(&run) {
            Some(SessionEvent::ToolUse { tool_kind, command, .. }) => {
                assert_eq!(tool_kind, ToolKind::Shell);
                assert_eq!(command.as_deref(), Some("make"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn file_change_result() {
        let item = json!({"type":"fileChange","id":"f","changes":[{"path":"/r/a.txt","kind":{"type":"add"},"diff":"x"}],"status":"completed"});
        match tool_use_of(&item) {
            Some(SessionEvent::ToolUse { tool_kind, path, .. }) => {
                assert_eq!(tool_kind, ToolKind::FileWrite);
                assert_eq!(path.as_deref(), Some("/r/a.txt"));
            }
            other => panic!("{other:?}"),
        }
        match tool_result_of(&item) {
            Some(SessionEvent::ToolResult { raw, .. }) => assert_eq!(raw["is_error"], false),
            other => panic!("{other:?}"),
        }
    }
}
