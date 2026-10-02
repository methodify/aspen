# Plugins: a mesh-managed library, activated by scope

**Status:** design reference for what is built (2026-09-07, v0.13).
Proposal: PROPOSALS-2026-09.md §7. Code: `crates/aspen-node/src/plugins.rs`
(registry types, sync, cache, effective set), the `marketplaces` and
`plugin_rules` tables and digest in `store.rs`, sync and ops in
`federation.rs`, the spawn hook in `node.rs`, routes in
`crates/aspen/src/api.rs`, the page `ui/src/pages/Plugins.tsx`, and the
session menu and nag in `ui/src/pages/Session.tsx`.

## 1. Why Aspen manages plugins itself

The harness's own plugin state (`~/.claude/plugins/…`) is per machine and
per user, and its enablement is per project: every session in a repo gets
the same set. Aspen has many nodes and many sessions per repo, and wants
one library for the mesh with activation at four grains. The harness
gives exactly the hook needed: `--plugin-dir <path>` (repeatable) loads a
plugin for one session. So Aspen keeps its own registry and cache under
its data dir, never touches the harness's tree, and starts each session
with the dirs its rules say.

## 2. The registry (mesh-synced)

- **marketplaces** `{name, source, added_at, updated_at, deleted}` —
  `source` is `github {repo}`, `git {url}`, or `directory {path}`
  (a local marketplace; the path is this node's).
- **rules** `{id, marketplace, plugin, scope_kind, scope, enabled, pin,
  updated_at, deleted}` — `scope_kind` is `mesh | node | repo | session`;
  `scope` is empty, a node name, a repo identity, or a bare agent name.

Both sync the way boards do: the roster carries `plugins_digest` (a hash
over both tables, tombstones included); a peer whose digest differs is
asked for its registry (`plugin_registry` op) and merged last writer
wins per row; a change re-broadcasts the roster and starts a sync so the
node caches what was just activated. Add a marketplace on any console
and every node has it within a roster tick.

## 3. Per node: checkout, catalog, cache

`<data>/plugins/marketplaces/<name>/` is the checkout (a shallow clone,
pulled `--ff-only`; a directory source is used in place).
`<data>/plugins/catalog.json` is the parsed union of every marketplace's
`.claude-plugin/marketplace.json`: per plugin its declared `version` or,
absent one, the checkout's short commit as the **current** version, the
`source`, and the versions cached here. `<data>/plugins/cache/<market>/
<plugin>/<version>/` holds materialized plugins: a relative source
(`./plugins/x`) is copied out of the checkout; `git-subdir`, `github`,
`url` sources are shallow-cloned at their `ref` into a temp dir and the
subdir copied. Only plugins some rule activates are materialized; a
failure is recorded per plugin in the catalog's errors, never fatal.

**Sync** runs at daemon start, every `plugins.sync_minutes` (settings,
default 60), on *sync now* (`POST /api/plugins/sync[?marketplace=]`),
when a marketplace is added, and when a rule is enabled.

## 4. Scope and the effective set

A rule encloses a session when it is `mesh`; `node` and the node name
matches; `repo` and the repo's identity matches — its git `origin` URL,
its basename, or its path (rules may name any; the console offers the
basename, which is the channel); `session` and the bare agent name
matches. For each plugin the **most specific** enclosing rule decides
(session > repo > node > mesh; on a tie, enabled wins). At spawn the node
resolves each enabled plugin to a cached version — the rule's `pin`, else
the catalog's current if cached, else the newest cached — and appends
`--plugin-dir <path>`. The session records its set (`ManagedSession.
plugins`; `plugins` in the agent JSON). A plugin with no cached version
is skipped and named in the spawn note.

## 5. Updates without surprises

**Current is content, not only a version string (v0.28).** For a plugin
whose source is a path inside the marketplace checkout, the sync hashes
the source tree (paths, sizes, mtimes) and records it in the cache dir
(`.aspen-content`) and the catalog (`content`). The same declared
version whose tree changed is cached again under `<version>-<hash8>`,
and that key is the catalog's `current`; a git-sourced plugin (a clone at
a ref) keeps version-only keys. So a directory marketplace under
development, whose `version` is not bumped for every edit, still moves.

**A new process starts on what is current (v0.28).** A spawn syncs the
marketplaces its effective plugins come from — skipped when synced
within the last minute, capped at 20 s, never fatal — and then
resolves. Before, spawns resolved against the last hourly sync, which
is why disabling and re-enabling a rule (a sync trigger) was the only
way to move a session forward.

A running session keeps the version it started with. When a sync caches
a newer current version for a plugin it runs, the agent JSON carries
`plugin_updates: [{plugin, running, available}]`, the session page shows
a nag ("newer plugin version cached: plumb 0.8.3 → 0.9.0 — this session
keeps what it started with until it restarts") with **restart to pick
them up** (stop, then revive in place: same session id, new process,
new set), and the *plugins ▾* menu marks the plugin. Nothing restarts on
its own. *Reload* stays for the parts the harness hot-reloads.

## 6. Surfaces

- `/plugins` (`p`): marketplaces with source, count, last sync, errors,
  *sync now*, *remove* (inline confirm); an add form (GitHub `owner/repo`,
  git URL, or a directory on this node); the catalog with search,
  current and cached versions, and *active for*; **activate…** opens the
  matrix — one row per scope (mesh; each node; each repo; each session)
  with on / off / unset, and a pin for new rules. From the plugin, its
  application.
- Session page: *plugins N ▾* (v0.28, PROPOSALS-2026-09-E.md) lists
  every plugin in the library with an **on/off for this session** toggle
  (writes a session-scope rule, which wins over repo, node and mesh),
  the version **running** vs the **latest** cached, **update** (sync that
  marketplace, restart in place), *starts with … next time* with a
  restart, *not cached yet*; below, **from the harness's own
  configuration**: what the runtime reports it loaded from its own tree
  (Claude's init inventory), read-only. Remote sessions show all of this
  too: `plugins` and `plugin_updates` ride the roster. The nag under
  the header stays.
- API: `GET /api/plugins` (registry + catalog), `PUT/DELETE
  /api/plugins/marketplaces/{name}`, `PUT/DELETE /api/plugins/rules/{id}`,
  `POST /api/plugins/sync`, `GET /api/plugins/effective?agent=` (local or
  via the `plugins_effective` mesh op).

## 6b. The library, the store, and every node (v0.49)

PROPOSALS-2026-10-P.md. A marketplace is a **store**; the **library** is
what the operator keeps at hand. A plugin is in the library when it was
**added** (`plugin_library` row, synced like rules: LWW, tombstones, in
`plugins_digest`), when **a rule names it**, or when **its marketplace is
wholly in the library**: `library: all`, or unset with 1–24 plugins
(`WHOLE_LIBRARY_MAX`; a marketplace that offers nothing yet is not
whole). `GET /api/plugins` carries `library`, `library_members [{marketplace,
plugin, why: picked|rule|marketplace}]` and `whole_library`; `PUT/DELETE
/api/plugins/library/{marketplace}/{plugin}`; `POST
/api/plugins/marketplaces/{name}/library {library: all|picked|null}`.
The library decides only what is offered at hand; turning a plugin on
is still a rule (§4).

**What a plugin brings** (`provides: {skills, commands, agents, hooks,
mcp, lsp}` in the catalog): counted at sync from the plugin's files
(`skills/*/SKILL.md`, `commands/**/*.md`, `agents/**/*.md`,
`hooks/hooks.json`, `.mcp.json`, `.lsp.json`, the manifest's inline
declarations) for a source inside the checkout or a cached copy, plus
what the marketplace entry declares. `POST /api/plugins/inspect
{marketplace, plugin}` (*look inside*) caches an external plugin on this
node without activating it and counts it. The catalog also keeps the
entry's `displayName`, `author` (as stated; no badge) and `homepage`.

**Surfaces.** `/plugins` has tabs (`?tab=`): **Library** (the shelf:
versions, *active for*, why it is there, *activate…*, *remove* for an
added one); **Browse** (search ranked name › display name › publisher /
category › description, category chips with counts, marketplace and
publisher filters, cards with where the code lives and what it brings,
40 at a time); **Marketplaces** (each one's library setting and counts;
adding one asks "keep it as a store, or put all N in the library?";
every node's state); **Templates** (the picker lists the library, plus
*find a plugin*). The session's *plugins ▾* lists what is on, then the
rest of the library, then *find a plugin…* (top 8 from every
marketplace; *on here* writes the session rule). Its header sums what the
plugins that are on bring.

**Every node (L-7).** Sync is per node, so the Marketplaces tab asks
every node (`GET /api/plugins/status`: the `plugins_status` op, or an
older node's `plugins_registry_view`) which marketplaces it knows,
whether it has a copy, when it synced and why it failed. **Sync the
mesh** (`POST /api/plugins/sync?scope=mesh`, or `?node=` for one) sends
this node's registry with the `plugins_sync` op; the receiver merges it
(LWW, row by row) and then syncs, and each node reports back. Git for
plugins never prompts (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`)
and is stopped after 180 s; an authentication failure reads "this node
has no git credentials for <url>". A directory marketplace records the
node it lives on (`source.node`); elsewhere it reads "a directory on
<node>; other nodes need this marketplace as a git repository". Syncs
take turns (one lock per process): concurrent syncs used to overwrite
each other's catalog. A registry pull that fails is logged, and one bad
row is skipped rather than the whole update. A spawn's note names why
each plugin was left out ("started without plugins: x@m (m failed to
sync on this node: …)").

## 7. Verified (rig, 2026-09-07)

A directory marketplace registered on j2 (six plugins parsed); a
repo-scoped rule for one plugin materialized it into the cache; the
registry reached j1 within a roster tick; a revived session started with
`--plugin-dir <cache>/methodical/plumb/0.8.3` (checked on the process)
and the model listed the plugin's skills and MCP tools; bumping the
marketplace's version to 0.9.0 and syncing cached it, the running session
reported the update, the nag showed, and restart brought it up on 0.9.0.

## 7b. Session templates (v0.17)

A **template** is a named recipe stored mesh-wide like boards
(`templates` table, `templates_digest` on the roster, LWW per row):
`{name?, repo?, model?, permission: ask|skip, charter?, extra_args?,
plugins: [{marketplace, plugin, pin?}], board?: {id, mode}}`. The repo
is a handle, a basename, an origin URL, or a path, resolved on the node
that spawns; blank means the panel asks.

Spawning from one (`POST /api/templates/{id}/spawn` with overrides;
`aspen session new --template <name> [--name] [--repo] [--node]`; the
palette's *new session from template …*; Now's panel with its template
select; *start…* on the Plugins page) writes **session-scope plugin
rules** for the new address first, so the process starts with its
`--plugin-dir`s, then spawns with the template's model / charter / args
/ permission (overrides win), and the console places it on the
template's board (first empty pane, or a split). The trust gate applies
as for any spawn. Templates are edited on the Plugins page.

## 8. Not built

Per-plugin usage from transcripts; marketplace auth (private git over
https needs a credential helper on the node — the sync now says when one
is missing); zip (`--plugin-url`)
sources; showing plugin counts on node and repo rows in Mesh; carrying a
pin through a move (the rules travel; the target resolves them).
