# Harnesses: the seam as built

**Status:** reference for v0.20–v0.23 (2026-09-07), phases 0–3 of
PROPOSALS-HARNESSES.md — the Codex swath as built. Code: `crates/aspen-core/src/{harness,permission,
store,adapter,event}.rs` (the vocabulary and traits),
`crates/aspen-claude/src/adapter.rs` (the Claude implementation),
`node.rs` (`adapters`, `store_for`, `harness_of`, the neutral spawn),
`permit.rs` (the neutral operator broker), `tools.rs` (the bus tools as a
`ToolProvider`).

## 1. The vocabulary (aspen-core)

- `Harness` — `claude | codex`; a property of a session, a repo default,
  a template, never a mode of Aspen.
- `ToolKind` — what a call does: `shell | file_write | file_edit |
  file_read | search | web | mcp | agent | question | other`. Adapters
  classify; the node's silent policy, `files_touched`, the artifact verbs
  and the ledger key on it.
- `PromptKind` — `permission | question | elicitation`; `DecisionOption
  {id, label, allow, scope: once|session|always, payload}` — the bounded
  answers a prompt takes, as the harness offers them; the console renders
  them and sends the chosen `id` back.
- `Posture` — `ask | edits | plan | auto | guarded`, Aspen's one
  operator-facing vocabulary; each adapter maps it to its own modes
  (`AgentAdapter::mode_for_posture`) and lists its modes with the
  posture they realize (`PermissionMode`).
- `HarnessCapabilities` — every flag gates a control and a method:
  streaming, interrupt, mid-turn inject, permission callback, in-process
  MCP, resume, fork, set model, set mode, context usage, reload, slash
  commands, skill mentions, subagents, plugin dirs, replay ack, question
  prompts, always-allow, transcript on disk, cost from harness.
- `RuntimeInfo` — model, mode, models, commands, skills, context window,
  plus the raw handshake.

## 2. The traits

- `AgentAdapter` — `harness()`, `capabilities()`, `permission_modes()`,
  `mode_for_posture()`, `spawn(SpawnSpec)`, `store()`, `version()`,
  `binary()`. The node keeps `NodeInner.adapters: HashMap<Harness, …>`;
  claude is always registered.
- `SessionHandle` — `id, harness, capabilities, runtime_info, send_user,
  send_user_content, interrupt, shutdown`, and capability-gated
  `set_model, set_mode, context_usage, reload` whose defaults return
  `Unsupported {harness, what}` (a 409 with a one-line reason at the API).
  `ManagedSession.handle` is `Arc<dyn SessionHandle>`.
- `SessionStore` — `exists, modified, enumerate, rehydrate,
  rehydrate_after, rehydrate_file, origin, files, main_path, usage,
  activities, activity_counts, subagent, project_dirs, discover_repos`.
  `NodeInner::store_for(harness)` returns it; every transcript,
  activity, usage, replication and preflight read in node/api/federation
  goes through it. `enumerate_all(repo)` merges every harness's sessions
  for a repo, each row tagged.
- `PermissionBroker` and `PermissionPolicy` live in core; the policy
  reasons on `ToolKind` and `PromptKind`; `PermissionRequest` carries
  kind, prompt kind, decisions and questions; `BrokerDecision` carries
  the chosen `decision_id`. The operator broker (`permit.rs`) is
  harness-neutral: questions always reach the operator, the bus tools
  are auto-allowed by name, everything else by kind.
- `ToolProvider` — the bus tools (`bus_send`, `bus_status`, `bus_inbox`)
  as `list()` + `call()`; Claude mounts it as its in-process MCP server
  (`McpServer::from_provider`); Codex will reach it through a stdio bridge.

### 2.1 The session's identity in the environment (v0.24.4)

Every harness process starts with the session's identity in its
environment, so hooks, MCP servers and scripts the harness launches know
which agent they serve without being told:

| variable | value |
|---|---|
| `ASPEN_AGENT` | the bus address, `name@repo` (what `bus_send` uses) |
| `ASPEN_AGENT_NAME` | the bare name the operator chose |
| `ASPEN_CHANNEL` | the repo's handle (the channel) |
| `ASPEN_NODE` | this node's name |
| `ASPEN_SESSION_ID` | Aspen's session id (Claude's session id; Codex assigns its own thread id later) |
| `ASPEN_NODE_API` | the node's local API base URL, when the daemon listens |
| `ASPEN_NODE_TOKEN` | the node's API token, when it has one |

The node fills `SpawnSpec::env`; each adapter passes it to its process
(Claude `extra_env`, Codex `ProcessSpec.extra_env`), validating names.

## 3. Events

`SessionEvent::ToolUse` gains `tool_kind`, `summary`, `path`, `command`;
`PermissionAsked` gains `prompt_kind`, `tool_kind`, `decisions`,
`questions`; `TurnEnded` gains `usage` (`{input, output, cache_read,
cache_create, cumulative}`). `UserReplay` stays the acceptance ack (a
harness without a replay emits it when input is accepted).

## 4. What is per harness now

`SpawnSpec` (neutral) → `ClaudeConfig` in `aspen-claude::adapter`; the
harness defaults key is the harness name (`settings.harness["claude"]`);
plugin dirs are passed only when `capabilities.plugin_dirs`; the
`agents.harness` column and `repos.default_harness` record the choice;
`POST /api/agents` and the `spawn` op take `harness` and `posture`;
`GET /api/node` lists `harnesses` with versions, capabilities and modes;
agent JSON carries `harness` and, while live, `capabilities`; prompts
carry `prompt_kind`, `tool_kind`, `decisions`, `questions`; permission
answers accept `decision_id`.

Still Claude-specific behind the seam (by design, per the proposal):
adoption's lineage scan, the trust gate's autorun surface, hooks, the
migration `PathCtx`, memory dirs, and the plugin library — each becomes
per-harness in phases 2–3.

## 5. Verified (rig, 2026-09-07)

With the seam in place and claude the only harness, the rig ran the
existing flows unchanged: spawn, message, permission prompt with allow,
question, interrupt, revive, transcript delta, activities, usage,
notices, boards, move — the console reading the same shapes plus the new
fields.

## 6. The Codex adapter (v0.21)

`crates/aspen-codex` (CODEX_RUNTIME_REFERENCE.md is the field-verified
wire and disk reference). Registered by the node when the `codex` binary
resolves on PATH (`ASPEN_CODEX_BIN` overrides the name); `GET /api/node`
lists it with its version, capabilities and modes.

- **Process**: `codex app-server --listen stdio://` per session
  (`rpc.rs`: JSON-RPC line client, request correlation, server→client
  requests, notifications). Trust for the cwd and the `aspen` MCP server
  go on the command line as `-c` overrides; `config.toml` is never
  written.
- **Session** (`session.rs`): `thread/start | resume | fork` with the
  posture's approval policy and sandbox and the charter as
  `developerInstructions`; `turn/start` per message (`turn/steer` when
  a turn is running); `turn/interrupt`; model and mode changes ride on
  the next turn; context usage from `thread/tokenUsage/updated`.
- **Approvals**: command, file-change, permission-profile and MCP-tool
  (elicitation) requests become neutral `PermissionRequest`s with the
  decision set Codex offers (`accept`, `acceptForSession`, the execpolicy
  amendment as "always", `decline`); the operator broker and the
  kind-based policy apply unchanged. Questions — both
  `item/tool/requestUserInput` and the async agent-message form — reach
  the console's question card; answers go back the way Codex expects.
- **Tools**: the bus tools reach Codex through `aspen mcp`, a stdio MCP
  server the node registers per session; it lists from
  `GET /api/bridge/tools` and forwards to `POST /api/bridge/call`. Tool
  names are `mcp__aspen__<tool>`, so the by-name auto-allow holds.
- **Store** (`store.rs`): rollouts under `$CODEX_HOME/sessions`;
  enumeration by `session_meta.cwd`; rehydration from `item_completed`
  lines into the console's item shape; a fork's `history_base` followed
  recursively; usage from `token_count` lines; `files()` lists the
  rollout and its history base.
- **Modes**: `on-request` (ask/edits), `untrusted`, `read-only` (plan),
  `full-access` (auto), `auto-review` (guarded).

Verified on the rig (2026-09-07, codex 0.153.4): spawn with posture,
streaming, command approval accept and decline by decision id,
sandbox-escape prompt, MCP-tool approval, bus round trip with a Claude
agent, async question answered from the console API, interrupt, revive
across a daemon restart (`thread/resume`), branch (`thread/fork`) with
the parent history rehydrated, usage, context, runtime inventory
(models, skills), session enumeration mixed with Claude's.

## 7. The console and the gates (v0.22)

What became harness-aware above the seam, and how each surface speaks
about a harness without naming one:

- **Choosing**: the new-session panel's runtime select (shown when the
  node lists more than one), a repo's default runtime (Library; `POST
  /api/repos/harness`), a template's `spec.harness`, and the session
  header's chip. Resolution order for a spawn: the request, the agent's
  last harness, the repo default, claude.
- **The session page**: the mode select lists the harness's own modes
  (`runtime.modes`, with hints), the model select its models; `$name`
  in the composer offers the runtime's skills (Codex's mention form;
  the adapter sends a skill block beside the text). Tool cards render
  Codex's `commandExecution`, `fileChange` (per-path diffs) and
  `webSearch` items; extension items (a sleep while a question waits)
  are status, not cards.
- **Prompts**: the gate card renders the harness's decision set; the
  question card serves Claude's `AskUserQuestion`, Codex's
  `requestUserInput` and Codex's async agent question alike.
- **Trust**: the review lists Codex's project surface beside Claude's,
  each entry labelled — `.codex/hooks.json` and `[hooks]` in
  `.codex/config.toml`, `[mcp_servers.*]`, skills under `.codex/skills`
  and `.agents/skills`.
- **Hooks and adoption**: `aspen hooks install --harness codex` writes
  the same SessionStart/SessionEnd relay into `$CODEX_HOME/hooks.json`
  (Codex's hook file shares Claude's shape and payload). The adoption
  scan walks every harness's store; for Claude the newest line's
  entrypoint says who wrote it, for Codex a fork inherits its parent's
  originator, so "ours" is what the registry knows. Verified: a Codex
  thread forked outside Aspen is raised as a fork of its agent and can
  be split into a new Codex agent; a Claude fork outside is still
  raised as before.
- **Defaults**: Library carries a defaults form per harness
  (`settings.harness.<name>.args`).

## 8. The mesh features (v0.23)

Everything that carries a session between machines reads the session
through its store and names the harness in the manifest:

- **Migration** (MIGRATION.md): `AgentSpec.harness` rides in the bundle;
  tier A is whatever `files()` lists — Claude's transcript (and the
  transcripts bookmarks point at), Codex's rollout and the rollouts its
  history base chains to — placed on the target where that harness
  keeps them (`{{CODEX_HOME}}/sessions/<date>/…`, a new path anchor).
  Tiers B and C are Claude-only (Codex has no subagent files and no
  per-project memory); tier D reads touched paths per harness
  (`artifacts::touched_paths_for`). Import registers the agent on the
  manifest's harness; revive resumes it there (`thread/resume` with
  the localized cwd). Verified: a Codex session moved j2 → j1 resumed
  with its memory and reported its new node over the bus.
- **Preflight** reports the harness name and the version on both ends;
  a target without the harness is not accepting, with the reason.
- **Replication** already listed files per store (v0.20); a replica
  carries its harness so the fallback reader rehydrates it right.
- **Artifacts**: the viewer serves the Codex sessions root and the
  files a Codex session changed (from `FileChange` items) or read (from
  the parsed commands).
- **Activity pump**: running-activity ids come from the session's
  store, not a Claude call.
- **Templates on peers**: the harness in a template's spec reaches the
  peer's spawn (the template spawn reads it on whichever node runs it).

## 9. Which model (v0.25.3)

`WorkSummary.model` is the model named in the latest assistant message
(`message.model`, both harnesses); rehydrated items carry `model` too. The
console shows it beside the model select, and resolves the select's
"default" through the harness's model list (Claude: the entry with
`value: "default"` carries `resolvedModel`).
