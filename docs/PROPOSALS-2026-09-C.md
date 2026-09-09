# Proposal: the operator's thread — catch me up, search, auto-start

**Status:** shipped 2026-09-09 as v0.25 (Tier 1 of the post-Codex slate,
with the supervisor parked as N-1 and auto-start in its place). Reference:
CATCH_UP_AND_SEARCH.md, AUTOSTART.md.

## 1. Catch me up (T-1)

**The ask.** Coming back to a session after an hour means scrolling to
find where things stand. The TUI shows a "recap:" line when you return
after five minutes away; Aspen shows the transcript.

**What the harness offers (verified 2026-09-09).** Claude Code has a
local `/recap` command ("Generate a one-line session recap now",
`supportsNonInteractive: true`): sent as a user message in stream-json
mode it runs a side query — the transcript records only a
`<command-name>/recap</command-name>` line and a `system/local_command`
line, nothing enters the model's context — and the recap arrives as an
assistant text event followed by a `result`. Codex has no equivalent.

**Design.**
- **The digest, always, both harnesses.** The console remembers, per
  session and per browser, the last item the operator saw. On reopening
  a session with newer items, a bar sits above the transcript: `since
  you last looked · 3 turns · 14 tool uses (Bash ×9, Edit ×3…) · 4 files
  · 1 prompt answered` and the first line of the agent's latest
  message. Computed from the transcript already in hand; no model call.
  A jump link scrolls to the first unseen item; × marks everything seen.
- **The recap, on demand and on return (Claude).** A `recap` button on
  the bar (and in the status line when idle) asks the node, which sends
  `/recap`, waits for the turn's result, and returns the sentence. The
  node refuses while the session is busy (a recap during a turn would
  queue behind it). Setting `console.recapOnReturn` (default on) asks
  automatically when the operator returns after 5+ minutes and the
  session is idle. Codex sessions show the digest only, and say why.
- API: `POST /api/agents/{name}/recap` → `{ text, took_ms }` or 409
  busy; proxied over the mesh.

## 2. Search across History (T-2)

**The ask.** "Where did we decide X" has no answer today: History is
browsable by time, not searchable by text, and with replication the
corpus is already on the node.

**Design.**
- `GET /api/search?q=…&limit=…&repo=…` on a node searches every
  session of every registered repo (both harnesses, through the store),
  and every replica it holds. Fast path: the main file is read once and
  pre-filtered by a case-insensitive substring; only files that match
  are rehydrated, and hits are the items whose text contains the query
  (user, assistant, and tool inputs/results), up to 5 per session and
  200 total, newest first. Runs in `spawn_blocking`.
- Mesh-wide: the console's node fans the query out to every up peer
  (`search` is an Observe op) and merges, each hit tagged with its node.
- Console: a Search page (sidebar) with a query box; results grouped by
  session — agent, node, repo, when — each hit a snippet with the match
  highlighted; clicking opens the session at that item (`?at=<uuid>`),
  or the History viewer for a session no agent owns. The palette's
  free-text falls through to it.

## 3. Auto-start (T-3)

**The ask.** The daemon should come up with the user's login, as the
user — not as a system service, which would put the operator's repos,
tokens and harness credentials under an identity that is not theirs.

**Design.** `aspen autostart enable | disable | status`, platform-
appropriate and user-scoped:

| platform | mechanism |
|---|---|
| Linux, WSL (systemd) | `~/.config/systemd/user/aspen.service` running `aspen --data-dir … up` in the foreground with `Restart=on-failure`; `systemctl --user enable --now`. Prints the `loginctl enable-linger` hint for headless boxes. |
| macOS | `~/Library/LaunchAgents/dev.aspen.node.plist`, `RunAtLoad` + `KeepAlive`, bootstrapped into `gui/<uid>`. |
| Windows | a Scheduled Task at logon, running as the user with limited rights (`schtasks /SC ONLOGON /RL LIMITED`), running `aspen up -d`, which detaches and exits. |

The unit runs whatever `aspen up` would with the node's config
(listen, headless, UI). `aspen status` and `GET /api/node` report
`autostart: on|off|unsupported`, and the Mesh list view shows it per
node with a toggle (proxied; the peer writes its own unit). Uninstall
removes the unit and stops nothing running.

## 4. Nit

The live-elsewhere note read "forked — resumed in place…" whichever
choice was made (the console prefixed every note with "forked"), and
stayed for the session's life. The note now reads as written and has a
dismiss that the browser remembers.
