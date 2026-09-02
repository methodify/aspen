# Aspen node API (localhost, v0)

The `aspen up` daemon serves HTTP + WebSocket on `127.0.0.1:7420` (configurable
via `--listen`). v0 is localhost-only and unauthenticated; the mesh security
model (DESIGN.md §8) arrives with federation. All bodies are JSON.

## Addresses

Agents are named **per repo**: the address is `name@repo`, and `name@repo@node`
when the same repo handle exists on more than one node. The repo segment is
the repo's **handle** (defaults to the directory basename; unique per node;
`POST /api/repos/rename`). `Agent.name` in every response IS the address —
route keys, bus addresses, and channel members all use it; `Agent.bare` is
the short name. When spawning, send the bare name; the node composes the
address. Bus sends resolve from the sender's context: a bare `name` reaches
the sender's own repo first, then the single agent of that name anywhere,
then the single one sharing a custom channel with the sender — anything
ambiguous is refused with the candidates. `operator` is global.

## REST

| Method & path | Body | Returns | Notes |
|---|---|---|---|
| `GET /api/node` | — | `{ node, version, sha, built }` | identity/health; `sha`/`built` are stamped at compile time |
| `GET /api/agents` | — | `Agent[]` | full roster: registered agents + live state |
| `POST /api/agents` | `{ name, repo, charter?, model?, allow_all?, resume?, skip_permissions?, acknowledge_trust?, title?, extra_args?, node? }` | `Agent` | start a session + join bus. 409 if name is live. `resume` = a session id from `/api/sessions` to continue an existing one. `skip_permissions` (bool) runs it in bypassPermissions; omit to use the repo's stored default. With `node`, the session spawns on that peer over the mesh (returns `name@node`). An untrusted repo that would auto-run hooks/MCP returns 428 + `{autorun}` until retried with `acknowledge_trust: true` (the trust gate) |
| `GET /api/repos` | — | `Repo[]` | remembered repos (path, skip default, session/live counts) |
| `POST /api/repos` | `{ path, skip_permissions? }` | `Repo` | remember a repo (must be a real directory) |
| `POST /api/repos/skip` | `{ path, skip_permissions, node? }` | `{ ok }` | set a repo's skip-permissions default; `node` acts on that peer's registry |
| `POST /api/repos/rename` | `{ path, handle, node? }` | `{ ok }` | rename a repo's handle (address segment + channel); refused while sessions in it run; cascades to agents, channel members, bus rows |
| `POST /api/repos/forget` | `{ path, node? }` | `{ ok }` | forget a repo (sessions on disk are untouched); `node` acts on that peer's registry |
| `POST /api/repos/discover` | `{ node? }` | `{ found: [{path, sessions, added}] }` | recover repos from Claude Code's session store (`~/.claude/projects`, real paths read from transcript `cwd`) and register the new ones. With `node`, runs on that peer (its repos register there) |
| `POST /api/shutdown` | `{}` | `{ ok, stopping }` | graceful stop (same ladder as SIGTERM); what `aspen down` uses on every platform — Windows has no SIGTERM and a detached process has no window for taskkill |
| `POST /api/mesh/reload` | `{}` | `{ ok, summary }` | apply mesh files to the running daemon (join live / pick up peers+relay); the mesh CLI calls it after every mutation |
| `GET /api/mesh/repos` | — | `{ nodes: [{node, self, reachable, repos}] }` | mesh-wide repo registry grouped by node (this node + each peer); an unreachable peer lists no repos |
| `GET /api/settings` | — | `Settings` | node settings: per-harness default CLI args |
| `PUT /api/settings` | `Settings` | `{ ok }` | replace settings; arg strings are validated (reserved protocol flags rejected) |
| `POST /api/agents/{name}/message` | `{ text }` | `{ uuid }` | operator input into the session |
| `POST /api/agents/{name}/interrupt` | — | `{}` | abort the in-flight turn |
| `POST /api/agents/{name}/permission/{request_id}` | `{ allow, message?, updated_input?, updated_permissions? }` | `{}` | answer a prompt. Deny: `message` shown to the model. Allow: `updated_input` replaces tool input — for AskUserQuestion send `{questions: <echo>, answers: {"<question text>": "<option label>"}, response?}` (§7.6). `updated_permissions`: echo the prompt's `suggestions` verbatim for "always allow" |
| `POST /api/agents/{name}/revive` | — | `Agent` | bring a registered-but-down agent back by resuming its stored session (history intact) |
| `DELETE /api/agents/{name}` | — | `{}` | shutdown ladder |
| `GET /api/agents/{name}/transcript` | — | `TranscriptItem[]` | rehydrated history from the runtime's on-disk transcript — render above the live stream when opening a session |
| `GET /api/sessions?repo=/path&node=` | — | `SessionInfo[]` | `node` enumerates on that peer. enumerate a repo's sessions from disk (newest first); `user_messages == 0` is a warm spawn, hide it. When the repo has an mcc register (`.mcc/sessions`), rows carry `mcc_name`/`mcc_args`/`mcc_skip` (permission flags filtered into `mcc_skip`); the console shows the name and carries name+args over on resume |
| `GET /api/bus/log?n=&sender=&recipient=&thread=&record=&urgency=&q=` | — | `BusMessage[]` | the trail, chronological; all filters optional (`q` = body substring) |
| `POST /api/bus/send` | `{ to, body, urgency?, thread?, record? }` | `{ notes: string[] }` | send **as @operator**. `to` is `@agent`, `#channel`, `@operator` |
| `GET /api/operator/inbox` | — | `BusMessage[]` | undelivered messages addressed to the operator |
| `POST /api/operator/inbox/read` | — | `{}` | mark operator inbox delivered |
| `GET /api/repo/skills?repo=/path` | — | `SkillEntry[]` | list a repo's skills + commands (from disk) |
| `GET /api/repo/skill?repo=/path&rel=.claude/skills/x/SKILL.md` | — | `{ content }` | read one skill/command file |
| `PUT /api/repo/skill` | `{ repo, rel, content, reload? }` | `{ ok, reloaded_sessions }` | write a file; `reload` (default true) reloads live sessions in that repo |
| `DELETE /api/repo/skill?repo=&rel=` | — | `{ ok }` | delete a skill/command file |
| `POST /api/agents/{name}/reload` | — | inventory | reload one live session's plugins/skills/commands |
| `GET /api/agents/{name}/runtime` | — | `{ handshake, inventory }` | the runtime's own view: handshake `commands[]`/`models[]`/`output_style`, and the `system/init` inventory (tools/skills/mcp as loaded). Source for slash autocomplete |
| `GET /api/agents/{name}/context` | — | context usage | rich breakdown from `get_context_usage`: `categories`, `gridRows`, `maxTokens`, `autoCompactThreshold`… Poll at turn end, never mid-turn |
| `POST /api/agents/{name}/model` | `{ model }` | `{}` | switch model (null/"default" resets). Takes effect next turn — say so |
| `POST /api/agents/{name}/mode` | `{ mode }` | `{}` | permission mode: default/acceptEdits/bypassPermissions/plan/dontAsk |
| `POST /api/agents/{name}/title` | `{ title }` | `{}` | operator display title (null clears; the @name stays the bus identity) |
| `POST /api/agents/{name}/charter` | `{ charter }` | `{}` | update the stored charter — applies at next spawn/revive (rides the system prompt) |
| `GET /api/needs` | — | `{ prompts[], inbox[] }` | THE MESH-WIDE OPERATOR INBOX: every open permission prompt/question (local + every connected node; remote agents named `name@node`) + @operator mail from every node (`node` field, null=local) |
| `POST /api/needs/read` | — | `{}` | mark operator mail read, locally and on every connected peer |
| `GET /api/dms` | — | `[{a,b,last_at,messages}]` | direct-message pairs (non-channel traffic), newest first |
| `GET /api/dm?a=&b=&n=` | — | `BusMessage[]` | one direct conversation, chronological |
| `GET /api/bus/post/{post}` | — | `BusMessage[]` | per-recipient receipts for one logical post — watch a routed message land (delivered/ingested per recipient) |
| `GET /api/mesh` | — | mesh info | `{ in_mesh, mesh, node, peers:[{node,url,link_up,agents}], relay:{url,connected_at} }` |
| `POST /api/repos/trust` / `untrust` | `{ path }` | `{ ok }` | record / revoke repo trust |

### Repo

```jsonc
{ "path": "/home/u/src/proj", "skip_permissions": false,
  "last_used_at": 1756500000.0, "sessions": 12, "live_agents": 1 }
```

### SkillEntry

```jsonc
{ "name": "greet", "rel": ".claude/skills/greet/SKILL.md",
  "kind": "skill" | "command", "description": "…" | null }
```

Remote addressing (`name@node`) works on message, interrupt, permission,
revive, shutdown, transcript, events, and reload — the console drives
agents on any node through this node.

### Agent

```jsonc
{
  "name": "arch",
  "repo": "/home/u/src/proj",
  "channel": "proj",           // auto repo channel
  "session_id": "uuid",
  "charter": "…" | null,
  "live": true,
  "turn_state": "idle" | "busy" | null,  // null when not live
  "pending": 0                  // undelivered bus messages held for it
}
```

### BusMessage

```jsonc
{
  "id": 7, "sender": "ping", "to_display": "@pong", "recipient": "pong",
  "urgency": "gating" | "normal" | "notice",
  "body": "…", "thread": "t-1" | null, "record": "…" | null,
  "created_at": 1756500000.0,
  "delivered_at": 1756500001.0 | null, "delivered_via": "wake" | "boundary" | "interrupt" | "inbox" | "rode-along" | null,
  "ingested_at": 1756500001.5 | null   // runtime replay-ack: proof of ingestion
}
```

### TranscriptItem

```jsonc
{ "role": "user" | "assistant",
  "text": "…",                    // full markdown text
  "bus": true,                     // user items only: an [aspen bus] injection
  "tools": [{ "id": "toolu_…", "name": "Read" }],  // assistant items only
  "uuid": "…", "timestamp": "2026-…" }
```

### SessionInfo

```jsonc
{ "session_id": "uuid", "title": "…" | null, "entrypoint": "aspen" | "cli" | null,
  "modified": 1756500000.0, "user_messages": 12 }
```

## WebSocket: `GET /api/agents/{name}/events`

Upgrades to a WS that streams the session's normalized events as JSON text
frames, exactly the `SessionEvent` shape (tagged by `kind`, snake_case):

```
runtime_init | text_delta | assistant_message | tool_use | tool_result
| user_replay | permission_asked | permission_settled | turn_ended
| status | stderr | raw | exited
```

Key kinds for the console:

- `text_delta { text, thinking }` — token streaming. Paint into the open tail
  bubble; see the snapshot-reconciling merge rules in
  `docs/reference/CLAUDE_RUNTIME_REFERENCE.md` §5.2.
- `assistant_message { message_id, raw }` — block snapshot for reconciliation.
- `tool_use / tool_result` — render as cards; `raw` carries everything.
- `permission_asked { request_id, tool_name, input, suggestions }` — render a
  permission card; answer via the REST endpoint. A later
  `permission_settled` for the same `request_id` (any source) closes it.
- `turn_ended { subtype, total_cost_usd (SESSION-CUMULATIVE — label it so),
  duration_ms, result_text }` — the only signal that unblocks the composer.
- `exited { code }` — the session is gone; offer resume.

The socket sends only live events; history comes from the transcript
endpoint. Multiple sockets per session are fine
(broadcast fan-out). A client may send text frames; they are ignored today.

## Static

Everything not under `/api` serves the SPA from `ui/dist` (SPA fallback to
`index.html`).
