# Backlog

Feature requests and design candidates, tagged and dated. Each entry names
the ask as the operator put it, the swath it belongs to, and where its
design lives once one exists. Shipped items move to the decisions log in
DESIGN.md §14b with the tag they carried here.

Tags: `console` (the web UI), `protocol` (mesh/bus/wire), `sessions`
(the claude process and its transcript), `servicing`, `docs`.

## Shipped — v0.10 (2026-09-06)

B-1 composer drafts, B-2 tool calls live-then-summary, B-3 artifact links
over the mesh, B-4 inline attachments — see DESIGN.md §14b, 2026-09-06.

## Open — 2026-09-06 feature dump

| id | tag | ask | design |
|---|---|---|---|
| ~~B-1~~ v0.10 | console | **Composer drafts persist.** Text typed in a session's composer survives navigating to another view and back (a message was lost flipping to another agent's view). | [PROPOSALS-2026-09.md §1](PROPOSALS-2026-09.md#1-composer-drafts) |
| ~~B-2~~ v0.10 | console | **Tool calls: live while running, summary after, click to expand.** Like the claude TUI: a running tool shows its contents; when the agent moves on it collapses to the summary line; any collapsed call can be clicked open. Keep our summary style. | [PROPOSALS-2026-09.md §2](PROPOSALS-2026-09.md#2-tool-calls-live-then-summary) |
| ~~B-3~~ v0.10 | console + protocol | **Open what the agent points at, from any node.** Paths the agent writes in its output (a doc it wrote, a screenshot it took) become links that open in a viewer in the console — images, markdown, text, PDFs — served from the agent's home node over the mesh, direct link or not. | [PROPOSALS-2026-09.md §3](PROPOSALS-2026-09.md#3-artifact-links-over-the-mesh) |
| ~~B-4~~ v0.10 | console + sessions | **Paste attachments inline.** Paste an image or document into the composer (the TUI's alt+v); it rides the message as an attachment with a marker in the text at the point of paste, so the agent sees both the content and where it fell in the conversation. | [PROPOSALS-2026-09.md §4](PROPOSALS-2026-09.md#4-inline-attachments) |
| **B-5** | protocol + sessions | **Session migration over the mesh.** Move a session to another node so it resumes there with full context, on a new home, with asymmetric paths between the nodes — the context-convergence idea, done over the mesh protocol rather than through git. Explore what else this implies. | [PROPOSALS-2026-09.md §5](PROPOSALS-2026-09.md#5-session-migration-and-what-it-implies) |

## Open — earlier

| id | tag | ask | notes |
|---|---|---|---|
| P-1 | protocol | Multi-mesh membership for one node (personal + work). | proposal sketched in conversation 2026-09-05; not written up |
| P-2 | servicing | Release signing (minisign) before `auto` policy is recommended in public. | SERVICING.md §14 |
| P-3 | protocol | Relay preference ordering (the list is ordered; nothing consumes it). | RELAY.md §10 |
| P-4 | protocol | Console-through-relay (the relay routes node↔node only). | RELAY.md §10 |
| P-5 | protocol | WSL nodes advertise only NAT-internal addresses; a forwarded port needs `aspen config advertise` by hand. Could detect the WSL case and say so in the console. | RELAY.md §9 |
