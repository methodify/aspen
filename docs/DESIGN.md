# Aspen — product design (working draft)

> **Aspen** — a rhizomatic organism: what looks like a forest of separate trees is one living root system. Component verbiage stays neutral and factual (node, mesh, bus, session, operator). This document expands the founding
> notes into product contours: what it is, the concepts it's made of, how the
> pieces work, and what we deliberately are not building. It is a conversation
> artifact, not a spec — sections marked **[open]** are decisions still to make.

---

## 1. What it is

**A control plane for a fleet of coding agents across all your repos and all
your machines.**

- One **node service** per machine that launches, supervises, and speaks the
  native protocol of any number of agent sessions, each running in the repo it
  works on.
- One **bus** that lets those agents talk to each other — automatically within
  a repo, deliberately across repos and machines — and that makes the human a
  first-class participant.
- One optional, deliberately minimal **rendezvous** in the cloud that stitches
  nodes together across any network topology, holding no state that matters.
- One **web UI** that is the window into all of it: the topology, the repos,
  the sessions, the mailboxes — and the single inbox of everything that needs
  the human.

The thesis, inherited from six months of bus work culminating in plumb:
multi-agent work fails at the *seams* — messages that arrive after the work
they governed, agents that go silent in a way indistinguishable from healthy,
humans reduced to couriers between sessions. Aspen attacks the seams by owning
the one thing plumb never could: **the pipe itself.** Every session is
Aspen-launched over the headless NDJSON protocol, so delivery is push, turn
state is exact, and nothing depends on hooks or monitors racing each other.

### What it is not

- **Not a hierarchy engine.** claude-orgtree (an acknowledged catalyst; MIT)
  aims at org-chart delegation — agents hiring reports under a credit/seat
  system, retirement dissolving subtrees. Aspen's shape is *peers on a bus plus
  an operator* — hierarchy can be expressed as a usage pattern (a charter can
  say "direct the impl sessions"), but the infrastructure doesn't privilege
  it. What we do take from orgtree: the notice/mail distinction (§4.2),
  charters (§3), and the standard of individually visible, addressable,
  persistent agents. What we deliberately don't: credits, hire/retire
  authority chains, and driving agents through a bespoke orchestrator instead
  of the runtime's own protocol.
- **Not a methodology.** plumb carries the norms and ceremonies; Aspen carries
  the wires. Aspen should be adoptable with no methodology at all.
- **Not a chat-history store.** The agent runtime owns its history and replays
  it (`-r`); Aspen enumerates transcripts from disk and never maintains a
  parallel registry (the Studio's registry-drift lesson).
- **Not commercial, for now.** Built for one demanding user, then open-sourced
  once polished. A hosted turnkey rendezvous is a *maybe later*.

---

## 2. Vocabulary

| Term | Meaning |
|---|---|
| **Node** | A machine running the Aspen node daemon (`aspen`). Owns its repos, sessions, and bus store. |
| **Mesh** | All of a user's nodes together, joined under one identity. |
| **Repo** | A project directory registered with a node. The unit of automatic agent community. |
| **Session / agent** | One live agent runtime (Claude today, Codex-shaped later) attached to a repo, with a stable name and address. |
| **Operator** | The human. A first-class bus participant with an address, not just a spectator. |
| **Channel** | A named multi-party stream. Every repo gets one automatically; custom channels span repos and nodes. |
| **Adapter** | The module that speaks one agent runtime's native protocol and normalizes it to Aspen's internal contract. |
| **Rendezvous** | The minimal cloud relay. Routing and presence only; nodes hold all real state. |

**Addressing.** Every participant has a stable address:
`agent@repo.node` (e.g. `arch@contextua.gpu-box`), with short forms resolved
contextually (`@arch` within a repo channel). The operator is `@operator`
(reachable from anywhere in the mesh). Channels are `#contextua` (auto,
per-repo) or `#release-train` (custom, cross-cutting).

---

## 3. The node service

One daemon per machine. Detached from any terminal (the supervision lesson:
runtimes must not die because a shell did). It embeds:

- **Session manager** — spawns agent processes via adapters, each with cwd =
  its repo; tracks lifecycle; enforces the shutdown ladder (`end_session` →
  stdin EOF → SIGTERM → kill); performs transparent restarts (`-r` same id)
  gated on turn state.
- **Adapter registry** — Claude first; the interface is the modularity seam
  (§5).
- **Bus store** — SQLite, WAL mode, per node. Carries messages, delivery
  records, channel membership, and the trail.
- **Local API** — HTTP + WebSocket on localhost. Serves the SPA, streams
  normalized session events, accepts operator actions, and serves the
  agent-side connector endpoint for non-Claude adapters.
- **Transport layer** — connections to peer nodes: direct where reachable,
  relay where not (§7).

### Session lifecycle specifics (Claude adapter, from the field reference)

- Spawn with the verified argv (`--print --verbose --input-format stream-json
  --output-format stream-json --include-partial-messages
  --replay-user-messages --permission-prompt-tool stdio --session-id <uuid>`),
  `CLAUDE_CODE_ENTRYPOINT=aspen` (stamps transcripts — Aspen sessions are
  distinguishable on disk forever).
- Warm start on registration; handshake eagerly so pickers and rosters are
  populated before the first message.
- **Session naming**: default from the runtime's own title chain
  (custom-title → ai-title → first user message), plus
  `generate_session_title`; the operator can override, and Aspen's name wins in
  every Aspen surface.
- **Session identity beyond a name**: an optional **charter** — a short role
  prompt ("you are the architect for this repo; impl sessions report findings
  to you") injected via `appendSystemPrompt` at spawn/resume, editable in the
  UI. Borrowed from orgtree's charters, minus the org chart: Aspen charters
  describe *who you are on the bus*, not whom you command.
- **Session enumeration is the filesystem**, not an Aspen registry. Resuming a
  session another entrypoint created (terminal `claude` in the same repo) is
  supported — it just joins the mesh.
- **Trust gate**: `-p` sessions never show the workspace-trust dialog, so Aspen
  owns it. Before first spawn in a repo: enumerate what would auto-run
  (hooks, `.mcp.json` servers, skills, plugins), show it in the UI, record
  consent. First-class feature, not a guard clause.

---

## 4. Agent interaction — the four flows

### 4.1 Out: the agent's stream

All frames flow to `aspen` over stdout; the adapter peels control frames and
normalizes the rest into Aspen's event vocabulary (§5). The runtime holds
history; Aspen holds only what its UI needs live plus the bus trail.

### 4.2 In: bus delivery

A bus message reaching an agent is **injected as a user-typed message with an
unmistakable envelope header**:

```
[aspen bus] from @arch (contextua @ gpu-box) · #contextua · thread t-7
```

followed by the body. Delivery semantics come straight from plumb's proven
model, upgraded by pipe ownership:

| Recipient state | `notice` | `normal` | `gating` |
|---|---|---|---|
| idle | held until they next run a turn — **never wakes anyone** | inject now (wakes them) | inject now |
| mid-turn | rides along with the next delivery / next turn | inject now — the CLI queues and coalesces it into the **next turn** (delivery at the boundary, for free) | `interrupt` control request (~110 ms), then inject |
| session not running | held in store; next session start | held in store; delivered at next session start | same, marked late |

The third class, `notice`, is stolen from claude-orgtree's cleanest idea: an
explicit event-vs-message distinction. Roster changes, channel membership,
"a peer went idle" — ambient facts an agent should *have* but that must never
cost anyone a wake-up. Most systems conflate these with messages; we won't.

- Everything pending delivers together, **in send order** — class never
  reorders (a gating message may depend on the normal ones before it).
- **No acks, no receipts, by inheritance and by scar.** The bus memorializes
  passively: `delivered_at`, `delivered_via`, repo commit hash at delivery —
  and now, because of `--replay-user-messages`, the replay ack closes the loop
  with *proof of ingestion*, something plumb never had. A sender who needs
  confirmation asks in the message.
- Delivery notes at send time: if the recipient session is down, the sender is
  told *at the moment of sending* how the message will actually land.

### 4.3 Agent → bus: in-process MCP

For Claude, Aspen doesn't spawn an MCP connector at all: **`aspen` registers
itself via `sdkMcpServers` in the handshake and serves the tools over the
control channel.** `bus_send`, `bus_status`, `bus_inbox`, channel tools —
zero extra processes, and the MCP `instructions` field carries the bus
contract as model-visible steering for free.

For adapters without an equivalent (Codex-shaped runtimes), the fallback is
the thin-connector pattern from the founding notes: a tiny stdio MCP script
that forwards to `aspen`'s localhost endpoint with a per-session token. The
capability lives in one place either way; only the last inch differs.

Tool descriptions carry the guidance (plumb's lesson: the contract travels
with the session, not with a skill that may not be loaded).

### 4.4 Roster: who's here

Agents learn who they can talk to by:

- **Pull**: `bus_status` — peers, channels, liveness, pending counts.
- **Push**: roster deltas ride along with bus deliveries ("@impl joined
  #contextua"), and session-start injection seeds the initial roster.
- **[open]** The protocol accepts `system`-typed stdin messages spliced into
  context (HOST_PROTOCOL §5) — a cleaner injection lane for roster/state
  updates than user-typed messages. Needs live verification before we depend
  on it; user-message injection with headers is the safe floor.

---

## 5. The adapter seam (modularity for Codex et al.)

One interface, one Claude implementation to start, designed so the second
adapter is an implementation project rather than an architecture project.

```
AgentAdapter
  capabilities(): { streaming, midTurnInject, interrupt, permissionCallback,
                    inProcessMcp, resume, forkSession, ... }
  spawn(repo, opts) / resume(sessionId) / shutdown() / interrupt()
  sendUser(content, meta)          // operator or bus traffic
  events → normalized stream:
    session-meta | turn-started | text-delta | block | tool-use | tool-result
    | permission-request | question | turn-ended(result) | error
```

Rules of the seam:

- **The adapter owns every protocol quirk.** Snapshot-reconciling merge,
  replay acks, `result`-is-the-only-turn-end — none of it leaks upward.
- **Capabilities degrade honestly.** A runtime without interrupt support gets
  `gating` delivered as `normal` *and the sender is told so* (the delivery
  note pattern). No silent downgrades.
- **The native layer stays thin and schema-ignorant** (the Studio's
  architecture that survived a protocol rebuild): process spawn/kill and line
  discipline in one place, all protocol semantics in the adapter.

---

## 6. The bus, properly

Inherited from plumb (each item is scar tissue, not taste):

- **Urgency is delivery timing, nothing else.** Two classes, `gating` and
  `normal`. Urgency rations *derailment*.
- **Silence is not loss.** The trail (`delivered_at` etc.) answers questions;
  nobody re-sends on a hunch.
- **A ruling goes on the durable record before the wire** — messages carry an
  optional `record` ref; the bus is the notification, not the record.
- **No subject line.** The body is the message.
- **The observer's window is never a participant's chair** — every UI bus
  view is read-only over the store except the operator's own composer, which
  sends *as the operator*.

New, because Aspen spans repos and machines:

- **Scopes.** Same-repo agents are auto-joined to the repo channel — zero
  configuration, the founding requirement. Custom channels are
  operator-created and span anything. DMs address any agent in the mesh.
- **The operator address.** Agents can `bus_send --to @operator`. That, plus
  permission prompts and AskUserQuestion cards, feeds one unified **operator
  inbox**: everything in the entire mesh that needs a human, in one queue.
  This is arguably the killer feature.
- **Cross-node delivery** is store-and-forward: the sending node keeps the
  message until the owning node confirms ingestion into *its* store
  (at-least-once, deduped by message uuid). The trail records which transport
  carried it.
- **plumb interop [open]**: a Aspen-launched session in a plumb project should
  ideally appear on plumb's per-project bus too, or plumb's bus should learn
  to defer to Aspen's when present. Decide when we get there; the semantics are
  deliberately identical to make bridging cheap.

---

## 7. Federation: nodes, transports, rendezvous

**Nodes do all the work.** Every node holds its own sessions, bus store, and
repo registry. Federation is only about moving envelopes between nodes and
letting a UI see the whole mesh.

### Transport ladder (pluggable, tried in order)

1. **Loopback** — same node: a SQLite write.
2. **Direct** — nodes that can reach each other (LAN, or an overlay network
   the user already runs — **Tailscale answers the "leverage what users
   already have" question**: it's not cloud compute users have lying around,
   it's connectivity; a tailnet makes every node mutually reachable and the
   cloud component optional entirely).
3. **Relay** — the rendezvous, for nodes with no path to each other.

### The rendezvous, minimal by construction

- Nodes connect **outbound WSS only**; no inbound ports anywhere, ever.
- It does four things: authenticate nodes to a mesh, route envelopes by node
  id, report presence, and (bounded, TTL'd) spool envelopes for offline
  nodes. Nothing else.
- **It cannot read or forge traffic.** Envelope payloads are end-to-end
  encrypted between nodes and signed by the sender (§8). A fully compromised
  rendezvous yields metadata and denial of service — not command and control.
- Small enough to implement twice: target it at both a ~one-file container
  (Fly/Hetzner, pennies) and Cloudflare Workers + Durable Objects (WS
  hibernation ≈ free tier). **[open]** which is primary; the protocol should
  not care.
- The SPA can also connect *through* the rendezvous to reach a node when it
  isn't on the same network — same E2E envelope rules, so the relay still
  can't drive anything.

---

## 8. Security model

Aspen is, frankly, a C2 system for machines that run code. Design accordingly:

- **A mesh is rooted in a user-held keypair** (created at mesh init; backed by
  passphrase or keyfile). Nodes each generate their own keypair and join via
  a single-use invite token signed by the root key. There is no
  username/password anywhere in the core.
- **Every envelope is signed by its sending node and encrypted to its
  receiving node.** The rendezvous authenticates nodes (challenge-response on
  the node key) purely to route and rate-limit.
- **Operator actions are commands and are signed too.** A UI session holds an
  operator credential (the root key unlocked locally, or a delegated,
  expiring, scope-limited key — **[open]** exact shape; WebAuthn/passkey
  wrapping is attractive for the browser). The rendezvous relaying a UI
  connection never gains the ability to originate commands.
- **Local surface**: `aspen`'s localhost API requires a token; agent-facing
  connector endpoints use per-session tokens minted at spawn. Nothing
  listens beyond localhost unless explicitly configured (direct transport
  binds to the overlay interface).
- **Blast-radius honesty in the UI**: the trust gate (§3) plus per-repo
  permission posture, visible grants (per-tool, per-scope — the "always allow
  is a rule, not a mode" legibility lesson), and an audit trail of who/what
  approved each grant.

---

## 9. The SPA

Served by any node (and, later, connectable via the rendezvous). One page,
several lenses:

- **Mesh** — nodes, repos, sessions, health, transports in use. The map.
- **Session** — live transcript (token streaming, the three-layer merge, tool
  cards, subagent nesting), composer, permission cards, question cards,
  context meter (`get_context_usage` is a gift — pre-computed grid included),
  session-cumulative cost labeled honestly.
- **Bus** — channels and DMs, mailbox per agent (busview's heir), the trail
  with filters (`--record`, `--thread`, sender/recipient), operator composer.
- **Repo** — sessions in the repo, trust status, the full discovered
  inventory (skills, commands, MCP servers, hooks, memory files — all
  enumerable over the wire with source attribution; never parse `.claude/`
  ourselves).
- **Operator inbox** — the cross-mesh queue of everything awaiting a human:
  permission prompts, agent questions, messages to `@operator`, trust gates.
- **Skills manager** — view/edit skills and commands, `reload_plugins` wired
  to save. (Minimum management surface from the founding notes.)

Design stance: the UI is an *operator's console*, not a chat app that happens
to have n tabs. The inbox and the mesh map are the home surfaces; individual
transcripts are drill-downs.

**But the drill-down is total.** Stepping into a session means *sitting down
at it*: a fully interactive session indistinguishable in capability from
having launched `claude` there yourself — send messages, run slash commands
(autocomplete from the handshake's `commands[]`), answer permission prompts
and questions, interrupt, switch model/mode, watch token streaming. Leave,
and the agent keeps working; return, and you're back in the chair. "Not n
chat tabs" is about the *home posture*, never a ceiling on interactivity.

The session lens has three render modes over the same transcript:

1. **Chat** — fully rendered markdown, tool cards, the works; the
   claude.ai-grade reading experience.
2. **Console** — a TUI-feel rendering that looks and feels like claude in a
   terminal (a natural fit for a later actual ratatui TUI sharing the model).
3. **Source** — the chat layout but with raw markdown bodies, for effortless
   copy/paste of formatted content.

---

## 10. What Aspen stores (and pointedly doesn't)

| Store | Where | Contents |
|---|---|---|
| Bus store | per node, SQLite | messages, deliveries, channels, trail |
| Mesh config | per node + root | node identity keys, peers, rendezvous, repo registry, trust records, session name overrides |
| Transcripts | **the runtime's own** `~/.claude/projects/...` | not ours; enumerated, never mirrored |
| Rendezvous | cloud | node registry, presence, bounded encrypted spool. Nothing durable that can't be regenerated. |

---

## 11. Phasing

1. **P0 — one node, alive.** `aspen` + Claude adapter + spawn/resume/converse
   + same-repo bus between two sessions + minimal web transcript. (The
   afternoon-host from the field reference, §13, is the seed.)
2. **P1 — the console.** Real SPA: session lens, bus lens, operator inbox,
   permission/question cards, trust gate, naming.
3. **P2 — the mesh.** Node identity/keys, direct transport (tailnet-first),
   then the rendezvous + through-relay UI access.
4. **P3 — breadth.** Skills manager, second adapter (Codex-shaped), plumb
   bridging, packaging, docs — the open-source-worthy polish pass.

Each phase is usable on its own; P0 already beats couriering between
terminals.

---

## 12. Platform posture

Targets: **Linux, Windows, macOS** — in that order. Linux (WSL2 dev box)
carries the whole functional suite first; the same box's Windows side is the
first Windows target; the MacBook closes it out. Platform *tuning* is a
polish-phase activity, but the node code is written **platform-aware from day
one**:

- No unix-only assumptions in core crates: process spawning, signals, paths,
  and file locking go through small platform seams, not inline `cfg(unix)`
  scattered everywhere.
- The known Windows scars from the references are pre-registered as
  requirements: `CREATE_NO_WINDOW` on spawn, console code-page/UTF-8
  handling, pid-liveness probes (`OpenProcess`, never `kill(pid, 0)`),
  path-slug non-canonicalization for transcript discovery, detached-process
  supervision.
- CI builds all three targets from early on, even while only Linux is
  *exercised*.

---

## 13. Naming **[resolved]**

Ruled: the collective noun is **mesh**; telephony names are out. The frame
for round two: name the *bigger thing* — a living, non-hierarchical network
of many workers across many places, coherent under one human.

Chosen: **Aspen** — not Rhizome directly, but a rhizomatic plant, keeping all the richness of the figure. Sub-components are NOT cute-named after the metaphor; verbiage stays neutral. The round-two candidates, for the record:

- **Rhizome** — the botanical and philosophical figure of
  exactly this shape: a root system with no trunk, no top, where any node can
  connect to any other and the whole survives any cut. The precise
  counterpoint to an org *tree*. "orgtree grows up; rhizome grows sideways."
- **Mycelium / Hyphae** — the fungal web stitching a forest into one
  organism; hyphae are its individual threads. Same family as rhizome,
  wilder connotation.
- **Continuo** — from music: the continuous line that runs under an ensemble
  of independent voices and keeps them one piece. The daemon *is* a continuo.
- **Consort** — an ensemble of equal voices; also the verb: to keep company,
  to communicate.
- **Orrery** — the working model of many bodies in motion under one
  observer's hands. Possibly best kept as the name of the mesh-map view
  rather than the product.

---

## 14. Decisions log

Resolved 2026-08-30 with the operator:

1. **Collective noun**: mesh. **Product name**: **Aspen** (§13).
2. **Transports**: direct/tailnet is the blessed first-class path; rendezvous
   is the universal fallback. Non-direct fully supported, just not first.
3. **Rendezvous hosting is modular by design.** First implementation:
   Cloudflare Workers + Durable Objects. An Azure Functions adapter may
   follow if needed. The node↔rendezvous protocol must not care.
4. **Stack**: **Rust** for `aspen`, relay, and rendezvous (speed, footprint,
   and ratatui keeps a first-class TUI open). **React + TypeScript** for the
   SPA.
5. **Operator credential**: as §8 (root key + delegated expiring keys;
   passkey wrapping explored during P2).
6. **Terminal-session attach**: punted indefinitely. All agents are
   aspen-launched in the operator's own usage; revisit only if a real need
   appears.
7. **Session view is fully interactive** (§9) — "operator's console" is a
   posture, not a limit.
8. **Platforms**: Linux → Windows → macOS; aware from day one, tuned at
   polish (§12).

Still to verify live, not blocking: `system`-typed stdin injection as a
cleaner lane for roster updates than user-message headers.
