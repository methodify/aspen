# Proposal: polish — the console as a crafted studio (v0.38)

**Status:** for the operator's read, 2026-09-22. Nothing built. Inputs:
a full audit of every surface in light and dark on the rig
(`docs/proposals/AUDIT-2026-09-22-console.md`, 65 screenshots in
`~/.cache/aspen-audit-2026-09-22/shots/`) and an independent review of
it (`docs/proposals/AUDIT-2026-09-22-console-review.md`). This page is
the reconciliation: what both agree on, where they differ and which
side is recommended, and the plan.

## 0. The ask

"Our UI isn't bad, but it is unpolished and doesn't look like a
well-designed, well-oiled, crafted AI studio. Apply the thinking that
went into the session bar to the whole app, in both themes."

## 1. Diagnosis (both reviewers agree)

The identity is real and worth keeping: graphite palette, amber
signature, three signal hues, a monospace data voice, square geometry
with a chamfer, the display face for labels. The session bar is what it
looks like carried through. The rest of the app does not carry it
through, for five traceable reasons:

1. **No type scale.** ~22 font sizes across the page sheets; almost
   everything the operator reads is 11–13 px mono in the dim colour, so
   wide pages (Now, Mesh) become a flat grey sheet. Section labels are
   the display face at 10–11 px with wide tracking, which turns to fuzz.
2. **Geometry and popover drift.** Root says square; newer sheets
   (boards, mesh switcher, branch card, attach chips) round to 3–6 px;
   popovers come in four skins (menu / panel / context / mesh switcher),
   plus toasts as a fifth. The ⋯ menu and its own panels look like two
   products side by side.
3. **Light mode inverts surfaces.** The "deepest" token used for inputs
   is darker than the page in light, so every input reads as disabled;
   dim text is ≈3.2:1 on the light page; dark-mode shadow alphas are
   reused in light.
4. **Amber and red are overloaded.** Amber is the brand, the primary
   button, the focus ring, the active nav, selection, the ⌘ glyph, links,
   the update pill and more; red is gate, stop, down, needs-you count,
   error. Signal has no scarcity.
5. **Components are per-page inventions.** ~20 button styles, ~12 chip
   styles, a dozen row paddings, native `<select>` everywhere, stray
   hard-coded hexes and four undefined tokens (`--sig-error`, `--accent`,
   `--mono`, `--bg-1`), a duplicated rule block in `styles.css`.

Plus: two near-identical circle glyphs for bell and theme; presence
drawn four ways; inconsistent empty states; panels stack on one anchor
and drawers stack under the bar; almost no motion; the rail and status
bar the least considered surfaces.

## 2. Direction (as amended)

Four principles. The first, second and fourth are the audit's; the
third is the review's amendment and is the recommended one.

1. **Two voices, one scale.** Sans for anything read as language
   (labels, prose, row titles, buttons); mono for anything a machine
   produced (names, hashes, counts, paths, states, times, code). One
   seven-step scale. Exemptions: console and source render modes and the
   composer stay terminal-voiced; machine names inside sans prose are
   inline mono in the high colour.
2. **One geometry, three surfaces.** Square, with the chamfer as the
   motif (panel head corners, kind chips). Depth from three surface
   levels and a seam colour; shadows only on floating panels, and
   theme-specific.
3. **Amber by subtraction, not replacement.** The audit proposed a
   blue-grey interactive accent; the review rejects it as erasing the
   signature for the default SaaS look, and that is the recommendation
   here. Amber keeps four interactive jobs (primary button, focus ring,
   active nav, the user turn) and loses the decorative ones (⌘ glyph,
   slash-command names, link buttons, operator chip, text selection, the
   update pill). Red means gate or error only; "blocked on a peer" is
   *waiting* (amber); "update" is *info* (cyan).
4. **Every surface has the session bar's shape.** Identity left,
   readouts and verbs right, one ⋯ for the rest; panels open beside it
   and replace at the same anchor; the same row, chip, button, field,
   panel and empty-state components everywhere.

### Tokens (delta from today)

- Type: `--t-title` 1.375 display · `--t-heading` 1.0 display ·
  `--t-body` 0.9375 sans · `--t-ui` 0.8125 sans 500 · `--t-meta` 0.75
  mono · `--t-label` 0.72 **display**, 0.1 em, mid colour (the review's
  correction: the fuzz was size and tracking, not the face) ·
  `--t-micro` 0.6875 mono, the floor. A `.dense` modifier for panes and
  menus keeps their current 12–12.5 px and 24 px verbs.
- Surfaces: `--surface-0/1/2` (page / bars, rows, cards / panels,
  hover), `--line`, `--line-strong`, one `--shadow-panel` per theme.
  **New** `--surface-input` (well in dark, white in light) used only by
  inputs; `--bg-well` stays what it is for code, pre, console mode and
  the rail glyphs (the review caught that aliasing it would lift every
  code block in light).
- Text: light dim colour lifted to clear 4.5:1 while keeping the mid /
  dim step visible (values tuned on the rig, not by number alone).
- Signals: `--sig-busy` (green), `--sig-idle`, `--sig-off`,
  `--sig-waiting` (amber; today's "normal"), `--sig-gating` (red),
  `--sig-info` (cyan), each with a `-dim` tint for chip and card
  backgrounds. Old names alias to the new ones.
- Radius: `--r: 0` everywhere; 2 px only on chips; 10 px only on the
  phone sheet. The 3–6 px drift is removed.

### Components (added beside the old ones, then migrated)

- `.btn` — base / `primary` / `quiet` / `danger`, sizes 28 and 32,
  `icon` for glyph-only with a 36 px hit area. The session bar's verbs
  are the reference the new icon button must match, and the bar
  migrates last.
- `.chip` — mono micro, 20 px, one border, variants by signal only.
- `.row` — status · title · meta · verbs grid, hover surface-2, active
  left bar; used for rail, mesh sessions, history log, search hits,
  notices, MCP servers, plugin rows, activity rows. Menus keep their
  own, denser row.
- `.field` — input, textarea and a styled select at 32 px (36 for the
  composer and search), replacing every native select.
- `.panel` — the ⋯ menu's recipe (square, strong line, one shadow) for
  the session menu, every ⋯ panel, the mesh switcher, the context
  popover, the notices panel, the palette and toasts. Esc and outside
  click close; a panel opened from a menu row replaces the one at the
  same anchor (per anchor, not a global singleton — the usage popover
  and an MCP panel may coexist). Phone: bottom sheet.
- `.empty` — glyph, one sentence, one quiet next action, centred in its
  column.
- `.table` — lifted from Usage, applied to Plugins and the usage
  popover.

## 3. Per-surface passes (in operator-time order)

| surface | the pass |
|---|---|
| Session view | Bar on surface-1 with a seam so it reads as a bar on the page and in panes. Tool cards capped to the bubble column (~72 ch) with `done`/duration inline after the name; user turns **stay right-aligned** (the review: the fastest "where did I speak" cue; the ragged look was the tool cards). Status line in `--t-ui` with mono numbers. Composer as a `.field` with a primary send. One drawer at a time (charter or history), both stay drawers. Menu selects as `.field`. |
| Boards + a board | Square panes, focus as a one-line accent outline (no inset ring). The pane's hairline cluster stays (no second ⋯) trimmed to pair · zoom · split · close; *open as page* and *change contents* go into the ⋯ under a `pane` group. The proxied-name error bar becomes the shared inline notice. Boards page as cards plus one panel with `.field` and `.seg`. |
| Now | Content width capped (~1100 px) or two columns; need-card with a sans body, kind chips as signal variants (`blocked` → waiting, `update` → info), meta on one mono line; `.empty` for the fleet; the fold as a link row. |
| Mesh list | Onboarding behind one disclosure in sans; node / add repository / runtime defaults consolidated into one node strip; `.field` selects; new session as a "new row" inside the repo group; sessions and repos as `.row`. |
| Mesh map | Graph centred; agents as the rail's presence dot + name pill; node card as `.panel`; legend as a chip row in the head. |
| History, Search, Flow, Plugins, Usage | `.row` / `.table` / `.empty` / `.field` adoption; History's three controls as one group; Search input as a 36 px field with a leading glyph; slash autocomplete grouped by source; Usage's seg groups labelled (range · group by), zero rows dimmed. |
| Bell | An inbox: the list of notices (last 24 h, `.empty` when quiet) with a gear row that opens *notification settings* as a panel; toasts owned by the same system; Esc and outside click close. |
| Status bar and rail | Bar zoned brand · context · spacer · presence strip · utilities; a real bell glyph (inline SVG); theme moves into the More/⌘ menu; counts become one presence strip. Rail: hotkey letters in a fixed dim column, labels in `--t-ui`, one presence dot per session, section labels in `--t-label`. Visible focus audited on every control, since keyboard-first use is the app's spine. |
| Phone | Status bar reduced to brand · ⌘ · bell (version and reload into More); need-card verbs in one overflow; Mesh onboarding hidden; shorter composer placeholder. |

The review's list of what neither pass may skip: the question and
permission cards (where amber, red and green meet — the "signal is
scarce" rule is tested there first), a running turn (caret, pulse,
thinking block, diff cards, permission dock, reconnecting, exited
banner), boards with 4–6 panes, the catch-up bar, a rail with 30
sessions and a Mesh list with ten repos. Each pass is shot in both
themes before and after; dark is the primary theme (`color-scheme:
dark`), light must be as considered.

## 4. Sequencing (the review's, adopted)

1. **Tokens with aliases** — one commit, verified pixel-identical
   against the audit's screenshots, plus the real bugs (undefined
   tokens, hard-coded hexes, the duplicated rule, the wide-rail pip).
2. **Additive components** — `.btn/.chip/.row/.field/.panel/.empty/
   .table` beside the old classes; `--surface-input`; the one popover
   skin; the label floor; amber subtraction. Nothing renamed yet.
3. **Page passes** in the order of §3, each re-shot in both themes.
4. **Delete the old classes** last; the session bar last of all, and
   only if the new icon button matches it exactly.
5. **Motion** (Tier 3): panel and sheet enter/exit (120 ms, reduced
   motion respected), presence crossfades, a streaming caret in the
   accent, a progress hairline on the bar during a turn. Not
   hover-revealed verbs (conflicts with the drag handle and touch).

## 5. Questions for the operator

1. **Accent.** Amber by subtraction (recommended) or the audit's
   blue-grey interactive accent with amber reserved for "waiting"?
2. **User turns.** Keep them right-aligned (recommended) or the audit's
   left-aligned bordered block?
3. **Labels.** Keep the display face at a larger size (recommended) or
   move section labels to sans?
4. **Density.** A `.dense` modifier for panes and menus (recommended)
   or the audit's single 36 px row everywhere?
5. **Scope of v0.38.** Steps 1–3 for the session view, boards, Now and
   Mesh, with the rest as v0.38.x (recommended), or everything in one
   release?

## 6. Decisions

(pending)
