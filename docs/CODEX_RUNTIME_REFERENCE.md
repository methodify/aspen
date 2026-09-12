# Codex runtime reference (field-verified)

> For a self-contained guide to driving Codex headless from any host —
> the same facts organised for someone who is not building Aspen — see
> CODEX_HEADLESS_GUIDE.md.

**Status:** verified against `codex-cli 0.153.4` on 2026-09-07 with a
real account, through `crates/aspen-codex`. Like the Claude reference,
every shape here was observed on the wire or on disk, not read from
docs. The protocol source is `~/src/codex/codex-rs/app-server-protocol`
(generated TypeScript under `schema/typescript/v2`); the wire log of the
probe that produced most of this lives in the session scratchpad.

## 1. The surface Aspen drives

`codex app-server --listen stdio://` — one process per session
(PROPOSALS-HARNESSES.md §6.1). Newline-delimited JSON-RPC 2.0 both ways:
our requests carry integer ids; the server's own requests (approvals,
questions, elicitations) carry ids we must answer; notifications have no
id and carry an `emittedAtMs`. `codex exec --json` is not used: it is
one-shot and hard-rejects approvals.

Config is passed as `-c key=value` overrides (TOML values), never by
writing `config.toml`:

| override | why |
|---|---|
| `projects."<cwd>".trust_level="trusted"` | Aspen's trust gate is the only trust decision; the override keeps `config.toml` untouched |
| `mcp_servers.aspen.command="<aspen binary>"`, `.args=["mcp"]`, `.env={ ASPEN_NODE_API=…, ASPEN_AGENT=…, ASPEN_NODE_TOKEN=… }`, `.startup_timeout_sec=20` | the bus tools, through the stdio bridge (§7) |

Env: `CODEX_INTERNAL_ORIGINATOR_OVERRIDE=aspen` stamps the rollout's
`originator` (the entrypoint, as Claude's `CLAUDE_CODE_ENTRYPOINT` does);
`ASPEN_DETACHED` is removed. **A fork inherits its parent's originator**
(verified: a thread forked by another app-server client from an Aspen
thread still says `aspen`), so the stamp identifies the lineage, not
the writer; adoption relies on the registry for Codex.

Hooks: `$CODEX_HOME/hooks.json` (and `.codex/hooks.json` in a project)
use Claude's shape — `{hooks: {SessionStart: [{hooks: [{type: "command",
command}]}]}}` — and the SessionStart payload carries the same fields
(`cwd, session_id, source, hook_event_name, model, permission_mode,
transcript_path`); events: SessionStart, SessionEnd, UserPromptSubmit,
PreToolUse, PostToolUse, PermissionRequest, PreCompact, PostCompact,
SubagentStart, SubagentStop, Stop, Interrupt. `aspen hooks install
--harness codex` writes the relay there.

## 2. Handshake and thread

```
→ initialize {clientInfo:{name:"aspen", title:"Aspen", version}, capabilities:null}
← {userAgent, codexHome, platformFamily, platformOs}
→ initialized (notification)
→ thread/start {cwd, approvalPolicy, approvalsReviewer, sandbox, developerInstructions?, model?}
← {thread:{id, path, model, cwd, cliVersion, source, forkedFromId, …}, model, approvalPolicy, sandbox:{type:…}, …}
```

- `thread/resume {threadId, cwd, approvalPolicy, sandbox, developerInstructions}`
  resumes with history (verified: the revived thread recalled its first
  request). `thread/fork {threadId, …}` makes a new thread whose rollout
  carries a `history_base` pointer to the parent (§3).
- `developerInstructions` is the charter (it lands in the rollout as a
  `developer` message). `sandbox` at thread level is a `SandboxMode`
  string (`read-only | workspace-write | danger-full-access`); per turn
  it is a `SandboxPolicy` object (§5).
- Inventory: `model/list {includeHidden:false}` → `{data:[{model,
  displayName, description, isDefault, …}]}`; `skills/list {cwds:[cwd]}`
  → `{data:[{cwd, skills:[{name, description, shortDescription, path,
  scope, enabled}]}]}`.
- `thread/started`, `mcpServer/startupStatus/updated {name, status:
  starting|ready}` follow. The MCP servers come up per thread.

## 3. Rollouts on disk

`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<YYYY-MM-DDThh-mm-ss>-<thread>.jsonl`
(`~/.codex` by default). Every line: `{timestamp, ordinal, type, payload}`.

| line type | payload |
|---|---|
| `session_meta` (first line) | `{id, session_id, cwd, originator, cli_version, source, forked_from_id?, history_base?:{thread_id, end_ordinal_exclusive, end_byte_offset}, base_instructions, model_provider, …}` |
| `event_msg` | `{type: task_started {turn_id, model_context_window} \| item_completed {item} \| token_count {info:{total_token_usage, last_token_usage, model_context_window}, rate_limits} \| task_complete {turn_id, last_agent_message, duration_ms} \| thread_settings_applied}` |
| `response_item` | the model API items: `message {role: developer\|user\|assistant, content:[{type:input_text\|output_text, text}]}`, `custom_tool_call {name:"exec", input:<code-mode script>}`, `custom_tool_call_output`, `reasoning` |
| `turn_context` | `{turn_id, cwd, approval_policy, sandbox_policy, model, …}` |
| `world_state`, `token_usage_record` | environment snapshot; per-response usage |

`item_completed` items use PascalCase types and snake_case fields:
`UserMessage {id, content:[{type:"text", text}]}`, `AgentMessage {id,
content:[{type:"Text", text}], phase: commentary|final_answer,
delivery?: "async", questions?}`, `CommandExecution {id, command:
["/bin/bash","-lc","<script>"], cwd:"file://…", parsed_cmd, status,
aggregated_output, formatted_output, exit_code, duration}`, `FileChange
{id, changes:{"<path>":{type:add|update|delete, content|unified_diff}},
status, stdout}`, `Reasoning`, `Extension {kind:"clock.sleep", …}`.
Aspen rehydrates from these (`aspen-codex::store::rehydrate_lines`); a
fork's rollout is read after its parent's lines below
`history_base.end_ordinal_exclusive`, recursively.

Codex's own tool calls are *code mode*: one `exec` custom tool call runs
a script that calls `tools.exec_command`, `tools.apply_patch`, …; the
app-server still emits one `commandExecution` / `fileChange` item per
underlying action, which is what Aspen shows.

## 4. Turns and items on the wire

```
→ turn/start {threadId, input:[{type:"text", text, text_elements:[]}], clientUserMessageId?, model?, approvalPolicy?, approvalsReviewer?, sandboxPolicy?}
← {turn:{id, status:"inProgress"}}
← turn/started {threadId, turn}
← item/started {item:{type:"userMessage", …}} / item/completed
← item/started {item:{type:"agentMessage", id, text:"", phase}} → item/agentMessage/delta {itemId, delta}* → item/completed {item:{…, text}}
← item/started {item:{type:"commandExecution", id, command:"/bin/bash -lc '…'", cwd, commandActions:[{type:read|listFiles|search|unknown, command, …}], status:"inProgress"}} → item/completed {…, status: completed|failed|declined, aggregatedOutput, exitCode, durationMs}
← item/started/completed {item:{type:"fileChange", id, changes:[{path, kind:{type:add|update|delete}, diff}], status}}
← item/started/completed {item:{type:"mcpToolCall", id, server, tool, arguments, status, result|error}}
← turn/diff/updated {diff}          — the turn's cumulative unified diff
← thread/tokenUsage/updated {tokenUsage:{total, last, modelContextWindow}}   — per response
← account/rateLimits/updated
← thread/status/changed {status:{type: active|idle}}
← turn/completed {turn:{id, status: completed|interrupted|failed, error?, items:[…summary], durationMs, startedAt, completedAt}}
```

- Reasoning arrives as `item/reasoning/textDelta` /
  `summaryTextDelta` and a `reasoning` item; Aspen streams it as
  thinking text.
- `turn/interrupt {threadId, turnId}` ends the turn with status
  `interrupted` (verified: the turn went idle; no partial message is
  kept in the rollout).
- `turn/steer {threadId, input, expectedTurnId}` injects into a running
  turn; Aspen uses it when a message arrives mid-turn and falls back to
  `turn/start` if it is refused.
- Token usage fields: `inputTokens, cachedInputTokens,
  cacheWriteInputTokens, outputTokens, reasoningOutputTokens,
  totalTokens`. `last` is the most recent response; `total` the thread.
  Aspen reports the neutral `{input, output, cache_read, cache_create,
  reasoning}` per turn from `last`, and folds `token_count` lines for
  the usage page.

## 5. Approval policy, sandbox, posture

| Aspen mode id | approvalPolicy | sandbox | reviewer | posture |
|---|---|---|---|---|
| `on-request` | `on-request` | `workspace-write` | user | ask (and edits) |
| `untrusted` | `untrusted` | `workspace-write` | user | — |
| `read-only` | `on-request` | `read-only` | user | plan |
| `full-access` | `never` | `danger-full-access` | user | auto |
| `auto-review` | `on-request` | `workspace-write` | `auto_review` | guarded |

Per-turn `sandboxPolicy` objects: `{type:"workspaceWrite", writableRoots:
[], networkAccess:false, excludeTmpdirEnvVar:false, excludeSlashTmp:
false}`, `{type:"readOnly", networkAccess:false}`, `{type:
"dangerFullAccess"}`. A mode change takes effect on the next turn (Aspen
sends the policy with every `turn/start`).

What prompts under `on-request` + `workspace-write` (verified): reads
anywhere and writes inside the workspace (and `/tmp`) run silently in
the sandbox; a write outside it fails in the sandbox and the model asks
to rerun outside (`reason: "May I run this outside the sandbox…"`).
Under `untrusted`, every command not on Codex's trusted list asks.

## 6. Server → client requests

### 6.1 `item/commandExecution/requestApproval`

```
{kind:"command"|"network", threadId, turnId, itemId, startedAtMs, environmentId, command:"/bin/bash -lc '…'", cwd, reason?,
 commandActions:[{type, command, …}], proposedExecpolicyAmendment:["cat","/etc/hostname"]?,
 availableDecisions:["accept", {acceptWithExecpolicyAmendment:{execpolicy_amendment:[…]}}, "cancel"]}
```
Answer: `{decision: "accept" | "acceptForSession" |
{acceptWithExecpolicyAmendment:{execpolicy_amendment:[…]}} | "decline" |
"cancel"}`. A decline completes the item with `status:"declined"` and
the model sees "rejected by user". Aspen offers `accept` (once),
`acceptForSession` (session), the amendment (always, labelled with the
script) and `decline`; the console's deny message is not carried to
Codex (the item's status is what the model sees).

### 6.2 `item/fileChange/requestApproval`

`{threadId, turnId, itemId, startedAtMs, reason?, grantRoot?}` — the
change itself was in the preceding `item/started fileChange`. Answer:
`{decision: accept | acceptForSession | decline | cancel}`.

### 6.3 Questions

Two forms. `item/tool/requestUserInput {threadId, turnId, itemId,
questions:[{id, header, question, isOther, isSecret, options:[{label,
description}]|null}], isBlocking, autoResolutionMs}` is answered with
`{answers:{<question id>:{answers:[…]}}}`. In practice (0.153.4) the
model asks **asynchronously**: an `agentMessage` item completes with
`delivery:"async"` and `questions:[{title, options:[…]}]`, then the turn
sleeps (`clock.sleep` 60s) waiting; the answer is ordinary user input
(`turn/steer` into the waiting turn). Aspen surfaces both as one
question prompt and sends the answers back the right way.

### 6.4 `mcpServer/elicitation/request` for MCP tool approvals

An MCP tool call under `on-request` asks through a form elicitation:
`{threadId, turnId, serverName, mode:"form", message:"Allow the aspen
MCP server to run tool \"bus_status\"?", requestedSchema, _meta:
{codex_approval_kind:"mcp_tool_call", persist:["session","always"],
tool_description, tool_params}}`. Answer: `{action:"accept", content:{},
_meta:{persist?: "session"|"always"}}` or `{action:"decline", content:
null, _meta:null}`. Aspen names the request `mcp__<server>__<tool>`, so
the bus tools auto-allow by name exactly as Claude's do. Other
elicitations (a server's own forms/urls) are declined for now.

### 6.5 `item/permissions/requestApproval`

`{threadId, turnId, itemId, cwd, reason, permissions:{network?,
fileSystem?}}` → `{permissions:{…granted…}, scope:"turn"|"session"}`.
Aspen grants what was asked (for the turn, or the session) on allow and
an empty profile on deny.

## 7. The tool bridge

Codex reaches tools only through MCP servers in its config. `aspen mcp`
is a stdio MCP server (newline JSON-RPC: `initialize`, `tools/list`,
`tools/call`, `ping`) that lists the node's bus tools from `GET
/api/bridge/tools?agent=…` and forwards each call to `POST
/api/bridge/call {agent, tool, args}` with the node token in
`x-aspen-token`. Verified: `bus_status`, `bus_send`, `bus_inbox` from a
Codex session; a Codex ↔ Claude round trip on the bus (question and
answer) completes.

## 8. What Aspen maps

| Codex | Aspen event |
|---|---|
| `item/agentMessage/delta` | `TextDelta` |
| `item/reasoning/*Delta` | `TextDelta{thinking}` |
| `item/completed agentMessage` | `AssistantMessage{id, content:[{type:text}]}` (+ a question prompt when `questions` is set) |
| `item/started commandExecution` | `ToolUse{commandExecution, kind: shell\|file_read\|search from commandActions}` |
| `item/completed commandExecution` | `ToolResult{aggregatedOutput, exit code, declined}` |
| `item/started|completed fileChange` | `ToolUse{fileChange, kind: file_write\|file_edit}` / `ToolResult` |
| `item/* mcpToolCall` | `ToolUse{mcp__<server>__<tool>, kind: mcp}` / `ToolResult` |
| `webSearch`, `collabAgentToolCall`, `subAgentActivity`, `imageView` | `ToolUse` kinds web / agent / agent / file_read |
| `turn/completed` | `TurnEnded{subtype: success\|interrupted\|failed, usage}` |
| `thread/tokenUsage/updated` | `Status` + context usage |
| `error`, `warning*` | `Stderr` (+ `Status`) |
| everything else | `Raw` (the source view) |

## 9. Known gaps (backlog)

- `files()` for replication lists the rollout and its history base; the
  fork's parent must exist on the target (Phase 3 places both).
- No per-project memory dir: Codex memory is global (`memories_1.sqlite`).
- Activities: `collabAgentToolCall` / `subAgentActivity` are shown as
  tool cards, not yet in the activity ledger.
- Elicitations from third-party MCP servers are declined.
- The console's deny message is not delivered to Codex (no channel for
  it in the approval response).
