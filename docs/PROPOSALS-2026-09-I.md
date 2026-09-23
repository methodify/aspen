# Proposal: the whole console on a phone

**Status:** audit 2026-09-19 with the approach recommended; Tier 1 built
as v0.35 (§4); Q-5 as v0.39 (the strip of numbered pane chips with the
waiting pip, one pane open, layout editing desktop-only). Reference: CONSOLE_APP.md §4.

## 0. The ask

"There's no way to manage plugins in the mobile UI, which makes me wonder
what else might be unreachable. Audit it, and come up with a good UX
approach to affording the missing functionality."

## 1. The audit

The phone layout (D-10, `ui/src/phone.css`, ≤ 720 px) was built as a
*reading* layout: one column, the rail as a bottom bar of six surfaces,
the status bar reduced. Everything that did not fit was hidden or told
to come back on a wider screen. Walking every route and control:

| surface | on a phone today | verdict |
|---|---|---|
| Now, Flow, Mesh (list), History, Search, Boards list | bottom bar | reachable |
| **Plugins page** (marketplaces, catalog, where-active matrix, session templates) | nav item hidden by CSS; reachable only by URL or the palette | **unreachable** |
| **Usage page** | same | **unreachable** |
| **The palette** | opens on ⌘K / Ctrl+K only; no button anywhere | **unreachable** — and it is the universal command surface every session control was registered into (SESSION_BAR.md §3) |
| Meshes / Connect page | the connection pill in the status bar (scrolls sideways) | reachable, hard to find |
| Bell (notices, push, hooks) | status bar, after a sideways scroll | reachable, hard to find |
| Install, theme | same | same |
| Session ⋯ menu | bottom sheet; the plugins / MCP / activity / artifacts panels open over it, full width | reachable |
| Session verbs (⏸ ⎇ ■) | icons with tooltips; no tooltip on touch | reachable, unlabeled |
| A board | "open one session at a time from Now" | **not usable** |
| Mesh map | "laid out for a wider screen" | usable, dense |
| History timeline detail, map hover detail | `onMouseEnter` only | **unreachable on touch** |
| Plugins where-active matrix | scope column `min-width: 240px`, rows do not wrap | overflows |

So the answer to "what else": the Plugins and Usage pages, the palette,
the boards, and the hover-only details. Everything session-level is
reachable through the ⋯ sheet; the gaps are at the app level.

## 2. The approach

Three principles, then the pieces.

- **Nothing is hidden, some things are folded.** The phone gets fewer
  *primary* slots, not fewer surfaces. Whatever leaves the bottom bar
  goes into a *More* sheet, in the same words as the desktop rail.
- **The palette is a button.** It became the command surface for every
  session control; on a phone it needs a tap target, not a chord. One
  ⌘ button in the status bar, first thing after the mark, opens it; the
  More sheet has it too.
- **Touch gets what hover had.** Anything that reveals on hover reveals
  on tap as well; icon verbs get their words in the ⋯ sheet.

### 2.1 The bottom bar and the More sheet

Bottom bar on a phone: **Now · Flow · Mesh · Search · Boards · More**.
History moves into More (it is a wide timeline; Search and the catch-up
bar carry the daily "what happened"). *More* opens a bottom sheet:

```
More
  H  History
  P  Plugins
  U  Usage
  ⌘  Command palette
  ─────────────
  Meshes · connect            (hosted)
  Notifications & push        (opens the bell's panel)
  Install as app              (when offered)
  Theme: dark ▾
```

Rows are the desktop rail's items in the desktop's words; the sheet is
the same `session-menu` bottom-sheet component the ⋯ menu uses.

### 2.2 Plugins on a phone

The page's four sections already stack. What breaks is the where-active
matrix: scope names are long. The row becomes two lines (kind + scope,
then the controls), the scope column loses its minimum, and the catalog
table scrolls sideways inside a wrapper as the usage table already does.
The plugin panel in the session sheet was reachable and stays as is.

### 2.3 Boards on a phone

A board is side-by-side panes; a phone has one column. The honest phone
rendering is **the panes stacked**, each a full-width session view with
its own bar, one open at a time: a strip of pane chips under the board
head (`1 @arch · 2 @pdt · 3 @impl`, the waiting pip on each), the chosen
pane below it filling the screen. Swapping panes is a tap on a chip;
layout editing (split, pair, close) stays desktop. This is the same
model as a phone mail client: the list is a strip, one item is open.
Tier 2 (§4): it is a real layout, not a CSS rule.

### 2.4 Touch

- History timeline spans and log rows, and map markers: a tap toggles
  the hover state (`onClick` beside `onMouseEnter`), so the detail
  shows on a phone.
- The ⋯ sheet gains a *verbs* group on phones — interrupt (while busy),
  branch, stop — with words; the bar's icons stay for the desktop.
- Every `title` tooltip that carries a meaning the screen does not
  otherwise show is paired with an `aria-label`; the More sheet and the
  ⋯ sheet spell the meanings out.

### 2.5 What stays desktop

The mesh map and board *editing* (splits, pairs). Both say so, and both
have a phone-sized alternative beside them (the mesh list; the stacked
board).

## 3. Slate

| id | ask | tier |
|---|---|---|
| Q-1 | Bottom bar → Now · Flow · Mesh · Search · Boards · More; the More sheet with History, Plugins, Usage, palette, Meshes, notifications, install, theme. | 1 |
| Q-2 | A palette button in the status bar on phones. | 1 |
| Q-3 | Plugins page phone layout: matrix rows wrap, catalog scrolls. | 1 |
| Q-4 | Touch: tap toggles hover details (History, map); verbs group in the ⋯ sheet on phones. | 1 |
| Q-5 | Boards stacked on a phone: pane strip + one open pane. | 2 |

## 4. Decisions

Built as Tier 1 without waiting: Q-1..Q-4 restore reachability and
change nothing on the desktop. Q-5 is the one design choice worth a
look before building — a stacked board versus a swipe-between-panes
board — and the recommendation is the stack with a chip strip (§2.3).
