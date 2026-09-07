# Backlog

Feature requests and design candidates, tagged and dated. Each entry names
the ask as the operator put it, the swath it belongs to, and where its
design lives once one exists. Shipped items are summarized here with the
tag they carried and detailed in the decisions log, DESIGN.md §14b.

Tags: `console` (the web UI), `protocol` (mesh/bus/wire), `sessions`
(a harness process and its store), `servicing`, `docs`.

## Open

| id | tag | ask | notes |
|---|---|---|---|
| H-2 | sessions | **Shared per-node app-server for Codex.** One `codex app-server` hosting every Codex thread on the node instead of one process per session; same adapter, fewer idle processes. Needs a liveness/revive model that does not equate a session with a process. | PROPOSALS-HARNESSES.md §6.1 |
| H-3 | sessions + protocol | **Codex memory convergence.** Codex memory is global (`$CODEX_HOME/memories_*.sqlite`), not per project; decide whether and how it takes part in memory sync (MEMORY.md), or is declared out of scope. | HARNESSES.md §6 |
| H-4 | sessions + console | **Codex plugins and skills in the library.** `$CODEX_HOME/plugins` and skills roots as library scopes beside Claude's marketplaces; activation per mesh / node / repo / session like PLUGINS.md. | PLUGINS.md |
| H-5 | sessions + console | **`turn/steer` as a first-class control.** Today a mid-turn message steers by default; expose "queue for next turn" vs "steer now" in the composer, for both harnesses where they can. | CODEX_RUNTIME_REFERENCE.md §4 |
| H-6 | sessions + console | **Third-party MCP elicitations.** Codex forwards a server's own form/url elicitations; Aspen declines them today. Surface as a prompt kind (`elicitation`) with a generic form card. | HARNESSES.md §1 (`PromptKind`) |
| H-7 | sessions | **Deny message to Codex.** The console's deny text is not delivered to Codex (the approval reply has no channel for it); consider a follow-up user message carrying it, or drop the field for Codex prompts. | CODEX_RUNTIME_REFERENCE.md §6.1 |
| H-8 | sessions + console | **Codex activity ledger.** `collabAgentToolCall` / `subAgentActivity` items show as tool cards; fold them into the activity ledger (ACTIVITY.md) with counts and a drawer, and read the agent threads' rollouts. | HARNESSES.md §6 |
| H-9 | console | **Harness badge on Usage rows.** Session and fleet rows carry the chip; the Usage table does not yet. | — |
| P-2 | servicing | Release signing (minisign) before `auto` policy is recommended in public. | SERVICING.md §14 |
| P-3 | protocol | Relay preference ordering (the list is ordered; nothing consumes it). | RELAY.md §10 |

## Shipped — 2026-09-07: Codex as a second harness (H-1, v0.20–v0.23)

Design first ([PROPOSALS-HARNESSES.md](PROPOSALS-HARNESSES.md)), then
four rounds, each verified on the rig with the Claude flows re-run:
v0.20 the seam (HARNESSES.md §1–5), v0.21 the Codex adapter
(`aspen-codex`, `aspen mcp`; HARNESSES.md §6, CODEX_RUNTIME_REFERENCE.md),
v0.22 the console and the gates (HARNESSES.md §7), v0.23 the mesh
features (HARNESSES.md §8, MIGRATION.md "Harnesses"). H-2..H-9 above are
what it left open.

## Shipped — 2026-09-07 slate (S-1..S-12, v0.15–v0.19)

Design: [PROPOSALS-2026-09-B.md](PROPOSALS-2026-09-B.md).

| id | tag | ask | round |
|---|---|---|---|
| S-1 | protocol + sessions | Transcript replication, opt-in (REPLICATION.md). | v0.16 |
| S-2 | console | Fleet-wide activity in Now. | v0.15 |
| S-3 | console + protocol | Notifications: toasts, bell, webhook and command hook (NOTIFICATIONS.md). | v0.15 |
| S-4 | console + sessions | Cost and usage roll-up (USAGE.md). | v0.15 |
| S-5 | protocol + sessions | Memory convergence with a stored base and 3-way merge (MEMORY.md). | v0.16 |
| S-6 | protocol | Hybrid logical clock for mesh-wide rows (SYNC.md §3). | v0.16 |
| S-7 | sessions + console | Session templates (PLUGINS.md §templates). | v0.17 |
| S-8 | servicing + console | Evacuate a node, bring it here, preflight readout (MIGRATION.md). | v0.17 |
| S-9 | protocol | Multi-mesh membership for one node (MESHES.md). Closes P-1. | v0.18 |
| S-10 | protocol + console | Console through the relay (RELAY.md §11). Closes P-4 and P-5 (`hint: wsl-nat`). | v0.18 |
| S-11 | console + protocol | Pair mode on boards (BOARDS.md §8). | v0.19 |
| S-12 | console | Task notifications collapsed to summary cards. | v0.15 |

S-1, S-5 and S-8 together close B-5b (migration phases 3–4).

## Shipped — 2026-09-06/07: the console round (B-1..B-8, v0.10–v0.14)

Design: [PROPOSALS-2026-09.md](PROPOSALS-2026-09.md).

| id | tag | ask | round |
|---|---|---|---|
| B-1 | console | Composer drafts persist. | v0.10 |
| B-2 | console | Tool calls live while running, summary after, click to expand. | v0.10 |
| B-3 | console + protocol | Open what the agent points at, from any node (the viewer). | v0.10 |
| B-4 | console + sessions | Paste attachments inline. | v0.10 |
| B-5 | protocol + sessions | Session migration over the mesh, phases 1–2 (MIGRATION.md). | v0.11 |
| B-6 | console | Boards (BOARDS.md). | v0.12 |
| B-7 | protocol + sessions + console | Plugins: a mesh-managed library, activated by scope (PLUGINS.md). | v0.13 |
| B-8 | sessions + console | Activity: tasks, monitors, subagents, workflows (ACTIVITY.md). | v0.14 |
