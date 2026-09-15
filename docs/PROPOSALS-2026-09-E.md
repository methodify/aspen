# Proposal: the plugins menu that tells the truth

**Status:** approved 2026-09-14 (the operator's ask, expanded); shipping as
v0.28. Reference once built: PLUGINS.md §5–6.

## 0. The ask, and what was actually happening

"No matter what, the plugins menu in a session says *no plugins* — even
with plugins activated in the library for that session, even with the
harness's own plugins on. And updates: I have to deactivate and
reactivate a plugin to get a session onto its new version, even across
a stop and start."

Four separate facts explain it, and none is the operator doing it wrong.

1. **Remote sessions never carried their plugins.** The agent JSON gets
   `plugins` and `plugin_updates` from the *local* live session only;
   the roster a node sends its peers has neither. Driving a session on
   another node — the normal way this mesh is used — the menu had
   nothing to show and said so.
2. **The harness's own plugins were invisible.** Claude reports the
   plugins it loaded from its own configuration in its init inventory
   (`plugins[]`). Aspen stored the inventory and never showed that list.
3. **Sessions started stale.** The catalog is synced at daemon start,
   hourly, and when a rule is *enabled*. A spawn resolves against the
   catalog as it is; a marketplace that moved since the last sync gives
   the old version. Disabling and re-enabling a rule happened to trigger
   a sync — which is why that ritual "worked".
4. **Same version, new content, never re-cached.** A cached version dir
   is never rebuilt. A directory marketplace under development, whose
   `version` field is not bumped for every change, is stale forever at
   that version.

## 1. Product design

**The menu is the session's plugin surface.** Not a readout of a rule
engine; the place you look at and change what this session runs with.
One list, every plugin the mesh's library knows, each row saying:

- **on / off for this session** — a toggle that writes a session-scope
  rule (the most specific scope, so it wins over repo, node and mesh
  rules) and says which scope currently decides ("via repo").
- **running** — the version this process started with, or "not
  running" / "starts next time".
- **latest** — the newest version the node has, when it differs, with
  **update** beside it: sync that marketplace now, then restart the
  session in place (same session id, new process, new set). The
  existing nag under the header stays; the menu is where the per-plugin
  action lives.
- **not cached yet** when a rule names it but no version is on disk.

Below the library's list, **from the harness's own configuration**: the
plugins the runtime reports it loaded on its own (Claude's
`~/.claude/plugins` enablement). Read-only, labelled as such — Aspen
does not manage the harness's tree (PLUGINS.md §1) — but visible, so
"I activated it in Claude" and "Aspen says no plugins" stop being the
same screen.

**Starting a session means starting on the latest.** A spawn syncs the
marketplaces its effective plugins come from — bounded: skipped when
synced within the last minute, capped at 20 s, never fatal — and then
resolves. A running session still keeps what it started with (§5's
rule: nothing restarts on its own); a *new* process gets what is
current.

**Content, not just the version string, decides "current".** A
plugin's cache key is its declared version; when the source tree's
content changes under the same declared version, the node caches it
again as `<version>-<hash8>` and that becomes current. The catalog
carries the content hash; `updates_for` compares keys, so the nag and
the menu see a same-version change as an update. Git-sourced plugins
(a clone at a ref) keep version-only keys — their ref is the content.

## 2. The slate

| id | ask | notes |
|---|---|---|
| G-1 | **Plugins on the roster.** `plugins` and `plugin_updates` per agent in the roster and in the remote agent JSON, so a session on another node shows what it runs with. | §0.1 |
| G-2 | **The harness's own plugins in the menu.** From the init inventory, read-only, labelled. | §0.2 |
| G-3 | **Per-session toggles and versions in the menu.** Every library plugin, on/off writing a session rule, running vs latest, not-cached state. | §1 |
| G-4 | **Update from the menu.** Sync the marketplace, restart in place. | §1 |
| G-5 | **Fresh at start.** Spawn syncs the involved marketplaces (bounded) before resolving. | §1 |
| G-6 | **Content-hashed cache.** Same declared version, new content → new cache key, new current. | §1 |

All six in one round.

## 3. Backlog

- **Adopting the harness's own plugins into the library**: a plugin
  Claude reports from its own tree, offered as "manage this in Aspen"
  (register its marketplace, write a rule, let the harness copy go).
- **Per-plugin usage** from transcripts (skills invoked, MCP tools
  called), on the row.
- **Hot reload where the harness can**: Claude's `reload_plugins`
  re-reads plugin dirs; a content update materialized into the *same*
  dir could reach a running session without a restart. Not done here:
  a running session's dir is never rewritten (the harness watches it),
  and a reload does not re-run MCP servers.
