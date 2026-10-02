# Proposal P — A plugin library you chose, and marketplaces you browse

**Status:** accepted 2026-10-02; L-1..L-7 ship together. Prompted by
the operator adding the official Claude plugins marketplace: 315 plugins
arrived at once, and every plugin surface became a 319-row alphabetical
scroll. Operator's decisions: the library is by choice plus anything a
rule names; 24 plugins is the "whole marketplace" line (try it and see);
publishers are shown as the marketplace states them, with no badge;
*look inside* fetches on the asking node only. L-7 (a work-mesh node that
never got a marketplace) was added the same day. **Shipped as v0.49.0**
(PLUGINS.md §6b). As built: tabs are `/plugins?tab=…`; the palette needed
nothing (it only opens the session's plugins menu, which is now short);
verification found and fixed a pre-existing race (concurrent syncs
overwrote each other's catalog), now one sync at a time per node.

---

## 1. What went wrong

Aspen treats "a plugin some marketplace offers" and "a plugin in my
library" as the same thing (PLUGINS.md §3: the catalog is the union of
every marketplace). That held while the marketplaces were the operator's
own (mcc: 4 plugins). A public marketplace is a **store**, not a shelf:

- **The session's plugin menu** (*plugins ▾*, ⋯ menu) lists all 319, each
  with a checkbox, sorted running → next → alphabetical. The four the
  operator uses are at the top today only because they are on. Turning
  on a fifth means scrolling past `42crunch-api-security-testing`,
  `activecampaign`, `adobe-for-creativity`… with no search.
- **The Plugins page catalog** is one table of 319 rows. It has a text
  filter, but no categories, no publisher, and no way to tell what you
  use from what merely exists.
- **The session template editor** lists all 319 as checkboxes.
- **Nothing says what a plugin brings in.** A plugin can add skills,
  commands, subagents, hooks, MCP servers or LSP servers. Each costs
  context or starts processes. "Off · ab024cdcfa7c in the library (cached
  when turned on)" repeated 315 times says nothing that helps choose.

What the marketplace gives to work with (official, 315 plugins):
`description` on all, `category` on 301 (development 123, productivity
69, database 39, monitoring 22, security 18, deployment 9, design 8, …),
`homepage` on 299, `author` on 233, `displayName` on 14, `tags`/`keywords`
on 4. Sources: 53 bundled in the marketplace's own repo, 262 in external
repos (`url` 164, `git-subdir` 98).

## 2. The idea: separate the shelf from the store

| | **Marketplaces** (the store) | **Library** (the shelf) |
|---|---|---|
| What | Everything the added marketplaces offer | The plugins you picked to have at hand |
| Size | Hundreds | A handful to a few dozen |
| Where you see it | *Browse* on the Plugins page; search from any plugin surface | The session menu, the template editor, the palette, the Library tab |
| How a plugin gets there | Adding a marketplace | **add to library**, or turning it on anywhere (any rule adds it) |

Turning a plugin on for a scope (mesh, node, repo, session) stays
exactly as it is (PLUGINS.md §4). The library only decides **what is
offered at hand**; it never turns anything on.

## 3. The design

### L-1 The library, as data

- A `plugin_library` row per plugin: `{marketplace, plugin, added_at,
  deleted}`. It syncs over the mesh with the marketplaces and rules: it
  is folded into `plugins_digest` and merged last writer wins, like
  rules.
- **Membership:** in the library = has a library row **or** is named
  by any rule (enabled or not). So nothing the operator has ever turned
  on can fall off the shelf, and writing a rule needs no second step.
- **Per marketplace, "whole marketplace":** a flag on the marketplace
  (`library: all | picked`). For the operator's own marketplaces (mcc)
  every plugin is at hand; for a store, only what was picked. A
  marketplace that adds a plugin under `all` puts it on the shelf at
  the next sync.
- **Migration:** existing marketplaces with ≤ 24 plugins become `all`.
  Larger ones become `picked`, with the shelf seeded from the rules. So
  after upgrading, the operator's shelf is mcc's 4 plus whatever has
  rules (2 session rules today).
- API: `GET /api/plugins` gains `library` and per-plugin `in_library`
  (and why: `picked | rule | marketplace`); `PUT/DELETE
  /api/plugins/library/{marketplace}/{plugin}`; the marketplace `PUT`
  takes `library`.

### L-2 The session's plugin menu: what this session has, and a search

Top to bottom, never the whole store:

1. **On for this session**: running, or starting next time. Each has its
   version, *update*/*restart* as today, and an off toggle. Shows why
   it is on ("via repo hub", "via mesh").
2. **In your library, off here**: the rest of the shelf, one line each,
   with a toggle.
3. **Find a plugin…**: a search field over the whole store (name,
   display name, description, author, category). It shows the top 8
   matches with *on for this session*. Turning one on writes the session
   rule, which also puts it in the library (L-1). Empty-state copy points
   at Browse.
4. From the harness's own configuration: read-only, unchanged.

With a 4-plugin shelf this is a short menu, and it stays short with 315
in the store.

### L-3 The Plugins page: Library · Browse · Marketplaces · Templates

Four tabs replace the one long page (`/plugins`, `/plugins/browse`, …;
the tab is in the URL).

- **Library** is today's catalog table, limited to the shelf: plugin,
  marketplace, current/cached, *active for*, *activate…* (the scope
  matrix as today), and *remove from library*. *Remove* is disabled while
  rules name it, and says which rules.
- **Browse** is the store, built for a few hundred entries:
  - A search box, plus filter chips with counts: **category**
    (development 123 · productivity 69 · …) and **marketplace**. A
    **publisher** filter offers the authors with 3 or more plugins
    (Anthropic 39, Google LLC 14, SAP SE 9, …).
  - Cards, not table rows: display name (or name), publisher, category,
    a two-line description, **where it lives** ("in the marketplace's
    repo", or the external repo's host and path; a trust signal), a
    homepage link, and **what it brings** (L-5). Actions: **add to
    library**, **activate…** (opens the matrix, which also adds it), and
    "in library ✓" for plugins already there.
  - 40 cards at a time, with *show more*. Sort is by name, or "in my
    library first".
- **Marketplaces**: today's section, plus the `library: all | picked`
  switch per marketplace and its count ("315 offered · 3 in library").
- **Templates**: today's section. Its plugin picker becomes the library
  plus the same *find a plugin* search (L-4).

### L-4 Template editor and palette

- The template editor's plugin list is the library, with *find a
  plugin…* to add from the store.
- The command palette offers "plugin: X on/off for this session" for
  library plugins only, not 319 entries.

### L-5 What a plugin brings

Knowing what turning something on costs is the judgment the operator
actually has to make.

- **Bundled plugins** (source inside the marketplace checkout, 53 of the
  official 315; all of mcc): at sync, scan the plugin dir. Count
  `skills/*/SKILL.md`, `commands/*.md`, `agents/*.md`, hooks
  (`hooks/hooks.json` or the manifest), MCP servers (`.mcp.json` or the
  manifest's `mcpServers`), and LSP servers (the manifest's or the
  marketplace entry's `lspServers`). Record them in the catalog as
  `provides: {skills, commands, agents, hooks, mcp, lsp}`.
- **External plugins** (262): unknown until fetched. Cards say "contents
  known once fetched" and offer **look inside**, which materializes the
  plugin into the cache without activating it (the same code path as
  activation) and scans it. The marketplace entry's `lspServers` and
  `skills` fields, where given, are shown without a fetch.
- **Shown as:** small chips on the card and the library row ("3 skills ·
  2 commands · 1 MCP server · hooks"). Hooks and MCP servers are worth
  noticing: hooks run on every tool call, and MCP servers start
  processes. The session menu's header adds them up for what is on: "4
  plugins · 9 skills · 2 MCP servers".

### L-6 Adding a marketplace asks how much of it to shelve

The add form, after the first sync, says "*name* offers 315 plugins"
and lets the operator choose: **keep it as a store — browse and pick**
(the default above 24 plugins) or **put all 315 in the library** (the
default at 24 or fewer). Either can be changed later on the Marketplaces
tab.

### L-7 Every node's plugin state, visible and syncable from any console

The operator registered a marketplace on the work mesh, and one node's
sessions never got its plugin: that node had no checkout of the
marketplace at all, and *sync now* changed nothing. Everything about
plugin sync was per node and invisible from elsewhere:

- *Sync now*, the sync times and the errors on the Plugins page are
  those of **the node the console is attached to**. A clone failing on
  another node (a private repo with no git credentials there, git not on
  the daemon's PATH) was recorded only in that node's catalog.
- The registry reaches a node only when a peer's roster fingerprint
  differs and the pull from that peer succeeds. A failed pull, or one
  malformed row, dropped the whole update with no log line. And the
  fingerprint rides only rosters for the node's **primary** mesh.
- A `directory` marketplace is a path on the node that added it; every
  other node only records "directory missing".
- Git ran with no prompt suppression and no timeout: a credential prompt
  could hold a sync, and every later sync behind it, forever.

What changes:

- **Per-node status on the Marketplaces tab:** for each mesh node, does
  it know the marketplace, does it have a checkout, when it last synced,
  and its error. This comes from the `plugins_registry_view` op, which
  now carries the node's sync state. A node that does not know a
  marketplace, or names it under a different source, is called out.
- ***Sync now* syncs the mesh:** the attached node pushes its registry
  (marketplaces, rules, library) with the `plugins_sync` op, the receiver
  merges it last writer wins as a pull would, then syncs. Results come
  back per node. There is also a per-node *sync* for one node.
- **Git never prompts and never hangs:** `GIT_TERMINAL_PROMPT=0` and
  `GCM_INTERACTIVE=never`, plus a time limit on clone and pull. An
  authentication failure says so: "this node has no git credentials for
  <url>".
- **Registry pulls that fail are logged,** and rows are merged one by
  one, so one bad row no longer drops the rest.
- **A directory marketplace records the node it lives on.** Other nodes
  say "lives on <node>; other nodes need it as a git repo" instead of a
  bare error.
- **The spawn note says why a plugin was left out:** "github-tools@acme
  not started: acme failed to sync on this node (no git credentials for
  …)", instead of "not cached".

Multi-mesh sharing stays as it is (the registry goes only to the primary
mesh's members). The per-node status makes it visible when that is why a
node lacks a marketplace.

## 4. Order and size

| Item | What | Size |
|---|---|---|
| L-1 | Library rows, membership, marketplace flag, migration, API | M (store + sync + API) |
| L-2 | Session menu: on / library / find | S–M (console) |
| L-3 | Plugins page tabs; Browse with chips and cards | M (console) |
| L-4 | Template editor and palette use the library | S |
| L-5 | `provides` for bundled plugins at sync; *look inside* for external ones | M (node scan + console chips) |
| L-6 | Marketplace add asks; per-marketplace switch | S |

| L-7 | Per-node plugin state, mesh-wide sync, git that cannot hang | M |

L-1 + L-2 alone end the drowning. L-3 + L-6 make the store usable. L-5
is the judgment aid. L-7 is the bug. All seven ship as one release (the
operator's call).

## 5. Questions for the operator

1. **Library by choice, or implied by use?** Proposed: by choice, plus
   anything a rule names. The alternative, "library = has a rule", has
   no new data but cannot keep a plugin at hand without turning it on
   somewhere.
2. **The 24-plugin line** for "whole marketplace" on migration and at
   add time: or always ask?
3. **Publisher prominence.** Browse shows the author exactly as the
   marketplace states it, with no "official"/"verified" badge (the
   marketplace's word, not Aspen's to vouch for). Is that right?
4. **Look inside** fetches an external repo on an explicit click
   without activating anything. Is that acceptable on every node, or
   only on the node whose console asked? Proposed: only that node. The
   catalog is per node, so other nodes learn the counts when they cache
   the plugin themselves (the console can show the asking node's
   counts meanwhile).
