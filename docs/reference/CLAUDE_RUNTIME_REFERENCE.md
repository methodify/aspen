# Running the Claude Agent Runtime — A Field Reference

**Everything we know about spawning, driving, and managing headless Claude
Code as an embedded agent runtime.** Distilled from three sources, in
ascending order of authority:

1. `HOST_PROTOCOL.md` (same directory) — a source-level reverse engineering
   of the CLI's host protocol, with file:line receipts. The static contract.
2. **Contextua Studio's implementation** — `app/src-tauri/src/claude.rs`
   (process layer), `app/src/surfaces/muse/museHost.ts` (protocol client),
   `studioMcp.ts` (host MCP), `useMuse.ts` (event semantics),
   `sessions.rs` (transcript layer). The contract, made to work.
3. **Live verification against claude 2.1.233** through weeks of real use —
   a spike, four e2e verification rounds, and a multi-week authoring drive
   that stress-tested every path with a human on the other end. Where this
   document disagrees with `HOST_PROTOCOL.md`, *this document observed it
   live and wins.*

This is written to be lifted into a different project. Nothing below
depends on Contextua except where explicitly labeled "pattern from the
Studio" — those are transferable designs, not dependencies.

---

## 1. The architecture that works

```
┌────────────────────────────────────────────────────────┐
│ Your app                                               │
│                                                        │
│  ┌──────────────┐  claude:event   ┌──────────────────┐ │
│  │ Native layer │ ──────────────► │  Protocol client │ │
│  │  (thin!)     │                 │  (owns schema)   │ │
│  │ spawn/kill,  │ ◄────────────── │  correlation,    │ │
│  │ line I/O     │   send_line     │  handshake,      │ │
│  └──────┬───────┘                 │  permissions,    │ │
│         │ stdin/stdout NDJSON     │  host MCP server │ │
│  ┌──────▼───────┐                 └────────┬─────────┘ │
│  │ claude -p    │                          │           │
│  │ (subprocess) │                 ┌────────▼─────────┐ │
│  └──────────────┘                 │ Conversation     │ │
│                                   │ store / reducer  │ │
└────────────────────────────────────────────────────────┘
```

**Keep the native layer thin and schema-ignorant.** The Studio's Rust side
does exactly four things: spawn with the right argv/env, enforce *line
discipline* on stdin (reject embedded newlines, validate JSON — a parse
failure on the CLI side is **fatal**, it exits), stream stdout/stderr lines
to the UI layer as opaque JSON, and kill. Every protocol decision lives in
one higher-level module. This split survived a complete protocol rebuild
(legacy hacks → real protocol) without touching the native layer, and it
means the protocol client is testable and hot-reloadable.

**One event subscription, one store.** All frames from the child flow
through a single subscription; control frames are peeled off by the
protocol client (`handleHostFrame` returns true = "consumed"), everything
else feeds the conversation reducer. Double subscriptions plus hot reload
create phantom listeners; one choke point never does.

---

## 2. Launching

### 2.1 The verified argv (2.1.233)

```
claude --print --verbose \
       --input-format stream-json --output-format stream-json \
       --include-partial-messages \
       --replay-user-messages \
       --permission-prompt-tool stdio \
       [--session-id <uuid> | -r <uuid> [--fork-session]] \
       [--permission-mode <mode>] \
       [--plugin-dir <path>]
```

| Flag | Why | Field notes |
|---|---|---|
| `--print` | headless mode | |
| `--verbose` | **mandatory** with stream-json output — CLI hard-exits without it | |
| `--input/--output-format stream-json` | NDJSON both ways | input=stream-json forces output=stream-json, not vice versa |
| `--include-partial-messages` | `stream_event` SSE deltas → token-level streaming | without it you only get block-level re-emissions |
| `--replay-user-messages` | accepted user messages echo back with `isReplay: true` | this is your **delivery acknowledgement** — the only way to distinguish queued from lost |
| `--permission-prompt-tool stdio` | **THE FLAG THE DOCS DON'T TELL YOU ABOUT.** Without it, `can_use_tool` never reaches the host — the CLI auto-decides from settings and your permission UI is dead code. Cost us a day in the spike. | verified: with it, Read and Edit prompt; safe commands (e.g. `echo`) still don't |
| `--session-id <uuid>` | pre-assign the id — deterministic transcript correlation | honored on 2.1.233 |
| `-r <uuid>` | resume a prior session's conversation | context survives; **session-scoped permission rules do not** (§7.5) |
| `--fork-session` | with `-r`: branch to a new id | |
| `--plugin-dir <path>` | load a plugin from a directory | how a bundled plugin ships inside an installer |

**Do not pass** `--bare`, `--strict-mcp-config`, or `--setting-sources`
unless you *want* a bare runtime: they strip exactly the repo-provided
plugins/hooks/MCP/memory that make an embedded agent worth embedding.
Watch the `--bare` trajectory — it is slated to become the `-p` default,
at which point you must opt out explicitly.

### 2.2 Environment

- `CLAUDE_CODE_ENTRYPOINT=<your-app-id>` — set your own stable identifier.
  It prevents the CLI self-labelling as `sdk-cli`, and it **stamps every
  transcript line**, which later lets you tell your app's sessions from
  terminal sessions in the same project (§10).
- **Secrets ride the spawn env.** Pattern from the Studio: user-wide API
  keys live in the OS keyring; at spawn they are injected as env vars
  (`OPENAI_API_KEY`, …) that downstream tools resolve with a
  config-first/env-fallback convention. Env changes require a process
  restart (§4.4); config-file choices your tools re-read per call do not.
  Validate injected names (`[A-Za-z0-9_]+`, non-empty values) so a UI bug
  can never blank `PATH`.
- `CLAUDE_CONFIG_DIR` relocates `~/.claude` if you need isolation.
- cwd of the child **is** the project root. No flag sets it; set it on
  spawn. Extra roots via `--add-dir`.
- Windows: spawn with `CREATE_NO_WINDOW` (0x08000000) or a console flashes.

### 2.3 Stdin discipline (worth its own heading)

One complete JSON value per line, `\n`-terminated, UTF-8. **A malformed
line kills the child** — `process.exit(1)`, no resynchronization. Enforce
at the last writer before the pipe: reject embedded `\n`/`\r`, parse-check
the JSON, write line + `\n`, flush. Empty lines are ignored (harmless);
two values on one line are fatal.

Read stdout and stderr on separate threads. Skip stdout lines that fail to
parse instead of crashing (the CLI guards its stdout, but trust and
verify); stderr carries real diagnostics — surface it in a debug channel.

---

## 3. The wire: two planes

Everything is NDJSON frames with a `type`. Two logical planes share the
pipe:

- **Stream plane** — the conversation: `user`, `assistant`,
  `stream_event`, `result`, `system/*`, `tool_progress`, …
- **Control plane** — request/response with correlation ids, both
  directions: `control_request` / `control_response` /
  `control_cancel_request`.

```jsonc
// request (either direction)
{ "type": "control_request", "request_id": "<uuid>",
  "request": { "subtype": "…", /* payload */ } }

// success
{ "type": "control_response",
  "response": { "subtype": "success", "request_id": "<uuid>", "response": { … } } }

// error
{ "type": "control_response",
  "response": { "subtype": "error", "request_id": "<uuid>", "error": "…" } }
```

### Invariants (each one broke something for us before we honored it)

1. **Always answer every CLI-initiated `control_request`**, even if only
   with an error. An unanswered request hangs the CLI's promise forever.
   This includes JSON-RPC *notifications* tunneled over `mcp_message`
   (§8) — the transport awaits a reply even where JSON-RPC says none
   exists.
2. **Correlate by `request_id`; emit snake_case** (`request_id`, not
   `requestId`). Envelope fields are snake_case; permission-reply payloads
   are camelCase (`updatedInput`, `updatedPermissions`). This
   inconsistency is real, not a typo in your code.
3. **Pre-attach a silent `.catch` to every pending control promise.** On
   session reset you reject everything in flight; fire-and-forget callers
   otherwise become uncaught-rejection noise (we shipped that bug —
   "session restarted" errors surfacing in the console during restarts).
   Callers that await still see the rejection.
4. **Timeout your own requests** (we use 15–60s by subtype) — a dead child
   must not leak promises.
5. **Your dialog can be cancelled out from under you** —
   `control_cancel_request` arrives for a permission request the CLI no
   longer needs (it races hooks; first resolver wins). Close the UI.

---

## 4. Session lifecycle

### 4.1 Handshake — and the 2.1.233 timing correction

Send `initialize` immediately after spawn:

```jsonc
{ "subtype": "initialize", "sdkMcpServers": ["your-app"] }
```

The response carries `commands[]` (name/description/argumentHint — build
your slash autocomplete from this), `models[]`, `account`, `output_style`,
`current_permission_mode`. Registering `sdkMcpServers` here is what makes
the CLI mount *your app* as an MCP server (§8).

**Correction to the reference doc:** on 2.1.233, `system/init` does *not*
arrive right after the handshake — it arrives **with the first turn**.
Don't block your UI waiting for it. Capture it whenever it shows up:
`capabilities[]` (feature detection), `session_id`, plus the inventory
(`tools[]`, `mcp_servers[]`, `slash_commands[]`, `skills[]`, `plugins[]`).

`initialize` is technically optional (first `user` message implicitly
initializes) but then you get no command list, no host MCP, no hooks.
Always initialize. A second `initialize` is an error.

### 4.2 Warm start (pattern from the Studio)

Spawn + handshake **eagerly when the workspace opens**, not on first
message. The handshake takes seconds (model list, 200+ commands, MCP
mounts); doing it early means pickers are populated and the first user
message has first-token latency instead of cold-boot latency. Make it
silent-on-failure — the first real send retries.

### 4.3 Shutdown ladder

`end_session` control request (clean; CLI drains and exits) → close stdin
(EOF; loop ends, in-flight requests reject with "stream closed") → SIGTERM
→ kill after a ~3s deadline. Transcripts persist in all cases.

### 4.4 Restart without losing the conversation (pattern from the Studio)

Some changes only land at spawn (env-injected keys, plugin dir). The
transparent restart: `end_session` → kill → respawn with `-r <same id>`.
The conversation resumes intact; the user sees nothing. Two rules:

- **Gate on turn state.** If a turn is streaming, set a `restartPending`
  flag, show a quiet "restarts after this reply" note, and perform the
  restart when `result` lands. Never yank a session mid-turn.
- **Session-scoped permission grants die with the process** (§7.5). Clear
  any UI that advertises them.

### 4.5 Process supervision (ops lesson, learned three times)

If your dev/host tooling runs the app as a *child of some supervising
shell*, killing the supervisor kills the runtime — and anything the user
was doing with it. Launch detached (`Start-Process` / `setsid`), track by
pid/port, and let lifetimes be independent. Related: your UI process dying
must not be able to corrupt the runtime — it can't; stdin EOF is a clean
shutdown path, and `-r` gets the conversation back.

---

## 5. Anatomy of a turn

### 5.1 Sending

```jsonc
{ "type": "user", "message": { "role": "user", "content": "…" },
  "parent_tool_use_id": null, "uuid": "<generate-this>" }
```

- **Generate the uuid BEFORE sending and stamp your optimistic UI first.**
  The replay ack can arrive within milliseconds of the write and will race
  any post-send bookkeeping (we lost this race; bubbles stuck "queued").
- With `--replay-user-messages`, the echo (`isReplay: true`, your uuid)
  marks the message delivered.
- **Mid-turn sends are queued and COALESCED by the CLI**: three messages
  sent during a running turn reach the model as one merged turn; only the
  last uuid survives as the turn identity, but replay acks arrive for all
  three. Don't build your own send queue — send immediately, always, and
  mark bubbles delivered on their acks. (We deleted a whole client-side
  queue/watchdog subsystem when we learned this.)
- `content` accepts a plain string or full content-block arrays (images…).
- **Slash commands are just text** — `/compact`, `/model sonnet`, plugin
  commands. Output returns as `system/local_command_output`.

### 5.2 Receiving — the three-layer reality

For one assistant message you receive, interleaved:

1. **SSE deltas** (`stream_event`): `message_start`,
   `content_block_start`, `content_block_delta`
   (`text_delta`/`thinking_delta`), `message_stop`. Token-level paint.
2. **Assistant envelopes** (`assistant`): block-level *snapshots*, emitted
   once per completed content block **and again as a final full-message
   re-emission**. `message.id` is the upstream identity.
3. **Tool traffic**: `tool_use` blocks inside assistant envelopes;
   `tool_result` blocks arrive inside **`user`-typed envelopes**.

The merge algorithm that survived contact (Studio's reducer):

- Maintain at most one *open* streaming bubble — **the tail of the
  transcript**. Deltas append to it; a text block boundary
  (`content_block_start` with prior text) inserts a paragraph break.
- On an assistant envelope: merge into the open bubble with
  *snapshot-reconciling* rules — if incoming contains current text →
  replace (fuller snapshot); if current endsWith incoming → drop
  (duplicate re-emission); else append with a paragraph break. Blind
  concatenation renders everything twice ("lighthouselighthouse").
- **Chronology is sacred**: if tool cards have been appended after the
  open bubble, a *different* upstream `message.id` means a new phase of
  the turn — finalize the buried bubble and open a new one after the
  tools. Merge into a buried bubble ONLY when the upstream id matches
  (that's the re-emission of a message whose tool cards you already
  rendered). Getting this wrong makes long agentic turns render all prose
  at the top and a wall of tool cards at the bottom — which reads as
  "still working" after the reply has landed.
- One agentic turn contains **several** message streams (assistant → tools
  → assistant → …). `stream_stop`/`message_stop` end a *message*, not the
  turn.

### 5.3 Turn end — the authoritative signal

**`result` is the end of turn** — on 2.1.233 the documented
`system/session_state_changed (idle)` **never fires**; feature-detect via
capabilities before relying on it. `result` carries `subtype`
(`success` / `error_during_execution` / …), `duration_ms`, `usage`,
`total_cost_usd`, and the final text.

- **`total_cost_usd` is session-cumulative**, not per-turn. Label it
  "session $X" or users think a 92-token reply cost $0.58.
- Unblock your composer ONLY on `result`. A composer unlocked on
  `stream_stop` lets a message go down stdin mid-turn where one code path
  silently dropped it.
- On `result`, settle any tool card still marked running — the turn is
  over; "running" would be a lie (cancelled turns end up here).

### 5.4 Interrupt

`{ subtype: "interrupt" }` control request. Measured 110–116ms to take
effect. The turn ends with an error-flavored `result` — track an
"interrupt requested" timestamp and render an error-result within a short
window as a clean stop, not a failure banner. The transcript also gets a
synthetic user message `[Request interrupted by user]` — suppress it or it
renders as something the user typed. The session survives; send the next
message normally.

---

## 6. The four requests you must serve

Only four subtypes originate at the CLI. Handle all; never hang any.

| Subtype | Reply |
|---|---|
| `can_use_tool` | permission decision (§7) — or, for AskUserQuestion, an *answer* (§7.6) |
| `hook_callback` | a HookJSONOutput; `{}` is valid |
| `mcp_message` | `{ mcp_response: <JSON-RPC reply> }` (§8) |
| `elicitation` | `{ action: "accept"\|"decline"\|"cancel", content? }` |

---

## 7. Permissions — the full model

Three mechanisms compose, and users conflate them unless your UI makes
each legible (we learned this from a confused bug report that was actually
three correct behaviors):

1. **Mode** — the session-wide posture.
2. **Rules/grants** — per-tool, per-scope exceptions layered on the mode.
3. **Per-call prompts** — `can_use_tool`, for whatever the first two
   don't already decide.

### 7.1 Modes

CLI vocabulary (2.1.233): `default` (a.k.a. `manual`), `acceptEdits`,
`bypassPermissions`, `plan`, `dontAsk`. `default` is the
cross-version-safe name for manual. Set at spawn (`--permission-mode`) and
live-switch via `set_permission_mode`. `system/status` echoes mode changes
from *any* source (including the model itself) — mirror it.

### 7.2 The prompt

```jsonc
{ "subtype": "can_use_tool", "tool_name": "Edit", "tool_use_id": "toolu_…",
  "input": { … }, "permission_suggestions": [ /* PermissionUpdate[] */ ],
  "decision_reason": "…", "agent_id": null }
```

Reply allow: `{ behavior: "allow", updatedInput: <echo or MODIFY the
input>, updatedPermissions? }`. **`updatedInput` is required on allow** —
and it is also a power tool: returning modified input implements
"edit-before-run", and it is the answer channel for interactive tools
(§7.6). Reply deny: `{ behavior: "deny", message: "<shown to the model>",
interrupt?: true }`. A thoughtful deny message steers the model — we let
users type one ("keep it as it is for now") and the model follows it.

Special case: sandboxed network access arrives as synthetic tool
`SandboxNetworkAccess` with `{ host }` — render it as a network question,
not a mystery tool.

### 7.3 Auto-allow tier (pattern from the Studio)

Mirror the interactive CLI's own behavior: answer read-only inspection
silently (Read, Glob, Grep, TaskOutput, NotebookRead, WebSearch, your own
GUI-navigation MCP tools, read-only MCP servers) and prompt for anything
that writes, executes, or leaves the machine. Note the CLI *already*
doesn't ask for tools the user's settings allow — your prompt tier only
sees what's left.

### 7.4 "Always allow" is a RULE, not a mode change

Echo the CLI's own `permission_suggestions` back as `updatedPermissions`.
Shape: `{ type: "addRules"|"replaceRules"|"removeRules", rules:
[{ toolName, ruleContent? }], behavior: "allow", destination }` plus
`setMode` and `addDirectories` variants. `destination` ∈ `userSettings |
projectSettings | localSettings | session | cliArg`. Malformed entries are
dropped (degrades to allow-once), so a UI bug fails safe.

**Make the grant legible**: the scope is whatever the CLI suggested —
typically ONE tool + one path pattern. Grant `Write` and `Edit` still
prompts; the mode chip still says "Ask me"; all three facts are correct
and jointly baffling. Show active grants (tool + scope) on the mode
control, preview what a grant will cover *before* the click, and say that
grants sit on top of the mode.

### 7.5 Grant lifetime

`destination: "session"` rules live in CLI process memory. They survive
nothing: not `end_session`+`-r` resume, not a crash. Clear grant UI on any
restart path.

### 7.6 AskUserQuestion — the answer contract (verified from the CLI binary)

The model's structured-question tool reaches the host **as a
`can_use_tool` request**, and the answers travel **inside the allowed
call's input**. A bare allow sends no answers; the tool returns "The user
did not answer the questions." and the model proceeds unanswered — while
your UI believes it helped.

Input shape:

```jsonc
{ "questions": [ { "question": "…", "header": "Short label",
    "multiSelect": false,
    "options": [ { "label": "…", "description": "…" }, … ] }, … ] }
```

Answer by allowing with:

```jsonc
{ "behavior": "allow",
  "updatedInput": {
    "questions": <echo verbatim>,
    "answers": { "<question text>": "<option label>",     // single-select
                 "<question text>": ["<label>", …] },     // multiSelect
    "response": "<optional free text>",                    // "The user responded: …"
    "annotations": { "<question text>": { "notes": "…" } } // optional
  } }
```

Keys of `answers` are the **question text**, values are option **labels**.
Omit a question's key to skip it. Free-text `response` is honored with or
without picks. Result strings the model sees: "Your questions have been
answered: …" (all picks valid) / "The user answered: … Read the answers
carefully…" (mixed/custom) / "The user did not answer the questions."

Render it as a question card (options as buttons, multiSelect, free-text
path, explicit Skip) — never as an Allow/Deny permission card.

### 7.7 The trust gate you owe your users

`-p` sessions **never show the workspace-trust dialog** — hooks in
`.claude/settings.json` and servers in `.mcp.json` run in a directory the
user has never trusted. Your app must gate: before first spawn in a
project, inspect what would auto-run (hooks, MCP servers, skills), show
it, and record consent. This is a feature, not a guard clause — "here is
everything this repository will run, before it runs" beats the terminal.

---

## 8. Your app as an MCP server (the reverse channel)

Register in `initialize` (`sdkMcpServers: ["your-app"]`); the CLI mounts
you as a server and performs a standard MCP handshake tunneled over
`mcp_message` control requests: `initialize`, `notifications/initialized`,
`tools/list`, `tools/call`.

- Reply to each with `{ mcp_response: <JSON-RPC message> }`.
- **Answer notifications too** (the transport awaits a reply even for
  JSON-RPC notifications — the quirk in §3's invariant 1). A benign value
  works.
- Return `instructions` from your MCP initialize — it is injected as
  model-visible steering for your tools, free of charge. Ours says
  "prefer showing over telling: when a file or entity is under
  discussion, surface it in the UI."
- Tools register as `mcp__<server>__<tool>` and are first-class to the
  model.
- Unsolicited *host→CLI* `mcp_message` requests are the reverse channel:
  push notifications into the CLI's client (selection changed, file
  saved, user clicked).

**This mechanism is the difference between a chat panel and an agentic
app.** The Studio exposes `open_in_loom`, `reveal_entity`,
`focus_surface` — and the model genuinely uses them, navigating the app
for the user mid-conversation ("open the file I just wrote"). Design your
tool set as "what can the model do to my UI," keep the tools
GUI-navigational (then they're safe to auto-allow), and it composes with
everything else in this document.

---

## 9. Control catalog — what we've exercised

Verified live: `initialize`, `interrupt`, `end_session`,
`set_permission_mode`, `set_model` (takes effect next turn — say so in
the UI), `get_context_usage` (percentage + totals + rich per-category
breakdown; poll at turn end, never mid-turn), `mcp_status`,
`mcp_message`, `rewind_files`.

Findings:

- **`rewind_files` answers "File rewinding is not enabled" in headless.**
  Checkpointing is decided at session start; probing
  `apply_flag_settings` with `checkpointing` / `enableCheckpoints` /
  `fileCheckpointing` / `enableFileCheckpointing` was declined on
  2.1.233. Degrade the affordance honestly (explain, don't gray out
  mysteriously) until the real enable is found.
- `end_session` may be unknown to older builds — catch and fall back to
  closing stdin.
- Not yet exercised but documented and promising: `get_settings`
  (effective + per-source layers + runtime-applied model/effort),
  `reload_plugins` (wire to a file watcher for live skill editing),
  `mcp_set_servers`, `seed_read_state`, `cancel_async_message`,
  `set_max_thinking_tokens`, and the OAuth suite (`claude_authenticate` /
  `claude_oauth_callback`) for in-app login.

---

## 10. Sessions on disk — the layer under the wire

The runtime writes the same transcripts headless as interactive:
`~/.claude/projects/<slug>/<session-id>.jsonl`.

### 10.1 Slug derivation

The project path (exactly as the CLI saw its cwd — do NOT canonicalize;
`\\?\`-style canonical Windows paths slug differently), NFC-normalized,
every non-`[a-zA-Z0-9]` character → `-`; >200 chars gains a wyhash suffix
you cannot reproduce — fall back to prefix matching against real directory
names, and to case-insensitive matching on Windows.

### 10.2 Transcript line schema (what we parse in practice)

Each line is one JSON object. Relevant `type`s: `user`, `assistant`,
`system`, `attachment`, `queue-operation`, plus metadata lines
`agent-name` / `custom-title` / `ai-title` (title chain, in that
priority). Useful fields on conversation lines: `uuid` (matches the
stdout uuids — the wire and the disk share identity), `parentUuid`,
`timestamp`, `sessionId`, `cwd`, `version`, `gitBranch`, and
**`entrypoint`** — your `CLAUDE_CODE_ENTRYPOINT` value, i.e. *which app
created this session*.

Filtering rules that make parsed transcripts match what the user
experienced: skip `isMeta`, `isCompactSummary`, `isSidechain` (subagent
traffic shares the file), harness-injected wrappers
(`<command-name>`, `<local-command-*>`, `<system-reminder>`), and
compact-summary continuations. Assistant messages arrive as multiple
lines sharing `message.id` — merge consecutive same-id lines.

### 10.3 There is no wire affordance for session enumeration

Resume requires an id you already know. **The filesystem is the source of
truth for "what sessions exist"** — enumerate `<slug>/*.jsonl` (UUID
stems), title via the chain custom-title → ai-title → first real user
message, sort by newest timestamp. Never maintain your own registry of
sessions; ours drifted (a UI action deleted a registry entry while the
transcript sat intact on disk) and the enumeration rewrite deleted the
whole bug class. A session with zero real user messages is a warm spawn —
hide it.

Rehydration: parse the JSONL into text turns for any session your app
never cached — full-fidelity for text, honest degradation for tool
traffic. `-r` then resumes it live, **including sessions created by other
entrypoints** (terminal `claude` in the same project resumes fine in your
app).

### 10.4 Cost of resume

`-r` replays the conversation; context and `total_cost_usd` continue from
where they were. What does *not* come back: in-memory session permission
grants (§7.5), and anything your UI cached about the old process.

---

## 11. Version drift — posture and 2.1.233 delta

Read `system/init.capabilities` (absent ⇒ pre-2.1.205 ⇒ assume nothing).
Ignore unknown types/subtypes/fields; treat absent optionals as "old
build"; always answer control requests. Malformed things you send degrade
politely (permission updates → allow-once; hooks → `{}`), so bugs fail
safe in both directions.

Observed drift, doc → 2.1.233 live:

| Doc says | We observed |
|---|---|
| `system/init` follows the handshake | arrives with the FIRST TURN |
| `session_state_changed` is the turn-over signal | never fires; use `result` |
| permission prompts route to host in `-p` | only with `--permission-prompt-tool stdio` |
| modes `default/acceptEdits/bypassPermissions/plan/dontAsk` | all accepted; `manual` also accepted as alias of default |
| `rewind_files` restores files | "File rewinding is not enabled" headless |

Cheapest re-verification on a new build: spawn with the §2 argv, send
`initialize`, send one trivial turn, and diff what arrives against §4–§5.
Then grep the installed binary for contract strings — it is a compiled
bundle but the JS is greppable, and it is the ground truth that settled
the AskUserQuestion contract for us (search for the tool_result strings
and read the surrounding code).

---

## 12. The bug museum

Every entry cost real debugging time. Check your implementation against
each.

| Symptom | Cause | Fix |
|---|---|---|
| Every reply rendered twice ("lighthouselighthouse") | blind concatenation of block-snapshot re-emissions | snapshot-reconciling merge (§5.2) |
| Reply renders ABOVE the tool cards; turn looks stuck after it landed | text merged into the turn's first bubble regardless of interleaved tools | tail-only merge keyed by upstream message id (§5.2) |
| Sent bubbles stuck "queued" | replay ack raced post-send state write | generate uuid before send; stamp optimistic UI first (§5.1) |
| Messages sent mid-turn vanish | composer unlocked on `stream_stop`; CLI dropped mid-turn stdin in one path | `result` is the only unlock (§5.3) |
| Edit card shows "applied" before user approved | `tool_use` envelope arrives before `can_use_tool` | permission card supersedes the record card; recreate on allow |
| Uncaught "session restarted" rejections | reset rejecting in-flight control promises | pre-attach silent catch (§3) |
| Stop button renders an error banner | interrupt ends the turn with an error-flavored result | interrupt-window grace (§5.4) |
| $0.58 for a 92-token reply | `total_cost_usd` is cumulative | label as session total (§5.3) |
| Permission UI never appears | missing `--permission-prompt-tool stdio` | §2.1 |
| Model asked questions; user clicked Allow; model says user didn't answer | AskUserQuestion answers must ride `updatedInput` | §7.6 |
| "Always allow" then a different tool asks; user reads it as broken | grants are per-tool rules, not mode changes | grant visibility (§7.4) |
| Host MCP "times out" | an unanswered JSON-RPC notification | answer everything (§8) |
| Session disappears from UI forever | app-maintained session registry drifted | enumerate the disk (§10.3) |
| Child dies when tooling restarts | runtime supervised by a killable shell | detach (§4.5) |

---

## 13. Minimal host, end to end

The irreducible host, in order:

1. Spawn with §2.1 argv + your entrypoint env; cwd = project; pipe all
   three streams; enforce stdin line discipline.
2. Read stdout lines → JSON; route `control_response` to your correlation
   map; serve the four §6 request subtypes (auto-allow reads, prompt the
   rest; `{}` hooks; decline elicitations; answer all `mcp_message`s);
   feed the rest to your conversation reducer.
3. Send `initialize` (with `sdkMcpServers` if you host tools); render
   `commands`/`models` from the response.
4. Send user turns with pre-generated uuids; render §5.2's merge;
   unblock on `result`.
5. Persist the session id per project; resume with `-r` on reopen; gate
   first spawn per project behind a trust prompt (§7.7).

That is a working, resumable, permission-correct host in an afternoon.
Everything else in this document is what turns it from working into
good.

---

*Companion: `HOST_PROTOCOL.md` (source-level receipts, control catalog in
full, hook event list, IDE lockfile channel, auth flows). Implementation:
Contextua Studio `app/` — the modules named in §0 double as reference
code for every pattern above.*
