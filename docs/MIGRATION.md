# Session migration: move, copy, export, import

**Status:** design reference for what is built (2026-09-06, v0.11).
Proposal and the ideas beyond it: PROPOSALS-2026-09.md §5. Code:
`crates/aspen-node/src/migrate.rs` (rules, bundle), `node.rs`
(`session_export`, `pull_session`, `session_moved`), mesh ops in
`federation.rs`, the `move` route in `crates/aspen/src/api.rs`, the CLI
verbs in `main.rs`, the dialog in `ui/src/pages/Session.tsx`.

## 1. The problem, precisely

A session is trapped on its machine twice over. The harness keys the
transcript directory by the absolute repo path (`~/.claude/projects/
<encoded path>/`), and absolute paths are embedded throughout the
transcript's content: `cwd` on every line, tool inputs, the agent's own
prose. Copying files is not enough; moving a session is a **path
rewriting problem**. Context-convergence (`~/src/context-convergence`)
established the rules; we port them and drop git as the intermediary,
because both ends are our own daemons with a sealed channel between them.

## 2. The rules

`PathCtx` names one machine's anchors in its native form: home, the repo,
the harness's project dir for that repo, the bare encoded dir name, the
temp dir, and the OS. **Canonicalize** replaces anchors with sentinels —
`{{CLAUDE_PROJECT_DIR}}`, `{{REPO}}`, `{{TMP}}`, `{{HOME}}`, then
`{{ENCODED_DIR}}` — longest anchor first, so nesting resolves right.

- **Boundary-anchored.** An anchor matches only when the character after
  it is not a word character: `…/catalog` never touches `…/catalog2` or
  `…/catalog-backup`; a trailing `.` is punctuation and stays outside.
- **JSON leaves and keys, never raw text.** A JSONL line is parsed, every
  string leaf and key rewritten, and re-serialized compactly. An
  unparseable line passes through untouched.
- **Windows dialects.** A Windows anchor is matched as `C:\…`, `C:/…`,
  `/c/…`, and `/mnt/c/…`, either drive-letter case; the path tail after
  a sentinel is normalized to `/` in canonical form and to the target's
  native separator on localize.
- **Never rewritten:** session ids, file names.
- **Round-trip guard** (`round_trip_stable`): canonical → local →
  canonical must be identity for the target; import reports *residue* —
  occurrences of the source's home that survived — as advisory.

Tests in `migrate.rs` cover the boundaries, nesting, WSL→mac, and the
Windows dialects.

## 3. The bundle

`manifest.json` plus files by tier, in a staging dir under
`<data>/staging/<bundle id>/` (or a tar for the file verbs):

| tier | what | default |
|---|---|---|
| A | the transcript (canonicalized), plus the transcripts its bookmarks point at; the agents row (name, channel, session id, charter, title, harness args); lineage; bookmarks | always |
| B | the session's subfolder: tool results, subagent transcripts | on |
| C | project memory (`memory/` under the project dir), canonicalized | on |
| D | files the session wrote outside the repo (the provenance index of artifacts.rs), and its attachments dir | on, ≤32 MB each |
| E | `git diff --binary HEAD` as `repo.patch` plus untracked files | opt-in (`--with-changes`) |

The manifest also carries the source node and its `PathCtx`, the harness
version stamped on the transcript's newest line, the repo's basename and
`origin` URL (to find a counterpart), and notes.

## 4. Verbs

**move** — `aspen move @agent --to <node> [--repo path] [--as name]`,
`POST /api/agents/{name}/move {to, mode:"move"}`, or the session page's
*move…*. The target node does the work; whichever node received the
request routes it there (`session_pull`). On the target:

0. **Preflight**, before the source is touched: `session_spec` from the
   source (repo basename and origin); the target repo is the one given,
   else the counterpart on this node by origin, else by basename. No
   match → the error, and nothing has stopped anywhere.
1. `session_export` on the source: for a move it **stops the session**
   (clearing its live mark, so the source will not revive it) and stages
   the bundle.
2. `bundle_read` in 256 KB chunks, sealed over whatever links exist,
   direct or relayed; then `bundle_done` clears the staging dir.
3. **Install**: every file localized into its place — the transcript
   under the target's project dir for that repo, the subfolder beside it,
   memory merged (a differing existing file is kept and the incoming one
   written as `<name>.from-<node>.<ext>`, reported as a conflict; a 3-way
   merge needs a base we do not hold yet), artifacts to their localized
   path when that path is on this node, else under
   `<data>/imported/<bundle>/` with a note. The agent is registered under
   the same bare name and the **target repo's handle** (so
   `far@repo1` may become `far@repo1-j1`), with title, lineage, bookmarks.
4. **Revive** in place: the live mark is set first, so the one-writer gate
   counts the transcript as ours.
5. `session_moved` on the source: the row becomes a **tombstone** carrying
   the full new address (`moved_to`), undelivered bus rows addressed to
   the old name are re-addressed to the new one, and the roster is
   re-broadcast. A message to the old address is refused with the pointer;
   the session page shows it as a link.

If anything fails after step 1 in a move, the target asks the source to
**revive** the session (`in_place`) and reports that it did.

**copy** — the same with `mode:"copy"` / `--copy`: the source keeps
running; the target registers the session and revives it as a **fork**
(new id, history kept, lineage recorded). This is convergence's
resume-then-branch discipline as the default, so two nodes never grow one
session id. It is the way to split work by platform: the WSL session
keeps going, the mac fork works the platform branch.

**export / import** — `aspen session export @agent -o file.aspen-session`
writes the bundle as a tar (system `tar`); `aspen session import file
[--repo path] [--as name]` installs it and registers the agent, not
started (revive from the console or `resume`). Same code, no mesh; for an
offline node, an archive, or handing a session to someone.

## 4b. Preflight and bring-it-here (v0.17)

`GET /api/agents/{name}/move/preflight?to=<node>` answers before
anything happens: from the source (`session_preflight` op) the tier
sizes (A transcript, B session folder, C memory), file count, branch,
dirty count, harness version, busy/live; from the target
(`node_preflight_target` op) the counterpart repo it would pick, its
harness version, and whether it accepts sessions; plus `blockers`
(target not accepting, unreachable) and `warnings` (no counterpart —
give a path; harness versions differ; uncommitted changes stay behind;
mid-turn). When the source is down, the answer says so and names a
replica held here if any (REPLICATION.md). The move dialog fetches it
whenever the target changes and shows the readout; a blocker disables
the button unless a repo path is given.

**Bring it here**: every remote session's header and Now card carry
*bring here* — the move dialog aimed at this console's node in move
mode (`/session/<name>?bring=1` opens it too), so pulling work to the
machine in front of you is one click and one confirm. A node's *evacuate*
(SERVICING.md §13b) is the same move applied to every session there.

## 5. What was verified (rig, 2026-09-06)

- Move over the mesh, j2 → j1, into a **different repo path**: 4 files,
  ~920 KB, under 4 s; the transcript localized (86 `cwd` fields
  rewritten, zero sentinels left); the agent answered a history question
  from before the move and reported its new working directory; the source
  row tombstoned and a message to it refused with the new address.
- Copy j1 → j2 as a fork: both live afterwards.
- Export to a tar on j1, import on the root under a new name: registered,
  not live.
- Preflight failure (no matching repo on the target) leaves the source
  running; the dialog shows the error.
- The console dialog: node picker (links down disabled), move/copy, repo
  path; on success the page navigates to the new address.

## 5b. Verified (rig, 2026-09-07)

Preflight for a live session on a peer reported 612 KB of transcript,
matching harness versions and the counterpart repo on the target;
evacuating a node with three live sessions moved one, skipped one whose
name was taken on the target and one with no transcript yet (which a
precheck now refuses before stopping it), and returned the node to ready.

## 6. Not built (phases 3 and 4 of the proposal)

Built since: memory convergence (MEMORY.md, v0.16), transcript
replication (REPLICATION.md, v0.16), evacuate (SERVICING.md §13b),
bring-it-here and the preflight readout (§4b, v0.17). Still open: a
patch-carry default for dirty trees; History's session lists for a down
node (the sizes, harness skew, dirty
state) before confirming; deleting the source's transcript files after a
move (they stay on disk beside the tombstone).

## Harnesses (v0.23)

A bundle names the session's harness (`agent.harness`). Tier A is what
the harness's store lists: Claude's transcript; Codex's rollout and the
rollouts its history base chains to, placed under `{{CODEX_HOME}}/
sessions/<date>/` on the target (a new path anchor beside
`{{CLAUDE_PROJECT_DIR}}`). Tiers B and C are Claude-only. Preflight
carries `harness_name` and the harness version on both ends, and a
target without the harness is not accepting. See HARNESSES.md §8.
