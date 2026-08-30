# Claude Code Host Protocol

**How the VS Code extension drives the `claude` binary, and how to build your own GUI shell on the same contract.**

> Derived by reading the source tree at `C:\temp\claude-code-fork-main` (no `package.json` in the drop; date the findings by the tree, not a version string). Every claim below is anchored to `file:line`. See §15 for known drift and how to feature-detect against a newer build, and §17 for what is already documented publicly.

---

## 1. Executive summary

When you open a Claude Code session in VS Code, the extension spawns `claude` as a **headless child process** and talks to it over **newline-delimited JSON on stdin/stdout**. There is no socket, no named pipe, no HTTP server, and no custom binary framing in the primary path.

The protocol has two interleaved layers on the same two pipes:

| Layer | Direction | Purpose |
|---|---|---|
| **Message stream** | CLI → host (stdout), host → CLI (stdin) | The conversation itself: user turns in, assistant/tool/result events out |
| **Control protocol** | Bidirectional, request/response with `request_id` correlation | Everything else: initialize, permissions, interrupt, model switching, MCP management, auth, settings, context introspection |

On top of the control layer sits a third, crucial trick: the **host can expose an MCP server to the CLI over the control channel**. The VS Code extension does exactly this under the server name `claude-vscode` (`src/services/mcp/vscodeSdkMcp.ts:65`). That is how the editor gives Claude editor-native capabilities without the CLI ever opening a socket back to the editor — and it is the single most important mechanism for anyone building a rival shell.

**The whole thing is the public Agent SDK protocol plus a private superset of control subtypes.** If you have used `@anthropic-ai/claude-agent-sdk` or `claude-agent-sdk` (Python), you already know 70% of this. The remaining 30% is documented in §9 and §11.

### The one-line version

```
claude --print --input-format stream-json --output-format stream-json --verbose
```

…with `CLAUDE_CODE_ENTRYPOINT=claude-vscode` in the environment, then speak NDJSON.

---

## 2. What this is NOT

Ruling these out saves you a week of chasing the wrong thing:

- **Not the IDE lockfile/WebSocket mechanism.** `~/.claude/ide/<port>.lock` (`src/utils/ide.ts:73-100`, `:298`, `:462`) is the *opposite* direction: it is how a **terminal-hosted** `claude` discovers an already-running editor and connects to it as an MCP **client**. It exists for the "I ran `claude` in the VS Code integrated terminal" case. It is not how the native sidecar is driven. See §13 if you want it anyway.
- **Not `claude server` / `cc://`.** There is a real HTTP/session-server mode (`src/main.tsx:3962`, `src/main.tsx:4059`, `src/server/`), but it is behind the `DIRECT_CONNECT` build feature and its implementation files (`server/server.js`, `server/sessionManager.js`) are not even present in this drop. Interesting, not load-bearing.
- **Not the bridge / Remote Control transports.** `src/bridge/**` and `src/cli/transports/{WebSocket,SSE,Hybrid}Transport.ts` are for claude.ai Remote Control (phone/web driving a local session). They *reuse* the same control-protocol envelopes, which is why you will see them referenced in `print.ts` — but they are a different product surface.
- **Not `--sdk-url`.** `src/main.tsx:3861` registers a hidden `--sdk-url <url>` that swaps stdio for a remote WebSocket carrying the identical message vocabulary. Useful to know exists (§14), irrelevant to a local GUI.

---

## 3. Launching the sidecar

### 3.1 Required argv

| Flag | Why | Source |
|---|---|---|
| `-p` / `--print` | Selects headless (non-REPL) mode. Without it you get the Ink TUI. | `src/main.tsx:976` |
| `--input-format stream-json` | Read NDJSON turns from stdin instead of a single prompt string | `src/main.tsx:976` |
| `--output-format stream-json` | Emit NDJSON events on stdout | `src/main.tsx:976` |
| `--verbose` | **Mandatory** with `stream-json` output — the CLI hard-exits without it | `src/cli/print.ts:787-790` |

Three validations will kill your process at startup if you get this wrong:

```ts
// src/main.tsx:1823
if (inputFormat === 'stream-json' && outputFormat !== 'stream-json') {
  console.error(`Error: --input-format=stream-json requires output-format=stream-json.`);
  process.exit(1);
}
```

```ts
// src/cli/print.ts:787
if (options.outputFormat === 'stream-json' && !options.verbose) {
  process.stderr.write('Error: When using --print, --output-format=stream-json requires --verbose\n')
```

Note the asymmetry: `--input-format=stream-json` forces `--output-format=stream-json`, but not vice versa. A read-only observer can use output-only streaming.

### 3.2 Strongly recommended argv

| Flag | Effect |
|---|---|
| `--include-partial-messages` | Emits `stream_event` messages carrying raw Anthropic SSE deltas → token-by-token rendering. Requires `--print` + `stream-json` output (`src/main.tsx:1848`). |
| `--replay-user-messages` | The CLI echoes each user message back on stdout with `isReplay: true` once accepted. This is your **delivery acknowledgement** — without it you cannot distinguish "queued" from "lost". Requires both formats to be `stream-json` (`src/main.tsx:1840`). |
| `--session-id <uuid>` | Pre-assign the session UUID instead of discovering it from the `system/init` message. Makes transcript correlation deterministic. |
| `--permission-mode <mode>` | Initial mode; can be changed later via `set_permission_mode`. |
| `--include-hook-events` | Surfaces `hook_started` / `hook_progress` / `hook_response` system messages so your UI can show hook activity. |
| `--fork-session` | With `--resume`, branch to a new session ID rather than appending to the original. |
| `-r <session-id>` / `-c` | Resume / continue. See §10.3. |

**Flags to deliberately *not* pass:** `--bare`, `--strict-mcp-config`, and `--setting-sources`. Public guidance recommends `--bare` for scripted callers, and it is slated to become the `-p` default — but it disables exactly the repo-provided plugins, MCP servers, hooks, skills, and `CLAUDE.md` that a GUI host exists to surface. See §12.1, and §12.5 for the trust obligation that omitting it creates.

### 3.3 Environment

```
CLAUDE_CODE_ENTRYPOINT=claude-vscode
```

This is the single most important env var and it is **not** a cosmetic analytics tag. `initializeEntrypoint()` early-returns if `CLAUDE_CODE_ENTRYPOINT` is already set (`src/main.tsx:518-520`), so setting it prevents the CLI from self-labelling as `sdk-cli`. Downstream it changes behaviour:

- `clientType` becomes `'claude-vscode'` (`src/main.tsx:823`), which suppresses the "non-interactive session" treatment in at least one place (`src/bootstrap/state.ts:1236`).
- It gates the `file_updated` notification back to your MCP server — though note in *this* drop that path is additionally gated on `USER_TYPE === 'ant'` (`src/services/mcp/vscodeSdkMcp.ts:44`), so a public build will not send it.
- It affects tool-registry decisions (`src/utils/embeddedTools.ts:17-21`), the setup/onboarding path (`src/setup.ts:421`), beta headers (`src/utils/betas.ts:244`), and the `User-Agent` (`src/utils/http.ts:34`).

**Recommendation for your own shell:** pick your own stable identifier (e.g. `CLAUDE_CODE_ENTRYPOINT=my-app`) rather than impersonating `claude-vscode`. You will forfeit the handful of vscode-specific branches above, but you avoid inheriting behaviour tuned for an editor you are not. If you find you need a specific vscode-gated branch, flip to `claude-vscode` deliberately and note why.

Other env worth knowing:

| Var | Effect | Source |
|---|---|---|
| `CLAUDE_CODE_INCLUDE_PARTIAL_MESSAGES` | Same as the flag | `src/main.tsx:1226` |
| `CLAUDE_CODE_SSE_PORT` | Pins IDE-lockfile discovery to a specific port (§13) | `src/utils/ide.ts:671` |
| `CLAUDE_CONFIG_DIR` | Relocates `~/.claude` | `src/utils/envUtils.ts:7-14` |
| `CLAUDE_CODE_SYNC_PLUGIN_INSTALL` / `..._TIMEOUT_MS` | Block the first turn until plugins install | `src/cli/print.ts:1886-1907` |

### 3.4 Working directory

The child's `cwd` is the project root. There is no flag that sets it; set it on `spawn()`. Additional roots go through `--add-dir <dirs...>`.

---

## 4. Wire format

**Newline-delimited JSON. One complete JSON value per line, `\n`-terminated, UTF-8.**

The reader is a straightforward incremental line splitter (`src/cli/structuredIO.ts:215-261`): it accumulates chunks, splits on `\n`, and parses each line. A trailing partial line at EOF is parsed as a final message.

Three properties matter for implementers:

1. **A parse failure is fatal.** `processLine` catches, logs to stderr, and calls `process.exit(1)` (`src/cli/structuredIO.ts:457-462`). There is no resynchronisation. Never write a partial line; never write two JSON values on one line.
2. **Empty lines are ignored** (`src/cli/structuredIO.ts:337`), so double-`\n` is harmless.
3. **stdout is guarded but not guaranteed.** Because stray library output would corrupt the stream, the CLI installs `installStreamJsonStdoutGuard()` when `--output-format stream-json` is set, diverting non-JSON lines to stderr (`src/cli/print.ts:594-596`). Trust it, but still skip lines that fail to parse rather than crashing — and **read stderr separately**; it carries real diagnostics.

Outbound writes go through `ndjsonSafeStringify` (`src/cli/structuredIO.ts:466`), which is where lone surrogates and similar hazards are handled.

### 4.1 Key-casing compatibility

`normalizeControlMessageKeys` (`src/utils/controlMessageCompat.ts`) rewrites a camelCase `requestId` to `request_id` on inbound control messages, at both the top level and inside `response`. Snake_case wins if both are present. This shim exists for old iOS builds — **do not rely on it.** Emit `request_id`.

Everything else in the protocol is snake_case at the envelope level (`session_id`, `parent_tool_use_id`, `tool_use_id`) but **camelCase inside permission payloads** (`updatedInput`, `updatedPermissions`, `toolUseID` — note the capital ID). This inconsistency is real; see §8.

---

## 5. Stdin vocabulary (host → CLI)

Authoritative union: `StdinMessageSchema`, `src/entrypoints/sdk/controlSchemas.ts:655-663`.

| `type` | Meaning |
|---|---|
| `user` | A user turn. The workhorse. |
| `control_request` | You are asking the CLI to do something. |
| `control_response` | You are answering a CLI-initiated control request (permissions, hooks, MCP). |
| `keep_alive` | Silently discarded (`src/cli/structuredIO.ts:344`). For WebSocket transports. |
| `update_environment_variables` | Mutates the child's `process.env` live (`src/cli/structuredIO.ts:348-360`). Used for token refresh. |

Additionally accepted but not in the public union — the loop takes `assistant` and `system` messages on stdin and splices them into `mutableMessages` as conversation context (`src/cli/print.ts:4042-4050`). This is the history-replay path used by the bridge. **You can use it to inject prior turns into a fresh process** rather than resuming from disk.

### 5.1 The `user` message

```jsonc
{
  "type": "user",
  "message": { "role": "user", "content": "Refactor the auth module" },
  "parent_tool_use_id": null,
  "uuid": "…",            // optional but you want it — see below
  "session_id": "…",      // optional; CLI uses its own
  "priority": "now",      // optional: "now" | "next" | "later"
  "timestamp": "2026-…",  // optional ISO8601
  "isSynthetic": false    // optional
}
```

`content` accepts either a plain string or the full Anthropic content-block array (so you can send images, documents, etc.).

**Send a `uuid`.** Three things depend on it:

1. **Deduplication.** Before enqueueing, the CLI checks both the on-disk transcript and an in-memory set; a duplicate is dropped (`src/cli/print.ts:4062-4100`). This makes reconnect-and-resend safe.
2. **Acknowledgement.** With `--replay-user-messages`, the echo carries your uuid, closing the loop.
3. **Cancellation.** `cancel_async_message` addresses a queued message by uuid (§9).

**Batching semantics you must design around:** consecutive queued prompt-mode messages are *coalesced into a single turn* (`src/cli/print.ts:1949-1961`, `canBatchWith` at `:443`). If the user types three messages while a turn is running, the model sees one merged turn, and only the last uuid survives as the turn's identity. The CLI emits replay acks for the others so your UI can still mark all three delivered (`src/cli/print.ts:1969-1982`). Render accordingly — do not assume 1 message in = 1 turn out.

### 5.2 Slash commands work

Send `/compact`, `/clear`, `/model sonnet`, `/cost`, or any project/plugin command as ordinary `user` message text. Headless input flows through the same `processUserInput` pipeline as the TUI (`src/QueryEngine.ts:70-71`, `:416`), including `handlePromptSubmit`'s slash dispatch (`src/utils/handlePromptSubmit.ts:229`). Output arrives as `system` / `local_command_output` (§6.4).

The set of available commands is handed to you in the `initialize` response (§7), so you can build a real autocomplete.

There is a `skipSlashCommands: true` flag on the internal queue used by the bridge (`src/cli/print.ts:3927`) to force literal treatment, but it is **not reachable from a stdin `user` message** in this drop. If you need literal text that begins with `/`, escape it at the application level.

---

## 6. Stdout vocabulary (CLI → host)

Authoritative union: `StdoutMessageSchema`, `src/entrypoints/sdk/controlSchemas.ts:642-653`, which is `SDKMessageSchema` (`src/entrypoints/sdk/coreSchemas.ts:1854-1881`) plus control frames.

Every message carries `uuid` and `session_id`.

### 6.1 Conversation

| type / subtype | Payload | Notes |
|---|---|---|
| `system` / `init` | `tools[]`, `mcp_servers[]`, `model`, `cwd`, `permissionMode`, `slash_commands[]`, `skills[]`, `agents[]`, `plugins[]`, `output_style`, `apiKeySource`, `claude_code_version`, `betas[]` | **First message. Grab `session_id` here.** `coreSchemas.ts:1457` |
| `assistant` | `message` = raw Anthropic assistant message (text / thinking / tool_use blocks), `parent_tool_use_id`, optional `error` | `coreSchemas.ts:1347` |
| `user` | Tool results come back as user messages with `tool_result` blocks; also replays (`isReplay: true`) | `coreSchemas.ts:1290`, `:1297` |
| `stream_event` | Raw Anthropic SSE event in `event` — only with `--include-partial-messages` | `coreSchemas.ts:1496` |
| `result` | End of turn. `subtype: "success"` or one of `error_during_execution` / `error_max_turns` / `error_max_budget_usd` / `error_max_structured_output_retries`. Carries `duration_ms`, `num_turns`, `total_cost_usd`, `usage`, `modelUsage`, `permission_denials[]`, `stop_reason`, and `result` (final text on success). | `coreSchemas.ts:1407`, `:1428` |

`parent_tool_use_id` is your subagent discriminator: non-null means the message came from inside a Task/agent invocation. Nest your UI on it.

### 6.2 Turn / session lifecycle

| type / subtype | Use |
|---|---|
| `system` / `session_state_changed` | `state: "idle" \| "running" \| "requires_action"`. The schema explicitly calls `idle` the *authoritative turn-over signal* — it fires after held-back results flush and the background-agent loop exits (`coreSchemas.ts:1735-1747`). **Drive your spinner off this, not off `result`.** |
| `system` / `status` | `status: "compacting" \| null`, plus `permissionMode` on mode changes. Emitted centrally whenever *any* code path mutates permission mode (`src/cli/print.ts:1060-1079`) — Shift+Tab equivalents, `/plan`, ExitPlanMode, rewind, control request. Mirror it into your mode indicator. |
| `system` / `compact_boundary` | `compact_metadata.trigger` (`manual`/`auto`), `pre_tokens`, and optional `preserved_segment.{head,anchor,tail}_uuid` | 
| `system` / `api_retry` | `attempt`, `max_retries`, `retry_delay_ms`, `error_status` (null for connection errors) — show "retrying…" instead of appearing hung |
| `rate_limit_event` | Full `rate_limit_info` (5-hour / 7-day windows, overage state, `resetsAt`, `utilization`). Emitted on every status change including back to `allowed`, so you can clear warnings (`src/cli/print.ts:1129-1140`). |

### 6.3 Tools, tasks, agents

| type / subtype | Use |
|---|---|
| `tool_progress` | `tool_use_id`, `tool_name`, `elapsed_time_seconds` — long-running tool spinner |
| `system` / `task_started` | `task_id`, `description`, `task_type`, optional `workflow_name`, `prompt` |
| `system` / `task_progress` | `usage.{total_tokens,tool_uses,duration_ms}`, `last_tool_name`, `summary` |
| `system` / `task_notification` | Terminal: `status: completed\|failed\|stopped`, `output_file`, `summary` |
| `tool_use_summary` | Human-readable roll-up over `preceding_tool_use_ids[]` |
| `system` / `post_turn_summary` | Background-generated: `status_category` (`blocked`/`waiting`/`completed`/`review_ready`/`failed`), `title`, `description`, `recent_action`, `needs_action`, `artifact_urls[]`, `is_noteworthy`, and `summarizes_uuid`. Excellent material for a sidebar. |

### 6.4 Misc

| type / subtype | Use |
|---|---|
| `system` / `local_command_output` | Rendered output of a local slash command (`/cost`, `/voice`) — display as assistant-style text |
| `system` / `hook_started`, `hook_progress`, `hook_response` | Hook lifecycle (needs `--include-hook-events` or `--verbose`; see `src/cli/print.ts:628`) |
| `system` / `files_persisted` | Uploaded-file results |
| `system` / `elicitation_complete` | URL-mode MCP elicitation finished |
| `prompt_suggestion` | Predicted next user prompt (opt in via `initialize`) |
| `auth_status` | AWS auth progress (needs `--enable-auth-status`) |
| `keep_alive` | Ignore |

### 6.5 Streamlined mode — ignore it

`streamlined_text` and `streamlined_tool_use_summary` (`coreSchemas.ts:1369`, `:1384`, transform in `src/utils/streamlinedTransform.ts`) are a deliberately lossy, "distillation-resistant" format: text kept, thinking dropped, tool calls collapsed to counts. Built for a different consumer. A GUI wants the full stream.

---

## 7. The control protocol

### 7.1 Envelopes

Request (either direction):

```jsonc
{ "type": "control_request", "request_id": "<uuid>", "request": { "subtype": "…", /* … */ } }
```

Response (either direction):

```jsonc
{ "type": "control_response",
  "response": { "subtype": "success", "request_id": "<uuid>", "response": { /* … */ } } }

{ "type": "control_response",
  "response": { "subtype": "error", "request_id": "<uuid>", "error": "message",
                "pending_permission_requests": [ /* SDKControlRequest[] */ ] } }
```

Cancellation:

```jsonc
{ "type": "control_cancel_request", "request_id": "<uuid>" }
```

Schemas: `src/entrypoints/sdk/controlSchemas.ts:578-619`. Emitters: `src/cli/print.ts:2736-2762`.

### 7.2 Invariants you must honour

1. **Always respond.** An unmatched `request_id` is silently discarded (`src/cli/structuredIO.ts:374-399`); the peer's promise hangs until the stream closes. The CLI is disciplined about this — its unknown-subtype fallback deliberately returns an error rather than staying quiet (`src/cli/print.ts:4021-4027`). Do the same.
2. **The CLI's stdin reader is serial.** One message is processed at a time. A handler that blocks on a response that can only arrive via a *later* stdin message deadlocks. The CLI hit this itself and fixed it by detaching the await — the comment at `src/cli/print.ts:3627-3631` is worth reading. Keep your own reader non-blocking for the same reason.
3. **Duplicate responses are tolerated but not free.** The CLI tracks resolved `tool_use_id`s in a 1000-entry LRU to swallow duplicate `can_use_tool` answers (`src/cli/structuredIO.ts:133`, `:176-187`) — because re-processing them would push duplicate assistant messages and earn a 400 from the API. Don't send them.
4. **Requests and stream events share one FIFO.** `structuredIO.outbound` is the only writer to stdout, so a `control_request` cannot overtake queued `stream_event`s (`src/cli/structuredIO.ts:162`). Ordering is meaningful.
5. **Closing stdin rejects everything in flight** with `Tool permission stream closed before response received` (`src/cli/structuredIO.ts:254-260`).

---

## 8. The two request directions

### 8.1 CLI → host (you must implement these)

Only four subtypes ever originate at the CLI:

| Subtype | When | You must reply with |
|---|---|---|
| `can_use_tool` | A tool needs permission | `{ behavior: "allow", updatedInput, updatedPermissions?, decisionClassification? }` or `{ behavior: "deny", message, interrupt? }` |
| `hook_callback` | An in-process hook you registered fires | A `HookJSONOutput` object (`{}` is valid) |
| `mcp_message` | JSON-RPC for an MCP server you host | `{ mcp_response: <JSONRPCMessage> }` |
| `elicitation` | An MCP server wants user input | `{ action: "accept"\|"decline"\|"cancel", content? }` |

#### `can_use_tool` in detail

```jsonc
{ "type": "control_request", "request_id": "…",
  "request": {
    "subtype": "can_use_tool",
    "tool_name": "Bash",
    "input": { "command": "rm -rf build" },
    "tool_use_id": "toolu_…",
    "permission_suggestions": [ /* PermissionUpdate[] */ ],
    "blocked_path": "…",        // optional
    "decision_reason": "…",     // optional, human-readable
    "title": "…",               // optional
    "display_name": "…",        // optional
    "description": "…",         // optional
    "agent_id": "…"             // optional — non-null means a subagent asked
  } }
```

Schema `controlSchemas.ts:106-122`; construction `src/cli/structuredIO.ts:590-606`.

The reply schema is `src/utils/permissions/PermissionPromptToolResultSchema.ts:43-76`:

- **allow** requires `updatedInput` — echo `input` back unchanged, or return a *modified* input to implement "edit before running".
- `updatedPermissions` is a `PermissionUpdate[]` (`coreSchemas.ts:263-299`) — this is your "always allow" button. Types: `addRules` / `replaceRules` / `removeRules` (each with `rules[]`, `behavior: allow|deny|ask`, `destination`), `setMode`, `addDirectories` / `removeDirectories`. `destination` ∈ `userSettings | projectSettings | localSettings | session | cliArg`. Malformed entries are dropped rather than rejecting the whole decision (`:50-58`), so a bug in your UI degrades to "allow once".
- `decisionClassification` ∈ `user_temporary | user_permanent | user_reject` — telemetry/attribution.
- **deny** requires `message` (shown to the model); `interrupt: true` also halts the turn.

**Three behaviours to design for:**

- **You are racing hooks.** A `PermissionRequest` hook runs in parallel with your prompt; first to resolve wins, loser is aborted (`src/cli/structuredIO.ts:561-638`). Your dialog may be cancelled out from under you via `control_cancel_request` — handle it.
- **You may be cancelled.** Aborts arrive as `control_cancel_request` for the `request_id` (`src/cli/structuredIO.ts:490-509`). Close the dialog.
- **Sandbox network prompts arrive here too.** Sandboxed network access is piggybacked on this subtype with the synthetic tool name `SandboxNetworkAccess` and input `{ host }` (`src/cli/structuredIO.ts:62`, `:731-753`). Special-case it or it renders as a mystery tool.

Note the casing trap: the envelope is snake_case (`tool_name`, `tool_use_id`) but the reply is camelCase (`updatedInput`, `updatedPermissions`, `toolUseID` — capital I, capital D).

Permission modes (`coreSchemas.ts:337-347`): `default`, `acceptEdits`, `bypassPermissions`, `plan`, `dontAsk`.

#### `hook_callback`

If you passed `hooks` in `initialize`, the CLI calls back with `{ subtype: "hook_callback", callback_id, input, tool_use_id? }` (`controlSchemas.ts:363-372`). Reply with a `HookJSONOutput`. Errors and timeouts degrade to `{}` (`src/cli/structuredIO.ts:661-689`), so a broken hook cannot wedge a turn. Full hook-event list at `coreSchemas.ts:355-383` — 26 events including `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop`, `SessionStart`, `SessionEnd`, `PreCompact`, `PostCompact`, `FileChanged`, `CwdChanged`, `ConfigChange`.

#### `mcp_message` — see §11.

#### `elicitation`

`{ subtype: "elicitation", mcp_server_name, message, mode?: "form"|"url", url?, elicitation_id?, requested_schema? }` (`controlSchemas.ts:522-536`). Reply `{ action, content? }`. Failure to answer degrades to `cancel` (`src/cli/structuredIO.ts:718-720`).

### 8.2 Host → CLI

Full catalogue in §9.

---

## 9. Control request catalogue (host → CLI)

The public Zod union is `SDKControlRequestInnerSchema` (`controlSchemas.ts:552-576`). The **actual dispatcher accepts more** — enumerated from the `if/else` chain in `src/cli/print.ts:2830-4028`. Subtypes marked ⚑ are internal-only: they are handled by the CLI but absent from the published schema, which means the VS Code extension uses a superset of the documented SDK. Expect these to be less stable across versions.

| Subtype | Request fields | Response | Line |
|---|---|---|---|
| `initialize` | `hooks?`, `sdkMcpServers?`, `jsonSchema?`, `systemPrompt?`, `appendSystemPrompt?`, `agents?`, `promptSuggestions?`, `agentProgressSummaries?` | See §10 | 2863 |
| `interrupt` | — | `{}` | 2831 |
| `end_session` ⚑ | `reason?` | `{}`, then the CLI exits its read loop | 2850 |
| `set_permission_mode` | `mode`, `ultraplan?` | `{}` | 2918 |
| `set_model` | `model?` (omit or `"default"` to reset) | `{}` | 2933 |
| `set_max_thinking_tokens` | `max_thinking_tokens: number\|null` (0 disables) | `{}` | 2945 |
| `mcp_status` | — | `{ mcpServers: McpServerStatus[] }` | 2957 |
| `get_context_usage` | — | Very rich: per-category token breakdown, `gridRows` for a visual meter, `memoryFiles`, `mcpTools`, `systemPromptSections`, `agents`, `skills`, `slashCommands`, `messageBreakdown`, `apiUsage`, `autoCompactThreshold` | 2961 |
| `mcp_message` | `server_name`, `message` | `{}` — pushes a JSON-RPC notification into the CLI's MCP client (§11) | 2979 |
| `rewind_files` | `user_message_id`, `dry_run?` | `{ canRewind, filesChanged[], insertions, deletions, error? }` | 2995 |
| `cancel_async_message` | `message_uuid` | `{ cancelled: boolean }` | 3011 |
| `seed_read_state` | `path`, `mtime` | `{}` — tells the CLI you observed a Read that left context, so Edit validation passes | 3017 |
| `mcp_set_servers` | `servers: Record<name, config>` | `{ added[], removed[], errors{} }` | 3055 |
| `reload_plugins` | — | `{ commands[], agents[], plugins[], mcpServers[], error_count }` | 3065 |
| `mcp_reconnect` | `serverName` | `{}` | 3133 |
| `mcp_toggle` | `serverName`, `enabled` | `{}` | 3206 |
| `channel_enable` ⚑ | `serverName` | `{}` | 3297 |
| `mcp_authenticate` ⚑ | `serverName` | `{ authUrl?, requiresUserAction }` | 3310 |
| `mcp_oauth_callback_url` ⚑ | `serverName`, `callbackUrl` | `{}` | 3463 |
| `mcp_clear_auth` ⚑ | `serverName` | `{}` | 3651 |
| `claude_authenticate` ⚑ | `loginWithClaudeAi` | `{ manualUrl, automaticUrl }` | 3514 |
| `claude_oauth_callback` ⚑ | `authorizationCode`, `state` | `{ account }` | 3609 |
| `claude_oauth_wait_for_completion` ⚑ | — | `{ account }` | 3610 |
| `apply_flag_settings` | `settings` (use `null` to clear a key) | `{}` | 3699 |
| `get_settings` | — | `{ effective, sources[] (low→high priority), applied: { model, effort } }` | 3756 |
| `stop_task` | `task_id` | `{}` | 3772 |
| `generate_session_title` ⚑ | `description`, `persist` | Fire-and-forget Haiku call | 3783 |
| `side_question` ⚑ | — | — | 3815 |
| `remote_control` ⚑ | `enabled` | `{ session_url, connect_url, environment_id }` | 3892 |

Two of these deserve a callout for a GUI:

- **`get_context_usage`** returns everything you need for a first-class context meter, including a pre-computed `gridRows` grid with colours. You do not have to reimplement token accounting.
- **`get_settings`** returns both the merged `effective` settings and the ordered per-source `sources[]` array, plus `applied.{model,effort}` — the runtime-resolved values after env overrides, which can differ from the disk merge. That distinction is documented at `controlSchemas.ts:505-515` and it matters: showing `effective.model` when `applied.model` differs will confuse users.

### 9.1 Auth over the control channel

The ⚑ auth subtypes exist because a headless CLI has no browser. The flow (`src/cli/print.ts:3514-3650`):

1. Host sends `claude_authenticate` → CLI starts PKCE, returns `{ manualUrl, automaticUrl }`.
2. Host opens `automaticUrl` if the browser is on the same host (a localhost listener in the CLI catches the redirect), else shows `manualUrl` and the user pastes back `code#state`.
3. Host sends `claude_oauth_callback { authorizationCode, state }`, or just `claude_oauth_wait_for_completion` if the listener will catch it.
4. Response carries the resolved `account` block.

MCP OAuth is the same shape one level down: `mcp_authenticate` → `{ authUrl }` → `mcp_oauth_callback_url` → then **you** must send `mcp_reconnect` (the CLI deliberately skips the auto-reconnect on the manual path — see the comment at `src/cli/print.ts:3385-3390`).

If you would rather not implement any of this: pre-authenticate out of band (`claude auth login`, or `ANTHROPIC_API_KEY` in the child's env) and skip the whole section.

---

## 10. Session lifecycle

### 10.1 Handshake

```
host: {"type":"control_request","request_id":"r1","request":{"subtype":"initialize", …}}
CLI : {"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{…}}}
CLI : {"type":"system","subtype":"init", …}
```

Request fields (`controlSchemas.ts:57-75`):

| Field | Purpose |
|---|---|
| `hooks` | `Record<HookEvent, [{ matcher?, hookCallbackIds[], timeout? }]>` — registers in-process hooks routed back via `hook_callback` |
| `sdkMcpServers` | `string[]` of server names you will host. **This is the key to §11.** |
| `systemPrompt` / `appendSystemPrompt` | Sent here rather than on argv specifically to dodge `ARG_MAX` (`src/cli/print.ts:4369`) |
| `agents` | `Record<name, AgentDefinition>` — same rationale |
| `jsonSchema` | Structured-output schema |
| `promptSuggestions` | Opt into `prompt_suggestion` messages |
| `agentProgressSummaries` | Opt into richer agent progress |

Response (`controlSchemas.ts:77-95`): `commands[]` (name / description / argumentHint — build your autocomplete from this), `agents[]`, `models[]`, `output_style` + `available_output_styles[]`, `account`, `pid`, `fast_mode_state?`.

**A second `initialize` is an error** (`src/cli/print.ts:4355-4367`) — and note the error response usefully carries `pending_permission_requests`, so a reconnecting host can recover in-flight prompts.

**`initialize` is optional.** The first `user` message implicitly initializes (`src/cli/print.ts:4059-4060`). But then you get no command list, no hooks, and no host-side MCP. Always initialize.

### 10.2 Interrupt

Send `{ subtype: "interrupt" }`. It aborts the in-flight query and clears pending suggestion state (`src/cli/print.ts:2831-2849`). It does not end the session — send another `user` message to continue.

### 10.3 Resume, fork, rewind

- **Resume:** relaunch with `-r <session-id>`. Add `--fork-session` to branch to a new ID. `--resume-session-at <message-id>` truncates to a specific assistant message.
- **In-process history injection:** push `assistant` / `system` messages on stdin (§5) to seed context without touching disk.
- **File rewind:** `rewind_files { user_message_id, dry_run }` restores the working tree to its state at that message. Call with `dry_run: true` first to render a confirmation — the response gives `filesChanged[]`, `insertions`, `deletions`.

### 10.4 Shutdown

Send `end_session` (clean — the CLI breaks its read loop and drains), or close stdin (the loop ends, pending requests reject). `SIGINT` triggers `gracefulShutdown` which persists session state and flushes analytics (`src/cli/print.ts:1027-1034`). Prefer `end_session`, fall back to closing stdin, then SIGTERM.

### 10.5 Transcripts still get written

Headless mode persists the same `~/.claude/projects/<slug>/<session-id>.jsonl` transcripts as the TUI, unless you pass `--no-session-persistence` (print-mode only, `src/main.tsx:1856`). Your GUI can read them for history and cross-reference by `uuid` — the stdout `uuid` values are the same ones written to the JSONL. (See `TRANSCRIPT_MODEL.md` in this repo for the transcript data model and through-line reconstruction.)

---

## 11. Host-provided MCP servers — the important part

This is the mechanism that makes a rich editor integration possible, and it is easy to miss.

### 11.1 How it works

1. In `initialize`, list your server names: `{ "sdkMcpServers": ["my-app"] }`.
2. The CLI synthesises a config `{ type: "sdk", name: "my-app" }` for each (`src/cli/print.ts:2866-2878`).
3. It constructs an MCP `Client` over `SdkControlClientTransport` (`src/services/mcp/client.ts:3262-3335`), a transport whose `send()` simply wraps the JSON-RPC message in a `control_request` (`src/services/mcp/SdkControlTransport.ts:60-95`).
4. Standard MCP handshake follows: `initialize`, `tools/list`, etc. — all as `mcp_message` control requests to you.
5. Discovered tools are registered as `mcp__my-app__<toolName>` and offered to the model like any other tool.

So: **your GUI implements an MCP server; the model can call into your GUI.** Open a file, reveal a symbol, show a diff, focus a panel, run a build task — whatever your app can do.

### 11.2 The wire

CLI → you:

```jsonc
{ "type": "control_request", "request_id": "…",
  "request": { "subtype": "mcp_message", "server_name": "my-app",
               "message": { "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} } } }
```

You → CLI:

```jsonc
{ "type": "control_response",
  "response": { "subtype": "success", "request_id": "…",
                "response": { "mcp_response": { "jsonrpc": "2.0", "id": 1, "result": { "tools": [ … ] } } } } }
```

The `{ mcp_response: … }` wrapper is mandatory — it is what the schema validates against (`src/cli/structuredIO.ts:758-773`).

You → CLI, unsolicited (server-initiated notifications):

```jsonc
{ "type": "control_request", "request_id": "…",
  "request": { "subtype": "mcp_message", "server_name": "my-app",
               "message": { "jsonrpc": "2.0", "method": "someNotification", "params": { … } } } }
```

The CLI injects it straight into the client's transport and replies `{}` (`src/cli/print.ts:2979-2994`). **This is the reverse channel** — how the editor tells Claude that the selection changed, a file was saved, the user clicked something.

### 11.3 A quirk worth knowing before you debug it

`SdkControlClientTransport.send()` awaits your response and then unconditionally passes it to `onmessage` (`SdkControlTransport.ts:74-86`) — **including for outbound notifications, which in JSON-RPC have no reply.** So when the CLI sends you a notification, it still expects a `control_response` carrying an `mcp_response`. Always answer every `mcp_message`. Return a benign JSON-RPC value rather than `null`; the transport will hand whatever you send to the MCP client layer.

### 11.4 What VS Code actually does with it

`src/services/mcp/vscodeSdkMcp.ts` is short and worth reading in full. Under the server name `claude-vscode`:

- **CLI → extension** `file_updated` `{ filePath, oldContent, newContent }` on every edit/write — so the editor can refresh buffers and show inline diffs. Gated on `USER_TYPE === 'ant'` in this drop (`:44`).
- **CLI → extension** `experiment_gates` `{ gates }` — pushes feature-flag state (`tengu_vscode_review_upsell`, `tengu_vscode_onboarding`, `tengu_quiet_fern`, `tengu_vscode_cc_auth`, `tengu_auto_mode_state`) so the extension's UI matches the CLI's.
- **extension → CLI** `log_event` `{ eventName, eventData }` — re-emitted as `tengu_vscode_<name>` analytics.

There is also an enterprise-policy carve-out worth knowing: when `allowManagedMcpServersOnly` is set, SDK servers are normally blocked, but `claude-vscode` specifically is exempted (`src/services/mcp/config.ts:1494-1504`, with the comment explaining it was to unbreak enterprise customers). If your own server name hits that policy in an enterprise environment, this is why — and you will need managed-settings cooperation, not a code trick.

### 11.5 Also available: `mcp_set_servers`

For *external* MCP servers (stdio / SSE / HTTP), `mcp_set_servers { servers }` replaces the dynamically-managed set at runtime and returns `{ added, removed, errors }` (`src/cli/print.ts:3055`, `handleMcpSetServers` at `:5353`). Different mechanism, different purpose: use `sdkMcpServers` for capabilities *your app* provides, `mcp_set_servers` for third-party servers you want to attach mid-session.

---

## 12. Project context: what gets discovered, and how to surface it

**Design goal for this host: the agent uses everything the repo provides — plugins, MCP servers, skills, agents, hooks, commands, memory — and the UI exposes all of it as first-class affordances the user can see and act on.** That inverts the assumptions in most public guidance, which is written for CI. Read this section before §3's flag table.

### 12.1 Do not use `--bare`

`--bare` (`src/main.tsx:976`) is the anti-goal. It skips hooks, LSP, plugin sync, auto-memory, keychain reads, and `CLAUDE.md` auto-discovery, and forces auth to `ANTHROPIC_API_KEY` / `apiKeyHelper` only. Anthropic's docs recommend it for scripted/SDK callers and note it is slated to become the `-p` default in a future release — which means **you may eventually have to opt back out explicitly.** Watch for that; a silent default flip would gut this host's entire value proposition.

Two neighbouring flags are also wrong here and worth naming so nobody adds them "for determinism":

- `--strict-mcp-config` — ignores every MCP config except `--mcp-config`, so `.mcp.json` and plugin-provided servers vanish.
- `--setting-sources <user,project,local>` — restricts which settings layers load (`src/utils/settings/constants.ts:128-153`). Omit it and you get all of them. Only reach for it if you deliberately want to exclude, say, `localSettings`.

### 12.2 What the CLI auto-discovers

Under the project's `.claude/` directory (`CLAUDE_CONFIG_DIRECTORIES`, `src/utils/markdownConfigLoader.ts:29-36`):

| Path | Becomes |
|---|---|
| `.claude/commands/**.md` | Slash commands |
| `.claude/agents/**.md` | Subagent definitions |
| `.claude/skills/<name>/SKILL.md` | Skills |
| `.claude/output-styles/**.md` | Output styles |
| `.claude/workflows/**.md` | Workflows |
| `.claude/templates/**` | Templates (only when the `TEMPLATES` feature flag is on) |
| `.claude/settings.json` | `projectSettings` — hooks, permissions, statusLine, env, model, MCP allow/deny |
| `.claude/settings.local.json` | `localSettings` (gitignored) |
| `.claude/CLAUDE.md`, `.claude/rules/*.md` | Project memory (`src/utils/claudemd.ts:898-919`) |

Plus, outside `.claude/`: root `CLAUDE.md` / `CLAUDE.local.md`, and `.mcp.json` for project MCP servers. Discovery walks up to the git root, deliberately stopping there so a parent directory can't leak config in (`src/utils/markdownConfigLoader.ts:226-271`). Settings merge low-to-high as `userSettings → projectSettings → localSettings → flagSettings → policySettings` (`src/utils/settings/constants.ts:7-22`).

Plugins layer on top of all of this and can contribute commands, agents, skills, hooks, MCP servers, and LSP servers (`src/types/plugin.ts:48-78`).

### 12.3 Reading all of it over the wire

This is the part that makes the UI possible. Everything discovered is enumerable — you never have to parse `.claude/` yourself.

**At startup, `system/init` gives you the inventory** (`coreSchemas.ts:1457-1494`): `tools[]`, `mcp_servers[]` (name + status), `slash_commands[]`, `skills[]`, `agents[]`, `plugins[]` (name / path / `source` in `name@marketplace` form), `output_style`, `model`, `cwd`, `permissionMode`, `betas[]`.

**The `initialize` response adds the detail `system/init` omits** (§10.1): `commands[]` with `description` and `argumentHint` — that is your autocomplete — plus `agents[]` with `whenToUse` descriptions, `models[]`, and `available_output_styles[]`.

**`mcp_status` gives you a full server panel** (`coreSchemas.ts:167-220`): per server, its `status` (`connected` / `failed` / `needs-auth` / `pending` / `disabled`), `serverInfo`, `error`, `config` (including URL), `scope` (project / user / local / managed), the server's `tools[]` with descriptions and read-only/destructive annotations, and `capabilities`.

**`get_context_usage` is the richest source, and its `source` fields are the key** — it attributes each item to where it came from:

| Field | Shape | UI use |
|---|---|---|
| `skills` | `{ totalSkills, includedSkills, tokens, skillFrontmatter: [{ name, source, tokens }] }` | "12 skills loaded — 4 from `methodical-cc`" |
| `agents` | `[{ agentType, source, tokens }]` | Group agents by origin |
| `mcpTools` | `[{ name, serverName, tokens, isLoaded }]` | Which server contributed which tool, and whether it's deferred |
| `memoryFiles` | `[{ path, type, tokens }]` | Show every `CLAUDE.md` / rules file in play |
| `slashCommands` | `{ totalCommands, includedCommands, tokens }` | Note `included` < `total`: not every command is in context |
| `deferredBuiltinTools`, `systemTools`, `systemPromptSections` | `[{ name, tokens, isLoaded? }]` | Full context budget breakdown |

`isLoaded` / `includedSkills` matter: Claude Code defers some tools and skills until needed. A UI that shows "available" without distinguishing "currently in context" will mislead.

**`get_settings` shows the merge** — `effective` (merged), `sources[]` (raw, ordered low-to-high), and `applied.{model,effort}` (runtime-resolved after env overrides). Render `sources[]` as a layered inspector so a user can see *why* a setting has its value.

**Hooks** are visible via `--include-hook-events` → `hook_started` / `hook_progress` / `hook_response`, each carrying `hook_name` and `hook_event` (`coreSchemas.ts:1604-1646`). That is how you show "the repo's PostToolUse hook is running" instead of an unexplained pause.

### 12.4 Making it live

Discovery is not a one-shot at startup:

- **`reload_plugins`** re-reads plugins from disk and returns refreshed `commands[]`, `agents[]`, `plugins[]`, `mcpServers[]`, and `error_count` (`src/cli/print.ts:3065`). Wire it to a file watcher on `.claude/` and a "Reload" button; the user edits a skill and sees it appear.
- **`mcp_toggle` / `mcp_reconnect` / `mcp_authenticate` / `mcp_clear_auth`** give you a complete per-server control surface (§9).
- **`mcp_set_servers`** attaches or detaches servers mid-session beyond what the repo declares.
- **`apply_flag_settings`** merges settings into the `flagSettings` layer live — use `null` to clear a key. This is how a UI toggle changes behaviour without a restart.
- **`system/status`** echoes permission-mode changes from *any* source, so your mode indicator stays truthful even when the model changes mode itself.

### 12.5 The trust problem you now own

This is the one real cost of rejecting `--bare`, and it is a genuine security consideration rather than a footnote.

Because `-p` sessions never show the interactive workspace-trust dialog, a `-p` session **runs the hooks in a project's `.claude/settings.json` and connects the servers in its `.mcp.json` even in a directory the user has never trusted.** Anthropic's own headless documentation states this explicitly. In a CLI that is a considered trade-off; in a GUI that opens arbitrary repositories — cloned, downloaded, handed over — it is a code-execution path that fires before the user has seen anything.

**Your app must implement its own trust gate.** A workable design:

1. On opening a repo, before spawning the child, check your own trust store.
2. If untrusted, inspect what would auto-run — `.claude/settings.json` hooks, `.mcp.json` servers, `.claude/skills/`, plugin declarations — and show the user exactly what will execute and from where.
3. On approval, record trust and spawn normally. On rejection, either don't spawn or spawn with `--strict-mcp-config` plus a restrictive `--setting-sources` as a read-only preview mode, then re-launch fully once trusted.

Treat this as a first-class feature, not a guard clause. Being able to say "here is everything this repository will run, before it runs" is a real advantage over the terminal — and it is the affordance that makes full auto-discovery safe to offer by default.

---

## 13. The other IDE channel (lockfile + WebSocket)

Not part of the sidecar protocol, but you will trip over it, so: this is how a **terminal-run** `claude` finds an editor.

- The editor extension writes `~/.claude/ide/<port>.lock` containing `{ workspaceFolders[], pid, ideName, transport: "ws"|"sse", runningInWindows, authToken }` (`src/utils/ide.ts:73-90`).
- `detectIDEs()` (`:664`) reads all lockfiles newest-first, matches one whose `workspaceFolders` contains the cwd, and (unless `CLAUDE_CODE_SSE_PORT` pins it) verifies the lockfile's `pid` is an ancestor of the CLI process (`:765-785`) so you attach to *your* window.
- URL is `ws://host:port` or `http://host:port/sse` (`:794-798`). `--ide` auto-connects when exactly one valid IDE is found.
- The CLI then connects as an MCP **client** to a server named `ide` and calls tools on it: `openDiff` (`src/hooks/useDiffInIDE.ts:284`), `close_tab` (`:339`), `closeAllDiffTabs` (`src/utils/ide.ts:1274`), `getDiagnostics` and `openFile` (`src/services/diagnosticTracking.ts:114-201`), `executeCode`, `set_permission_mode` (`src/hooks/usePromptsFromClaudeInChrome.tsx:52`).
- Notifications flow the other way: `selection_changed` `{ selection: {start:{line,character}, end:{…}}, text, filePath }` (`src/hooks/useIdeSelection.ts:32-52`), `at_mentioned` (`src/hooks/useIdeAtMentioned.ts:16`). The CLI announces itself with `ide_connected { pid }` (`src/utils/ide.ts:829-836`).
- Only `mcp__ide__executeCode` and `mcp__ide__getDiagnostics` are exposed to the model as callable tools (`src/services/mcp/client.ts:568`); the rest are CLI-internal RPC.

**For your GUI, do not build this.** Everything it provides you can provide better through a host MCP server over the control channel (§11), without lockfiles, port scanning, or PID-ancestry checks. The tool names above are still a good shopping list for what an editor integration should offer.

---

## 14. Alternative transports (context only)

- **`--sdk-url <url>`** (`src/main.tsx:3861`) swaps stdio for a remote WebSocket carrying the identical vocabulary; auto-enables `-p`, both stream-json formats, and `--verbose` (`src/main.tsx:1236-1250`). If you ever need to run the runtime on a different machine from the GUI, this is the seam — the message contract does not change.
- **`claude server` / `claude open cc://…`** (`src/main.tsx:3962`, `:4059`) — a real multi-session HTTP server behind the `DIRECT_CONNECT` build feature, with bearer-token auth, unix-socket support, idle timeouts, and a lockfile at `~/.claude`. Implementation files are absent from this drop. Worth re-checking in a current build if you want one runtime serving several GUI windows.
- **`claude ssh <host>`** (`src/main.tsx:4046`) deploys the binary over SSH and tunnels auth back — orthogonal, but the same headless core runs on the far side.

---

## 15. Version drift and feature detection

### 15.1 Read `system/init.capabilities` first

**Newer builds advertise protocol capabilities explicitly, and you should feature-detect off that rather than off version strings or the hand-rolled heuristics below.**

`system/init` carries an optional `capabilities: string[]` naming the protocol behaviours the running build implements — documented examples are `interrupt_receipt_v1` and `interrupt_cancel_queued_v1`. Anthropic's guidance is to check it and **ignore values you don't recognise**. It requires Claude Code v2.1.205 or later and is absent from earlier builds, including the tree this document was derived from — which is exactly why it does not appear in §6.1.

So the rule is:

```js
const caps = new Set(initMsg.capabilities ?? [])   // absent ⇒ old build ⇒ assume nothing
if (caps.has('interrupt_receipt_v1')) { /* await the receipt */ }
else { /* fire-and-forget, as in §10.2 */ }
```

Absence of the field is itself the signal for "pre-2.1.205, degrade gracefully."

### 15.2 Known drift since this drop

Everything in this table is **from Anthropic's current published documentation, not from source I have read.** It is absent from `coreSchemas.ts` / `controlSchemas.ts` in this tree, which means it postdates the drop. Verify against a live binary before building on it.

| Addition | Why it matters here |
|---|---|
| `capabilities[]` on `system/init` | §15.1. Supersedes hand-rolled version detection. |
| `plugin_errors[]` on `system/init` — each `{ plugin, type, message }` | **Directly relevant to §12.** Plugins that failed to load are demoted and absent from `plugins[]`. Surface these or a broken plugin looks like a missing one. Key omitted when empty. |
| `mcp_server_errors[]` on `system/init` — each `{ name, type, message }` | Same: `--mcp-config` entries skipped by validation (`unknown_type`, `url_missing_type`, `invalid_config`, `reserved_name`). Requires v2.1.219+. |
| `system/plugin_install` events (`status`: started / installed / failed / completed) | Install progress for `CLAUDE_CODE_SYNC_PLUGIN_INSTALL`. Show it instead of a startup stall. |
| `SDKCommandsChangedMessage` | Commands changed mid-session — refresh autocomplete without a `reload_plugins` round trip. |
| `ttft_ms` on `stream_event` / `message_start` | Time to first token. |
| `--forward-subagent-text` / `CLAUDE_CODE_FORWARD_SUBAGENT_TEXT` | By default only subagent `tool_use`/`tool_result` blocks are forwarded. This adds text and thinking, at every nesting depth, so you can rebuild the full agent tree from `parent_tool_use_id`. v2.1.211+; nested-depth forwarding v2.1.219+. |
| Expanded `api_retry` error categories | Adds `oauth_org_not_allowed`, `overloaded`, `model_not_found` to the enum in `coreSchemas.ts:1256-1266`. |
| `createSdkMcpServer` gains `instructions` and `alwaysLoad` | `instructions` is returned from your MCP `initialize` and surfaced to the model as an instructions block — free steering for your host tools (§11). |
| SIGTERM semantics | Aborts the in-flight turn, kills the Bash process tree, runs `SessionEnd` hooks, exits 143. Wire your shutdown path to this rather than SIGKILL. |
| Background-task exit behaviour | Background Bash killed ~5s after the final result; background subagents/workflows waited on, capped at 10 min (`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`, `0` = unlimited). |
| Output drain on exit | The CLI waits for queued stdout to drain, scaled to backlog, capped at 30s (was ~2s before v2.1.214). A slow consumer no longer truncates the tail. |
| Slash-command arguments in `-p` | `/model sonnet`, `/effort`, `/fast`, `/color`, `/rename` take values; `/config key=value`; `/mcp` prints a text summary. `/login` is unavailable in `-p` — hence the `claude_authenticate` control subtype (§9.1). v2.1.205+. |
| Piped-stdin cap | 10 MB, hard error above. Irrelevant with `--input-format stream-json`, but relevant if you ever shell out. |
| `--bare` trajectory | Slated to become the `-p` default. See §12.1 — if that lands, this host must opt out explicitly. |

### 15.3 Re-verification order, cheapest first

1. Send an `initialize` and inspect `system/init.capabilities`, `plugin_errors`, `mcp_server_errors`. One round trip tells you roughly where you are.
2. `claude --help` plus the hidden options — does `--input-format stream-json` still exist? Has `--bare` become implicit?
3. Diff the `initialize` response against §10.1.
4. `src/entrypoints/sdk/controlSchemas.ts` — `SDKControlRequestInnerSchema` is the fastest read on what the public protocol accepts.
5. `src/entrypoints/sdk/coreSchemas.ts` `SDKMessageSchema` — the stdout vocabulary. New `system` subtypes are the likeliest addition.
6. The `if/else` chain in `src/cli/print.ts` (search `message.request.subtype ===`) — the real dispatcher, including the ⚑ internal subtypes.
7. `src/services/mcp/vscodeSdkMcp.ts` — new methods on the `claude-vscode` server tell you what new editor affordances exist.

### 15.4 Forward-compatibility rules

Even with `capabilities`, keep these — they are what let one host implementation span versions:

- Ignore unknown message `type` / `subtype`; never treat one as an error.
- Ignore unknown fields; never validate strictly against a closed schema.
- Treat an absent optional field as "old build", not as "empty".
- Always answer a control request, even if only to error.

---

## 16. Minimal host implementation

```js
import { spawn } from 'node:child_process'
import readline from 'node:readline'
import { randomUUID } from 'node:crypto'

const child = spawn('claude', [
  '--print',
  '--input-format', 'stream-json',
  '--output-format', 'stream-json',
  '--verbose',
  '--include-partial-messages',
  '--replay-user-messages',
], {
  cwd: projectRoot,
  env: { ...process.env, CLAUDE_CODE_ENTRYPOINT: 'my-app' },
  stdio: ['pipe', 'pipe', 'pipe'],
})

const send = msg => child.stdin.write(JSON.stringify(msg) + '\n')
const pending = new Map()

function request(req) {
  const request_id = randomUUID()
  return new Promise((resolve, reject) => {
    pending.set(request_id, { resolve, reject })
    send({ type: 'control_request', request_id, request: req })
  })
}

function respond(request_id, response) {
  send({ type: 'control_response', response: { subtype: 'success', request_id, response } })
}

function respondError(request_id, error) {
  send({ type: 'control_response', response: { subtype: 'error', request_id, error } })
}

// stderr is a separate diagnostic channel — never let it corrupt parsing
readline.createInterface({ input: child.stderr })
  .on('line', l => console.error('[claude]', l))

readline.createInterface({ input: child.stdout }).on('line', line => {
  let msg
  try { msg = JSON.parse(line) } catch { return }   // skip, never crash

  switch (msg.type) {
    case 'control_response': {
      const p = pending.get(msg.response.request_id)
      if (!p) return                                 // orphan — ignore
      pending.delete(msg.response.request_id)
      msg.response.subtype === 'error'
        ? p.reject(new Error(msg.response.error))
        : p.resolve(msg.response.response)
      return
    }

    case 'control_request':
      return handleCliRequest(msg)

    case 'control_cancel_request':
      return ui.cancelPrompt(msg.request_id)

    case 'system':
      if (msg.subtype === 'init') session.id = msg.session_id
      if (msg.subtype === 'session_state_changed') ui.setBusy(msg.state === 'running')
      return ui.onSystem(msg)

    case 'assistant':
    case 'user':
    case 'stream_event':
    case 'result':
    case 'tool_progress':
      return ui.onMessage(msg)

    default:
      return ui.onUnknown(msg)                       // forward-compatible
  }
})

async function handleCliRequest({ request_id, request }) {
  try {
    switch (request.subtype) {
      case 'can_use_tool': {
        // Sandbox network prompts arrive here with a synthetic tool name
        const decision = request.tool_name === 'SandboxNetworkAccess'
          ? await ui.askNetwork(request.input.host)
          : await ui.askPermission(request)
        return respond(request_id, decision.allow
          ? { behavior: 'allow', updatedInput: request.input,
              updatedPermissions: decision.always ? decision.rules : undefined }
          : { behavior: 'deny', message: decision.reason ?? 'Denied by user' })
      }

      case 'mcp_message':
        return respond(request_id, { mcp_response: await myMcpServer.handle(request.message) })

      case 'hook_callback':
        return respond(request_id, await myHooks[request.callback_id](request.input))

      case 'elicitation':
        return respond(request_id, await ui.elicit(request))

      default:
        return respondError(request_id, `Unsupported subtype: ${request.subtype}`)
    }
  } catch (err) {
    respondError(request_id, String(err))            // never leave one hanging
  }
}

// --- startup ---
const init = await request({
  subtype: 'initialize',
  sdkMcpServers: ['my-app'],
  hooks: { PostToolUse: [{ hookCallbackIds: ['refresh-editor'] }] },
})
ui.setCommands(init.commands)     // slash-command autocomplete
ui.setModels(init.models)
ui.setAccount(init.account)

// --- a turn ---
send({
  type: 'user',
  message: { role: 'user', content: 'Refactor the auth module' },
  parent_tool_use_id: null,
  uuid: randomUUID(),
})
```

### Feature-parity map

| GUI affordance | Mechanism |
|---|---|
| Token-by-token streaming | `--include-partial-messages` → `stream_event` |
| Busy / idle spinner | `system`/`session_state_changed` (not `result`) |
| Permission dialog | `can_use_tool` request/response |
| "Always allow" | `updatedPermissions` on the allow decision |
| Stop button | `interrupt` request |
| Model picker | `initialize.models[]` + `set_model` |
| Permission-mode toggle | `set_permission_mode` + `system`/`status` echo |
| Context meter | `get_context_usage` (`gridRows` is pre-rendered) |
| Slash-command autocomplete | `initialize.commands[]`; send as plain user text |
| MCP server panel | `mcp_status`, `mcp_toggle`, `mcp_reconnect`, `mcp_authenticate` |
| Settings inspector | `get_settings` (`effective` + `sources[]` + `applied`) |
| Editor callbacks into the app | Host MCP server via `sdkMcpServers` (§11) |
| Push editor state to Claude | Unsolicited `mcp_message` control_request (§11.2) |
| Undo file changes | `rewind_files` (`dry_run` first) |
| Rate-limit banner | `rate_limit_event` |
| Cost / usage display | `result.total_cost_usd`, `usage`, `modelUsage` |
| Session history | Read the JSONL transcripts; `uuid`s match the stream |
| Repo capability inventory | `system/init` + `initialize` response + `mcp_status` + `get_context_usage` (§12.3) |
| Live reload on `.claude/` edit | `reload_plugins` + a file watcher (§12.4) |
| Pre-execution trust review | Your own gate before spawn (§12.5) |

---

## 17. Public documentation map

**The wire protocol is not officially documented.** Anthropic documents the language-SDK API surface and the stdout message vocabulary; the control protocol appears publicly only in community reverse-engineering work, and only in part. Knowing which tier covers what saves re-deriving things that are already written down — and tells you which parts of this document are load-bearing.

| Tier | Covers | Gap |
|---|---|---|
| **Official — stdout vocabulary.** [Headless / programmatic](https://code.claude.com/docs/en/headless), [streaming output](https://code.claude.com/docs/en/agent-sdk/streaming-output) | `system/init`, `api_retry` (full field table), `plugin_install`, `stream_event`, `result`, `parent_tool_use_id` subagent attribution, CLI flags | Nothing on the control protocol |
| **Official — abstracted control.** [TypeScript reference](https://code.claude.com/docs/en/agent-sdk/typescript) | The control protocol as methods on a `Query` object: `getContextUsage()`, `mcpServerStatus()`, `interrupt()`. `createSdkMcpServer()` as an API. The `capabilities` list. | The envelope is never shown. Nothing reveals that `createSdkMcpServer` is implemented by tunnelling JSON-RPC over the control channel — so a non-TS/Python host has no way to learn it *can* provide an MCP server |
| **Community — best public wire source.** [`Roasbeef/claude-agent-sdk-go` `docs/cli-protocol.md`](https://github.com/Roasbeef/claude-agent-sdk-go/blob/main/docs/cli-protocol.md) | `initialize`, `permission`, `mcp_message`, the `sdk_mcp_servers` field, the MCP handshake sequence, `CLAUDE_CODE_ENTRYPOINT` | Written for an SDK port, so it covers the three subtypes an SDK needs and stops there |

Also circulating: a [`claude-cli-agent-protocol` skill](https://raw.githubusercontent.com/NeverSight/skills_feed/refs/heads/main/data/skills-md/bohdan-shulha/skills/claude-cli-agent-protocol/SKILL.md) — not assessed here.

### 17.1 What in this document has no public counterpart

Roughly, the parts worth keeping current by hand:

1. **The `claude-vscode` server name and the VS Code architecture** (§11.4). Nothing public says the extension registers itself as an SDK-MCP server, names it, or documents `file_updated` / `experiment_gates` / `log_event`. The Go doc gives you the mechanism; only the source proves this is how the shipping editor integration works — which is what makes it a supported path rather than a clever idea.
2. **The nine ⚑ internal control subtypes** (§9): `end_session`, the three `claude_*` OAuth subtypes, the three `mcp_*` auth subtypes, `channel_enable`, `generate_session_title`, `side_question`, `remote_control`. Absent from the docs, from the published Zod union, and from the Go doc. The auth ones are how a headless sidecar signs in with no browser (§9.1).
3. **Schema-present but prose-absent subtypes**: `get_context_usage`, `get_settings`, `rewind_files`, `seed_read_state`, `apply_flag_settings`, `mcp_set_servers`, `cancel_async_message`, `reload_plugins`. Visible only by reading `controlSchemas.ts`. Several are load-bearing for §12.
4. **Behavioural invariants**, which is where the real time savings are: message coalescing (§5.1), `session_state_changed` as the authoritative turn signal (§6.2), the notification-reply quirk in the SDK-MCP transport (§11.3), the serial-stdin deadlock (§7.2), `SandboxNetworkAccess` as a synthetic tool (§8.1), the hook-vs-prompt race (§8.1), the `allowManagedMcpServersOnly` carve-out (§11.4).
5. **`CLAUDE_CODE_ENTRYPOINT` is behavioural, not cosmetic** (§3.3). The Go doc names the variable; nothing documents the eight branches it changes.
