//! The in-process MCP server: hub mounted inside the CLI as server name
//! "aspen", tunneled over `mcp_message` control requests (reference §8).
//!
//! This is the reverse channel that makes hub an agentic surface rather than
//! a spectator: the bus tools live here, served by the node daemon itself — no spawned
//! MCP process, no connector script, for Claude-family runtimes.
//!
//! The one transport quirk that costs people a day: the CLI awaits a reply
//! even for JSON-RPC *notifications*. Every `mcp_message` gets answered; a
//! benign value suffices for notifications.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

pub const SERVER_NAME: &str = "aspen";

/// Model-visible steering, returned from MCP initialize — free instruction
/// budget (reference §8). Kept terse; the tool descriptions carry the
/// per-tool contracts.
const INSTRUCTIONS: &str = "You are connected to aspen, the mesh your session runs in. \
Peer agent sessions may message you over the bus; such messages arrive prefixed with a \
[aspen bus] header naming the sender. Use the aspen tools to message peers and to see who is \
reachable. Silence from a peer is never evidence a message was lost.";

/// One tool: schema + handler. Handlers are sync and fast by contract;
/// anything slow belongs behind a channel to the hub layer.
pub struct Tool {
    pub name: &'static str,
    pub description: String,
    pub input_schema: Value,
    pub handler: Box<dyn Fn(Value) -> Result<String, String> + Send + Sync>,
}

#[derive(Default)]
pub struct McpServer {
    tools: HashMap<&'static str, Arc<Tool>>,
    order: Vec<&'static str>,
}

impl McpServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Tool) {
        self.order.push(tool.name);
        self.tools.insert(tool.name, Arc::new(tool));
    }

    /// Handle one JSON-RPC message arriving over `mcp_message`. Always
    /// returns *something* to send back as `mcp_response` — never None,
    /// because the transport awaits a reply even for notifications.
    pub fn handle(&self, message: &Value) -> Value {
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let is_notification = message.get("id").is_none();

        if is_notification {
            // notifications/initialized and friends: benign value.
            return json!({});
        }

        match method {
            "initialize" => {
                let requested = message
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .cloned()
                    .unwrap_or_else(|| json!("2024-11-05"));
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {
                        "protocolVersion": requested,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS,
                    }
                })
            }
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => {
                let tools: Vec<Value> = self
                    .order
                    .iter()
                    .filter_map(|n| self.tools.get(n))
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.input_schema,
                        })
                    })
                    .collect();
                json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } })
            }
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match self.tools.get(name) {
                    None => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("unknown tool {name:?}") }
                    }),
                    Some(tool) => {
                        let (text, is_error) = match (tool.handler)(args) {
                            Ok(t) => (t, false),
                            Err(e) => (e, true),
                        };
                        json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": text }],
                                "isError": is_error,
                            }
                        })
                    }
                }
            }
            _ => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("unknown method {method:?}") }
            }),
        }
    }
}

/// The P0 tool set: enough to prove the in-process path live. The bus tools
/// replace/extend this as the bus lands.
pub fn p0_tools(session_label: String) -> McpServer {
    let mut server = McpServer::new();
    server.register(Tool {
        name: "aspen_ping",
        description: "Confirm your connection to hub and see your own identity on the mesh."
            .into(),
        input_schema: json!({ "type": "object", "properties": {} }),
        handler: Box::new(move |_args| {
            Ok(format!(
                "pong — you are session {session_label}, connected to aspen (in-process MCP over the control channel)."
            ))
        }),
    });
    server
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_get_a_benign_reply() {
        let s = p0_tools("test".into());
        let out = s.handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        assert_eq!(out, json!({}));
    }

    #[test]
    fn tools_list_and_call() {
        let s = p0_tools("dev".into());
        let listed = s.handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        assert_eq!(listed["result"]["tools"][0]["name"], "aspen_ping");
        let called = s.handle(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"aspen_ping","arguments":{}}}),
        );
        let text = called["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("pong"));
        assert_eq!(called["result"]["isError"], json!(false));
    }

    #[test]
    fn unknown_method_errors_rather_than_hangs() {
        let s = McpServer::new();
        let out = s.handle(&json!({"jsonrpc":"2.0","id":9,"method":"resources/list"}));
        assert!(out.get("error").is_some());
    }
}
