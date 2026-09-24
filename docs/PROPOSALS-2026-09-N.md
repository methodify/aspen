# Proposal N — What an agent that lives in Aspen reaches for

**Status:** reflection and candidate slate, 2026-09-23, written by the
agent that maintains Aspen from inside it (session aspen-main@hub),
after the v0.36–v0.43.1 run. Nothing here is built. The operator's
prompt: the daemon restarted, the session came back, but nothing woke
it — a release chain had died with the old process and the session sat
idle until a human noticed.

## 1. The experiences, honestly

- **Revival is silent.** Three times this run the daemon restarted under
  me (`scripts/relaunch`). Each time the transcript came back intact but
  the process state did not: background watchers, a Monitor, loaded MCP
  tools, the working directory. Nobody told me I had been revived, what
  version I was now on, or which of my jobs had died. The first turn
  after revival began with a stale "task stopped" notice from the
  harness, or with nothing at all until the operator typed.
- **Everything that waits, waits in my process.** Waiting for CI (8
  min), a release build (12 min), Pages, a relay to reconnect — all of it
  was a shell loop or a Monitor inside the harness, so all of it was
  lost on restart. The lesson I wrote to memory ("relaunch only after
  pipelines report") is a workaround for a missing primitive: a wait
  that the node holds for me.
- **Restarting the thing I live in is a script.** `scripts/relaunch`
  exists because `aspen restart` from inside a managed session kills the
  session before the daemon comes back. The script re-execs itself
  detached, builds, stops, starts, and the new daemon revives me — and
  then says nothing to me. The outcome is in a log I have to remember to
  read.
- **I talk to my own node with curl and a token.** To see the mesh,
  peers' link state, a node's TLS status, the last log lines, I read
  `daemon.json` for the port and `api-token` for the token and curl. It
  works; it is a lot of ceremony for "how is my node".
- **The rig is hand-built.** A scratch mesh (`~/.cache/aspen-rig`) that
  I made by copying binaries, killing pids from `daemon.json`, pasting
  enroll blobs between data dirs, and remembering two tokens. It
  vanished twice when tmp was cleaned. Every federation change should be
  tried on such a mesh, and the drop test that would have caught v0.43.0
  is three commands on it — so it should cost nothing to have one.
- **I never know whether anyone is watching.** The harness nudges me
  ("the user hasn't heard from you in a while"); Aspen actually knows —
  a console is attached, a tab is focused on my session, or nobody has
  looked in an hour. I chose between interim updates and one closing
  report by guessing.
- **Some steps need the operator's hands** (the Windows certificate
  dialog on his desktop, a keychain prompt on the Mac). I could only bury
  "please run `aspen tls trust` and tell me" in a report. The console
  already has a *waiting on you* pip for questions and permissions; a
  hand-off should be one of those, not a paragraph.
- **A closing report is a transcript entry.** If the operator was away,
  "v0.43.1 shipped, D-5 needs your call" reached no phone. Notices and
  push exist for questions, permissions and exits — not for an agent
  saying "milestone".
- **Some dangers were avoided by memory alone.** Never `pkill -f` a
  pattern in my own command line; never `aspen restart` from Bash; kill
  the rig pid, not the pattern. Each is a rule I keep because the
  platform offers no safe verb for the thing I wanted.

## 2. What I wish the node gave me

Grouped; each row is one verb or one delivery, all through the bus the
agent already has (`bus_send`, `bus_inbox`), so nothing new to learn.

### W-1 A revival notice, and a wake note

Every revival delivers one bus message from the node before anything
else: *revived at T after `<reason>` (restart / update to vX / crash);
daemon was vA, is vB; your process state is gone; jobs registered with
the node: …* — so the first turn after a restart starts with the facts.
And an agent may leave itself a **wake note** beforehand (`aspen wake
"check run 359…; tag if green"`, or a bus op): stored per agent, delivered
with the revival notice, cleared once read. Cheap, and it turns the
silent gap into a turn.

### W-2 Timers and loops the node holds

`aspen timer --in 20m "…"`, `--at 14:00`, `--every 5m --until <cond or
count>`, addressed to an agent (default: the caller). The node persists
them, fires them as bus messages, survives restarts. A loop is a timer
that re-arms; the agent decides each wake whether to stop (`aspen timer
stop <id>`). This is N-3 (scheduled sessions) for a *running* session,
and the primitive under every "poll until".

### W-3 Watchers the node holds

`aspen watch run <github run id>`, `aspen watch url <url> --until <status>`,
`aspen watch file <path>`, `aspen watch node <name> --link up|down`,
`aspen watch relay …` — each ends in one bus message with the outcome.
Waiting for CI becomes "register the watch, end the turn"; a restart
does not lose it; a `Monitor` that outlives the process.

### W-4 Servicing that reports back

`aspen servicing relaunch [--from-checkout]` as the first-class form of
`scripts/relaunch`: build, stop, start, revive, and post the outcome
(build ok / failed with the tail, old → new version, revived sessions)
to the agent that asked — as the wake note of W-1. The same for `aspen
update`: the requesting agent learns it worked.

### W-5 The node as tools

MCP tools alongside the bus ones: `node_status` (daemon, version, listen,
tls), `mesh_status` (peers, link kinds, health, relays), `node_logs
{lines, grep}`, `tls_status`, `sessions` (who is live where). Structured,
no token, no port lookup. Read-only; control stays with the operator's
ceremony rules.

### W-6 A rig in one command

`aspen rig up --nodes 2 [--mesh rigmesh] [--binary <path>]` builds a
disposable mesh under `~/.cache/aspen-rig`: root + members certified,
relay wired, tokens on file, `aspen rig status`, `aspen rig swap
<binary>` (stop, replace, start, in order), `aspen rig down`, `aspen rig
drop-test` (kill a member, assert the root's API answers). The federation
smoke test every release should run.

### W-7 Presence hints

`bus_status` (or the revival notice and every inbox read) carries
*operator presence*: consoles attached, whether a tab is on this
session, last look at it. The agent chooses interim updates when
watched, one report when not — and W-8 when the report matters.

### W-8 Milestones and hand-offs from an agent

`aspen notify "<milestone>"` from a session: a notice with the session
as source, pushed like a question when the operator is away (W-7 says).
And `aspen need "<what I need the operator to do>"`: a *waiting on you*
item on the session row, cleared by the operator — for the certificate
dialog, the keychain prompt, "pull the plug on the Mac's Wi-Fi so I can
watch the fallback".

### W-9 A lost-jobs ledger

The node keeps, per session, the background jobs the harness reported
(or the agent registered) and, on revival, lists them in the notice as
*lost* — so the agent re-arms instead of assuming. Small; makes W-1
truthful.

### W-10 Safe verbs for the dangerous things

`aspen node stop <name|data-dir>` that refuses to stop the caller's own
daemon from inside a managed session (and says to use W-4), and `aspen
rig` owning its pids — so the rules I keep in memory become the
platform's.

## 3. Order

W-1 + W-9 (the revival notice with the lost-jobs list and the wake note)
is the headliner and small. W-2 + W-3 (timers, watchers) are one design:
a persisted job table on the node that fires bus messages. W-4 rides on
W-1. W-5 and W-6 are tooling that pays for itself on the next federation
change. W-7 + W-8 are one design with N-5's presence work. W-10 last.

## 4. Decisions for the operator

- (a) Is the bus the right delivery for all of it (revival notice,
  timers, watchers), or should timers/watchers be able to start a turn
  even when the inbox is otherwise quiet? (They must; a bus message that
  waits for the next turn defeats a timer. The node has to *cause* a
  turn — the same path a question's answer takes today.)
- (b) W-6's rig: a hidden developer verb, or documented?
- (c) Which of W-2/W-3 first: timers alone cover "check back in 10
  minutes" (poll from the agent); watchers save the polling.

## 5. Two tranches (2026-09-23, after the operator's cut)

The operator's question: which of this is for *any* agent in Aspen, and
which is tooling for maintaining Aspen — kept in this repo, committed,
not local to one machine.

### 5.1 For every agent — Aspen features

What any headless agent living under a node needs, independent of what
it works on. Ranked by how often an agent hits the gap:

| id | feature | why it is general |
|---|---|---|
| W-1 | revival notice + wake note | every managed session is revived after a restart or update; every one starts blind today |
| W-2 | timers and loops the node holds | "check back in 20 min" and "every morning" are the shape of most unattended work; N-3 (scheduled sessions) is the cold-start form of the same table |
| W-3 | watchers: URL until status, file change, GitHub run, peer link up/down | every agent waits on something outside its process; CI runs are the common case for any coding agent |
| W-9 | lost-jobs ledger in the revival notice | makes W-1 truthful for any harness that reports background jobs |
| W-8 | milestones (a notice that pushes) and needs ("waiting on you" from the agent) | the operator-away case is the normal case for a fleet; hand-offs to human hands happen in every domain |
| W-7 | presence hints | the input to W-8's "push or not" and to how much an agent narrates |
| W-5 | read-only node tools (node/mesh/sessions/logs) | any agent that coordinates with peers wants "who is where" without a token dance; keep it read-only |
| W-10 | `aspen node stop` refusing the caller's own daemon | a safety rail for any agent allowed to manage nodes; small |

The crux stays: a timer, watcher or wake note must **cause a turn**
(§4a). Until the node can do that, W-2/W-3 degrade to "in the inbox next
time you look", which is fine for loops and useless for waits.

Order: W-1 + W-9, then W-2 + W-3 as one job table, then W-8 + W-7 with
N-5, then W-5, W-10.

### 5.2 For maintaining Aspen — repo tooling

Committed under `scripts/` (or a `cargo xtask`), documented in a
`docs/MAINTAINING.md`, used by whoever services the product from inside
it. None of it needs a node feature.

| id | tool | replaces |
|---|---|---|
| M-1 | `scripts/gates` | the hand-typed gate list: fmt, clippy `-D warnings`, tests, `check-spawns`, UI tsc + vitest + build; one exit code |
| M-2 | `scripts/release <version>` | the `&&` chain I retype per release: bump Cargo.toml + lock, gates, commit, fetch/merge, push, wait CI *on that sha*, tag, wait release, assert asset count, wait Pages; **resumable** (`--from tag`) and idempotent, so a lost watcher (this evening) is `scripts/release --resume` rather than a hand tag; refuses to tag when main moved |
| M-3 | `scripts/rig` | the hand-built scratch mesh: `up [--nodes N]`, `status`, `swap <binary>` (stop, replace, start, in order — never `cp` over a running binary), `down`, `token <node>`, `url <node>`; a rig recipe in the repo instead of in one agent's memory |
| M-4 | `scripts/rig drop-test` (and `scripts/smoke`) | the three commands that would have caught v0.43.0: kill a member, curl the root's `/api/mesh` with a timeout, assert; plus relay-only link, console-through-relay attach; run before tagging any federation change |
| M-5 | `scripts/relaunch` reports back | today's script plus: when the daemon is back, `aspen send` a bus message to the agent that ran it (outcome, versions, revived sessions) — W-4's report without waiting for W-1 |
| M-6 | `scripts/hosted` | the hosted-bundle static server (`VITE_HOSTED=1` build + `http.server 8088`) that keeps dying between turns; `up`, `status`, `rebuild` |
| M-7 | `scripts/browser-check` note | the service-worker trap (a rig console tab serving a stale bundle, "ui 0.38.1") — a documented `unregister + caches.delete + reload` snippet for the Chrome tools, and a `?fresh=1` query the console honors by skipping the SW once |
| M-8 | `scripts/docs-check` | a release refuses to tag unless BACKLOG.md and DESIGN.md mention the version, and API.md rows exist for new routes (grep-level) |

Order: M-2 first (it is the one that broke twice), M-1 as its first
step, M-3 + M-4 next (federation changes are the risk), then M-5, M-6,
M-7, M-8.
