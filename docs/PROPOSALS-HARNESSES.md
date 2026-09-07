# Proposal — Aspen as a multi-harness control plane: Codex beside Claude Code

**Status:** design, 2026-09-07 (BACKLOG H-1). Written before any code,
from a read of Codex `98a5cb46b1` (`~/src/codex`, CLI 0.153.4 installed
locally) and an inventory of every place Aspen is coupled to Claude Code.
The phases at the end are the implementation plan; each ships with its
own reference doc and a rig verification that the claude path did not
regress.

---

## 0. The one-paragraph version

Aspen today is a Claude Code meta-harness that happens to have a seam
drawn around the harness (`SessionHandle`, `SessionEvent`) and then
leaks Claude through it in nine places. Codex is a good second harness
because its headless surface — `codex app-server`, line-delimited
JSON-RPC over stdio, threads and turns, approvals as server→client
requests, token telemetry, resume and fork by id — is *richer* than
Claude's, not poorer; and its on-disk world (rollouts under
`$CODEX_HOME/sessions`, `AGENTS.md`, `SKILL.md`, a hook set that mirrors
Claude's, a project trust flag) rhymes with Claude's closely enough that
Aspen's product — the fleet, the bus, boards, needs, history, migration,
replication, templates, usage — stays *one product*. The work is: (1)
finish the seam so the node and console see harness-neutral sessions,
events, prompts, tools and stores; (2) write `aspen-codex` as a second
implementation of that seam; (3) let every surface say which harness a
session runs on and degrade honestly where a capability is missing;
(4) re-verify claude at each step. Harness is a **property of a
session**, never a mode of Aspen.

---

## 1. What Codex is, for our purposes

### 1.1 Two headless surfaces; one is right

- `codex exec --json` — one process per turn, prompt on stdin, JSONL
  `thread.started / turn.* / item.*` out, exit 0/1, `resume <thread_id>`
  for the next turn. **Approvals are hard-rejected** in exec mode
  (`exec/src/lib.rs:562-569`, `:1976-2075`): the driver pre-commits to a
  policy. Fine for the SDK's fire-and-forget; wrong for an operator's
  console, which lives on the permission prompt.
- `codex app-server --listen stdio://` — a long-lived process, newline
  -delimited JSON-RPC (no `jsonrpc` field), `initialize` handshake
  (`clientInfo`, `capabilities.experimentalApi`), then `thread/start |
  resume | fork`, `turn/start | steer | interrupt`, ~100 notifications
  (`item/started`, `item/agentMessage/delta`, `item/completed`,
  `turn/completed`, `thread/tokenUsage/updated`, `thread/status/changed`
  …) and server→client **requests** for approvals
  (`item/commandExecution/requestApproval`,
  `item/fileChange/requestApproval`, `item/permissions/requestApproval`,
  `item/tool/requestUserInput`) answered by a JSON-RPC response with a
  decision (`accept | acceptForSession | acceptWithExecpolicyAmendment |
  decline | cancel`). Many threads per process, per-thread ordering,
  per-connection notification opt-out. This is the surface. It is what
  Codex's own TUI and SDK sit on.

### 1.2 The world on disk

- `$CODEX_HOME` (default `~/.codex`) is the isolation unit: `config.toml`
  (layered: system → user → profile → project `.codex/config.toml` →
  session flags), `sessions/YYYY/MM/DD/rollout-<ts>-<thread>.jsonl`
  (append-only, sometimes compressed, lines `{timestamp, ordinal, type,
  payload}` with types `session_meta | response_item | event_msg |
  turn_context | compacted | token_usage_record …`), `history.jsonl`
  (user prompts only), `memories/` (global, background-consolidated —
  not per repo), `skills/`, SQLite indexes (`state_5.sqlite` etc.; thread
  listing prefers them and falls back to a filesystem scan).
- Thread identity: `SessionMeta.id` (thread) and `session_id` (root),
  `forked_from_id` + `forked_from_ordinal_exclusive` for forks, `originator`
  (the SDK sets `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`; we will set
  `aspen`), `cwd`, `cli_version`. The file's `rollout_id` can differ from
  the thread id after a revert — key on ids, not paths.
- Instructions: `AGENTS.md` from the project root (`.git`) down to cwd,
  plus `$CODEX_HOME/AGENTS.md`; untrusted projects load none.
- Trust: `[projects."<abs path>"] trust_level = "trusted"` in config;
  untrusted projects get an internal `untrusted` approval policy and no
  project docs or project config layer.
- Permissions: `approval_policy` (`on-request` default, `never`,
  granular), `sandbox_mode` (`read-only | workspace-write |
  danger-full-access`, writable roots with `.git`/`.codex` carved out),
  named `permissions` profiles, an `auto_review` reviewer (a guardian
  subagent that answers approvals), execpolicy amendments ("always allow"
  persisted as rules), network domain allow/deny.
- Hooks: twelve events (`PreToolUse, PermissionRequest, PostToolUse,
  PreCompact, PostCompact, SessionStart, SessionEnd, UserPromptSubmit,
  SubagentStart, SubagentStop, Stop, Interrupt`) with `command |
  mcp_tool | prompt | agent` handlers and a trust hash; the legacy
  `notify = [argv]` fires on `agent-turn-complete`.
- MCP: Codex is a **client** (`mcp_servers.<name>` stdio or streamable
  HTTP, per-server allow/deny lists, OAuth). There is no `codex
  mcp-server`; the app-server replaced it.
- Skills: `SKILL.md` with frontmatter — the same format as Claude's —
  under `.agents/skills`, `.codex/skills`, `$CODEX_HOME/skills`, invoked
  by `@mention` or by detection. Plugins and marketplaces exist
  (`plugin/*`, `marketplace/*` requests) with their own registry; plugin
  `commands/` are migrated into generated skills.
- Multi-agent: `collabAgentToolCall` / `subAgentActivity` items, agent
  threads with `agent_nickname/role/path` in `session_meta`, background
  terminals (`thread/backgroundTerminals/list`).
- Usage: `thread/tokenUsage/updated {total, last, modelContextWindow}`
  per turn; `account/usage/read` and `thread_usage` with
  `estimatedUsageUsdMicros` per model/effort. Money comes from the
  harness here too.

### 1.3 What it does not have (that Claude does)

No replay ack for user input (the turn's `userMessage` item is the
acknowledgement); no `AskUserQuestion` tool (the nearest is
`item/tool/requestUserInput` and `agentMessage.questions`); no
`--plugin-dir` (plugins are installed into `$CODEX_HOME`); no
per-session system-prompt append on the wire — `developer_instructions`
is config, settable per thread through the `config` override map; no
slash commands on the wire (a fixed TUI enum); no `cost-state` line in
the transcript (cost is an API answer). Sub-agents are threads, not
sidechain files.

---

## 2. What is cross-harness and what is per-harness

The inventory (§7 of this doc lists every coupling) sorts into three
bands.

**Already harness-neutral, keep as is.** The bus (addresses, urgency,
threads, delivery physics, topology), federation and everything mesh-
wide (rosters, boards, templates, plugin registry rows, memory digests,
replication frames, the capability layer, the console-as-peer tunnel),
needs aggregation, notices, the fleet trail, servicing, evacuate,
usage *rows*, the artifact viewer, drafts, hotkeys. None of these
know what a session is made of.

**Per-harness by nature, behind the seam.** Process argv and env; the
wire protocol and its normalization; permission prompt shapes and
decisions; permission *modes*; the on-disk session store (paths, line
format, enumeration, lineage, subagent files); usage and activity
derivation; instruction injection (charter); plugin dirs; slash
commands vs skill mentions; the trust gate's autorun surface; the
hook contract and the "is this ours" marker; version inventory.

**Cross-harness *concepts* that today wear Claude's clothes — the
design work.** A session (one process, one identity, a resumable
thread on disk); a turn (starts on input, ends once); a tool call
(something the agent did, with a kind, a summary, a result); a
permission prompt (a question with a bounded set of answers); a
question to the operator; background work (subagents, tasks); tokens
and money; a transcript to render; a repo's instructions, skills and
memory; trust. Each of these gets a neutral shape in `aspen-core`, and
each harness maps into it.

---

## 3. The seam, finished

### 3.1 `AgentAdapter` and `SessionHandle`

```rust
pub trait AgentAdapter: Send + Sync {
    fn harness(&self) -> Harness;                       // Claude | Codex
    fn capabilities(&self) -> HarnessCapabilities;
    fn permission_modes(&self) -> Vec<PermissionMode>;  // {id, label, hint, posture}
    async fn spawn(&self, spec: SpawnSpec) -> Result<Box<dyn SessionHandle>>;
    fn store(&self) -> Arc<dyn SessionStore>;
    fn version(&self) -> Option<String>;
}

pub trait SessionHandle: Send + Sync {
    fn capabilities(&self) -> HarnessCapabilities;
    async fn send_user(&self, input: UserInput) -> Result<SendRef>;   // text + attachments
    async fn interrupt(&self) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
    async fn answer(&self, prompt_id: &str, decision: Decision) -> Result<()>;
    async fn set_model(&self, model: Option<&str>) -> Result<()>;
    async fn set_mode(&self, mode: &str) -> Result<()>;
    async fn context_usage(&self) -> Result<ContextUsage>;
    async fn reload(&self) -> Result<Value>;             // plugins/skills/commands
    fn runtime(&self) -> RuntimeInfo;                    // models, commands/skills, mode
}
```

Every method a capability may not cover returns `Unsupported(cap)`;
the node turns that into a 409 with a one-line reason the console
shows in place of the control ("not available for codex sessions").
`ManagedSession.handle` becomes `Arc<dyn SessionHandle>`; the seven
inherent `ClaudeSession` methods the node calls today become trait
methods. `HarnessCapabilities` grows: `fork, set_model, set_mode,
context_usage, reload, slash_commands, skill_mentions, subagents,
plugins_dir, replay_ack, question_prompts, always_allow, transcript_on_disk,
cost_from_harness`.

### 3.2 Events, normalized for real

`SessionEvent` keeps its variants; three of them change shape so the
node and console stop switching on Claude names:

- `ToolUse { id, name, kind: ToolKind, summary, path, command, input, parent }`
  — `kind ∈ {Shell, FileWrite, FileEdit, FileRead, Search, Web, Mcp,
  Agent, Question, Other}`; the adapter classifies (Claude by tool
  name, Codex by item type). `files_touched`, the artifacts verb map,
  the tool card renderers and the activity ledger key on `kind` first
  and `name` second.
- `PromptAsked { id, kind: PromptKind, tool: Option<ToolUse>, title,
  detail, questions: Option<Questions>, decisions: Vec<DecisionOption> }`
  — `kind ∈ {Permission, Question, Elicitation}`. `decisions` is the
  bounded answer set the harness offers, with labels and a `scope`
  (`once | session | always`): Claude yields `allow / allow always
  (suggestions) / deny`; Codex yields `accept / accept for session /
  accept always (execpolicy amendment) / decline / cancel`. The
  permission card renders `decisions` generically; `questions` renders
  the question card for both `AskUserQuestion` and `requestUserInput`.
- `UserAccepted { r#ref }` replaces `UserReplay`: Claude emits it on the
  replay frame, Codex on the turn's `userMessage` item (or immediately
  when `turn/start` returns). The console's "sending…" state keys on it
  unchanged.
- `TurnEnded` gains `usage: Option<TurnUsage>` (in/out/cached/reasoning,
  cumulative flag) and `cost_usd_cumulative` stays optional.
- `RuntimeInit` carries a `RuntimeInfo {model, mode, models[], commands[],
  skills[], context_window}` built by the adapter, not a raw init.

The snapshot-reconciling merge and the interrupt marker move **out of
the console into the Claude adapter**: the adapter emits well-formed
`AssistantMessage`/`TextDelta` and the console's `transcript.ts` keeps
one reducer for both harnesses. Codex's `item/agentMessage/delta` →
`TextDelta`, `reasoning/*Delta` → `TextDelta{thinking}`, `item/completed
{agentMessage}` → `AssistantMessage{text}`; `commandExecution` → `ToolUse
{Shell}` + `ToolResult{aggregated_output, exit_code}` with
`outputDelta` streamed as `ToolProgress`; `fileChange` → `ToolUse
{FileEdit}` with the diff; `mcpToolCall` → `Mcp`; `webSearch` → `Web`;
`collabAgentToolCall` / `subAgentActivity` → `Agent` (feeding the
ledger); `todo/plan` → `Status{plan}`.

### 3.3 Permissions and the posture

`PermissionPolicy`, the broker, `READ_ONLY_TOOLS` move to `aspen-core`
in neutral terms: the policy decides on `ToolKind` (read kinds auto-
allow under `ReadOnlyAuto`), and every harness maps its own decision
payloads. Aspen's operator-facing choice becomes a **posture**, one
vocabulary across harnesses, with a per-harness table underneath:

| posture | claude | codex |
|---|---|---|
| `ask` (default) | `default` | `approval_policy=on-request`, `sandbox=workspace-write` |
| `edits` | `acceptEdits` | `on-request` + execpolicy rule set from the console's "always allow" answers |
| `plan` | `plan` | `sandbox=read-only`, `on-request` |
| `auto` ("skip permissions") | `bypassPermissions` | `approval_policy=never`, `sandbox=danger-full-access` |
| `guarded` | — | `approvals_reviewer=auto_review` (Codex only) |

The harness's native mode ids stay available in the session's mode
select (from `permission_modes()`), so nothing is lost; templates,
repo defaults (`skip_permissions`) and the new-session panel speak
posture. The trust gate (§3.6) is orthogonal and stays Aspen's.

### 3.4 The session store

```rust
pub trait SessionStore: Send + Sync {
    fn harness(&self) -> Harness;
    fn exists(&self, repo, sid) -> bool;
    fn enumerate(&self, repo) -> Result<Vec<SessionInfo>>;         // title, entrypoint/originator, modified, prompts
    fn rehydrate(&self, repo, sid, after: Option<&str>) -> Result<Rehydrated>;
    fn origin(&self, repo, sid) -> Option<Origin>;                  // fork parent, first ref, originator
    fn files(&self, repo, sid) -> Vec<(rel, PathBuf)>;              // for replication and migration
    fn usage(&self, repo, sid) -> Arc<SessionUsage>;
    fn activities(&self, repo, sid, since) -> Vec<Activity>;
    fn project_dirs(&self, repo) -> ProjectDirs;                    // memory dir, artifacts roots, sentinels
    fn subagent(&self, repo, sid, id) -> Result<Rehydrated>;
}
```

Claude's implementation is today's `transcript.rs / activity.rs /
usage.rs`. Codex's reads rollouts: `enumerate` walks
`$CODEX_HOME/sessions` (date shards, the `session_meta` first line —
`cwd` filters by repo, `originator` is the entrypoint), `rehydrate`
folds `response_item` lines (messages, function calls and outputs,
reasoning) and `event_msg` lines into the same rehydrated shape, with
the compressed-rollout variant handled and `history_base` prefixes
followed; `usage` folds `token_usage_record` lines and, when a process
is live, the `thread/tokenUsage/updated` totals; `activities` derives
from `collabAgentToolCall`/`subAgentActivity`/`turn_context` (and, live,
`thread/backgroundTerminals/list`); `files` is the rollout file (and its
agent threads' rollouts); `project_dirs` gives no per-repo memory dir
(Codex memory is global; §5). `session_id` in the agents table becomes
the harness's thread id — the same column, harness-qualified by the
new `agents.harness` column.

### 3.5 Instructions, charter, bus tools

- **Charter**: Claude `appendSystemPrompt`; Codex `developer_instructions`
  in the thread's `config` override at `thread/start`. Same text (the
  bus explanation, the topology guidance).
- **Bus tools** (`bus_send`, `bus_inbox`, `bus_status`): Codex reaches
  MCP servers by config, so Aspen ships a stdio bridge — `aspen mcp
  --agent <key> --token <t>` — a small MCP stdio server that forwards
  `tools/call` to the node's HTTP API. Registered per thread through the
  `mcp_servers.aspen` config override at start. The same bridge can
  replace Claude's control-channel tunnel later (it would let `claude`
  run with `--mcp-config` instead of the in-process server), but that is
  not required for v1.
- **Attachments**: Codex `UserInput::localImage {path}` for images;
  non-image attachments are saved under the attachments dir and
  referenced by path in the text (`[attachment n: name]` stays).

### 3.6 Trust, hooks, adoption

- The **trust gate** stays Aspen's, per repo, and its autorun surface
  becomes per harness: claude reads `.claude/settings*.json` hooks,
  `.mcp.json`, `.claude/skills`; codex reads `.codex/config.toml`
  (`hooks`, `mcp_servers`), `.agents/skills`, `.codex/skills`. One repo,
  one consent, both surfaces shown. On acknowledge Aspen also writes
  Codex's own trust (`[projects."<path>"] trust_level = "trusted"`),
  because Codex refuses `AGENTS.md` and project config without it.
- **Hooks**: `aspen hooks install --harness codex` writes
  `SessionStart`/`SessionEnd` command hooks into `$CODEX_HOME/config.toml`
  (the same relay endpoint `POST /api/hooks/session`, payload tagged
  with the harness); "is this ours" is `originator == "aspen"` for Codex
  and `CLAUDE_CODE_ENTRYPOINT == aspen` for Claude.
- **Adoption** works from the store trait's `origin()`: a Codex thread
  forked from ours (`forked_from_id`) or driven from the TUI while the
  agent is down raises the same fork/resumed needs.

### 3.7 Skills, commands, plugins

- **Skills are the shared currency.** `SKILL.md` is one format; the
  Skills section edits per-harness roots (`.claude/skills`,
  `.agents/skills`) and offers *share with both*. Codex has no slash
  commands on the wire; its `skills/list` becomes the composer's
  `@skill` autocomplete for Codex sessions, as `/command` is for Claude.
- **Plugins**: Aspen's library and `--plugin-dir` are Claude-only;
  rules and templates carry a `harness`, the Plugins page says so, and
  a Codex session shows its own installed plugins read-only
  (`plugin/installed`). Managing Codex marketplaces through Aspen is
  a later round, if wanted.

### 3.8 Migration, replication, memory, templates

- **Migration** carries what the store's `files()` names: for Codex,
  the rollout (tier A) and agent-thread rollouts (B); no C (memory is
  not per repo); D and E unchanged. Import places a rollout under the
  target's `$CODEX_HOME/sessions/<shard>` (the shard comes from the
  filename), then `thread/resume` by id; the SQLite index falls back to
  a filesystem scan. `PathCtx` gains `{{CODEX_HOME}}`; `session_meta.cwd`
  is rewritten like Claude's `cwd`.
- **Replication** ships the same files; the replica reader uses the
  store trait of the replica's harness (recorded on the row).
- **Memory convergence** stays Claude's per-repo memory; Codex's global
  `memories/` is out of scope for v1 (a later round could converge
  `$CODEX_HOME/memories` across nodes the same way, keyed by node).
- **Templates** and the new-session panel gain a `harness` field;
  a template for Codex can carry `model`, posture, `reasoning_effort`,
  `sandbox`; plugins apply only to Claude templates.
- **Usage** rows carry `harness`; the Usage page groups by it.

---

## 4. What the console affords

One product, one vocabulary, a harness badge:

- **Now / fleet / boards / rail**: every session shows a harness glyph
  (`cl` / `cx`) beside the repo chip; dynamic boards accept `harness:`.
- **New session panel**: a harness selector first (defaults to the
  repo's last-used harness), then model (from the adapter's model list),
  posture, charter, args; template select filters by harness.
- **Session page**: the mode select shows the harness's modes with the
  posture as the first row; tool cards render Codex items natively
  (command with streaming output and exit code, file change with diff,
  MCP call, web search, reasoning summary, plan/todo); permission cards
  render the decision set the harness offered ("accept for session",
  "accept always (rule)"); questions render for both; the `@skill`
  autocomplete for Codex, `/command` for Claude; context meter from
  `context_usage` or the token-usage notification; the status bar's
  cost from `thread_usage`.
- **Needs you / notices / activity / usage / history**: unchanged, with
  the badge; the activity drawer lists Codex agent threads and
  background terminals; the subagent view opens an agent thread's
  rollout.
- **Mesh list**: per-repo skills for both harnesses; the trust review
  shows both autorun surfaces; the harness defaults form has one row
  per installed harness (`claude args`, `codex config overrides`);
  inventory shows both versions.
- **Degradation is explicit**: a control the harness lacks is shown
  disabled with the reason, never hidden silently — the operator learns
  what each harness can do by using Aspen.

---

## 5. Non-goals for v1 (and why)

Codex plugin/marketplace management through Aspen (Codex's own registry
is fine and the two do not overlap); Codex memory convergence (global,
background-consolidated, a different thing); realtime/voice threads;
`turn/steer` mid-turn injection (the bus's boundary delivery is enough
for now; a later capability); guardian auto-review beyond exposing the
`guarded` posture; Codex cloud tasks; the app-server daemon shared with
the Codex TUI (Aspen runs its own app-server per session — §6.1).

---

## 6. Decisions that need stating

### 6.1 One app-server process per session, not per node

Codex's server can host many threads; Aspen still spawns one
`codex app-server --listen stdio://` per session. Reasons: the node's
model of liveness, revive, the shutdown ladder, the `live` mark, per-
session env and cwd, and crash isolation all map to a process; the
cost is one idle process per session (Codex unloads idle threads
itself). A shared per-node server is a later optimization behind the
same adapter.

### 6.2 Rollouts are read from disk, live state from the wire

For enumeration, rehydration of a session that is not running, usage
and lineage, Aspen reads rollout JSONL (the format is plain JSON per
line; the caveat in Codex's source concerns their own serde envelope,
not the file). For a running session, the adapter's event stream is
the source and `thread/items/list` is the delta path — the same split
Claude has today (transcript vs stream).

### 6.3 The Claude adapter absorbs its quirks

The snapshot merge, the replay ack, `AskUserQuestion` as a question,
`suggestions` as "always allow", the interrupt marker, task
notifications as activity ends — all move below the seam, where DESIGN
§5 said they belonged. This is the one refactor that risks Claude
regressions and it is done first, alone, and verified on the rig
before Codex code exists.

### 6.4 Harness is per session and per repo default, never global

`agents.harness`, `templates.spec.harness`, `repos.default_harness`,
`settings.harness[<name>]`. There is no "switch Aspen to Codex".

---

## 7. The coupling inventory (from the code, 2026-09-07)

Real design problems — solved by §3: no adapter trait (`node.rs:52`,
`:616-699`, seven inherent-method call sites); transcript-as-registry
(`transcript.rs`, ~15 call sites across node/api/federation/adoption/
artifacts/migrate/replicate/memory); permission types in the Claude
crate (`session.rs:23-40`, `broker.rs`, `permit.rs`), five Claude modes
in the console (`Session.tsx:146`), AskUserQuestion special-cased in
three layers; activity from tool names and `<task-notification>`
(`activity.rs`); the console's snapshot merge and replay state
(`transcript.ts:1-60`); `--plugin-dir` and the marketplace model
(`plugins.rs`); trust surface and hooks (`trust.rs`, `main.rs:985-1080`,
`CLAUDE_CODE_ENTRYPOINT`); `cost-state` pricing (`usage.rs`); no
`harness` column.

Trivially parameterizable — done alongside: argv/binary/version
inventory, `"claude"` literals in settings/CLI/labels, tool-name→verb
tables (`artifacts.rs:96`, `node.rs` files_touched), `toolViews.tsx`
renderers, model/context/status normalizers, slash-command list, skills
directory names, `harness` in migration preflight strings, model label
stripping.

Already neutral: the bus, federation, boards, templates rows, needs,
notices, servicing, evacuate, the tunnel, `settings.harness`, the MCP
tool registry, `normalize.rs`'s role.

---

## 8. Plan

Each phase is a tagged release with its reference doc and a rig run of
the existing claude flows (spawn, message, permission, question,
interrupt, resume/fork, move, replicate, board, template, usage,
activity, notices).

**Phase 0 — the seam (v0.20).** `aspen-core`: `Harness`, `AgentAdapter`,
the extended `SessionHandle`, `HarnessCapabilities`, `ToolKind`,
`PromptAsked` with `decisions`/`questions`, `UserAccepted`, the neutral
permission policy and broker, `SessionStore`, `SessionInfo`, `Origin`,
`ProjectDirs`. `aspen-claude` implements all of it and absorbs the
merge/replay/question quirks. Node: `ManagedSession.handle: Arc<dyn
SessionHandle>`, a `harnesses: HashMap<Harness, Arc<dyn AgentAdapter>>`
registry, `agents.harness` column, every `aspen_claude::` call routed
through the store trait, tool-kind tables, posture mapping. Console:
generic permission card from `decisions`, tool cards keyed by kind, mode
select from capabilities, harness badge (all `claude` for now). Docs:
HARNESSES.md (the seam as built). **No behavior change for claude.**

**Phase 1 — `aspen-codex` (v0.21).** The crate: process spawn
(`codex app-server --listen stdio://`, env `CODEX_INTERNAL_ORIGINATOR_
OVERRIDE=aspen`), the JSON-RPC client (ids, pending map, server→client
requests routed to the broker, notifications normalized), thread
start/resume/fork with `cwd`, model, posture mapping, `developer_
instructions`, `mcp_servers.aspen` bridge; `aspen mcp` stdio bridge;
the rollout store (enumerate, rehydrate, origin, usage, files); trust
write; version inventory. CLI/API: `harness` on spawn. Live-verified
against codex 0.153.4 on the rig: spawn, turn, streaming, command
approval accept/decline/for-session, file-change approval, interrupt,
resume, fork, usage, exit. Docs: CODEX_RUNTIME_REFERENCE.md (field-
verified, like Claude's).

**Phase 2 — the console and the gates (v0.22).** Harness selector and
badges; Codex tool renderers; `@skill` autocomplete; question prompts
from `requestUserInput`; the trust review with both surfaces and the
Codex trust write; `aspen hooks install --harness codex` and adoption
from rollouts; Usage/History/Activity with the badge; templates and
repo defaults with harness; the harness defaults form per harness.

**Phase 3 — the mesh features (v0.23).** Migration and replication of
Codex sessions (rollout placement, `{{CODEX_HOME}}`, resume by id on the
target), evacuate and bring-here, preflight per harness, replica
reading by harness, templates spawning Codex sessions on peers, the
bus bridge over the mesh. Backlog after: shared per-node app-server,
Codex memory convergence, Codex plugins in the library, `turn/steer`.
