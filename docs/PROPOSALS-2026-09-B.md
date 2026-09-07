# Proposals — the 2026-09-07 slate (S-1 … S-11)

**Status:** designs for the three-tier slate approved 2026-09-07 (BACKLOG
S-1..S-12). Shipped: §2 §3 §4 §12 as v0.15 (ACTIVITY.md, NOTIFICATIONS.md,
USAGE.md). Rounds: v0.15 (§2 §3 §4 §12), v0.16 (§6 §1 §5), v0.17 (§7 §8),
v0.18 (§9 §10), v0.19 (§11). Each section: the ask, what exists, the
design, what it implies, the cost, open questions. As each round ships
its section gets a `docs/<NAME>.md` reference and a DESIGN.md log entry.

The maps this was written from: the roster is a JSON object built by
`roster_payload` and stored per peer in `MeshState.remote` /
`MeshState.health`; every node-to-node request is an `api_req` frame
dispatched by `serve_api_req`; the console only ever talks to its own
node, which proxies by `remote_parts`; mesh-wide tables converge by
roster digest + pull + newest `updated_at` per row (SYNC.md).

---

## 1. Transcript replication, opt-in (S-1)

**Ask.** A session's transcript is also held somewhere else, so a move
is near-instant, sessions on a dead node stay readable, and History
reaches every node. Not the default mode of operation: opted into.

**Today.** A transcript lives only on its home node
(`<claude home>/projects/<slug>/<sid>.jsonl` plus `<sid>/subagents/`).
A move copies it in a bundle at move time (MIGRATION.md). A node that
is down takes its sessions with it: the console shows the roster it
last heard and nothing more.

**Design.**

*Opting in.* A node setting, `replication.to = <node name>`, with an
optional `replication.repos` list (handles; default all). Set from the
Mesh panel (a "replicate to" picker on the node's own row) or
`aspen config replicate <node>`. Both sides are members already; the
target needs no setting — it accepts replicas from any peer it trusts,
capped by `replication.accept = false` to refuse.

*Mechanism: an append stream.* The pump already sees every event of a
live session. After each turn end (and every 5 s while busy, since tool
results land between turns) the source stats the transcript file and
the session's `subagents/` files; anything longer than the last sent
offset is sent as `{t:"tx", agent, session_id, repo, rel, offset, data}`
frames on the sealed link, 256 KB a chunk, in order, one in flight per
session. The target appends to `<data>/replicas/<node>/<agent key>/<rel>`
and records `(node, agent, session_id, repo handle, rel, bytes,
updated_at, title)` in a `replicas` table. Files never shrink except by
truncation on the source, which the source detects (size < sent offset)
and handles by re-sending from zero with `truncate: true`.

*Catch-up.* On link-up the target answers the source's roster with
`replica_offsets` for every session it holds from that node; the source
resumes from there. A session that was not live during the outage is
caught up lazily the next time it is spawned, or immediately by
`aspen replicate --now`.

*Reading a replica.* When the console asks for the transcript of
`x@repo@node` and `node` is down, the local node serves the replica if
it holds one, marking the response `{replica: {as_of, node: me}}`; the
session page shows a banner "node is down · showing the replica held
on <me> as of <time>" and the composer stays disabled. The Mesh list
gains a "Replicas held here" section per source node, each row opening
the transcript, with size and age. `GET /api/replicas` lists them.

*Move from a replica.* When a move's source is unreachable and the
target holds a replica, the move dialog offers "start from the replica
held here": the target imports the replica as a bundle (tier A + B,
memory from its own copy if any), forks it, and marks the new row with
a note that the source may still run the original. It cannot tombstone
the source; when the source returns, the operator resolves the pair
(the adoption machinery's *fork* verbs apply).

*History mesh-wide.* The History page's session enumeration
(`GET /api/sessions`) includes replicas for nodes that are down, marked
`replica: true`.

**Implies.** A replica is a second copy of everything the session ever
saw, on another machine: the setting is per node on purpose, and the
Mesh panel says what is being copied where. Replicas obey the same
eviction the operator chooses: `replication.keep_days` (default 30)
prunes replicas of sessions the source no longer lists.

**Cost.** Rust: a `tx` frame and offsets table in federation.rs; a
replicator task in node.rs driven by the pump; store table `replicas`;
transcript fallback in api.rs; move-from-replica in migrate.rs. UI: the
banner, the Replicas section, the settings picker. Docs: REPLICATION.md.

**Open.** Whether to replicate to more than one node (design allows a
list; first version takes one). Whether the harness's `.claude/projects`
sidecar files (tool results dir) are worth carrying — yes, they are the
`subagents/` and tool-result files, and tier B already ships them.

---

## 2. Fleet-wide activity in Now (S-2)

**Ask.** One list of everything running across the estate.

**Today.** Each live session's ledger is derivable (ACTIVITY.md);
counts ride the roster but `RemoteAgent` drops them on receipt, so
remote agents show no chips in the fleet list.

**Design.** `RemoteAgent` gains `activities`; `get_agents` emits them
for remote agents too (chips appear everywhere they already render).
A new `GET /api/activities` scatter-gathers: local live sessions'
running items plus `activities` ops to every up peer (list timeout),
each item tagged with its agent's full address and node. Now gains an
**Activity** band between *Needs you* and *The fleet*, present only
while something runs: one row per item — kind glyph, agent, label,
elapsed — grouped by agent, clicking through to the session's drawer
(or the subagent view when it has a transcript). Polled every 3 s.

**Cost.** Small. Rust: the field, the fan-out route. UI: the band.

---

## 3. Notifications (S-3)

**Ask.** Aspen knows when a session finishes a turn, asks a question,
needs a permission, or when a background activity settles. Tell the
operator when the tab is not in front, and optionally somewhere else.

**Today.** Nothing. `Needs you` is a poll; the tab has no title badge.

**Design.**

*Notices.* The pump raises a `Notice {id, ts, node, agent, kind,
title, body, link}` for: `turn_ended` (body: the reply's first line),
`question` (an `AskUserQuestion` prompt), `permission` (any other
prompt), `activity_settled` (an item that was running at the previous
turn end and is not now — the ledger diff), `exited` (non-zero code or
unexpected), and `inbox` (a bus message to @operator). Notices are
kept in a store table `notices` (pruned at 7 days) and are per node.

*Console.* A node-level `GET /api/notices?since=<id>` returns local
notices and, with `mesh=true` (default), fans out to up peers with
their own cursors. The console polls every 3 s and: shows a toast
stack (bottom right, dismissible, click opens the link); prefixes the
document title with a count of unseen notices; and, when the operator
has turned it on, raises a browser Notification per notice while the
page is hidden. A **Notifications** panel in Now's header (a bell)
holds the toggles: browser notifications on/off (asks permission),
per-kind on/off, and per-node on/off — all in `localStorage`, since
they describe this browser. Marking a notice seen is local too.

*Outbound hook.* Node settings `notify.webhook` (a URL that receives
the notice as JSON by POST) and `notify.command` (a program given the
JSON on stdin), each with `notify.kinds` (default: question,
permission, exited). Fired by the raising node, at-most-once, with a
5 s timeout, failures logged. This is how a phone gets told (ntfy,
Pushover, a Slack webhook) without Aspen knowing any of them.

**Implies.** A notice is the unit "Needs you" was missing: the
`needs` count and the notices are two views of the same moments.
Notices are not needs — a turn ending is informational — so Now keeps
its bands and the bell is beside them.

**Cost.** Rust: `notices` table, raising in the pump, the route and
op, the hook sender. UI: toast stack, title badge, the bell panel,
Notification API. Docs: NOTIFICATIONS.md.

---

## 4. Cost and usage roll-up (S-4)

**Ask.** What each session, repo and node has cost and consumed, with
subagents folded in.

**Today.** `WorkSummary.cost_usd` is the harness's cumulative figure
for the *current process* only; per-turn deltas are recorded as fleet
events (`cost_delta`); rehydration drops `message.usage` and
`message.model` entirely.

**Design.**

*Tokens from the transcript.* `aspen_claude::usage::session_usage(repo,
sid)` folds every assistant line's `usage` — input, output,
cache_read, cache_creation — and `model`, per model, over the main
transcript and every `subagents/*.jsonl`; cached by (size, mtime) like
the ledger. Assistant items from rehydration also carry `usage` and
`model` so the session view can show per-turn figures on the turn-end
marker (tokens in / out, cache hit ratio).

*Money from the harness.* Cost is the harness's number, not ours: the
sum of `cost_delta` over the `turn` events for the agent (survives
restarts) is the authoritative spend Aspen observed. An *estimate* from
a price table is shown beside it only when a model has a known rate
(settings `usage.prices` — per-model input/output/cache-write/cache-read
per MTok — with defaults for the current families and blanks for
unknown models); the estimate is labeled as such.

*Surfaces.* A **Usage** page (`/usage`, `u`): a table of sessions with
node, repo, model mix, tokens in/out/cached, turns, observed cost,
estimate; group-by node / repo / model; a day/week/all range using the
`turn` events for time and the transcript for totals. The session
status bar's `session $X` becomes a popover with tokens by model and
the subagent share. Node rows in the Mesh list carry today's spend.
`GET /api/usage` fans out (`usage` op per peer).

**Cost.** Rust: usage.rs, rehydrate fields, the route and op, the
price table setting. UI: the page, the popover. Docs: USAGE.md.

---

## 5. Memory convergence (S-5)

**Ask.** Project memory edited on several nodes converges.

**Today.** Memory (`<project dir>/memory/`) moves with a session as
tier C; a differing file on import is kept beside as
`<name>.from-<node>` and reported (MIGRATION.md §6). No standing sync.

**Design.**

*Opting in.* Node setting `memory.sync = true` (default off), from the
Mesh panel. Only repos that have a counterpart on the peer take part
(same git origin, else same basename — `find_counterpart`).

*Base.* The node keeps `memory_base(repo, rel, content_hash, content)`
— the last content both sides agreed on. First sync with no base: if
the files are equal, that is the base; if only one side has the file
it is copied; if both differ with no base, keep both (today's rule)
and record a conflict need.

*Exchange.* The roster carries `memory_digest` per exposed repo (hash
over `(rel, content hash)`). A peer whose digest differs for a repo
it holds pulls `memory_files {repo}` → `[{rel, hash, content, mtime}]`
(text only, canonicalized with `PathCtx` sentinels so paths translate)
and merges per file: remote == base → keep local; local == base → take
remote; else a line-level 3-way merge (`diffy`); a clean merge writes
the result and the new base; a conflict writes `<name>.from-<node>`
beside and raises a `memory_conflict` need in Now (with a resolve
verb: keep mine / take theirs / open both in the viewer). Deletions
are tombstones in the base table for 30 days.

*Sessions.* A live session reads memory when it recalls; a changed
file is picked up on its next read. Nothing is injected.

**Cost.** Rust: table, digest, op, merge, need. UI: the need card and
verbs, the setting. Docs: MEMORY.md. Crate: `diffy`.

**Open.** Whether the automerge text CRDT earns its place after this:
only if line-merge conflicts turn out common (SYNC.md §3).

---

## 6. Hybrid logical clock (S-6)

**Ask.** Last-writer-wins on mesh-wide rows should mean causally last,
not whose clock runs fast.

**Design.** `Store::hlc_now()` issues `max(wall, last_issued + 1 ms,
max_seen + 1 ms)` and persists `last_issued`; every merge of a foreign
row calls `hlc_observe(updated_at)`. Values stay seconds-as-f64, so
older nodes interoperate unchanged and digests keep their `{t:.3}`
form. Equal timestamps keep the local row (the existing `>=` gate); a
node name tiebreak is unnecessary at 1 ms resolution for one operator.
Applies to boards, marketplaces, plugin rules, and every table added
after (templates §7, replication rows §1, memory base §5).

**Cost.** Tiny. Rust only. Documented in SYNC.md.

---

## 7. Session templates (S-7)

**Ask.** A named recipe, spawnable in one click or from the CLI.

**Design.** A mesh-wide table `templates {id, name, repo (handle or
origin or null = ask), model, permission ("ask"|"skip"), charter,
extra_args, plugins [{marketplace, plugin, pin}], board {id, mode:
"split"|"replace"|"none"}, updated_at, deleted}` synced by digest.
Spawning from a template: resolve the repo (the template's, else the
counterpart on this node, else ask), spawn with its model/charter/args/
permission, write session-scope plugin rules for the new name (so the
session starts with `--plugin-dir`s), then place it on the board.
Surfaces: a **Templates** section on the Plugins page (renamed
*Library* in the rail: plugins and templates), a "new from template"
verb in the palette and on each repo strip, and `aspen session new
--template <name> [--repo] [--name] [--node]`.

**Cost.** Rust: table, digest, sync, spawn path, CLI. UI: the section,
the palette verb. Docs: in PLUGINS.md as a new section (the page is
shared).

---

## 8. Evacuate a node; bring it here (S-8)

**Ask.** Move every live session off a node before servicing; a
one-click on any session to pull it to this node; a preflight readout.

**Design.**

*Evacuate.* `POST /api/mesh/{node}/evacuate {to}` (CLI `aspen evacuate
<node> --to <node>`). The node enters `Draining {purpose: evacuate}`
(spawns refused, roster says so), and the target pulls each live
session in turn — busy ones wait for their turn boundary, up to the
quiet gate — reporting per agent; sessions whose repo has no
counterpart on the target are skipped and named. The node returns to
Ready when the list is empty or on cancel. Mesh panel: an *evacuate…*
two-step on each peer row with a target picker and a live readout;
the same on the node's own row (to a chosen peer).

*Bring it here.* Every remote session's header and Now card gets
*bring here*: move to this node with the counterpart repo, one click,
with the preflight shown inline before confirming.

*Preflight.* `GET /api/agents/{name}/move/preflight?to=` →
`{tiers: {A: bytes, B, C, D, E}, harness: {source, target}, dirty:
bool, counterpart: path|null, replica: {as_of}|null, busy: bool}`,
from a `session_preflight` op on the source plus the target's own
lookups; the move dialog shows it and disables the button while a
blocker stands (no counterpart, target updating).

**Cost.** Rust: servicing state variant, the evacuate driver, the
preflight op. UI: the panel verbs, the readouts, the one-click. Docs:
SERVICING.md and MIGRATION.md sections.

---

## 9. Multi-mesh membership (S-9)

**Ask.** One node in more than one mesh (docs/proposals/multi-mesh.md).

**Design** — the proposal's §8 order, in one round:

1. *Capability layer.* Every op in `serve_api_req` is classed
   `observe | control | spawn | trust`. Each mesh has a `policy` in its
   config: `full` (all four, today's behavior) or `observe`. The
   policy is set at join (`--policy`; the first mesh defaults to full,
   later ones to observe) and changed with `aspen mesh policy <mesh>
   <policy>`; the guard runs before dispatch and refuses with
   `forbidden: <cap> not granted to mesh <m>`; every `spawn`/`trust`
   op from a peer is recorded as a fleet event with node and mesh.
2. *Per-mesh state.* `meshes.json` (list; `mesh.json` migrates to one
   entry), `identity.json` gains `certs: {mesh: cert}`. `NodeInner
   .meshes: RwLock<Vec<Arc<MeshState>>>`; `inner.mesh()` becomes
   `inner.meshes()` + `inner.mesh_for_peer(node)`; the listener accepts
   a hello for any mesh we hold a cert in and verifies against that
   mesh's root; dialers, relay clients and roster broadcasts run per
   mesh. Joining refuses a mesh whose peer names collide with any
   existing peer or with us. `aspen mesh leave <mesh>`.
3. *Exposure.* `repo_meshes(path, mesh)`; with one mesh everything is
   exposed; joining a second pins every existing repo to the first;
   new repos default to none while multi-mesh. Roster, bus delivery,
   `node_repos`, `node_sessions`, `spawn`, needs and history filter by
   it. Mesh list: an "exposed to" chip per repo.
4. *Console.* Mesh page grouped by mesh; remote agents carry a mesh
   tag; the ceremony panel takes a mesh selector; a channel or link
   spanning meshes is labeled *bridge*.

**Cost.** The largest item: federation.rs, mesh.rs, node.rs, meshops,
api.rs, the SPA's mesh surfaces. Docs: MESHES.md (from the proposal).

---

## 10. Console through the relay; WSL detection (S-10, P-4, P-5)

**Ask.** Reach a node without a forwarded port from a browser that
has no daemon of its own. Say so when a WSL node is only advertising
NAT-internal addresses.

**Design.**

*The browser is a peer.* The relay stays blind: the console holds its
own identity (ed25519 + x25519 generated in the browser, kept in
IndexedDB) and a **console cert** — a root-signed `NodeCert` with
`node: "console-<id>"` minted by `aspen mesh certify --console`
(or the ceremony panel), scanned in as a blob or deep link. It
registers with the relay like any node, and speaks the federation
protocol to one chosen member: hello, sealed envelopes (XChaCha20
-Poly1305 over x25519, `@noble/ciphers` + `@noble/curves`), then
`api_req`/`api_res` and `sub`/`ev`. Console certs are capability
`control` (no spawn, no trust) and are listed and revocable in the
Mesh panel; the node keeps them in `consoles.json`.

*One op to carry REST.* The node gains an `http` op: `{method, path,
body}` dispatched into its own axum router (`Router::oneshot`), so the
tunnel needs no per-endpoint mapping; `api.ts` gets a transport
switch (`fetch` or tunnel) and the events socket becomes a `sub`.

*Hosting.* The standalone `aspen-relay` and the embedded relay host
serve the SPA at `/console` when started with `--console`; the page
asks for a relay URL, a cert blob, and which node to attach to.

*WSL.* `advertised()` notes when every interface address is in the
WSL NAT range with no operator-set advertise URL; the roster carries
`advertise_hint: "wsl-nat"` and the Mesh panel says "this node is
reachable only through a relay or a forwarded port
(`aspen config advertise …`)".

**Cost.** UI: a crypto/tunnel module and the attach page. Rust: `http`
op, console certs, relay `--console`. Docs: RELAY.md §11.

---

## 11. Pair mode on boards (S-11)

**Ask.** Two panes' sessions joined on a bus thread.

**Design.** Select a pane, *pair with…* another: the board stores
`pairs: [[paneA, paneB]]`; pairing adds a two-way topology link
between the agents with purpose "paired on board <name>", and sends
each a `notice` bus message naming the other and the thread id
`pair:<board>:<a>:<b>`; their `bus_send` calls that carry the thread
are grouped. The board shows a **thread strip** under the pair (the
bus log filtered by thread, live, with an operator composer that posts
to both on the thread), the pane bars carry a pair glyph, and the
operator's own messages to either pane can be mirrored to the other
with the thread (a per-pair toggle, like broadcast). Unpair removes
the link and the strip; the messages remain in Flow.

**Cost.** Rust: nothing new beyond `thread` already existing; the
link/notice calls exist. UI: Board pairs, the strip. Docs: BOARDS.md.

---

## 12. Task notifications collapsed (S-12)

**Ask.** A `<task-notification>` line (the harness telling the model a
background task or subagent finished, with the whole result inline)
renders as a user bubble; one spanned dozens of screens.

**Design.** Rehydration and the live reducer recognize a user line
that is a task notification (starts with `<task-notification>`) and
tag it `notification: true`. The transcript renders it as a
**notice card**, the tool-card style: one line — glyph, "task
notification", the `<summary>` text, the status — collapsed by
default, click to expand the full text. The activity ledger already
consumes these lines for its end times, so the card links to the
activity drawer item by task id.

**Cost.** Tiny: a predicate in transcript.rs (Rust) and in
transcript.ts, a card in Session.tsx.
