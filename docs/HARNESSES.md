# Harnesses: the seam as built

**Status:** reference for v0.20 (2026-09-07), phase 0 of
PROPOSALS-HARNESSES.md. Code: `crates/aspen-core/src/{harness,permission,
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
