# Backlog

Feature requests and design candidates, tagged and dated. Each entry names
the ask as the operator put it, the swath it belongs to, and where its
design lives once one exists. Shipped items move to the decisions log in
DESIGN.md §14b with the tag they carried here.

Tags: `console` (the web UI), `protocol` (mesh/bus/wire), `sessions`
(the claude process and its transcript), `servicing`, `docs`.

## Shipped — v0.10, v0.11 (2026-09-06), v0.12, v0.13, v0.14 (2026-09-07)

B-1 composer drafts, B-2 tool calls live-then-summary, B-3 artifact links
over the mesh, B-4 inline attachments (v0.10); B-5 session migration,
phases 1–2: move/copy over the mesh, export/import files (v0.11,
MIGRATION.md); B-6 boards (v0.12, BOARDS.md); B-7 plugins (v0.13, PLUGINS.md); B-8 activity (v0.14, ACTIVITY.md). See DESIGN.md §14b.

## Open — 2026-09-07 slate (three tiers, all approved)

Design: [PROPOSALS-2026-09-B.md](PROPOSALS-2026-09-B.md). Rounds: v0.15
(S-2, S-3, S-4, S-12), v0.16 (S-6, S-1, S-5), v0.17 (S-7, S-8), v0.18 (S-9,
S-10, P-5), v0.19 (S-11).

| id | tag | ask | design |
|---|---|---|---|
| ~~S-1~~ v0.16 | protocol + sessions | **Transcript replication, opt-in.** A session's transcript tail streams to a designated node so move is near-instant, History is mesh-wide, and a dead node's sessions stay readable. Off by default; enabled per mesh, node, or repo. (B-5b phase 4) | §1 |
| ~~S-2~~ v0.15 | console | **Fleet-wide activity in Now.** One list of everything running across the estate, with the v0.14 chips. | §2 |
| ~~S-3~~ v0.15 | console + protocol | **Notifications.** Turn end, question, permission prompt, activity settled: browser push from the console, plus an optional outbound hook. | §3 |
| ~~S-4~~ v0.15 | console + sessions | **Cost and usage roll-up.** Per session, repo, node; subagents folded in; from transcript usage fields. | §4 |
| ~~S-5~~ v0.16 | protocol + sessions | **Memory convergence** with a stored base and 3-way merge over the bus. (B-5b phase 3) | §5 |
| ~~S-6~~ v0.16 | protocol | **Hybrid logical clock** for mesh-wide rows (boards, marketplaces, rules). | §6 |
| S-7 | sessions + console | **Session templates.** Named recipe: repo, plugins, model, prompt additions, board placement; one click or CLI. | §7 |
| S-8 | servicing + console | **Evacuate a node; bring it here.** Move every live session off a node before servicing; one-click pull on any session; preflight readout. (B-5b) | §8 |
| S-9 | protocol | **Multi-mesh membership** for one node. (P-1, docs/proposals/multi-mesh.md) | §9 |
| S-10 | protocol + console | **Console through the relay** (P-4), with WSL address detection (P-5). | §10 |
| S-11 | console + protocol | **Pair mode on boards.** Two panes' sessions joined on a bus thread. | §11 |
| ~~S-12~~ v0.15 | console | **Task notifications collapsed.** A `<task-notification>` in the transcript renders as a one-line summary card (agent name, status) with click-to-expand, like tool cards; one spanned dozens of screens. | §12 |

## Next major swath — after S-12: Codex as a second harness (H-1)

| id | tag | ask | design |
|---|---|---|---|
| H-1 | sessions + protocol + console | **Multi-harness: OpenAI Codex.** Codex is open source (clone at `~/src/codex`), so its protocol is read, not reverse-engineered. Before any code: a full design of how Aspen becomes multi-harness and stays coherent — what is cross-harness (bus, mesh, boards, activity, notices, usage, migration, plugins?) and what is per-harness (transcript shape, events, permissions, tools, memory, plugin dirs), what the adapter seam (DESIGN.md §5) really needs, what the product affords per harness, and how nothing regresses for claude. Then methodical implementation. | to be written as docs/PROPOSALS-HARNESSES.md |



| id | tag | ask | design |
|---|---|---|---|
| ~~B-1~~ v0.10 | console | **Composer drafts persist.** Text typed in a session's composer survives navigating to another view and back (a message was lost flipping to another agent's view). | [PROPOSALS-2026-09.md §1](PROPOSALS-2026-09.md#1-composer-drafts) |
| ~~B-2~~ v0.10 | console | **Tool calls: live while running, summary after, click to expand.** Like the claude TUI: a running tool shows its contents; when the agent moves on it collapses to the summary line; any collapsed call can be clicked open. Keep our summary style. | [PROPOSALS-2026-09.md §2](PROPOSALS-2026-09.md#2-tool-calls-live-then-summary) |
| ~~B-3~~ v0.10 | console + protocol | **Open what the agent points at, from any node.** Paths the agent writes in its output (a doc it wrote, a screenshot it took) become links that open in a viewer in the console — images, markdown, text, PDFs — served from the agent's home node over the mesh, direct link or not. | [PROPOSALS-2026-09.md §3](PROPOSALS-2026-09.md#3-artifact-links-over-the-mesh) |
| ~~B-4~~ v0.10 | console + sessions | **Paste attachments inline.** Paste an image or document into the composer (the TUI's alt+v); it rides the message as an attachment with a marker in the text at the point of paste, so the agent sees both the content and where it fell in the conversation. | [PROPOSALS-2026-09.md §4](PROPOSALS-2026-09.md#4-inline-attachments) |
| ~~B-5~~ v0.11 (phases 1–2) | protocol + sessions | **Session migration over the mesh.** Move a session to another node so it resumes there with full context, on a new home, with asymmetric paths between the nodes — the context-convergence idea, done over the mesh protocol rather than through git. Explore what else this implies. | [PROPOSALS-2026-09.md §5](PROPOSALS-2026-09.md#5-session-migration-and-what-it-implies) |
| ~~B-6~~ v0.12 | console | **Boards: layouts of sessions across the estate.** Terminal-style split layouts (1\|2, 1\|2/3, grids) of sessions from any node; several boards; a session in any number of them; boards stored on the node and synced across the mesh; dynamic boards from a query. | [PROPOSALS-2026-09.md §6](PROPOSALS-2026-09.md#6-boards-layouts-of-sessions-across-the-estate-b-6) |
| ~~B-7~~ v0.13 | protocol + sessions + console | **Plugins: a mesh-managed library, activated by scope.** Marketplaces registered once for the mesh; plugins activated for mesh / node / repo / session; sessions spawned with `--plugin-dir` from a per-node versioned cache; timer + on-demand sync; restart nag when a newer version is cached; activation matrix from the plugin and rosters from each scope. | [PROPOSALS-2026-09.md §7](PROPOSALS-2026-09.md#7-plugins-a-mesh-managed-library-activated-by-scope-b-7) |
| ~~B-8~~ v0.14 | sessions + console | **Activity: tasks, monitors, subagents, workflows.** Derive a per-session activity ledger from the event stream; signal counts everywhere; a drawer with details; subagent transcripts viewable live over the mesh. | [PROPOSALS-2026-09.md §8](PROPOSALS-2026-09.md#8-activity-tasks-monitors-subagents-workflows-b-8) |

## Open — earlier

| id | tag | ask | notes |
|---|---|---|---|
| P-1 | protocol | Multi-mesh membership for one node (personal + work). | proposal sketched in conversation 2026-09-05; not written up |
| P-2 | servicing | Release signing (minisign) before `auto` policy is recommended in public. | SERVICING.md §14 |
| P-3 | protocol | Relay preference ordering (the list is ordered; nothing consumes it). | RELAY.md §10 |
| P-4 | protocol | Console-through-relay (the relay routes node↔node only). | RELAY.md §10 |
| B-5b | protocol + sessions | Migration phases 3–4: memory convergence over the bus with 3-way merge; transcript replication (instant move, mesh-wide History); evacuate-a-node; bring-it-here one-click; dialog preflight readout. | [PROPOSALS-2026-09.md §5.4](PROPOSALS-2026-09.md#54-what-it-implies--the-product-ideas) |
| P-5 | protocol | WSL nodes advertise only NAT-internal addresses; a forwarded port needs `aspen config advertise` by hand. Could detect the WSL case and say so in the console. | RELAY.md §9 |
