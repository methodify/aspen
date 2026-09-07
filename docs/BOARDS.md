# Boards: layouts of sessions across the estate

**Status:** design reference for what is built (2026-09-07, v0.12).
Proposal: PROPOSALS-2026-09.md §6. Code: `ui/src/pages/Board.tsx`
(page, split renderer, picker, boards list), the pane mode of
`SessionView` in `ui/src/pages/Session.tsx`, the `boards` table and sync
in `crates/aspen-node/src/store.rs` and `federation.rs`, routes in
`crates/aspen/src/api.rs`.

## 1. The object

A board is a named **split tree**: a node is a split (row or column,
with percentage sizes) or a pane. Panes are references — a session by
its full address (`main@hub@anindor-wsl`), an artifact viewer (agent +
path), or empty. A session may sit in any number of boards; each pane
mounts its own session view with its own event stream, so two panes of
one session are two independent views of one transcript.

```
Board { id, name, layout: Node, query?: DynamicQuery, updated_at, deleted }
Node  = { kind:"split", id, dir:"row"|"col", sizes:[%], children:[Node] }
      | { kind:"session", id, agent }
      | { kind:"view",    id, agent, path }
      | { kind:"empty",   id }
```

**Boards ride the mesh.** They are stored on the node (`boards` table;
deletes are tombstones with a bumped `updated_at`). Every roster carries
`boards_digest`, a hash over (id, updated_at) of all rows including
tombstones; a peer whose digest differs is asked for its boards (`boards`
op) and merged **last writer wins per id**. A change on any node reaches
every node within a roster tick, and every console shows one set of
desks. `PUT /api/boards/{id}` and `DELETE` on the console's node are the
only writes; a delete propagates as a tombstone.

## 2. Editing

- **Presets** on the boards page: `1`, `1 | 2`, `1 / 2`, `1 | 2/3`,
  `2 × 2`, `main + stack`. A new board's panes are empty; each offers
  *choose a session…* (a picker over the fleet, remote nodes included,
  with state dots and *needs you* chips) or an artifact viewer by agent
  and path. Sessions also drop onto a pane.
- **Dividers** drag (pointer events, 10% minimum per side); double-click
  equalizes. **Split right / split down**, **close** (a split left with
  one child collapses), **swap** by dragging one pane's bar onto
  another, **change** what a pane shows (⋯), **zoom** a pane to fill the
  board and back (`alt+z`).
- **Add to board** from any session page: *board ▾* lists the ordinary
  boards; *add* fills the first empty pane or splits right; ⫿ / ⫽ split
  the whole board right or down.
- The name is edited in place; **export** copies the board's JSON; the
  boards page **imports** JSON into a new id.
- Below 900px the board becomes **tabs** across the top, one pane
  visible; the layout is untouched.

## 3. Working in a board

- **Focus.** One pane has focus (its border says so); its composer holds
  the keyboard. Click, `alt+1…9` by pane order, or `alt+arrows` by
  screen position. The session view's own hotkeys are inactive on the
  board route; the board's live in the `board` scope and appear in the
  palette. `alt` was chosen over `ctrl`+digit, which browsers own.
- **Compact panes.** Vertical space is the scarce thing in a pane, so
  the session chrome collapses to two slim rows: the pane bar carries
  identity and state (the session header keeps only reload/branch/stop),
  and one scrolling control row holds model, mode, render mode and the
  charter/history/artifacts/move/board menus. Zoom or open as a page
  (↗) for the full chrome.
- **Cached transcripts.** A session view keeps its transcript state
  across mounts (zoom, navigation, the same session in two boards). On
  return it shows the cached state at once and fetches only the items
  after its last user line (`GET …/transcript?after=<uuid>`, also over
  the mesh); if that line is gone it refetches everything. The cache
  lives in memory for the page and in IndexedDB across page loads
  (saved at every turn end, on unmount, and on `pagehide`; ≤8 MB per
  agent), so a reload also asks only for the tail. No more "no
  transcript yet" flash and no full re-transfer per zoom or reload.
- **Hotkeys**: `b` goes to Boards, `[` collapses or expands the rail —
  both registered in the global scope, so the `?` help lists them from
  the registry like everything else. The narrow rail shows sessions as
  two-letter glyphs colored by presence.
- **The rail collapses** («/» at its top) to a strip of keys and dots,
  remembered per browser.
- **Attention.** The board polls open prompts (the needs endpoint) every
  3 s; a pane whose agent is waiting gets a red border and a *needs you*
  chip; `alt+.` cycles focus through them.
- **Broadcast.** A header toggle, confirmed inline the first time on a
  board: after the focused pane sends, the same text goes to every other
  session pane (`sendMessage` per pane, best effort). Every session pane
  shows a *bcast* chip while it is on. Off by default.
- **Drafts** are per pane (`aspen.draft.<agent>.<board>:<pane>`), so two
  panes of one session keep separate drafts.
- **Viewer panes** are the artifact viewer embedded: a report or log the
  agent keeps writing, beside the session writing it.

## 4. Dynamic boards

A board with a `query` computes its panes from the fleet on every roster
poll: `state` (busy / live / needs attention / any), `node`, `channel`,
`name contains`, laid out as a grid by count or main + stack (with the
panes needing attention first). Panes come and go with the fleet;
editing is locked. **pin** freezes the current membership into an
ordinary board. Built-in uses: everything busy now; one repo across
nodes; needs attention (empty when all is well); everything on a node.

## 5. A fix that fell out

Testing copy-then-board exposed that a session **forked** (branch, copy)
holds its parent's id until the runtime announces its own on the first
turn; a daemon restart in that window revived it in place on the
**parent's** transcript — two writers. Rows now carry `fork_pending`,
set at a forking spawn and cleared when the runtime announces the new
id; a revive while it is set forks again. Verified: copy, restart before
any turn, first turn → its own id and file.

## 6. Verified (rig, 2026-09-07)

A `1 | 2` board on j2 filled with two j1 sessions through the picker;
both live and streaming; focus and per-pane composers; the board synced
to j1 and r within a roster tick; broadcast from pane 2 reached both
agents (each answered with its own name); inline confirmations (no
browser dialogs). Dynamic boards, zoom, tabs and drag were exercised by
hand.

## 7. Not built

Pane-to-pane text drag; pair mode (two panes' sessions on a bus thread);
per-repo default boards; capturing per-pane render mode in the layout;
a Flow pane filtered to the board's agents (the artifact viewer and
session panes are the two kinds).
