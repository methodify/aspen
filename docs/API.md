# Aspen node API (localhost, v0)

The `aspen up` daemon serves HTTP + WebSocket on `127.0.0.1:7420` (configurable
via `--listen`). v0 is localhost-only and unauthenticated; the mesh security
model (DESIGN.md §8) arrives with federation. All bodies are JSON.

## REST

| Method & path | Body | Returns | Notes |
|---|---|---|---|
| `GET /api/node` | — | `{ node: string, version: string }` | identity/health |
| `GET /api/agents` | — | `Agent[]` | full roster: registered agents + live state |
| `POST /api/agents` | `{ name, repo, charter?, model?, allow_all?, resume? }` | `Agent` | spawn + join bus. 409 if name is live |
| `POST /api/agents/{name}/message` | `{ text }` | `{ uuid }` | operator input into the session |
| `POST /api/agents/{name}/interrupt` | — | `{}` | abort the in-flight turn |
| `POST /api/agents/{name}/permission/{request_id}` | `{ allow, message?, updated_input? }` | `{}` | answer a pending permission prompt. `message` is shown to the model on deny; `updated_input` (optional) replaces tool input on allow |
| `POST /api/agents/{name}/revive` | — | `Agent` | bring a registered-but-down agent back by resuming its stored session (history intact) |
| `DELETE /api/agents/{name}` | — | `{}` | shutdown ladder |
| `GET /api/agents/{name}/transcript` | — | `TranscriptItem[]` | rehydrated history from the runtime's on-disk transcript — render above the live stream when opening a session |
| `GET /api/sessions?repo=/path` | — | `SessionInfo[]` | enumerate a repo's sessions from disk (newest first); `user_messages == 0` is a warm spawn, hide it |
| `GET /api/bus/log?n=50` | — | `BusMessage[]` | the trail, chronological |
| `POST /api/bus/send` | `{ to, body, urgency?, thread?, record? }` | `{ notes: string[] }` | send **as @operator**. `to` is `@agent`, `#channel`, `@operator` |
| `GET /api/operator/inbox` | — | `BusMessage[]` | undelivered messages addressed to the operator |
| `POST /api/operator/inbox/read` | — | `{}` | mark operator inbox delivered |

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
