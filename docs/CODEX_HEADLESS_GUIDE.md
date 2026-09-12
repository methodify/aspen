# Driving Codex headless: a field guide

How to run OpenAI Codex as a long-lived, programmatically driven agent
from your own software — the protocol, the process, the files on disk,
the approvals, the traps. Written from Aspen's integration
(`crates/aspen-codex`), where every shape below was observed on the wire
or on disk against `codex-cli 0.153.4` (September 2026), not read from
documentation. The generated protocol schema lives in the Codex source
under `codex-rs/app-server-protocol/schema/typescript/v2`; it is the
authority when this guide and a newer Codex disagree.

This guide is self-contained: it names Aspen only where an example
needs a concrete value. Everything applies to any host.

## 1. The shape of the thing

Codex has three ways to run without a human at a terminal, and only one
of them is a session:

| surface | what it is | use it for |
|---|---|---|
| `codex exec --json` | one prompt, one answer, then exit; approvals are hard-rejected | scripts, CI, one-shot tasks |
| `codex mcp-server` | Codex as an MCP tool for another agent | a `codex` tool inside another harness |
| **`codex app-server --listen stdio://`** | a JSON-RPC 2.0 server over stdin/stdout that hosts *threads*: start, resume, fork, turn by turn, with approvals and questions as server→client requests | **a driven session** — this guide |

The app-server is what the Codex desktop app and IDE extensions speak.
One process serves one thread comfortably; it can serve many, but a
process per session keeps liveness simple (the process is the session).

Wire format: newline-delimited JSON-RPC 2.0, both directions.

- Your requests: `{"id": <int>, "method", "params"}` → `{"id", "result" | "error"}`.
- Notifications from the server: `{"method", "params"}` with no `id`;
  params carry an `emittedAtMs`.
- **Requests from the server**: `{"id", "method", "params"}` that *you*
  must answer with `{"id", "result"}` — approvals, questions,
  elicitations. A host that does not answer these hangs the turn.

Stderr is diagnostics; keep reading it or the pipe fills.

## 2. Launching

```
codex app-server --listen stdio://  [-c key=value ...]
```

Run it with `cwd` = the project. Configuration goes on the command line
as `-c` overrides whose values are TOML literals, so `config.toml` is
never touched by the host:

| override | purpose |
|---|---|
| `projects."<abs cwd>".trust_level="trusted"` | marks the project trusted for this process; without it Codex refuses to work in an untrusted directory. Make your own trust decision first; this override *is* that decision. |
| `mcp_servers.<name>.command="<path>"`, `.args=["..."]`, `.env={ K = "v", ... }`, `.startup_timeout_sec=20` | an MCP server for this session (§8). TOML inline table for `env`. |
| `model="..."` | a default model (or pass it per thread/turn) |

Quote TOML strings yourself (`"` with `\"` and `\\` escapes).

Environment:

- `CODEX_INTERNAL_ORIGINATOR_OVERRIDE=<your-app>` stamps `originator` in
  the rollout's first line (§4) — how you later recognise sessions that
  were yours. **A fork inherits its parent's originator**, so it marks
  lineage, not the current writer.
- `CODEX_HOME` moves the whole state dir (default `~/.codex`): config,
  `sessions/`, `hooks.json`, `memories_*.sqlite`, `plugins/`.
- Pass through whatever your MCP server needs (see the `env` override,
  which is per server, versus process env, which the server also sees).

Version probe: `codex --version` prints `codex-cli X.Y.Z`. It is a Node
start on Windows and takes seconds — probe off the request path.

Windows: spawn with `CREATE_NO_WINDOW` (0x08000000) or a console flashes
for every probe.

## 3. Handshake, thread, turn

```
→ initialize   {clientInfo:{name, title, version}, capabilities:null}
← {userAgent, codexHome, platformFamily, platformOs}
→ initialized  (notification, empty params)

→ thread/start {cwd, approvalPolicy, approvalsReviewer?, sandbox, model?, developerInstructions?}
← {thread:{id, path, model, cwd, cliVersion, source, forkedFromId, ...}, model, approvalPolicy, sandbox:{type}, ...}
← thread/started
← mcpServer/startupStatus/updated {threadId, name, status: starting|ready|failed|cancelled, error?}   (per MCP server)
```

- `developerInstructions` is your system-level charter for the thread;
  it lands in the rollout as a `developer` message and is honoured
  across resumes.
- `thread/resume {threadId, cwd, approvalPolicy, sandbox, developerInstructions?}`
  brings a thread back with its history (verified: it recalls earlier
  turns). `thread/fork {threadId, ...}` makes a *new* thread whose
  rollout points at the parent's (§4).
- Inventory, optional: `model/list {includeHidden:false}` →
  `{data:[{model, displayName, description, isDefault, ...}]}`;
  `skills/list {cwds:[cwd]}` → `{data:[{cwd, skills:[{name,
  description, shortDescription, path, scope, enabled}]}]}`.

A turn:

```
→ turn/start {threadId, input:[{type:"text", text, text_elements:[]}], clientUserMessageId?, model?, approvalPolicy?, approvalsReviewer?, sandboxPolicy?}
← {turn:{id, status:"inProgress"}}
← turn/started {threadId, turn}
← item/started    {item:{type:"userMessage", ...}} / item/completed
← item/started    {item:{type:"agentMessage", id, text:"", phase}}
← item/agentMessage/delta {itemId, delta}   ...repeated
← item/completed  {item:{type:"agentMessage", id, text, phase: commentary|final_answer, delivery?, questions?}}
← item/started    {item:{type:"commandExecution", id, command, cwd, commandActions:[...], status:"inProgress"}}
← item/completed  {item:{..., status: completed|failed|declined, aggregatedOutput, exitCode, durationMs}}
← item/started/completed {item:{type:"fileChange", id, changes:[{path, kind:{type:add|update|delete}, diff}], status}}
← item/started/completed {item:{type:"mcpToolCall", id, server, tool, arguments, status, result|error}}
← item/reasoning/textDelta | summaryTextDelta   (reasoning, if the model emits it)
← turn/diff/updated {diff}                         (cumulative unified diff for the turn)
← thread/tokenUsage/updated {tokenUsage:{total, last, modelContextWindow}}
← thread/status/changed {status:{type: active|idle}}
← turn/completed {turn:{id, status: completed|interrupted|failed, error?, items:[...], durationMs, startedAt, completedAt}}
```

Input blocks: `text` (with `text_elements: []`), `image` with a
`url` (`data:image/png;base64,...` works), `localImage {path}`. A skill
mention is an extra block beside the text.

Other item types you will see: `webSearch`, `collabAgentToolCall` and
`subAgentActivity` (Codex's own sub-agents), `imageView`, `reasoning`,
and `extension` items such as `clock.sleep` (§6). Treat unknown item
types as opaque and keep going.

Control:

- `turn/interrupt {threadId, turnId}` → the turn ends with
  `status:"interrupted"`; no partial assistant message is kept.
- `turn/steer {threadId, input, expectedTurnId, clientUserMessageId?}`
  injects into a *running* turn. It is refused if the turn just ended;
  fall back to `turn/start`. This is also how you answer an async
  question (§6.3).
- Model and policy changes are per turn: send them with the next
  `turn/start`. `thread/settings/updated {threadSettings:{model, ...}}`
  tells you what took effect; `model/rerouted` when the service
  substituted a model.
- `thread/compacted` when the context was compacted.
- Token usage: `{inputTokens, cachedInputTokens, cacheWriteInputTokens,
  outputTokens, reasoningOutputTokens, totalTokens}`; `last` is the most
  recent response, `total` the thread. There is no cost figure.

To end a session: interrupt any running turn (short timeout), then
kill the process. There is no graceful "thread/close"; the rollout is
complete at every line.

## 4. Rollouts on disk

`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<YYYY-MM-DDThh-mm-ss>-<thread-id>.jsonl`.
The thread id is the tail of the filename; the directory is the day the
thread started. Every line: `{timestamp, ordinal, type, payload}`.

| line type | payload |
|---|---|
| `session_meta` (first line) | `{id, cwd, originator, cli_version, source, forked_from_id?, history_base?: {thread_id, end_ordinal_exclusive, end_byte_offset}, base_instructions, model_provider, ...}` |
| `turn_context` | `{turn_id, cwd, approval_policy, sandbox_policy, model, ...}` — **the model in use**, per turn |
| `event_msg` | `{type: task_started {turn_id, model_context_window} \| item_completed {item} \| token_count {info:{total_token_usage, last_token_usage, model_context_window}, rate_limits} \| task_complete {turn_id, last_agent_message, duration_ms} \| thread_settings_applied}` |
| `response_item` | the raw model API items: `message {role: developer\|user\|assistant, content:[{type: input_text\|output_text, text}]}`, `custom_tool_call {name:"exec", input}`, `custom_tool_call_output`, `reasoning` |
| `world_state`, `token_usage_record` | environment snapshot; per-response usage |

`item_completed` items on disk use PascalCase types and snake_case
fields, unlike the wire (camelCase): `UserMessage {id, content:[{type:
"text", text}]}`, `AgentMessage {id, content:[{type:"Text", text}],
phase, delivery?, questions?}`, `CommandExecution {id, command:
["/bin/bash","-lc","<script>"], cwd:"file://...", parsed_cmd, status,
aggregated_output, exit_code, duration}`, `FileChange {id, changes:
{"<path>": {type, content|unified_diff}}, status}`, `Reasoning`,
`Extension`.

**Reconstructing a transcript**: read `event_msg/item_completed` lines
in order; `turn_context` gives the model; `token_count` gives usage.
For a fork, first read the parent's lines with `ordinal <
history_base.end_ordinal_exclusive` (recursively, a fork of a fork),
then the fork's own. Keep the parent file around: a fork's rollout is
not self-contained.

**Finding a project's sessions**: there is no per-project index; scan
`sessions/` and match `session_meta.cwd` (canonicalise both sides —
symlinks, case on Windows). Cache the first line per file by mtime.
Files are appended while the session runs; read them whole each time,
tolerating a half-written last line.

**Which model answered**: `turn_context.model` per turn on disk;
`thread/settings/updated` and the `thread/start` response on the wire.

Codex's own tool use is *code mode*: one `exec` custom tool call runs a
script that calls `tools.exec_command`, `tools.apply_patch`, …; the
app-server still emits one `commandExecution` / `fileChange` item per
underlying action, which is what you show.

## 5. Approval policy, sandbox, and what actually prompts

Two knobs, set at thread start and overridable per turn:

- `approvalPolicy`: `untrusted` (ask for anything not on Codex's
  trusted list), `on-request` (the model asks when it must),
  `on-failure`, `never`.
- `sandbox` (thread level, a string): `read-only | workspace-write |
  danger-full-access`. Per turn it is a `sandboxPolicy` **object**:
  `{type:"workspaceWrite", writableRoots:[], networkAccess:false,
  excludeTmpdirEnvVar:false, excludeSlashTmp:false}`,
  `{type:"readOnly", networkAccess:false}`, `{type:"dangerFullAccess"}`.
- `approvalsReviewer`: `user` (you get the requests) or `auto_review`
  (Codex reviews its own proposed actions).

Combinations that make sense as user-facing modes:

| mode | approvalPolicy | sandbox | reviewer |
|---|---|---|---|
| ask when needed | `on-request` | `workspace-write` | user |
| ask for everything | `untrusted` | `workspace-write` | user |
| plan / read only | `on-request` | `read-only` | user |
| full auto | `never` | `danger-full-access` | user |
| guarded auto | `on-request` | `workspace-write` | `auto_review` |

What prompts under `on-request` + `workspace-write` (verified): reads
anywhere and writes inside the workspace and `/tmp` run silently in the
sandbox; a write outside fails in the sandbox and the model then asks to
rerun outside it (`reason: "May I run this outside the sandbox…"`).
Network is off in the sandbox unless `networkAccess:true`; the model asks
for it with `kind:"network"` (§6.1).

## 6. Server → client requests: what you must answer

Every one of these arrives as a JSON-RPC request with an `id`. Answer
with `{"id": <same>, "result": {...}}`. Not answering hangs the turn.

### 6.1 `item/commandExecution/requestApproval`

```
{kind:"command"|"network", threadId, turnId, itemId, startedAtMs, environmentId,
 command:"/bin/bash -lc '…'", cwd, reason?,
 commandActions:[{type: read|listFiles|search|unknown, command, ...}],
 proposedExecpolicyAmendment:["cat","/etc/hostname"]?,
 availableDecisions:["accept", {acceptWithExecpolicyAmendment:{execpolicy_amendment:[…]}}, "cancel"]}
```

Answer `{decision: "accept" | "acceptForSession" |
{acceptWithExecpolicyAmendment:{execpolicy_amendment:[...]}} | "decline"
| "cancel"}`. `acceptForSession` stops further prompts for the same
command this session; the amendment adds a rule to Codex's exec policy
("always allow this shape"). A decline completes the item with
`status:"declined"` and the model sees "rejected by user" — there is no
channel for a deny *message*; put it in your next user input if it
matters. `commandActions` is Codex's own parse of the script — use it to
classify (read / search / list / other) without parsing shell yourself.

### 6.2 `item/fileChange/requestApproval`

`{threadId, turnId, itemId, startedAtMs, reason?, grantRoot?}`. The
change itself came in the preceding `item/started fileChange`. Answer
`{decision: accept | acceptForSession | decline | cancel}`.

### 6.3 Questions — two forms

**Synchronous**: `item/tool/requestUserInput {threadId, turnId, itemId,
questions:[{id, header, question, isOther, isSecret, options:[{label,
description}] | null}], isBlocking, autoResolutionMs}` → answer
`{answers: {<question id>: {answers: [<string>, ...]}}}`.

**Asynchronous** (what 0.153.4 actually does most of the time): an
`agentMessage` item completes with `delivery:"async"` and
`questions:[{title, options:[...]}]`, then the turn *sleeps* — an
`extension` item of kind `clock.sleep` (60 s) — waiting for the user.
The answer is ordinary user input: `turn/steer` into the waiting turn
(or `turn/start` if it has ended). Surface both forms as one "question"
to your users and route the answer by which form asked.

### 6.4 `mcpServer/elicitation/request` — MCP tool approvals

Under `on-request`, an MCP tool call asks via a *form elicitation*:

```
{threadId, turnId, serverName, mode:"form",
 message:"Allow the <server> MCP server to run tool \"<tool>\"?",
 requestedSchema, _meta:{codex_approval_kind:"mcp_tool_call", persist:["session","always"], tool_description, tool_params}}
```

Answer `{action:"accept", content:{}, _meta:{persist?: "session" |
"always"}}` or `{action:"decline", content:null, _meta:null}`.
`_meta.codex_approval_kind` is how you tell this apart from a server's
own elicitation (a form or URL a third-party MCP server asks the user to
fill) — those carry no such meta; decline them unless you render forms.

### 6.5 `item/permissions/requestApproval`

`{threadId, turnId, itemId, cwd, reason, permissions:{network?,
fileSystem?}}` → `{permissions:{...what you grant...}, scope:"turn" |
"session"}`. Grant what was asked or an empty profile.

## 7. Hooks

`$CODEX_HOME/hooks.json`, and `.codex/hooks.json` in a project, use the
same shape as Claude Code's: `{hooks: {SessionStart: [{hooks: [{type:
"command", command}]}]}}`. Events: SessionStart, SessionEnd,
UserPromptSubmit, PreToolUse, PostToolUse, PermissionRequest,
PreCompact, PostCompact, SubagentStart, SubagentStop, Stop, Interrupt.
The SessionStart payload on stdin carries `cwd, session_id, source,
hook_event_name, model, permission_mode, transcript_path`. A hook is a
way to learn about sessions you did *not* start (adoption); for your
own, the protocol already tells you everything.

Project-level surfaces worth showing a user before trusting a
directory: `.codex/hooks.json`, `[hooks]` and `[mcp_servers.*]` in
`.codex/config.toml`, skills under `.codex/skills` and `.agents/skills`.

## 8. Giving Codex tools: an MCP server over stdio

Codex reaches host tools only through MCP servers in its config. The
smallest useful server is a stdio process speaking newline JSON-RPC that
answers `initialize`, `tools/list`, `tools/call` and `ping`, and ignores
notifications (`notifications/initialized` has no id; send nothing
back). Register it with the `mcp_servers.<name>.*` overrides (§2), pass
what it needs in `.env`, and keep its startup fast (Codex waits
`startup_timeout_sec`).

Tool names surface to the model as-is; when an MCP tool is called you
see `item/started mcpToolCall {server, tool, arguments}` and, under
`on-request`, the elicitation in §6.4. Naming your tools consistently
(`<server>__<tool>` in your own approval rules) lets you auto-allow
your own bridge while prompting for others.

Status and control of MCP servers:

| request | shape |
|---|---|
| `mcpServerStatus/list {threadId?, detail?: full \| toolsAndAuthOnly, cursor?, limit?}` | `{data:[{name, runtimeStatus: notStarted\|starting\|connected\|authenticationRequired\|failed\|cancelled\|disabled, pluginId?, serverInfo?, tools:{name → Tool}, toolsError?, resources[], resourceTemplates[], authStatus: unknown\|unsupported\|notLoggedIn\|bearerToken\|oAuth}]}` |
| `config/mcpServer/reload` | re-read config, restart every server (there is no per-server reconnect, and no per-server enable/disable — disabling is a config edit) |
| `mcpServer/oauth/login {name}` → notification `mcpServer/oauthLogin/completed` | the OAuth flow; the result carries the authorization URL |
| `mcpServer/startupStatus/updated` (push) | per server, at thread start and after reload — the signal to re-list |

## 9. Errors and warnings

Notifications `error {message, willRetry?}`, `warning`,
`guardianWarning`, `configWarning`, `deprecationNotice` carry a
`message`; log them and show them. A `turn/completed` with
`status:"failed"` has `error`. Process exit without a turn end is the
session's death: watch the child.

## 10. Traps, in the order we hit them

1. **Not answering a server request** hangs the turn silently. Answer
   everything, even with a decline.
2. **`codex exec` for a session.** It is one-shot and rejects
   approvals; use the app-server.
3. **Writing `config.toml` for trust or servers.** Overrides on the
   command line are per process and leave the user's file alone.
4. **Wire vs. disk casing.** `commandExecution` on the wire,
   `CommandExecution` with `aggregated_output` on disk. Two parsers.
5. **Forks are not self-contained.** Read `history_base` and keep the
   parent file when copying a session elsewhere.
6. **Originator marks lineage.** A fork made by someone else still says
   it is yours; keep your own registry of thread ids.
7. **Questions are usually async.** Watch for `delivery:"async"` on an
   agent message plus a `clock.sleep`; answer with `turn/steer`.
8. **`turn/steer` can be refused** when the turn has just ended; fall
   back to `turn/start`.
9. **Sandbox object vs. string.** Thread-level `sandbox` is a string,
   turn-level `sandboxPolicy` is an object.
10. **No per-server MCP reconnect or toggle.** Reload is global; tell
    the user so.
11. **Slow `--version` and console flashes on Windows.** Probe on a
    thread, spawn without a window.
12. **Memory is global.** `memories_*.sqlite` under `CODEX_HOME`, not
    per project — a session moved to another machine leaves its memory
    behind unless you move that too.

## 11. A minimal driver, in prose

Spawn `codex app-server --listen stdio://` in the project directory
with the trust override and your MCP server. Send `initialize`, then
`initialized`. Send `thread/start` with cwd, `on-request`,
`workspace-write`, and your charter as `developerInstructions`; keep
the returned thread id — it is the session's identity, on disk and for
resume. For each user message send `turn/start` (or `turn/steer` while
a turn runs). Stream `item/agentMessage/delta` to the screen; render
`commandExecution` and `fileChange` items as they start and complete;
answer every `requestApproval`, `requestUserInput` and `elicitation`
request from your own policy or your user; watch for async questions;
finish on `turn/completed`. To stop, `turn/interrupt` then kill. To come
back later, `thread/resume` with the same id in the same cwd. To show
history without a process, read the rollout.
