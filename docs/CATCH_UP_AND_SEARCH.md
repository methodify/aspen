# Catch me up, and search

Two answers to "what happened while I was away?" and "where did we decide
that?" (PROPOSALS-2026-09-C.md §1–2, v0.25).

## 1. Since you last looked

The console remembers, per session and per browser (`localStorage`
`aspen.seen.<agent>`), where the operator's eyes were: the last user
line's uuid and how many items followed it, and when. It writes the
marker whenever the transcript changes while the page is in view, and
freezes it while the tab is hidden or closed — so on return it can say
what happened meanwhile.

On opening a session (or when the tab comes back into view after five
minutes or more) with items after the marker, a bar sits above the
transcript: `since you last looked · 2h 10m ago · 3 turns · 14 tool uses
(Bash ×9, Edit ×3, Read ×2) · 4 files · 1 prompt` and the first line of
the agent's latest message. It is computed from the items already in
hand — no model call, both harnesses. `jump ↓` scrolls to the first
unseen item; `×` marks everything seen; sending a message does too. When
the marker's line is gone (a fresh session, a compaction) there is
nothing honest to say and no bar.

## 2. The harness's recap

Claude Code's `/recap` ("Generate a one-line session recap now") is a
local command that runs headlessly: sent as a user message in stream-json
mode it runs a side query — the transcript records only a
`<command-name>/recap</command-name>` line and a `system/local_command`
line, nothing enters the model's context — and the answer arrives as an
assistant message and a `result`. Aspen's transcript rehydration already
drops those marker lines.

`POST /api/agents/{name}/recap` → `{text, took_ms, at}`. The node marks
the session as capturing, sends `/recap`, and the event pump routes the
side turn's assistant text into the capture and its `result` to the
waiting request — nothing reaches observers, the ledger, the notices or
the work summary: the recap is an answer to the operator, not a turn of
the conversation. 409 while the session is mid-turn (the recap would
queue behind the turn and answer late) or while a recap is already in
flight; 501 for a harness without one (Codex — the capability flag
`recap` is false; the bar says "no recap on this harness"). 90 s timeout.
Proxied over the mesh as a control op.

The bar's `recap` button asks on demand; with "on return" checked (the
default, `aspen.recapOnReturn`) it asks by itself when the operator
comes back after five minutes or more and the session is idle.

## 3. Search

`GET /api/search?q=&limit=&repo=&mesh=` → `{q, sessions, scanned, nodes,
nodes_failed, took_ms}`. The node searches every session of every
registered repo (both harnesses, through the stores) and every replica
it holds, newest first. The cheap gate: the main file is read once and
tested for the query's first word, case-folded; only files that pass are
rehydrated, and the hits are the items whose text — user, assistant, or
a tool chip's name/summary/path/command — contains the whole query, up
to 5 per session, 200 sessions in all. Files over 256 MB are skipped. It
runs in `spawn_blocking`.

Mesh-wide: the console's node fans the query out to every up peer
(`search` is an observe op, 25 s) and merges, each session tagged with
its node; a replica of a session whose home answered is dropped in
favour of the live copy. Peers that did not answer are named in
`nodes_failed`.

Each hit is `{uuid, role, timestamp, snippet: {before, match, after}}`,
about 90 characters either side of the match. The Search page (sidebar,
`S`; the palette's free text falls through to it) groups hits by session
— agent, harness, repo, node, when — and clicking one opens the session
at that line: `/session/<agent>?at=<uuid>` shows everything, scrolls to
the bubble carrying that uuid and flashes it. A session no agent owns is
listed by title and id; attach to it from its repo to open it.

## 4. Verified (rig, 2026-09-09)

`/recap` on a live Claude session returned a one-sentence recap in a few
seconds with no assistant bubble appearing in the console and no turn
counted; the same request mid-turn answered 409. Search from one node
returned hits from four nodes' sessions with the node tag on each, and a
hit opened the remote session at its line.
