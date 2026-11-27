# Proposal 2026-11-D: The operator's mail

**Status:** accepted 2026-11-27 (the operator asked for the slate: design and build); built for v0.57.0. As built: an away node keeps its last known mail and is named (found on the rig: it vanished at first).

## 1. What happened

A project with many workflows agreed a way of working: workflow lanes write
to the operator when they need something (a ticket on disk, a message on
the bus), no reply expected; the operator answers the main agent later, at
leisure. It is the first heavy use of agent → operator mail, and it showed
that the mail is not a place yet:

| What the operator saw | Why (read in the code and this node's store) |
|---|---|
| Messages appear on Now "after a while", and it is never clear whether nothing came or something is still on its way. | Now's list is `/api/needs`, which asks every linked peer **one after another**, each with up to 5 s; one slow peer holds the whole list, on every 2–3 s poll. Nothing says when the mesh was last asked, or who did not answer. |
| Messages it meant to come back to are gone, with nowhere to find them. | Answering an agent in its chat marks **every** waiting message from that agent read (`operator-replied`): on 2026-11-27 18:10 one reply took eight open ticket messages off Now at once. Read mail is shown nowhere. |
| Flow's Direct conversations show the operator's messages to the agent, not the agent's messages to the operator. | Operator mail is stored on the **sender's** node. Flow reads only the console's node (`anindor`); the 46 messages from `@main@cyberdeck` are on `anindor-wsl`. |
| *Dismiss* takes 3–5 s to register. | It waits for the node (and the peer) to answer, then for the next slow `/needs`. |

## 2. The design

### D-1 Mail is kept, and read is a state

- Agent → operator messages are the operator's **mail**. Each is unread or
  read; nothing ever removes it. Read is only ever the operator's act: a card
  dismissed, *mark read*, a reply sent to that message.
- Answering an agent in its own chat **no longer** marks its mail read.
- *Mark unread* puts a message back.

### D-2 One mailbox, mesh-wide and quick

- `GET /api/operator/mail` gathers the mail of this node and every linked
  peer **at once** (in parallel), each peer's answer **kept** for a short
  while: a call returns at once with what is known and refreshes in the
  background, so a slow peer never holds the page. It says, per node, when
  it last answered or that it did not.
- `POST /api/operator/mail/read {items: [{node, id}], read}` marks read or
  unread on each message's own node; the console removes or restores the
  card at once (it does not wait).
- Now's *Needs you* takes the unread mail from it, with a line saying when
  the mesh was last asked and which nodes did not answer.

### D-3 A place for it: Flow → Inbox

- Flow's rail starts with **Inbox**: every message to the operator, newest
  first, from every node; unread first-class (a count in the rail and on the
  nav item), read kept below.
- Filter by agent and by state (unread, all), search the text; each message
  shows its sender, node, project, time and urgency.
- On each: *reply* (to that agent, on the bus, and it marks that message
  read), *open the session*, *mark read / unread*.
- *Mark all read* for what the filter shows.

### D-4 Direct conversations across the mesh

- Flow's **Direct** list and threads gather their messages from every
  linked node, so the operator's conversation with an agent is whole: what
  it sent (stored on its node) and what the operator sent. Messages are
  merged by uuid; each carries its node.

### D-5 Mail tells you it came

- A message to the operator already raises a notice (the bell). The nav's
  Flow item and the Inbox carry the unread count, from D-2.

## 3. Order

One release (v0.57): D-1 … D-5.
