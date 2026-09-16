# The session bar (v0.31)

What sits above a session's transcript, on the session page and in a
board pane alike. Designed in PROPOSALS-2026-09-G.md; built in
`ui/src/sessionBar.tsx` (the pieces), `ui/src/pages/Session.tsx` (the
bar and its menu, in `SessionView`), `ui/src/sessionCommands.ts` (the
palette registry) and `ui/src/pages/Board.tsx` (what a pane hands in).

## 1. One row

```
page:  ● @arch@plank@lt-bryon-wsl [CLAUDE] "board triage"  ~/src/hub · #12      ctx ▓▓▓▓░░ 62%  opus-5   ⏸ ⎇ ■  ⋯ 2 bg
pane:  1 ● @arch@plank@lt-bryon-wsl [CLAUDE] [needs you] [⇄ @pdt]    ctx 48%  ⎇ ■  ⋯ │ ⇄ ↗ ⤢ ⇅ ⫿ ⫽ ×
phone: ● @arch@plank…                                  5%  ⎇ ■  ⋯
```

Left to right:

- **Presence**, one glyph with a tooltip, and the only copy: ● live
  (idle, connected), ◐ busy (pulsing), ◌ reconnecting (the event socket
  is not open), ○ not running, ◇ read-only (a subagent's transcript).
  `barPresence()` derives it from the agent's liveness, the turn state,
  the socket and the view mode.
- **`@name`** — never truncated; the tooltip carries repo, channel and
  title. The **harness chip**, whose tooltip also names the model the
  latest reply came from.
- On the page: the **title** (click to edit) and **repo · #channel**;
  the repo path gives way first when the row is tight (ellipsis on the
  left, so the tail stays readable). In a pane the board's **chips**
  (needs you, bcast, paired) sit here instead.
- Transient notes (reloaded, model changed, an error) in the middle.
- **Readouts**: the context meter (bar + % on the page; % only in a
  compact pane and on a phone; click for the breakdown) and the **model
  in use** as text (hidden in compact panes and on phones — it is then
  in the ⋯ menu's head and the harness chip's tooltip).
- **Verbs**, icons with tooltips, three slots: ⏸ interrupt (only while
  busy; also `i`), ⎇ branch (the inline label/as form opens in place;
  also `/branch`), ■ stop (confirm in place; also `x`).
- **⋯**, the session menu, with badges for what it folds away: a
  pulsing `N bg` while activities run, `↑` when a plugin update is
  available, `mcp N` in red when servers are down or need auth.
- In a pane, after a hairline: the pane's **layout buttons** (pair ⇄,
  open as a page ↗, zoom ⤢, change contents ⇅, split ⫿ ⫽, close ×). The
  whole bar is the drag handle for swapping panes.

## 2. The menu

Always mounted (its rows own the plugins, MCP and activity panels, and
the status line opens those by signal), hidden when closed. Opens under
the ⋯ button, right-aligned; on a phone it is a bottom sheet. Closes on
Esc, on a click outside it and outside any panel it opened, and when a
row acts. Arrow keys walk the rows; the first row takes focus on open.

- **head**: `@name`, harness · model in use.
- **setup**: model (select), mode (select), render (chat / console /
  source), reload plugins & skills, plugins ▸, mcp ▸.
- **inspect**: charter, history (both open drawers under the bar),
  artifacts ▸, activity ▸.
- **move**: move or copy to another node…, bring here (for a session on
  another node), board ▸ (on the page only — inside a board it is
  meaningless).
- **foot**: a reminder that every row is in the palette.

(v0.31.1: the closed menu is hidden by an explicit `[hidden]` rule —
the menu's own `display: flex` had been beating the attribute, so once
opened it never went away — and the ⋯ button's own mousedown is not an
"outside click", so a second press closes rather than reopening.)

The panels (plugins, MCP, activity, artifacts, boards) are the same
components as before, their trigger buttons restyled as rows. Opening
one hands over: the menu closes and the panel — portaled to the body,
placed from the row's position at the click — stands alone, above
anything else (v0.31.2; before that it opened behind the menu). It
still closes on mouse-leave.

## 3. The registry and the rule

`sessionCommands.ts`: the session in view — the page's, or a board's
focused pane — registers its commands (`{ id, group, label, hint, run }`)
while mounted, and the palette lists them under *Session @name*:
interrupt (while busy), stop, branch, reload, charter, history, move,
and *session menu* (which opens the ⋯).

**Where the next control goes.** The bar has three verb slots and two
readout slots, and they are taken. A new session feature ships as a
menu row in one of the three groups and a `SessionCommand`; it never
touches the bar. Promotion to the bar needs a demotion, and the bar is
for what most operators use more than once per session. When a folded
feature has state worth a glance, it gets a badge on the ⋯ button, not
a button of its own.

## 4. Verified (2026-09-15)

On the rig from the hosted bundle: the page bar with a long repo path
kept the name whole and ellipsized the path; the menu opened with model,
mode and render live, plugins/MCP/artifacts/activity/board panels
opening from their rows; Esc and an outside click closed it; the palette
showed *Session @far@repo1 · charter*; a two-pane board rendered one row
per pane with the pair chip, the context % and the layout buttons after
the hairline; under the phone rules the menu was a full-width bottom
sheet and the bar dropped the chip, title, repo and model text.
