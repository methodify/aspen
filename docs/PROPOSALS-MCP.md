# Proposal: MCP servers as a first-class surface, and harness defaults mesh-wide

**Status:** proposed 2026-09-08, for discussion. Field-verified against
Claude Code 2.1.265 (the control channel) and read from Codex 0.153.4's
protocol; nothing here is built yet.

## 0. The ask, as put

A session started with a plugin that provides several MCP servers and
another that runs a monitor for the session's life. The agent reported
the primary MCP server down. Aspen had no surface that showed MCP
status or offered anything to do about it; `/mcp` was listed among the
commands and, sent, answered that it must be run in the TUI. The operator
found out from the agent, after the fact.

## 1. What exists today

- **At start, once.** Claude's `system/init` frame lists
  `mcp_servers: [{name, status}]` with `status ∈ connected | failed |
  needs-auth | pending | disabled`. Aspen keeps the whole frame as the
  session's `inventory` (`RuntimeInit.raw`) and reads `commands`,
  `models`, `skills` and `plugins` from it. It never reads
  `mcp_servers`, so a server that failed at start is in a JSON blob the
  console fetches and does not look at.
- **`/mcp`** is a TUI-local command: the CLI's stream-json mode does not
  implement it and says so. The console lists it because the init's
  `slash_commands` includes it. Same for `/plugins`-style local commands.
- **The trust review** lists the repo's `.mcp.json` servers by name and
  command before first spawn — configuration, not state.
- **Monitors** (the second plugin) are visible when the plugin runs them
  through the harness's `Monitor` / `ScheduleWakeup` / `CronCreate`
  tools: they land in the activity ledger (ACTIVITY.md) with counts on
  the fleet card and a drawer on the session page. A monitor that a
  plugin starts as a hook-launched process, outside the tool stream, is
  invisible to Aspen; nothing in the harness reports it either.
- **Codex** sessions surface MCP startup as `mcpServer/startupStatus/
  updated` notifications (Aspen logs them as `Raw`) and nothing more.

So the gap is real and narrow: the state exists on the wire, and the
controls exist, and Aspen shows neither.

## 2. What the harnesses offer (verified)

### 2.1 Claude Code, over the control channel

Probed live (2026-09-08) with a repo `.mcp.json` naming a working stdio
server (the `aspen mcp` bridge) and a broken one (`/bin/false`):

| control request | reply |
|---|---|
| `mcp_status` | `{ mcpServers: [ {name, status, serverInfo?: {name, version}, config: {type: stdio\|http\|sse\|claudeai-proxy, command?, args?, url?, id?}, scope: project\|user\|local\|plugin\|claudeai, tools?: [{name, annotations}], error?: string} ] }` — `status ∈ connected \| failed \| needs-auth \| pending \| disabled` |
| `mcp_reconnect {serverName}` | `{}` on success; a control **error** carrying the failure text on failure (`"Connection closed"` for the broken one) — the error is the diagnosis the operator needs |
| `mcp_toggle {serverName, enabled}` | `{}`; the next `mcp_status` shows `disabled` / `connected` |
| `mcp_authenticate {serverName}` | `{ authUrl?, requiresUserAction }` — OAuth servers (the `needs-auth` ones) |
| `mcp_clear_auth {serverName}` | `{}` |
| `mcp_set_servers {servers}` | `{ added[], removed[], errors{} }` — add or remove servers for this session without a restart |
| `reload_plugins` | `{ commands, agents, plugins, mcpServers, error_count }` — re-reads plugin manifests; Aspen already calls it for the plugins menu |

The CLI also lists servers in `system/init` (`mcp_servers`) and in
`reload_plugins`' reply. There is no push notification when a server
drops mid-session; status is pull.

### 2.2 Codex, over the app-server

| request / notification | shape |
|---|---|
| `mcpServerStatus/list {threadId?, detail?: full\|toolsAndAuthOnly, cursor?, limit?}` | `{ data: [ {name, runtimeStatus: notStarted\|starting\|connected\|authenticationRequired\|failed\|cancelled\|disabled, pluginId?, serverInfo?, tools: {name → Tool}, toolsError?, resources[], resourceTemplates[], authStatus: unknown\|unsupported\|notLoggedIn\|bearerToken\|oAuth} ] }` |
| `config/mcpServer/reload` | re-reads the MCP config and restarts servers (the reconnect) |
| `mcpServer/oauth/login {name}` → `mcpServer/oauthLogin/completed` | the auth flow |
| `mcpServer/startupStatus/updated {threadId, name, status: starting\|ready\|failed\|cancelled, error?, failureReason?}` | **push**, per server, at thread start and on reload |

Codex has no per-server toggle; disabling is a config edit. It does have
the push notification Claude lacks.

## 3. The design

The rule from HARNESSES.md holds: one neutral vocabulary, each adapter
maps its own.

### 3.1 The seam (aspen-core)

```rust
pub struct McpServerState {
    pub name: String,
    pub status: McpStatus,          // Connected | Failed | NeedsAuth | Pending | Disabled
    pub error: Option<String>,      // the failure text, when the harness gives one
    pub scope: Option<String>,      // project | user | plugin:<id> | claudeai | …
    pub transport: Option<String>,  // stdio | http | sse | proxy
    pub command: Option<String>,    // stdio: the command line; http: the url
    pub server: Option<String>,     // serverInfo name/version when connected
    pub tools: Vec<String>,
    pub plugin: Option<String>,     // the plugin that provides it, when known
}

// SessionHandle, capability-gated like the rest:
async fn mcp_servers(&self) -> Result<Vec<McpServerState>>;
async fn mcp_reconnect(&self, name: &str) -> Result<()>;      // Err carries the harness's reason
async fn mcp_toggle(&self, name: &str, enabled: bool) -> Result<()>;
async fn mcp_authenticate(&self, name: &str) -> Result<McpAuth>; // { url?: String, requires_user: bool }
```

`HarnessCapabilities` gains `mcp_status`, `mcp_reconnect`, `mcp_toggle`,
`mcp_auth`. Claude: all four. Codex: status and reconnect (reload), auth
via its OAuth login; toggle absent, shown as such.

`SessionEvent` gains `McpChanged { servers: Vec<McpServerState> }`,
emitted by the adapter whenever it learns a new picture: Claude at init
(from `mcp_servers`), after any of the calls above, and after
`reload_plugins`; Codex on every `startupStatus/updated` (one server) and
after a list.

### 3.2 The node

- `ManagedSession.mcp: Mutex<Vec<McpServerState>>` — the last picture,
  refreshed from `McpChanged`. `agent_json` carries a summary
  `mcp: {connected, failed, needs_auth, disabled, pending}` while live;
  the roster carries the same so a peer's fleet view has it.
- **Notices** (NOTIFICATIONS.md): `mcp_failed` when a server is `failed`
  or `needs-auth` at session start or drops later, with the error text —
  the toast the operator should have had. `mcp_recovered` when it comes
  back. Same plumbing as `permission` / `question`: bell, toast, webhook,
  command hook.
- **Needs you**: a failed server is a *needs you* row beside prompts and
  questions — "MCP `linear` failed: Connection closed — reconnect /
  disable" — on the Now page and the bell. It clears when the server
  connects or is disabled. This is the fix for "found out from the
  agent".
- **Refresh**: Claude has no push, so the node re-asks `mcp_status` at
  turn boundaries (cheap; one control request) and on demand. Codex is
  push.
- API: `GET /api/agents/{name}/mcp`, `POST …/mcp/{server}/reconnect`,
  `POST …/mcp/{server}/toggle {enabled}`, `POST …/mcp/{server}/auth`;
  proxied over the mesh like `runtime`, `model`, `mode`.

### 3.3 The console

- **An "mcp ▾" menu in the session header**, beside plugins and activity:
  one row per server — status chip (green connected, red failed, amber
  needs auth, grey disabled), the server's name and the plugin it comes
  from, tool count with the names on hover, the transport and command,
  and the error text in red under a failed row. Actions per row:
  **reconnect** (with the harness's reason inline when it fails again),
  **disable / enable**, **authenticate** (opens the auth URL in a new
  tab; polls status). A footer "reload plugins" reuses the existing call.
  The header button shows a count and turns red when anything is
  failed: `mcp 4 · 1 down`.
- **`/mcp` in the composer opens that menu** instead of sending the
  command to a harness that cannot run it. The same for the other
  TUI-local commands the init lists (`/plugins`, `/config`, `/status`…):
  the console maps the ones it has a surface for and greys the rest
  with "terminal only" in the autocomplete, rather than sending them.
- **Fleet**: a red chip on the Now card (`1 mcp down`) and the needs-you
  row above; Board panes show the chip on the pane header.
- **Session start**: the trust review already lists MCP servers; after
  the first init the mcp menu is populated, and a failure at start
  raises the notice immediately.

### 3.4 Monitors and plugins

- A plugin's monitor that runs through the tool stream already shows in
  the activity drawer; the ask suggests the drawer is not where people
  look. Proposal: the header's activity button takes the same treatment
  as mcp (a count, red when a monitor has *stopped* unexpectedly), and a
  monitor's end raises `activity_settled` as today.
- A plugin that starts a monitor as a **hook-launched process** is
  outside every harness's view. Aspen could watch its own process tree
  for children of the session (a `ps` walk on Linux/WSL, `CreateToolhelp`
  on Windows) and list them as "processes" in the drawer, with the
  command line and a stop button. Worth doing, but it is inference, not
  a contract; second phase.
- The plugins menu gains, per plugin, the MCP servers it provides with
  their status — the operator's question was "is my plugin working",
  and the answer is its servers.

### 3.5 Codex specifics

`mcpServerStatus/list {threadId, detail: "toolsAndAuthOnly"}` maps to the
same `McpServerState`; `runtimeStatus` folds `notStarted | starting →
pending`, `authenticationRequired → needs-auth`, `cancelled → failed`.
`config/mcpServer/reload` is the reconnect (all servers; the console
says so). `mcpServer/oauth/login` is authenticate; completion arrives as
a notification. The `aspen` bridge shows up as a server like any other,
which is a useful self-check.

## 4. Phasing

- **v0.24 — the surface and the defaults** (one round): §3 in full,
  plus §5 (harness defaults as a synced table with per-node overrides,
  the console panel with the effective-value table, the migration).
  Also: core types and handle methods,
  Claude and Codex adapters, node cache + notices + needs-you row + API,
  the mcp menu, `/mcp` and the other local commands mapped or greyed,
  fleet chip. Verified on the rig with a working and a broken server on
  both harnesses, including reconnect failing with the reason shown,
  toggle, and the needs-you row clearing.
- **v0.25 — processes** (if wanted): child processes of a session in the
  activity drawer, for hook-launched monitors.

## 5. Second feature: harness defaults are mesh-wide (M-3)

### 5.1 The problem

The console's "Claude defaults" / "Codex defaults" forms sit on the Mesh
tab's list view, beside every node's repos, but they write
`settings.harness.<name>.args` in *this node's* `settings.json`
(SETTINGS: per-node, never synced). A session spawned on a peer uses the
peer's file, which the console cannot edit from here. The form's
placement promises mesh-wide; the storage is node-local.

### 5.2 The design

Follow SYNC.md §4 exactly — rows, a digest in the roster, a pull op,
newest-wins by HLC — the same fifty lines boards and templates use:

```
harness_defaults(scope TEXT, harness TEXT, args TEXT, updated_at REAL, deleted INTEGER, PRIMARY KEY(scope, harness))
```

- `scope` is `mesh` (every node) or `node:<name>` (one node's override).
- Resolution at spawn, on the node that spawns: `node:<me>` row →
  `mesh` row → the legacy `settings.harness[<name>].args` → nothing.
  The legacy file keeps working for a node that has not been touched
  from the console; the first console save for that node writes a
  `node:<name>` row and the file stops mattering.
- Args are still parsed and guarded per node at spawn (`settings.rs`:
  protocol-owned flags refused), so a bad mesh-wide value is refused on
  every node with the same message.
- Roster: `harness_defaults_digest`; op `harness_defaults` (pull all
  rows); merge newest-wins per `(scope, harness)`; `deleted` for a
  cleared override. HLC via `hlc_now()` like every other table.
- Capability: writing defaults is `Control` (a peer in observe-only
  policy cannot push them), same as templates.

### 5.3 The console

The two forms become one panel, "Runtime defaults", still on the Mesh
list view (where the operator looked for it), with a scope selector:
**all nodes** (the `mesh` row) or a node name (its override, shown with
"overrides the mesh default" and a clear button). Each row shows the
effective value per node in a small table — node, harness, args, and
which scope it came from — so "why did that session start with those
flags" has an answer. The per-session "runtime args" field on the
new-session panel stays: it appends after the defaults, as today.

Templates keep their own `extra_args` (per template, appended after
the defaults) — unchanged.

### 5.4 Migration

At daemon start, if `settings.harness` has args and the store has no
`node:<me>` row for that harness, write one with the file's value (HLC
now). Existing nodes keep their behavior; the console then sees and can
edit every node's defaults from anywhere.

## 6. Open questions for the discussion

1. Should a failed MCP server be a *needs you* row (interrupting, like a
   prompt) or only a notice plus the red chip? The proposal says row: it
   is the case that bit.
2. Refresh cadence for Claude, which has no push: turn boundaries plus
   on-demand, or also a slow timer (every 60s) so a mid-turn drop shows
   before the turn ends?
3. `mcp_set_servers` lets the console add a server to a running session
   without a restart. Expose it (an "add server" form) now, or leave it
   to the repo's `.mcp.json` and the trust review?
4. The process-tree phase: worth the inference, or is "monitors go
   through the tool stream" the rule plugins should follow?
