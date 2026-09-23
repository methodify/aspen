# Aspen console — visual and interaction audit (input to v0.38)

Rig driven at 1568×783 (light and dark) and inside a 400 px iframe (light and dark). Stylesheets read: `ui/src/styles.css`, `ui/src/phone.css`, `ui/src/pages/*.css`; skimmed `App.tsx`, `sessionBar.tsx`. Nothing edited. Screenshots are in `shots/` beside this file.

## 1. Diagnosis — why it reads as unpolished

The bones are good. The Switchboard idea (one calm graphite palette, three signal hues, a mono data voice, square geometry with one chamfer) is a real identity, and the session bar shows what it looks like when the idea is carried through: one row, one glyph per verb, one menu with grouped rows. The rest of the app does not carry the idea through. The unpolished feeling comes from five specific things, all visible in the screenshots and traceable in the CSS.

**1. The type system has no scale, and mono has swallowed the hierarchy.** `styles.css` declares three families (Chakra Petch display, Plex Sans body, Plex Mono) and a 15 px body, but the pages use ~22 distinct font sizes (0.6, 0.62, 0.625, 0.65, 0.66, 0.68, 0.6875, 0.7, 0.72, 0.74, 0.75, 0.78, 0.8, 0.8125, 0.82, 0.83, 0.85, 0.875, 0.9, 0.9375, 0.95 rem, plus 10/11.5/12/12.5/14 px). Almost everything the operator actually reads is 11–13 px mono in `--text-dim`: labels, meta, session state, need-card bodies, mesh onboarding copy, history rows, notices settings. The result on a wide screen (Now, Mesh list) is a flat sheet of small grey mono where nothing is louder than anything else. Mono is a strength — it is the right voice for names, hashes, counts, states and paths — but it is being used for prose (the Mesh onboarding paragraphs, the notices help text, hints under drawers) and for labels, where it reads as "unstyled". The display face is used at 10–11 px uppercase for section labels, which is too small for a 0.14 em-tracked face to hold its shape; it turns into grey fuzz (`NEEDS YOU`, `THE FLEET`, `RUNTIME DEFAULTS`).

**2. Two geometries and four popover skins coexist.** The root declares `--r: 0` and a chamfer. Newer sheets went their own way: `board.css` uses 6/4/3 px radii on panes, cards, tabs, inputs; `view.css` restyles `.seg` with a 4 px radius (the same class `session.css` defines square, so the winner depends on import order); `mesh-switch-menu`, `artifacts-menu`, `branch-choice`, `attach-chip`, `tool-cmd`, `rail-toggle` all round. Popovers come in four skins: `.session-menu` (square, `--line-hi` border, shadow .35), `.panel-frame/.artifacts-menu` (6 px radius, `--line`, shadow .18), `.ctx-pop` and the usage popover (square, shadow .35), `.mesh-switch-menu` (6 px, shadow .25). Side by side on the session view (screenshot 31, 44) the ⋯ menu and its panels visibly belong to different products.

**3. Light mode inverts the surface semantics.** `--bg-well` is defined as "deepest" and is the input background. In dark that is correct (inputs sink). In light `--bg-well: #d3dae0` is *darker* than the page (`#e6ebef`), so every input — composer, filter, search, "add repository", the notices hook fields — renders as a grey slab that reads as disabled (screenshots 04, 21, 28, 36). Disabled primary buttons (`opacity: .4` on amber) become beige blobs next to them ("send", "start", "queue: start mesh", "add marketplace"). Contrast is also thin: `--text-dim #778799` on `#e6ebef` is ≈3.2:1, below AA for the 11–12 px text it is used on; dark `#64798a` on `#0e141b` is ≈4.3:1, borderline. Popover shadows use the same dark-mode alphas in light, so panels float on a grey-on-grey haze instead of separating.

**4. Amber and red are overloaded.** Amber is at once: primary button, focus ring, active-nav marker, selection colour, the "normal" delivery class, the user bubble tab, the brand E, link buttons, the update pill, the palette glyph, the ⌘ hint. Red is: gating, the stop verb, the `mcp 2` down badge, the needs-you count, the attention pane, the pane error bar, the bell-with-unseen. When everything important is amber-or-red, nothing is. There is no neutral accent for "interactive" versus "signal".

**5. Components are per-page inventions.** About twenty button styles (`.btn`, `.bar-verb`, `.bar-menu-btn`, `.btn-reload`, `.btn-interrupt`, `.btn-allow/deny/quiet/always/answer/skip`, `.charter-toggle`, `.q-opt`, `.q-option`, `.pane-btn`, `.board-tabs button`, `.picker-row`, `.rail-toggle`, `.composer button`, `button.link`, `.status-activity`, two `.seg`s, `.class-select`), heights from 22 to 32 px. About a dozen chip styles (`.chip`, `.class-badge`, `.needs-kind`, `.node-chip`, `.harness-chip`, `.menu-btn-badge`, `.badge-count`, history kind boxes, search roles, MCP state chips, `.attach-chip`, `.pair-chip`). A dozen row styles with padding 4/6/8/10 px. Native `<select>` everywhere (Mesh runtime defaults, Boards, Plugins, the ⋯ menu's model/mode rows) in the system font at a different size from everything around it. Stray hard-coded colours and undefined tokens in page sheets: `#d33`, `#d66`, `#6c6`, `#2a6`, `#27a`, `#b5651d`, `rgba(221,51,51,.08)`, `rgba(128,128,128,.25)`, `var(--sig-error)` (not defined), `var(--accent)` (not defined), `var(--mono)` (not defined; `view.css` and `.tool-card-tui` fall back to the browser default monospace), `var(--bg-1)` (not defined). `styles.css` 549–556 contains a duplicated `.rail-wait-pip` block with a comment inside a selector, and line 556 makes `.rail-activity-pip` absolutely positioned for the *wide* rail too.

Smaller but visible:

- **Iconography.** The status bar ends in two near-identical circle glyphs (`◔` bell, `◐/◑/◒` theme) that nobody can tell apart without hovering; the session bar's `⎇ ■ ⋯` set is fine, but the board pane bar adds `⇄ ⤢ ⤡ ⇅ ⫶ ×` at 12 px, unlabeled, in a second row of verbs beside the session bar's own verbs. Presence is expressed four ways (`.meter` VU bars in the rail, `.presence-dot`, `.presence-glyph ●◐◌○◇`, `.live-dot` in Usage, coloured square "page" icons on the map).
- **Empty states** are inconsistent: a lone `—` and one line (Now fleet), a `◇` (Flow), a `?` (Search), a red error bar plus "no transcript yet" (board panes — the rig's `@far@repo1@anindor` proxied-name error is surfaced raw), bare sentences with no mark (Plugins "no marketplaces yet — add one below", MCP panel, artifacts panel).
- **Panels stack instead of replacing.** Opening plugins → MCP → activity → artifacts from the ⋯ menu piles four panels on the same anchor (screenshot 31). Charter and history open as full-width drawers under the bar, both at once, newest on top (32). The notices panel does not close on Esc or outside click.
- **Motion/feedback.** Only the VU meter, pips and `perm-pulse` animate; hover states are a flat background swap; nothing enters or leaves. Focus rings are a 2 px amber outline with 2 px offset — good, but the same colour as active nav and primary buttons.
- **The rail and status bar** are the least considered surfaces: the rail's `N F M H S B P U` hotkey letters sit beside labels at the same weight as the labels; boards get a `■`, sessions get the VU meter; the "FLEET · 0 BUSY · 3/5 LIVE" line, the `RECENT` header and the collapse `«` are three different treatments of "section". The status bar has no left/centre/right structure — the update pill, counts, reload button and two glyphs are just appended.

## 2. Per-surface findings

Light vs dark is noted where it differs; otherwise both are the same.

### Status bar and rail (every page)

| What works | What is off | Fix |
|---|---|---|
| Wordmark with the amber E; hotkey letters; the rail collapses to glyphs; phone bottom bar mapping | Bell (`◔`) and theme (`◐`) are twins; "new console — reload" is a full `.btn` at the same weight as the brand; `0 BUSY 3 IDLE 2 OFF` micro reads as debug output; rail hotkey letters compete with labels; VU meter in rail vs dot elsewhere. Light: `.btn.ghost` borders nearly vanish on `#f1f5f8` | Structure the bar as brand · context · (spacer) · signal cluster · utilities. Replace `◔` with a real bell glyph or a 16 px inline SVG; make theme a row in the More/⌘ menu, not a bar button. Counts become a single presence strip (three coloured dots with numbers). Rail: hotkey letters at `--text-dim` 10 px mono in a fixed 16 px column, labels at 13 px sans 500; one presence dot per session row |

### Now

| What works | What is off | Fix |
|---|---|---|
| The order (needs-you, fleet, folded quiet); need-cards carry their verbs; the fold | Everything is 12 px mono grey at full 1300 px width — spreadsheet feel; `update`/`blocked` kind boxes are grey slabs (light) with no hue; `THE FLEET 0 active` label is fuzz; the empty fleet state (`—`) sits in a void; the fold is a full-width bordered button. Light: inputs and kind boxes both grey; dark: fine but flat | Cap content width (~1100 px) or use the width for a two-column layout (needs-you left, fleet right). Need-card: 14 px sans body, a coloured kind chip (amber `update`, red `blocked`), meta on one mono line beneath. One empty-state component (mark, sentence, one verb). Fold becomes a text link row |

### Flow

| What works | What is off | Fix |
|---|---|---|
| Three-column model; channel list; class selector on the composer; right-hand channel card in the display face | Channel list is all mono; the empty state (`◇`) floats mid-page; composer input slab (light); `+ new channel` primary in the stage head competes with `send` | Channel list rows as the list system (13 px sans name, mono count); empty state anchored at the top of the message column with the next action ("Send the first message below" as a link that focuses the composer) |

### Mesh — list

| What works | What is off | Fix |
|---|---|---|
| The repo strip with session rows; row meta (msgs, age, hash, verb) on the right; the inline new-session flow is quick | Densest page in the app: five stacked strips before the repo, three columns of 11 px mono onboarding prose, native selects in the system font, a table (`node / runtime / starts with / from`) with mono headers; `less ▴` and `how a mesh works ▾` are two different disclosure styles; the new-session flow is a bare input + tan `start` between the repo head and the rows | Move "not in a mesh" onboarding behind one disclosure, prose in 14 px sans. Nodes / Add repository / Runtime defaults collapse to a single "Node" strip with rows. Selects become the app select (styled, mono, 28 px). New session: an inline row *inside* the repo group styled as the list system's "new row" (dashed border, name input, template select, `start` primary) |

### Mesh — map

| What works | What is off | Fix |
|---|---|---|
| Node card with version; legend; the "likely waiting" dashed arrow is a nice idea | Card sits in the top-left of an empty canvas; agent icons are grey "page" shapes with names below in mono; console `◇ @operator` in amber above; legend chips are square swatches. Dark: readable; light: icons wash out | Centre the graph; agents as the same presence dot + name pill used in the rail; the node card uses the panel system; legend in the stage head as a chip row |

### History

| What works | What is off | Fix |
|---|---|---|
| Lanes + overview brush; kind chips; the summary line in the title | Overview brush is a plain grey slab; axis labels 10 px; `today ◂ ▸` + `1H 3H DAY` + agent filter are three unrelated controls; legend row mixes swatches and prose; log rows all mono. Phone: legend wraps onto three lines | One control group in the stage head (range seg · date stepper · filter). Brush gets a subtle gradient/selection colour. Log rows use the list system: time in mono dim, kind chip coloured, agent name 13 px sans, text sans |

### Search

| What works | What is off | Fix |
|---|---|---|
| Results grouped by session; `mark` highlight; role column | Input in the stage head is a slab (light); `SEARCH` label is fuzz; `?` empty state; group headers in `bg-strip-2` look like a second nav. Dark: the amber focus ring on the input is the only accent — good | Search input as a 36 px field with a leading glyph, no label; results in the list system; empty state via the shared component |

### Boards page + a board

| What works | What is off | Fix |
|---|---|---|
| Board card; layout picker seg; pane bar reuses the session bar | Page is a plain form (H1, 14 px bold H2s, native selects); rounded card/inputs vs square app; on the board, panes have 6 px radius, focused pane gets a white 1 px + inset ring (harsh in dark), each pane shows a red error bar from the proxied name, the pane bar has *two* verb clusters (session `⎇ ■ ⋯` and pane `⇄ ⤢ ⤡ ⇅ ⫶ ×`) at 12 px; `broadcast` is a native checkbox. Phone: board renders one pane with a tab strip — actually usable | Panes square with 1 px `--line`; focus = 1 px `--focus` outline offset −1, no inset. Pane verbs fold into the pane's ⋯ (swap/expand/close) leaving the session bar as the only bar. Error bar becomes the shared inline-notice component. Boards page: cards grid + one "new board" panel with the app's select and seg |

### Plugins

| What works | What is off | Fix |
|---|---|---|
| Clear copy; catalog table | Doc-like page: 14 px bold H2s, form rows, native selects, lowercase table headers at 12.5 px, `no plugins yet` as a table row. Both themes flat | Three panels (Marketplaces, Templates, Catalog) using the panel system; table header style from Usage; empty rows via the empty-state component |

### Usage

| What works | What is off | Fix |
|---|---|---|
| Best table in the app: uppercase display headers, right-aligned mono numbers, live dot; summary in the title | Two unlabeled seg groups in the head (`TODAY WEEK ALL` and `SESSION NODE REPO MODEL`); the note paragraph is 12 px; `$0.000` rows are noise. Light: header grey too faint | Make this table the canonical `.table` and apply it to Plugins and the usage popover. Label the segs (range · group by). Dim zero rows |

### Session view + ⋯ menu + panels

| What works | What is off | Fix |
|---|---|---|
| The bar (identity, readouts, three verbs, ⋯); menu grouping (setup / inspect / move) with the palette hint; MCP panel rows with state chips and verbs; the usage popover table | Bar shares `--bg-base` with the page so it does not read as a bar; menu (square, `--line-hi`) vs panels (rounded, `--line`) vs drawers (full-width) are three skins; panels stack on the same anchor (31); charter/history drawers both open (32); user bubbles right-aligned with an amber tab and the tool cards at 100ch make a ragged two-column transcript; `done` floats at the far right of each tool card; ● bullets on tool cards; status line `idle session $—` mono; composer slab + tan send (light). Dark: the transcript is the strongest surface in the app | Bar gets `--bg-panel` and a 1 px seam. One popover skin for menu and panels; a row's panel *replaces* the open panel (one panel at a time beside the menu). Charter and history become panels too, not drawers. Transcript: left-aligned column of max 80ch; user turns as a left-bordered block (no right alignment); tool cards `max-width: 80ch`, `done`/duration inline after the name; status line in sans with mono numbers |

### Palette

| What works | What is off | Fix |
|---|---|---|
| Layout, grouping, hints, esc kbd, dimmed backdrop; consistent in both themes | The `⌘` glyph is amber like everything else; selected row marker is an amber bar; 6 px radius here too | Keep. Swap the marker to the accent token from §3; radius to the panel token |

### Slash autocomplete

| What works | What is off | Fix |
|---|---|---|
| Name amber mono, args, description, footer with keys; capped height | 71 rows at 26 px with no grouping; descriptions truncate mid-word | Group by source (built-in / skills / plugins); description at 12 px sans |

### Bell (notices)

| What works | What is off | Fix |
|---|---|---|
| Kind filters; per-device toggles; hook config is reachable | It is a settings sheet, not an inbox: filters, checkboxes, three hook inputs and `save` surround an empty list; native checkboxes; inputs are slabs (light); does not close on Esc or outside click | Split: bell = list of notices (empty state + last 24 h); a gear row at the bottom opens "notification settings" as a panel. Use the panel system and its close behaviour |

### Phone (400 px)

| What works | What is off | Fix |
|---|---|---|
| Bottom bar; status bar keeps mark, ⌘, version, reload; need-cards wrap sensibly; session bar collapses to name + CTX + verbs; the ⋯ menu is a bottom sheet; More sheet; History stacks its controls | Status bar scrolls sideways (the reload button is cut off); the need-card's chip row wraps to a second line below the body; Mesh onboarding prose at 11 px mono is unreadable at this width; history legend takes three lines; the composer placeholder wraps to three lines. Dark and light behave the same | Status bar: brand + ⌘ + bell only; version/reload into More. Need-card verbs in a single overflow menu on phone. Mesh onboarding hidden by default on phone. Shorter composer placeholder ("Message @far — / for commands") |

## 3. Direction — a crafted AI studio

Keep the wordmark, the graphite palette, the mono data voice, the square geometry, the three signal hues. Sharpen them with four principles.

**Principle 1 — Two voices, one scale.** Sans is for anything a human reads as language (labels, prose, row titles, buttons). Mono is for anything a machine produced (names, hashes, counts, paths, states, timestamps, code). Neither is ever below 11 px. A seven-step scale replaces the 22 ad-hoc sizes.

**Principle 2 — One geometry, three surfaces.** Square corners everywhere; a 2 px radius is allowed only on chips and the phone sheet. Depth comes from three surface levels and one seam colour, not from shadows; shadows exist only on floating panels and are theme-specific.

**Principle 3 — Signal is scarce.** Amber, red, green, cyan are reserved for state. Interaction gets its own neutral accent (a cool blue-grey that reads as "clickable" in both themes). Primary buttons use the accent, not amber; amber returns to meaning "normal priority / needs a look".

**Principle 4 — Every surface has the session bar's shape.** Identity left, readouts and verbs right, one ⋯ for the rest; panels open beside, one at a time; the same row, chip, button and empty-state components on every page.

### Proposed tokens

Type (rem, body 15 px):

| Token | Size / line | Face | Use |
|---|---|---|---|
| `--t-title` | 1.375 / 1.2 | display 600 | page titles |
| `--t-heading` | 1.0 / 1.3 | display 600, 0.02 em | panel titles, board/session names |
| `--t-body` | 0.9375 / 1.55 | sans 400 | transcript, prose, need-card body |
| `--t-ui` | 0.8125 / 1.3 | sans 500 | buttons, menu rows, row titles, nav |
| `--t-meta` | 0.75 / 1.4 | mono 400 | names, hashes, counts, states, paths |
| `--t-label` | 0.6875 / 1 | sans 600, 0.08 em, uppercase | section and table headers (sans, not display) |
| `--t-micro` | 0.6875 / 1 | mono 500 | badges, kbd, pips |

Spacing: keep `--sp-1..12` (4-based); add `--sp-0: 2px`. Rows use 8/12, panels 12/16, page gutters 24, phone gutters 16. Control heights: 28 (compact, bars and rows), 32 (default), 36 (search/composer).

Radius: `--r-0: 0` (default), `--r-chip: 2px`, `--r-sheet: 10px` (phone sheet only). Retire the other radii.

Surfaces and lines:

| Token | Dark | Light | Use |
|---|---|---|---|
| `--surface-0` | #0b1016 | #eef1f4 | page (stage) |
| `--surface-1` | #121a22 | #f7f9fb | bars, rail, cards, rows |
| `--surface-2` | #1a2430 | #ffffff | panels, popovers, hover |
| `--surface-input` | #0b1016 | #ffffff | inputs (dark sinks, light lifts) |
| `--line` | #263340 | #cfd8e0 | seams |
| `--line-strong` | #36475a | #a9b8c5 | panel borders, focused rows |
| `--shadow-panel` | 0 8px 24px rgb(0 0 0 / .45) | 0 6px 20px rgb(16 23 31 / .12) | floating panels only |

Text: `--text-hi` #e8eff4 / #10171f; `--text-mid` #a7b6c3 / #3f5263; `--text-dim` #7a8c9c / #5e7183 (both ≥4.5:1 on surface-0).

Accent and signals:

| Token | Dark | Light | Meaning |
|---|---|---|---|
| `--accent` | #7fb3e6 | #2a6fb3 | interactive: primary button, focus ring, active nav, links, palette marker |
| `--sig-busy` | #37e08b | #0e9e56 | a turn is running (dot, meter, `busy` chip) |
| `--sig-idle` | #8a9aa8 | #5f7180 | live, idle |
| `--sig-off` | #46545f | #9ba7b0 | not running |
| `--sig-waiting` | #ffb02e | #a9640a | waiting on the operator (question, notice, update) — the former "normal" |
| `--sig-gating` | #ff3b57 | #d01029 | permission gate, error, stop |
| `--sig-info` | #35c2d1 | #0c7e8c | bus notices, search marks, context meter |
| `*-dim` | 12 % tint of each on surface-1 | | chip and card backgrounds |

`--focus` becomes `--accent`. Selection stays amber-on-well in dark; light selection uses `--accent` at 25 %.

### Component rules

**Button** — one class, three variants, two sizes: `.btn` (surface-2, `--line-strong`), `.btn.primary` (accent fill, `--surface-0` text), `.btn.quiet` (no fill, `--line`), plus `.btn.danger` (gating outline). Sizes 28 and 32. Icon-only is `.btn.icon` (square, 28/32, 36 px hit area, tooltip). Everything in the list under diagnosis §5 maps onto these; `.bar-verb` becomes `.btn.icon.quiet`.

**Chip** — one class: mono `--t-micro`, 20 px tall, `--r-chip`, 1 px border. Variants by signal token only: `.chip.busy/.idle/.off/.waiting/.gating/.info`. Kind boxes on Now and History, MCP state, class badges, harness, node origin all become this chip.

**Row / list** — `.row`: 36 px min, 8/12 padding, grid of `[status 16px] [title 1fr] [meta auto] [verbs auto]`; title `--t-ui`, meta `--t-meta` dim, hover surface-2, active left 2 px accent. Used for rail sessions, mesh sessions, history log, search hits, menu rows, notice rows, MCP servers, plugin catalog rows, activity rows.

**Panel / popover** — `.panel`: surface-2, 1 px `--line-strong`, `--shadow-panel`, square, 320–640 px, head row (`--t-label` title, spacer, extra, ×). Esc and outside-click close; opening a panel from a menu row replaces the open panel; phone → bottom sheet. Session menu, MCP/plugins/activity/artifacts/usage/charter/history, notices, mesh switcher, context popover, palette all use it.

**Empty state** — `.empty`: a 20 px glyph in `--line-strong`, one sentence in `--t-body` mid, one `.btn.quiet` next action; centred in the column it belongs to, never in the page.

## 4. Prioritised plan for v0.38

### Tier 1 — foundation (lifts every page)

1. **Tokens** (`styles.css` `:root`, `[data-theme]`, `prefers-color-scheme`): add the type, surface, accent and signal tokens above; alias old names (`--bg-well → --surface-input`, `--sig-normal → --sig-waiting`, `--live → --sig-busy`, `--focus → --accent`) so nothing breaks; fix light `--bg-well`. Remove hard-coded hexes and undefined tokens in `board.css`, `session.css` (diff/artifact colours, `--sig-error`, `--accent`, `--mono`, `--bg-1`), `view.css`, `plugins.css`. Delete the duplicated `.rail-wait-pip` block and scope `.rail-activity-pip` absolute to `.mesh-col.narrow`.
2. **Type utilities**: replace `.t-display*`, `.label`, `.mono-meta`, `.micro` with `.t-title/.t-heading/.t-body/.t-ui/.t-meta/.t-label/.t-micro`; sweep the 22 sizes in `pages/*.css` onto them.
3. **Base components** in `styles.css`: `.btn` (variants/sizes), `.chip` (signal variants), `.row`, `.panel`, `.empty`, `.field` (input/textarea/select at 32 px with a styled select arrow), `.seg` (one definition, delete the `view.css` copy), `.table` (lifted from `usage.css`). Retire `.bar-verb`, `.btn-reload`, `.btn-interrupt`, `.btn-allow/deny/quiet/always/answer/skip`, `.charter-toggle`, `.q-opt`, `.q-option`, `.pane-btn`, `.picker-row`, `.rail-toggle`, `.composer button` into `.btn` variants.
4. **Status bar and rail** (`App.tsx`, `styles.css`): three-zone bar; presence strip; real bell glyph; theme into the More/⌘ menu; `.btn.quiet` for reload. Rail: hotkey column, `.row` for nav and sessions, one presence dot, section labels in `--t-label`.
5. **Popover behaviour** (`sessionBar.tsx` `PanelFrame`): one panel at a time per anchor; notices panel adopts `PanelFrame` (Esc/outside close); theme-specific shadow.

### Tier 2 — per-page passes, in operator-time order

1. **Session view** (`session.css`, session component): bar on `--surface-1` with seam; transcript column max 80ch, user turns left-bordered, tool cards 80ch with inline status; status line in `--t-ui` with mono numbers; composer `.field` 36 px + `.btn.primary`; charter and history become panels; menu selects use `.field`.
2. **Boards** (`board.css`): square panes, accent focus outline, pane verbs folded into a pane ⋯, error bar → shared inline notice, boards page as cards + one panel with `.field`/`.seg`.
3. **Now** (`now.css`): content width cap or two-column; need-card with coloured chips and sans body; `.empty` for the fleet; fold as a link row.
4. **Mesh** (`styles.css` mesh/sess sections, mesh page): onboarding behind one disclosure in sans; node/repo/runtime strips consolidated; `.field` selects; new-session as a "new row" in the repo group; map centred with rail-style agent pills.
5. **History, Search, Flow, Plugins, Usage**: `.row`/`.table`/`.empty`/`.field` adoption; labeled seg groups; grouped slash autocomplete; bell split into inbox + settings panel.
6. **Phone** (`phone.css`): status bar reduced to brand · ⌘ · bell; need-card verbs into an overflow; Mesh onboarding hidden; shorter composer placeholder.

### Tier 3 — delight and motion

1. Panel and sheet enter/exit (120 ms fade + 4 px translate, `--ease-seat`), respecting `prefers-reduced-motion`.
2. Presence transitions: dot colour crossfades; the VU meter only in the session bar and rail when busy.
3. Row hover reveals verbs (opacity 0 → 1 at 90 ms) instead of always-on verb clusters on Mesh and Now.
4. Streaming caret and `bubble.flash` in `--accent`; a subtle top progress hairline on the session bar while a turn runs.
5. Palette: recently-used group; keyboard hint chips in `--t-micro`.

## 5. Screenshots

In `shots/` (65 files, numbered). Light: 01–36; dark: 40–60; phone: 61–68.
