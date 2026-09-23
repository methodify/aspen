# Review of the v0.38 audit — second opinion (2026-09-22)

## 1. Disagreements

**Blue-grey interactive accent (Principle 3): reject.** The diagnosis (amber is overloaded) is right; the cure erases the identity. Amber *is* the wordmark, the focus ring, the active nav, the user turn, the primary button — that is the studio's signature. A cool blue-grey also collides with the existing cyan `--sig-notice` and is the default SaaS accent the product is trying not to resemble. The overload is fixed by subtraction: amber keeps the four *interactive* jobs (primary, focus, active nav, user tab) and loses the decorative ones (⌘ glyph, `.ac-name`, `button.link`, `.chip.op`, `::selection`, update pill, `+ new session`).

**Square everywhere: agree, but the audit drops the chamfer.** Keep `--r: 0`, treat the 6/4/3 px radii in `board.css`, `mesh-switch-menu`, `branch-choice`, `attach-chip`, `tool-cmd`, `rail-toggle` as drift, and put the chamfer on panel head corners and kind chips.

**Seven-step scale: right in shape, wrong at the floor.** "Never below 11 px" and "36 px rows / 28 px minimum controls" break the deliberate density: the compact pane runs 12/12.5 px with 24 px verbs, the ⋯ menu has 20 rows at 6/12 padding and already hits 70 vh on a phone. Keep the scale, add a `.dense` modifier for panes and menus, keep `--t-micro` at 11 px as the floor.

**Labels in sans (`--t-label`): reject.** The tracked Chakra Petch label is identity. The fuzz is a size problem (10–11 px at 0.14 em), not a face problem. Fix: 0.72 rem, 0.1 em, `--text-mid`.

**Sans-for-language / mono-for-machine: right, with two exemptions.** Console and source render modes and the composer are terminal-voiced by design; and Now's prose needs a rule for machine names inside sans prose (inline mono, `--text-hi`).

**Transcript: 80 ch left-aligned column, user turns left-bordered: partial reject.** Right-aligned user turns are the fastest scan cue for "where did I speak"; the ragged look comes from tool cards at `min(100ch, 100%)`. Cap tool cards to the bubble column (~72 ch), put `done`/duration inline after the name, keep alignment.

**Pane verbs into a pane ⋯: reject.** Two ⋯ in one bar, which SESSION_BAR.md §3 forbids. Keep the hairline cluster, trim it to four (pair, zoom, split, close); move *open as page* and *change contents* into the existing ⋯ under a `pane` group.

**Charter and history become panels: reject.** The charter is edited text; a drawer under the bar is right. Make it one drawer at a time.

**Kind chips "red `blocked`": reject.** Red means gate/error. Blocked-on-a-peer is *waiting* → amber; `update` → cyan (info).

## 2. Risks in the plan

- Tier 1.2–1.3 is a mass rename with no safety net. Sequence: (a) tokens-only commit with aliases, verified pixel-identical; (b) new `.btn/.chip/.row/.panel/.empty/.field` *beside* the old classes; (c) migrate page by page, re-shooting each; (d) delete old classes last.
- `--bg-well → --surface-input` alias is wrong as written: `--bg-well` is also code blocks, `pre`, `.mesh-inspect`, `.class-select`, `.rail-glyph`, console mode. Add `--surface-input` used only by inputs.
- Leave the session bar out of Tier 1. Make it the reference the new `.btn.icon` must match; migrate it last, if at all.
- "One panel at a time" contradicts the designed flow ("go down the list"). Replace-at-the-same-anchor, by anchor key inside `PanelFrame`.
- Deleting `view.css`'s `.seg` changes Usage and History (`.on` vs `aria-pressed`): a JSX change.
- Light `--text-dim #5e7183` collapses the mid/dim distinction; check hierarchy, not only contrast.
- `.row` at 36 px would make the ⋯ menu ~40 % taller; exclude menus.
- Hover-revealed verbs conflict with the pane bar being the drag handle and with touch.

## 3. What the audit missed

The question and permission cards (highest-stakes, not shot); console/source render modes; a running turn (caret, pulse, thinking, diff cards, perm-dock, reconnecting, exited banner); boards with 4–6 panes and `.compact`; the catch-up bar; toasts as a fifth popover skin; keyboard-first use and visible focus; which theme is primary (`color-scheme: dark`); scale (30 sessions in the rail, ten repos).

## 4. Top ten

1. Fix the real bugs: undefined `--sig-error/--accent/--mono/--bg-1`, hard-coded hexes, duplicated `.rail-wait-pip`, wide-rail `.rail-activity-pip`.
2. Add `--surface-input` (white in light, well in dark) for inputs only.
3. One popover skin from `.session-menu`'s recipe for `.panel-frame`, `.mesh-switch-menu`, `.ctx-pop`, `.toast`; notices adopt `PanelFrame`.
4. Label floor: 0.72 rem / 0.1 em, same face.
5. Amber subtraction: ⌘ glyph, `.ac-name`, `button.link`, `.chip.op`, `::selection`, update pill go neutral.
6. Session bar on `--bg-panel` with a seam.
7. Tool cards capped to the bubble column with `done` inline.
8. Kill radius drift to `var(--r)`; retire `view.css .seg` with the `aria-pressed` JSX change.
9. Now need-cards: sans body, kind chips as `.class-badge` variants, one shared `.empty` for Now/Flow/Search.
10. Status bar zones, a real bell glyph, theme into the More/⌘ menu, a styled `.field` select replacing native ones.

## 5. Verdict

Adopt with the amendments above. Principles 1, 2 and 4 are right; Principle 3's blue-grey accent should be rejected and replaced with amber subtraction. Resequence Tier 1 as tokens-with-aliases, then additive components, then page-by-page migration, with the session bar kept as the untouched reference.
