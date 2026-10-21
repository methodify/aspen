# Proposal T — The repo at hand, the bench, and boards as tabs

**Status:** accepted 2026-10-20; shipped as v0.51.0 (BOARDS.md §11, SESSION_BAR.md §8). Not built: dropping a bench chip on a pane *edge* to split there (a drop swaps). The repo menu opens with `alt+r` (the hotkeys have no two-key `g r`). The operator's calls: the bench is a
strip along the bottom; it syncs with the board; the tabs choice is
remembered per board on each console, the last choice the default; ×
asks "bench or remove from team"; no dormancy offer (the operator benches
as they see fit). Three pieces of one idea:
a board is a project's **roster**, not a screen layout, and the repo a
session lives in is a **place you can look around**. (S is kept for the
Antigravity runtime proposal.)

---

## 1. What prompted it

- **The repo chip is inert.** The session bar shows `C:\…\next · #next`.
  To reach a sibling agent in the same repo, the operator goes to Mesh,
  finds the node, expands the repo, and picks the row. The bar already
  knows the repo; it should open it.
- **Boards accumulate.** A board ends up holding everyone who ever worked
  on a project. Some go quiet for weeks. They are still the team (they
  should stay board-mates, keep their place, come back easily), but they
  take up a pane each. The only verb today is *close*, which removes
  them from the team and tells everyone so.
- **The phone layout is good on a desk too.** On a phone a board is tabs:
  one agent, full size, a strip to switch. On a desktop with a busy board,
  that is often what the operator wants: one conversation at full width,
  the rest a keystroke away. Today it happens only below 900px.

## 2. T-1 The repo menu

The repo segment of the session bar (`C:\…\next · #next`) becomes a
button. It opens a popover (the `.session-menu` recipe, like ⋯) anchored
under it. This is not a new control on the bar: the text that is already
there becomes the trigger, and the menu holds the verbs.

```
 #next · C:\LocalData\projects\next · anindor            3 agents · 2 live
 ─────────────────────────────────────────────────────────────────────────
 ● @main          current · this one                                  ·
 ● @arch          busy · "spec the export path"      2m      open  ⋯
 ○ @tuning        stopped · "round 3 listening"      3d    revive  ⋯
 ─────────────────────────────────────────────────────────────────────────
 new agent here   @ [name________]  [claude ▾]  start
 all transcripts (Mesh) →     files →     export…
```

- **Rows** are the repo's named agents on this node: live ones first
  (busy, then idle), then stopped ones, most recent first. Each row has a
  state dot, the name, a waiting pip if it waits on the operator, the
  title, and when it last worked.
- **The primary verb follows the state**, as in the Mesh list (`SessionRow`
  verbs, `sessionRows.tsx`): *open* for a live agent, *revive* for a
  stopped one. ⋯ holds *preview*, *resume as…*, and *rename*. The
  current agent's row says "this one" and has no verb.
- **Inside a board pane**, two more verbs make the menu a way to rearrange
  a team without leaving the board:
  - *show here* puts that agent in this pane. The current one goes to the
    bench (T-2), so nobody leaves the team by accident.
  - *add to board* splits this pane and puts the agent in the new half.
- **New agent here** uses the name and runtime fields of the Mesh list's
  *new session* (`trust.start`, the trust gate included). It opens the new
  agent: in a pane, in that pane, with the current one benched; on a
  page, as a page.
- **Elsewhere.** The same handle can exist on other nodes (a repo cloned
  on two machines). Those agents are listed under a folded *on other
  nodes* group, with node chips.
- **Footer links:** the repo's transcripts in Mesh (all of them,
  including unnamed ones), its Files page (Proposal R), and export.
- **Keyboard:** `g r` opens it; a palette entry is "agents in this repo".
  Arrow keys move, `enter` runs the row's primary verb.

## 3. T-2 The bench

A board gets a second place for members: the **stage** (the panes, as
today) and the **bench** (members not shown right now). Both are the
team.

### 3.1 The model

```
Board { …, layout, bench?: [{ agent, at, slot? }] }
slot = { parent: <split id>, index, size }   // where it sat, to go back
```

- **Members are stage ∪ bench.** `topology::board_panes` (board-mates,
  bare-name resolution, closed topology) and the K-3 membership notices
  read both. **Benching or unbenching sends nothing**: the team did not
  change.
- `bench` is a top-level board field, so it syncs with the board
  (last writer wins per board, as today). The arrangement of a team's
  roster is the team's, the same on every console and on the phone.
- A node before this version drops the unknown field when it rewrites a
  board (its `Board` struct is typed). Edits made there would lose the
  bench. Every node on the mesh updates together in practice; the release
  notes say so.
- **Dynamic boards have no bench**: their members are a query.

### 3.2 On the board

- **Bench a pane:** a new pane-bar verb, ⤓ *bench*, among the layout
  buttons (pair, open, zoom, change, split, close). The pane collapses as
  if closed. The slot it held is remembered.
- **Close asks now.** Today × removes a member from the team and notifies
  everyone. It becomes an inline choice: *bench @x* (the default) or
  *remove from team*. Empty and viewer panes close at once, as now.
- **The bench strip** is a slim row along the bottom of the board, above
  any pair strips. It reads `bench · @tuning 3d · @design 2w ● · @ops 5w`.
  Each chip shows:
  - the state dot;
  - how long since the member last worked;
  - a red pip if it waits on the operator, a green one if it runs
    background work.

  Members that wait on the operator sort first, then by recency. The strip
  folds to `bench (5)` when the operator wants the room.
- **Bring back:**
  - click a chip, then *bring back*: it returns to its slot if that split
    still exists; otherwise it splits the largest pane;
  - drag a chip onto a pane: they swap (that pane's member goes to the
    bench);
  - drag it onto a pane edge: it splits there.
- **Peek** (the part this proposal reaches for): click a chip to open
  the member in a **side sheet** over the right part of the board. It is
  the full pane view: transcript, composer, and permission cards. Answer
  it, glance, and close it (`esc`); the layout never moves. The sheet
  carries *bring back* for when a glance turns into work. Most of the
  time a dormant member needs one answer, not a pane.
- **All benched.** A stage with no panes shows the bench large, with
  *bring back* on each member, instead of an empty board.
- **The meter and counts** cover the whole team. The header adds
  `· 3 benched`. *Start the stopped* starts stopped stage members only;
  benched ones are dormant by choice.

## 4. T-3 Boards as tabs, on any screen

A board has two views: **layout** (the split tree, as today) and **tabs**
(one member at full size, a strip to switch). The phone's narrow mode is
the tabs view.

- **The switch** is a two-way toggle in the board header:
  `layout | tabs`.
- **It is remembered per board, on this console** (scoped per mesh
  profile, in browser storage). A board never set takes the console's
  last choice. It is not stored with the board: the same board can be
  tabs on the laptop and a 2×2 on the big monitor.
- **The strip** lists stage members in layout order, then a hairline,
  then benched members as dimmer tabs. In tabs view, choosing a benched
  member simply shows it, so the view needs no peek. Each tab shows:
  - the number (`alt+1…9`) and `@name`;
  - the state dot, with a busy member's dot moving;
  - the waiting pip;
  - an **unread dot** when the member has written since the operator
    last looked at its tab.
- **Keys:** `alt+1…9`, `alt+[` and `alt+]` for previous and next, and
  `alt+.` for the next one waiting, as today.
- **Switching is instant.** The last three tabs stay mounted (hidden), so
  streams stay live and scroll positions hold. Older ones unmount and come
  back from the transcript cache, which already serves zoom and
  navigation.
- **One component.** The phone strip (Q-5) and the narrow strip in
  `Board.tsx` become this component. Below 900px a board is always tabs;
  above, it is the operator's choice.
- **Pairs and broadcast** work as in layout view. The pair strip sits
  under the active tab.

### 4.1 The unread mark

Per member, per console: the time of the last transcript item the
operator saw (when its tab or pane was visible and the window focused),
against the member's last activity from the agents list. It is the same
signal the rail would use for "new since you looked". The bench chips
show it too, so a dormant member that wrote something is visible without
opening it.

## 5. Built on what exists

| Piece | Reuses |
|---|---|
| Repo menu rows and verbs | `SessionRow` and its verbs (`sessionRows.tsx`), `api.sessions(repo, node)`, the Mesh list's *new session* (`trust.start`) |
| Popover | the `.session-menu` recipe (the ⋯ menu), `panel-in` motion |
| Bench membership | `topology::board_panes` reads `bench` beside the layout; K-3 notices compare member sets, which do not change |
| Bench strip and tabs | the narrow `board-tabs` strip, the rail's waiting and background pips, `boardStatus.ts` for state |
| Peek | the session view's pane mode (`SessionView`), as any pane mounts it |
| Instant tabs | the transcript cache (memory and IndexedDB) |
| Remembered view | `scoped()` browser storage, like drafts and the rail width |

New: the `bench` field (node struct and console type), the bench strip,
the side sheet, the tabs toggle, and the unread mark.

## 6. Order and size

| Item | What | Size |
|---|---|---|
| T-1 | Repo menu: rows, verbs, new agent, board verbs, `g r` | M |
| T-2 | Bench: field, topology, pane verb, close choice, strip, bring back, drag, peek | L |
| T-3 | Tabs view: toggle, remembered choice, one strip component, mounted tabs, unread mark | M |

Proposed: one slate, one release (v0.51), built in that order, verified
on the rig and in the browser (both themes, phone width).

## 7. Questions for the operator (answered 2026-10-20)

1. Bench along the bottom: **yes**.
2. Bench synced with the board: **yes, absolutely**.
3. Tabs remembered per board, the last choice the default: **yes**.
4. Close asks "bench or remove from team": **yes**.
5. Dormancy offer: **dropped**; the operator manages the bench as they see fit.
