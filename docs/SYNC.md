# Mesh-wide state: how it converges, and whether it should be a CRDT

**Status:** reference for what is built (2026-09-07), with the automerge
question answered. Code: the `*_digest` functions in `store.rs`, the
roster fields and `sync_*_from` in `federation.rs`.

## 1. What is mesh-wide today

Three small tables are meant to read the same on every node: **boards**
(v0.12), **marketplaces** and **plugin rules** (v0.13). Everything else
is either per node by nature (sessions, transcripts, caches, checkouts)
or already flows over the bus with its own at-least-once rows.

## 2. The mechanism

Each table is a set of rows with `id`, `updated_at`, and a `deleted`
tombstone. A node's roster — sent to every peer every 10 s and on change
— carries a **digest** per table: a hash over `(id, updated_at)` of all
rows, tombstones included. A peer whose digest differs asks for the full
table (`boards` / `plugin_registry` ops), merges **last writer wins per
row** (a row is taken when its `updated_at` is newer), and, if anything
changed, re-broadcasts its roster so the next peer pulls from it. Deletes
are tombstones with a bumped `updated_at`, so they win and propagate;
tombstones are never purged (the tables are tiny). Writes happen on
whichever node the console is served from; nothing is coordinated.

Properties: convergent (every node ends with the max-`updated_at` row
per id), partition-tolerant (a rejoining node pulls what it missed the
moment it sees a differing digest), one roster tick of latency, no
extra connections, and the sync payload is the whole table — fine while
tables are hundreds of rows, not tens of thousands.

Weaknesses, honestly:
- **Wall clocks decide.** `updated_at` is the writing node's clock. Two
  nodes editing the same board within their clock skew resolve by whose
  clock is ahead, not by who wrote last. Skew between the three nodes
  here is small, but nothing enforces it.
- **Row granularity.** Two nodes changing different panes of one board
  concurrently: one edit is lost whole. For a single operator moving
  between consoles this is rare; it is not impossible (a board open on
  two screens).
- **Delete vs edit** races resolve by the same clock rule; an edit after
  a delete resurrects the row if its clock is later, which is arguably
  right.

## 3. Automerge?

No — not for these tables, and not yet for anything.

A CRDT such as automerge buys field-level merging (two edits to different
panes both survive), causal ordering instead of wall clocks, and
offline convergence with proofs. It costs a dependency in both Rust and
the console (the document has to be the same on both ends), a document
that grows with history unless compacted, a binary sync protocol
alongside our JSON frames, and a second source of truth beside SQLite
for the same rows. For three tables of a few hundred rows written by one
person, that trade is wrong: the failure modes above are rare and
recoverable (edit again), and the current scheme is fifty lines a table.

What *is* worth doing, cheaply, is fixing the clock: replace wall-clock
`updated_at` with a **hybrid logical clock** — a per-node counter folded
into the timestamp and advanced past any timestamp received — so "last
writer" means causally last, not whose clock runs fast. Same tables,
same digests, no new dependency. That is the next step if a skew
problem ever shows up.

Where a CRDT would earn its place is **memory convergence** (PROPOSALS
§5.4, backlog B-5b): project memory files edited on several nodes need
*text* merging, and automerge's text type does exactly that with a
common base it manages itself, which is what our import lacks today
(it writes the incoming copy alongside and reports a conflict). If
memory convergence is built, that is where automerge — or a
line-oriented 3-way merge with a stored base, the cheaper half-step —
should be evaluated first.

## 4. Rules of thumb for new mesh-wide state

1. Model it as rows with `id`, `updated_at`, `deleted`.
2. Add a digest to the roster and a pull op; merge newest-wins per row.
3. Cascade: a delete removes what hangs off the row on every node (a
   marketplace removal tombstones its rules and clears the local catalog,
   checkout, and unused cache — v0.14.1).
4. Keep the table small; if it will not stay small, page the pull op.
5. Reach for a CRDT only when the value is text people edit
   concurrently.
