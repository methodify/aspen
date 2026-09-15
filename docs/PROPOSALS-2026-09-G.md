# Proposal: the top of a session — one row, one menu

**Status:** approved 2026-09-15 (§6); shipped as v0.31 (U-1..U-4).
Reference: SESSION_BAR.md.

## 0. The ask

"There's room to tighten the space atop chat windows. In a board pane
there are three rows: the pane bar, the session head ([CLAUDE] … reload
branch stop ● LIVE), and the controls strip (model, mode, charter,
history, artifacts, move, bring here, board, plugins, mcp…). The middle
row can fold into the top; its pip and LIVE repeat the pane bar's
indicator; LIVE/RECONNECTING could be icons. The regular session page
should follow suit so the two feel the same. The board pane is missing
the context indicator. And the dropdown buttons keep piling up — find a
way to keep the functionality, knowing we will keep adding, while giving
the space back to the agent/operator conversation."

## 1. What is above the transcript today

| item | row | kind | how often |
|---|---|---|---|
| pane index, @name, presence dot, needs-you / bg / bcast / pair chips | pane bar | state | always read |
| pair ⇄, open ↗, zoom ⤢, change ⋯, split ⫿ ⫽, close × | pane bar | layout actions | rare |
| Meter (presence bars) | head | state | duplicates the pane dot |
| @name, harness chip, title, repo · #channel | head | state | always read |
| reload | head | action | rare |
| branch (+ inline form) | head | action | per session; also `/branch` |
| stop (+ confirm) | head | action | per session, destructive |
| ● live / reconnecting / read-only / not running | head | state | third copy of presence in a pane |
| model select + model-in-use | strip | setup + readout | the readout is the useful half |
| mode select | strip | setup | per session |
| chat / console / source | strip | setup | per session |
| charter ▾, history ▾ | strip | inspect (drawer) | occasional |
| artifacts ▾ | strip | inspect (menu) | often |
| move…, bring here | strip | move | rare |
| board ▾ | strip | move | rare; meaningless inside a board |
| plugins ▾, mcp ▾, activity ▾ | strip | inspect + setup | occasional |
| ctx meter | strip, right | readout | always read — and hidden in compact panes |

Three copies of "is it alive"; eleven word-buttons in a strip that
scrolls sideways in a pane; every ▾ a peer of every other regardless of
purpose or frequency.

## 2. Recommendation

Sort every control into four buckets. The bucket, not the age of the
feature, decides where it lives.

- **Identity + presence** (left, always): one presence glyph, `@name`,
  harness chip; title and repo·#channel inline on the page, in the
  name's tooltip in a pane.
- **Readouts** (right, always): the context meter (bar + % on the page,
  % in a pane, % only on a phone) and the model in use as text. The two
  things glanced at every turn.
- **Verbs** (right, icons with tooltips): stop ■, branch ⑂, and
  interrupt ⏸ while busy. Three slots, no more.
- **Everything else → one `⋯` session menu**, grouped with separators:
  *setup* (model, mode, render mode, reload, plugins ▸, mcp ▸),
  *inspect* (charter, history, artifacts, activity), *move* (move…,
  bring here, add to board — hidden inside that board). The button
  carries badges so folding hides no signal: running activities count,
  plugin updates, MCP trouble.

Delete: the Meter in the head, the ● LIVE pill, the `model`/`mode`
labels, `board ▾` inside a board. Presence becomes one glyph with a
tooltip: ● live, ◐ busy (pulsing), ◌ reconnecting, ○ not running,
◇ read-only.

Mockups at ~900 px:

```
page:  ● @arch@plank@lt-bryon-wsl [CLAUDE] "board triage"  ~/src/hub · #12      ctx ▓▓▓▓░░ 62%  opus-5   ⑂  ■  ⋯
pane:  1 ● @arch@plank@lt-bryon-wsl [CLAUDE] [needs you] [2 bg]   ctx ▓▓▓░░ 48% opus-5  ⑂ ■ ⋯ │ ⇄ ↗ ⤢ ⫿ ⫽ ×
phone: ● @arch [CLAUDE]      ▓▓▓ 62%  ⑂ ■ ⋯
```

The pane bar *is* the session bar: `SessionView` renders one
`SessionBar` in both modes, the board passing its index as `leading` and
its layout buttons as `trailing`. The pane's own "change contents" ⋯
becomes a different glyph so a pane has one ⋯. The `compact` mode
collapses to one CSS class instead of the twenty `display:none` rules
in session.css.

**The rule for the next dropdown.** The bar has three verb slots and two
readout slots. A new feature ships as a menu entry in one of the three
groups — a `MenuSpec { group, label, icon, badge?, hotkey?, run }` — and
never touches the bar. Promotion needs a demotion, and the bar is for
what most operators use more than once per session. Every menu entry
is registered in the command palette from the same spec, so adding one
adds a palette entry (and a slash alias where it makes sense) for free.

## 3. Risks

- Eleven visible words become one ⋯: grouped labels, badges, a `⌘K`
  hint in the menu footer, tooltips kept verbatim.
- The menu must be a real menu: arrow keys, Esc, close on outside click
  — not the hover-close the portal menus use today.
- Touch: no hover menus; under 720 px the ⋯ opens as a bottom sheet;
  icon buttons get a 36 px hit area.
- Stop as an icon keeps its confirm step; the tooltip says "stop
  session".
- Plugins and MCP are large panels (560–640 px); they launch from the
  menu as panels, not as hover cascades.

## 4. The slate (proposed)

| id | ask |
|---|---|
| U-1 | `SessionBar` shared by page and pane; presence one glyph; ctx meter in both. |
| U-2 | The `⋯` session menu with groups, badges, keyboard; `MenuSpec` registry feeding the palette. |
| U-3 | Verbs as icons (stop, branch, interrupt) with confirm kept. |
| U-4 | Phone: bottom-sheet menu, truncated name. |

## 5. Open questions

1. Icons for stop/branch, or short words? Icons win space; words win
   the first week. (Recommended: icons with tooltips, and the palette
   lists them by name.)
2. Model in use as a readout on the bar, with the selector in the menu —
   or keep the selector visible? (Recommended: readout on the bar.)
3. Should the ⋯ menu *replace* the strip outright, or should the strip
   remain reachable as an expanded state ("show controls")? (Recommended:
   replace; the menu is the controls.)

## 6. Decisions (2026-09-15)

1. Icons with tooltips for the verbs.
2. The model in use is a readout on the bar; where space is short it
   tucks into the ⋯ menu's head and the harness chip's tooltip — both,
   as the operator allowed.
3. The menu replaces the strip: the menu is the controls.
