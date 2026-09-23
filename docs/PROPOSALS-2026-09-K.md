# Proposal: start and stay; a board is a team

**Status:** two operator asks from 2026-09-22, built the same day as
v0.37 (§3). References: BOARDS.md §9, CONSOLE_APP.md, `topology.rs`.

## 1. Start from the Mesh list and stay

The ask: "I go to the Mesh, find a number of agents in repos on disk I
want to start, often in the same repo. I click revive/resume, the view
navigates to that session, and I lose where I was."

Every start path on the page (new session, resume as a new name, revive,
move a name here) ended in `navigate(session)`. That is right for one
session and wrong for assembling several. A dialog per start would be a
question asked every time; the operator's phrase was "if I choose", so
the choice is a setting on the page: **stay here after starting a
session**, a checkbox above the repos, remembered per browser (scoped to
the mesh profile). With it on, every start path stays, reloads the
repo's rows, refreshes the fleet, and a *started:* strip above the list
collects what was started as links, so nothing is lost either way.

## 2. A board is a team

The ask: "Agents together on a board should become aware of each other,
like agents in the same repo are, and it should just work across repos
and nodes."

What exists: an agent's *neighborhood* (topology.rs) is what it is told
about and, in closed topology, all it may address — repo-mates, links,
channels — derived live, written into the charter at spawn and led with
by `bus_status`. Pair mode on a board already declares a link between two
panes. Boards ride the mesh (every node holds every board), and a pane
is a session's full address.

So a board is one more source of neighborhood, and needs no new object:
**every session pane on a board is a board-mate of every other**. It is
derived, not stored — edit the board and the neighborhood follows on the
next `bus_status` or spawn. Concretely:

- `Neighborhood.boards`: (board name, the other panes' addresses as the
  bus writes them from this node). Dynamic boards (a fleet query) do not
  count: their membership is a moment's view.
- The charter and `bus_status` gain a line per board: *On the operator's
  board "dw" with @arch, @pdt@lt-bryon-win — the operator works with
  you together there; they are your team for it.*
- `bus_status` stars board-mates as in-neighborhood; closed topology
  admits them; **bare names resolve across a board** ("@pdt" from a
  plank agent reaches pdt on the board even from another repo), the same
  rule as links.

Pane addresses come from the console (`bare@repo@node`, the node by
hostname when there is no mesh) and the node knows itself by mesh name
only, so a pane counts as local when its key is a local agent and its
node is this node's mesh name or there is no mesh.

### Not built: telling a running agent when the board changes

A spawn reads the charter and an idle agent learns on its next
`bus_status`, but a running agent is not told when it is put on a board
or a teammate arrives. A notice on the bus would do it, and since v0.34
every message lands, which means a turn for each agent on every board
edit. Left for the operator's call: worth a turn, or leave it to pull?

## 3. Slate

| id | ask | shipped |
|---|---|---|
| K-1 | Stay on the Mesh page after a start; the started strip. | v0.37 |
| K-2 | Board-mates in the neighborhood: charter, `bus_status`, closed topology, bare-name resolution. | v0.37 |
| K-3 | A bus notice to the agents on a board when its membership changes. | v0.39 (the operator: worth the turn) |
