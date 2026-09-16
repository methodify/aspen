# Proposal: delivery that always lands, and is always on the record

**Status:** discussed 2026-09-16; §4 rewritten to the operator's
direction (§8); shipped as v0.34 (B-1..B-4).

## 0. The report

1. Agent A sends agent B a message while B is idle. B does not wake;
   nothing happens until the operator next writes to B.
2. When the operator does, A's message arrives at that point too.
3. The operator's own message, though the agent acknowledges it, is
   missing from the console's chat history afterwards.

Plus: bus messages sometimes render as the blue `[aspen bus]` bubble and
sometimes as plain text in the agent's own colour; they are injected as
user-typed content, indistinguishable in the transcript from the
operator; and received messages should collapse by default.

## 1. What the design says, and why

DESIGN.md §4.2 (inherited from plumb, restated in `delivery.rs`):

| recipient | `normal` | `gating` |
|---|---|---|
| idle | write now → wakes | write now |
| busy | write now — "the CLI queues and coalesces it into the **next turn**, which IS boundary delivery, for free" | interrupt, then write |

And CLAUDE_RUNTIME_REFERENCE.md §5.1: *"Mid-turn sends are queued and
COALESCED by the CLI: three messages sent during a running turn reach the
model as one merged turn … Don't build your own send queue — send
immediately, always."* A client-side queue was deleted on the strength
of that observation.

The reasoning was sound as far as it went: we own the pipe, the CLI
queues stdin while a turn runs, so a mid-turn write costs nothing and
arrives at the boundary. Two things were assumed that turn out false.

## 2. What actually happens (rig, 2026-09-16, claude 2.1.265)

Three experiments, each against a live Claude session on the rig, the
raw JSONL read afterwards.

**Idle recipient, `normal`, same node and cross-node:** wakes within a
second, a user line with the `[aspen bus]` envelope in the transcript,
row marked `wake`. The engine is right here.

**Busy recipient (a 25 s tool call), one operator message and one
`normal` bus message written mid-turn:** both were consumed *inside the
running turn* — the CLI appends queued input to its **next model
request**, whichever that is — and the model answered them in the same
reply ("done · acknowledged · also received a bus message from
fresh1 …"). The turn ended; the session went idle; **neither message
was written to the JSONL as a user line.** The bus row was marked
delivered `boundary`. The console, rehydrating from the transcript,
shows neither: the operator's text survives only where the model quoted
it, in the assistant's own colour. That is items 2 and 3 and the first
rendering nit in one.

**The corollary that gives item 1:** "appended to the next model
request" means that when a write lands after the turn's *last* model
call — during the final reply, or a tail tool result — there is no next
request in that turn. The CLI holds the input until the next turn,
which only the next stdin write starts. Nothing wakes the recipient;
the message rides along with whatever the operator sends next, and the
operator's own message shares that fate. The reference's "reach the
model as one merged turn" was observed when a later model call happened
to follow; it is not a boundary guarantee.

So: the CLI's queue is *mid-turn context injection*, not boundary
delivery. Fine for steering the model between tool calls; wrong as the
mechanism that carries agent-to-agent mail, and invisible to the
record either way.

## 3. What we want (the operator's expectation, made precise)

- **A message to a running agent always produces a turn** — now when
  idle; when busy, at the next point the recipient's harness takes
  input, and never later than the end of the turn. Not optional, not
  urgency-dependent.
- **Everything that reaches the model is on the record**, so the
  console, *catch up*, search and the trail all see it. No ghost inputs.
- **Chronology.** Mail and the operator's messages land in the order
  they were sent, and show that way.

## 4. The change (as decided): every message enqueues like the operator's

The first draft here held busy-time messages on the node until the
boundary. The operator's objection stands, and it is the plumb scar:
the *sender* choosing timing selects for politeness — an agent an hour
into a task that needs an answer at minute five gets it at minute sixty,
after compaction. The recipient is the one who knows. So:

- **Every message is written now**, idle or busy, exactly as an
  operator's message is. Idle: it wakes the recipient. Busy: the harness
  appends it to its next model request, the recipient sees it at the
  next sensible point in its own work and decides — act now, or park it.
- **Urgency is advisory.** `gating` no longer interrupts, `notice` is no
  longer withheld; the word rides in the envelope for the recipient.
  Interrupt stays the operator's verb.
- **Chronology, not rank.** Pending mail and the operator's message land
  in the order they were sent; nothing is reordered for the sender's
  sake, because the operator is usually replying to what an agent said.
- **The record is the node's.** Every write to a session — operator,
  bus, boundary — is stored (`inputs`: text, uuid, time, mid-turn, ack).
  Reading a transcript merges in the writes the harness consumed without
  a line, at their time, marked *delivered mid-turn*; a line the harness
  merged from several writes is split into its segments (`[aspen bus
  end]` closes each message, which is what makes the split exact).
- **The boundary guard.** The replay ack marks consumption, so at every
  turn end the node knows what the harness still holds. The harness
  opens that turn itself in every case measured; if it has not within
  four seconds and the session is idle, a carrier line starts one and
  the held text rides along. A message to a running agent always
  produces a turn — the operator's requirement, without a queue of our
  own.

Codex: `send_user` steers a running turn through the app-server; its
transcript store is Codex's own. The record and the split apply; the
guard is Claude-only (it is Claude's ack).

## 5. Rendering: unmistakable, and collapsed

The protocol offers one input type — `user` — so the envelope stays the
structural marker; there is no system-message lane in stream-json.
What we can do:

- **Every delivered message is its own user line** (§4), so the
  classification `starts with [aspen bus]` holds for every real
  delivery. The model quoting a message in its reply is the model's
  prose and stays in its colour — after §4 the underlying message is
  also on screen as a bus bubble, so the quote is no longer the only
  trace.
- **Mixed lines are split.** A user line that contains an envelope
  header after other text (a coalesced write from before this change,
  or a future one) renders as its segments: operator text as a user
  bubble, each `[aspen bus] …` block as a bus bubble. The node's parser
  and the console's both.
- **Bus bubbles collapse by default**: one row — the envelope's sender
  and channel, then the body's first line, ellipsized to the width — a
  click expands; expanded state per item, not persisted. The bar's
  activity badge already tells about background work; this is the same
  gesture for mail. `gating` messages open expanded.
- **The envelope closes**: `[aspen bus] normal from @arch · #proj`
  stays as the first line (models and humans read it), and each message
  ends with `[aspen bus end]`, so a merged line splits exactly. Nothing
  else changes on the wire.

## 6. Slate

| id | ask |
|---|---|
| B-1 | Every message writes now; urgency advisory; chronology; the reference corrected (`delivery.rs`). |
| B-2 | The input record (`inputs`), the merged transcript view, the boundary guard (`node.rs`). |
| B-3 | Split merged lines into segments, `[aspen bus end]` (node + console). |
| B-4 | Bus bubbles collapsed by default: sender + first line; gating open. |
| B-5 | Codex on the rig — later. |

## 7. Questions

1. §4 trades mid-turn steering for the record. Default to the recorded
   path, with "inject now" as an explicit per-send option later if
   needed? (Recommended: yes; interrupt covers urgent steering today.)
2. Should the operator's held message wait behind pending mail at the
   boundary (mail first, then the operator, as separate turns) or go
   first? (Recommended: mail first — the operator is usually reacting
   to it — and both as their own turns.)
3. Collapsed preview: sender + first line, or first line only?
   (Recommended: `@arch · #proj — first words of the body…`.)

## 8. Decisions (2026-09-16)

1. Messages between agents enqueue exactly like the operator's while the
   recipient is mid-turn, and deliver like the operator's when it is
   idle. No sender-chosen timing; urgency advisory. (The operator; the
   plumb lesson.)
2. Chronological order always; the operator's message is not put ahead
   of mail.
3. Collapsed preview: sender and the body's first line.
