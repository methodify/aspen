# Proposals — September 2026 feature dump

**Status:** §1–§4 shipped as v0.10, §5 phases 1–2 as v0.11 (2026-09-06;
MIGRATION.md), §6 as v0.12 (2026-09-07; BOARDS.md). §5 phases 3–4 are
open (BACKLOG B-5b). Backlog ids in BACKLOG.md. Each section says what is asked, what exists today, the
design, what it implies beyond the ask, the cost, and open questions.
Nothing here is built.

---

## 1. Composer drafts (B-1)

**Ask.** Text typed in a session's composer survives leaving the view and
coming back.

**Today.** The draft is React state in the session page; navigating away
unmounts it.

**Design.**
- Key `aspen.draft.<agent name>` in `localStorage`; write on every change
  (debounced ~300 ms); restore on mount; delete on a successful send. The
  same for Now's reply box (`aspen.draft.now.<agent>`) and the charter
  editor.
- Restore is silent; a small "draft restored" hint only if the restored
  text is longer than a line.
- Per browser, per device — a convenience, not synced state. Nothing goes
  to the node.

**Implies.** Nothing beyond the ask. Half a day.

---

## 2. Tool calls: live, then summary (B-2)

**Ask.** A running tool shows its contents; when the agent moves on it
collapses to our summary line; any collapsed call opens on click. Keep
our summary style.

**Today.** Every tool is a `<details>` card, collapsed; the running one
gets a busy dot and a "running" chip. Rehydrated history carries only
`{id, name}` (a chip, not openable): the transcript endpoint drops
inputs and results.

**Design.**
- *Active* = the newest item in the transcript while the turn is busy.
  An active tool card is open. When a later item arrives (the next tool,
  assistant text, a turn end) it closes — unless the operator touched it.
- Operator clicks toggle the card and pin that state for the item (a
  `Map<itemId, open>` in the page; not persisted). Auto behavior only
  applies to untouched cards.
- Content: input rendered per tool, not as raw JSON — `Bash` shows the
  command as a terminal block and the output below it; `Read` the path
  and line range; `Edit` a unified diff of old→new; `Write` the path,
  line count, and the text collapsed past 40 lines; `Grep`/`Glob` the
  pattern and hits; everything else pretty JSON. A running card shows a
  live elapsed timer.
- The summary line gains a result hint once done: `exit 0`, `wrote 17
  lines`, `12 matches`, `error`. Cheap, and it is what the TUI's
  collapsed line carries.
- Console (TUI) render mode follows the same rules with the same
  renderers in monospace.
- Rehydration: the transcript endpoint (local and the `transcript` mesh
  op) includes tool inputs and results, size-capped per item (~16 KB,
  with "truncated" marked) so long-lived sessions stay light. Then
  history cards open like live ones.

**Implies.** Per-tool renderers are the seed of §3's provenance: the
`Write`/`Edit`/`Read` inputs name files; a session knows what it touched.
Two to three days including rehydration.

**Open.** Partial output while a tool runs: the harness delivers tool
results whole, so a running `Bash` shows its command and the timer, not
streaming output. Streaming would need the harness to expose it.

---

## 3. Artifact links over the mesh (B-3)

**Ask.** Paths the agent writes in its output become links that open a
viewer in the console — images, documents, whatever — served from the
agent's home node over the mesh, whether or not the viewer's node has a
direct link.

**Today.** Assistant text is markdown; a path is inert text. No file
endpoint exists, local or mesh.

**Design.**

*Recognizing links.* In assistant text and in tool results, detect
absolute paths (`/…`, `~/…`, `C:\…`, `file://`) and repo-relative paths
that look like files (an extension or a known filename). Render each as a
link. The console does not check existence up front; opening a path that
isn't there says so. Fenced code blocks are left alone unless the whole
line is a path.

*The viewer.* Route `/view/<agent>?path=…`, also openable in a new tab.
By type: images inline (png/jpg/gif/webp/svg); markdown rendered with a
source toggle; text and code with line numbers and highlighting; PDF in
an iframe; JSON pretty-printed; anything else as a download. Shows the
node it came from, size, mtime. Caps at 32 MB; larger files download
only.

*Serving.* `GET /api/agents/{name}/file?path=…` on the console's node.
Local agent: read and stream. Remote agent: two mesh ops on the home
node — `file_stat` (exists, size, mtime, media type) and `file_read`
(offset, length → base64 chunk of ≤256 KB). The console node assembles
chunks and streams the body. This rides the existing `api_req` frames,
sealed end to end, through direct links or relays alike; a mailbox never
carries it (a file read is a live request).

*What may be served.* Not the whole disk. A path is viewable for an
agent when it is inside the agent's repo, inside the harness's own data
for that session (`~/.claude/projects/<slug>/…`, `image-cache`,
`plans`), inside the system temp dir, or **named by a tool call in that
session's transcript** (a `Write`/`Edit`/`Read` target, a `Bash` output
path). That last rule is provenance: if the agent touched it, the
operator may see it. Denied paths return 403 with the rule that would
admit them. The console token gates the whole thing; mesh peers are
already members.

**Implies.**
- *An artifacts drawer per session*: the files this session wrote or
  read, newest first, from the same provenance index — "what did it
  produce?" answered without reading the transcript. Falls out of §2's
  per-tool renderers.
- *Attachments (§4) are artifacts*: a pasted file lands on the home node
  and is viewable by the same route.
- *Migration (§5) knows what to carry*: the provenance index is the list
  of session-created files outside the repo.
Three to four days: detection + viewer, file ops over the mesh,
provenance index.

**Open.** Windows paths in a mac browser (render as-is; the home node
resolves). Symlinks (resolve, then apply the rules to the target).

---

## 4. Inline attachments (B-4)

**Ask.** Paste an image or document into the composer; it rides the
message as an attachment with a marker in the text where it was pasted,
so the agent gets both the content and its place in the conversation.

**Today.** The composer sends `{text}`; the node writes a stream-json
user message whose content is that string.

**Design.**

*Composer.* Paste (`image/*` from the clipboard, files) and drag-drop
both attach. Each attachment gets a chip under the box (thumbnail or
icon, name, size, remove) and a marker inserted at the caret:
`[attachment 1: screenshot.png]`. Markers are plain text — moving or
deleting them in the text is how the operator places them. Send is
disabled while any attachment is over the cap (8 MB each, 24 MB per
message).

*Wire.* `POST …/message` body becomes `{text, attachments:[{n, name,
media_type, data}]}` (base64). Remote agents: the `message` mesh op
carries the same body; a sealed frame of a few MB is fine on a link, and
if the link is down the bus queue stores it as it does text today.

*To the harness.* The node splits the text at markers and builds a
content array: text, then for each marker the attachment as an `image`
block (raster types) with the text continuing after — so the model sees
the image exactly where it was pasted. Non-image files (PDF, text, csv,
anything) are written to `~/.aspen/attachments/<session>/<n>-<name>` on
the home node and the marker is rewritten in the text to name that path,
so the agent reads it with its own tools. Images are saved there too, so
they can be referred to later and viewed via §3.

*Transcript.* User bubbles render thumbnails at marker positions;
rehydration reads the `image` blocks already in the JSONL.

**Implies.** Bus messages between agents could carry attachments by the
same shape (`bus_send` with a file) — an agent handing a screenshot to a
peer. Not in this round. Two days.

**Open.** Whether the harness's stream-json accepts `document` blocks for
PDFs; until confirmed, the file-on-disk path is the contract for
non-images.

---

## 5. Session migration, and what it implies (B-5)

**Ask.** Move a session to another node so it resumes there with its full
context, on a new home, over the mesh — the context-convergence idea
without git as the intermediary. Then: what else does that point at?

### 5.1 What context-convergence established

`~/src/context-convergence` (Python, ~3.5k lines) moves Claude Code
context between machines through a private git repo. The parts that
transfer to us as *knowledge*, whatever the transport:

- **The encoded project dir is the absolute path**, lossily (`[^A-Za-z0-9]`
  → `-`), and absolute paths are embedded throughout transcript content.
  Copying files is not enough; moving a session is a **path rewriting
  problem**. It rewrites JSON-decoded string leaves (never raw text),
  boundary-anchored (`catalog` must not touch `catalog2`), longest
  anchor first, in four tiers: project root, the project's context dir,
  the bare encoded dir name, home. Windows in three dialects
  (`C:\`, `C:/`, `/c/`), separators normalized per target OS. Session
  ids and filenames are never rewritten. A round-trip guard proves the
  rewrite is stable before anything is published.
- **Resume on the target needs two things**: the transcript under the
  directory the harness will derive from the target cwd, with its
  original `<uuid>.jsonl` name. Then `claude -r <uuid>` works.
- **Diverged transcripts are not merged.** A pure extension unions; two
  copies that both grew past a common prefix are flagged, never
  concatenated. The recommended discipline is resume-then-branch so each
  machine grows its own id. Memory files get a line-level 3-way merge.
- **Scope it chose**: transcripts plus the whole `memory/` subtree of a
  project; not the per-session subfolders (tool results, subagent
  transcripts), not todos, plans, image caches, settings.

Our situation is different in three ways, and each makes this easier:
both ends are **our own daemons** with an authenticated, sealed channel
between them; a session's identity is already ours (the agents table:
name, repo, session id, charter, title, lineage, bookmarks); and we
already know how repos map across nodes (the mesh repo registry
identifies a repo by its origin, and per-node paths are recorded).

### 5.2 The primitive: a session bundle

Everything below builds on one format, whether it travels over the mesh
or as a file.

```
manifest.json         session identity, source node/os/home/repo root,
                      tiers included, per-file {path-tier, rel, size,
                      sha256}, harness version, lineage, bookmarks
transcript/<uuid>.jsonl            (canonicalized: sentinel paths)
transcript/<uuid>/…                tool-results, subagents (tier B)
memory/…                           project memory subtree (tier C)
artifacts/…                        session-created files outside the
                                   repo, from the provenance index (D)
repo.patch, repo.untracked.tar     uncommitted work (tier E)
```

Canonical form uses sentinels exactly in convergence's spirit —
`{{REPO}}`, `{{CLAUDE_PROJECT_DIR}}`, `{{ENCODED_DIR}}`, `{{HOME}}`,
`{{TMP}}` — applied to JSON leaves and to memory/markdown text, with the
same boundary and Windows-dialect rules, and the same round-trip guard.
The receiving node **localizes** against its own home, its own path for
that repo (from the registry, or chosen by the operator), and its own
harness data dir. We port the rules; the tests come with them (fixtures
across WSL, mac, Windows are exactly this mesh).

Tiers, and the default:

| tier | what | default |
|---|---|---|
| A | transcript of the session; agents row; lineage; bookmarks (and the transcripts bookmarks point at) | always |
| B | the session's subfolder: tool results, subagent transcripts | on — resume reads them lazily |
| C | project memory subtree, **merged** (3-way with a common base when one is known, else union for `MEMORY.md`, else keep-both with markers) | on |
| D | files the session wrote outside the repo (§3's provenance index) and attachments (§4) | on, size-capped |
| E | uncommitted repo changes as a patch plus untracked files | opt-in; refused onto a dirty target unless forced |

Not carried: harness settings and plugins, global memory, credentials,
anything not in a tier. Code travels by git; the bundle carries only what
git does not.

### 5.3 Verbs

- **move** `aspen move @agent --to <node>` — the session stops here,
  resumes there, the name follows: `@aspen-main@hub@anindor-wsl` becomes
  `@aspen-main@hub@macbook`. The source row becomes a tombstone ("moved
  to macbook, <when>") so a bus message to the old address is answered
  with the new one rather than swallowed; pending bus rows are re-homed.
  The one-writer gate is satisfied by construction: the source is shut
  down before the target imports, and the import sets the target's live
  mark, so the target's revive treats the transcript as its own.
- **copy** `aspen copy @agent --to <node> [--as name]` — the target gets
  a **fork** (new session id, history kept, lineage recorded to the
  source); the source keeps running. This is convergence's
  resume-then-branch, made the default so two nodes never grow one id.
  The mac-build case is this verb: the WSL session keeps going, the mac
  fork works the platform branch, and both know their sibling.
- **export / import** — the same bundle as a file (`.aspen-session`),
  for a node that is offline, for archiving, for handing a session to
  someone. The mesh verbs are export-transfer-import with no file.

Over the mesh the transfer is three ops on the source: `session_export`
(build the manifest, canonicalize into a staging dir), `file_read`
chunks (§3's op, 256 KB, sealed), and `session_export_done`; and on the
target `session_import` (localize, write, register, revive). Progress is
reported to the console as chunks land. A bundle for a long session is
tens of MB; over a direct LAN link that is seconds, over the relay a
minute or two. A mailbox never carries it.

Preflight, shown before the operator confirms: target has the repo (same
origin) and at which path, or asks for one; harness versions on both
sides; bundle size by tier; uncommitted changes present (offer tier E);
the target's dirty state if E is chosen; the source's live state (a
running session is interrupted at its next turn boundary, or the
operator waits).

### 5.4 What it implies — the product ideas

The mechanism above is the necessary base. What it enables is more
interesting than the move itself.

1. **Bring it here.** The console knows which node it is served from.
   "Bring @aspen-main here" on any session is `move --to this node`. The
   operator moves between machines and the work follows, rather than the
   operator remembering where the work lives.

2. **Evacuate a node.** A laptop is about to leave, a machine is going
   down for a while: "move everything on macbook to mini" — one command,
   sessions arrive on the stay-up node and keep running headless. This is
   a servicing verb next to drain and update; drain already knows how to
   wait for turn boundaries.

3. **Fork across machines** (copy) as the deliberate way to split work
   by platform or by experiment, with lineage visible in History so the
   two branches can be compared and, later, one carried forward.

4. **Memory convergence** — the tier C merge run continuously, not only
   at a move. What one node's agent learned about a repo shows up on the
   others: project memory as a small, sealed bus stream (a memory file
   changed → the delta to every node that has that repo → 3-way merge
   there, conflicts surfaced in the console as a needs card). This is
   convergence's best idea, and the bus is a better transport for it than
   a git branch: sub-second, no clone, no commit noise.

5. **Transcript replication** — the big one. Tier A as a *standing*
   stream rather than a one-shot bundle: every session's transcript is
   tailed to a designated node (the mini; or every peer that holds the
   repo), append-only, exactly convergence's union semantics — an
   extension is appended, anything else is a divergence and is refused
   and flagged. Then:
   - a lost machine loses no context;
   - **move is instant**: the target already has the transcript up to
     the last flushed line; the move ships only the tail and the small
     tiers, then promotes the replica;
   - the console can show any session's transcript from any node without
     asking the home node, and History/Library become mesh-wide by
     construction.
   Cost is disk and a little bus traffic per turn. Ordering and
   backpressure are the bus's existing at-least-once rows.

6. **Archive and cold storage.** Sessions past their retention on a
   laptop are moved, not deleted, to a node with disk — lineage intact,
   resumable later from anywhere. "Forget here, keep there."

7. **Portable sessions** as files — export for support, for sharing a
   reproduction with a collaborator, for keeping a session outside any
   mesh. Import registers it as an adoption candidate (the adoption flow
   already knows how to take in a transcript it did not create).

What it does not imply: multi-user. The mesh is one operator's; a
session handed to another person is a bundle file, not a mesh verb.

### 5.5 Risks and open questions

- **Path rewriting completeness.** Convergence's own doctor reports
  "residue": paths it cannot classify (sibling roots outside home, other
  projects' dirs). Those stay machine-specific; a resumed session may
  reference a file that is not at that path on the new node. Acceptable
  with a warning in preflight (count of unresolvable paths); §3's viewer
  makes the old node's copy one click away while a link to it exists.
- **Harness compatibility.** A transcript written by one harness version
  resumed by another has worked so far (2.1.x rewrote copied lines
  gracefully); preflight shows the skew, and copy (fork) is the safer
  verb when versions differ.
- **Bookmarks and lineage** reference other session ids; tier A carries
  the bookmark set's transcripts so `resume bookmark` keeps working.
  Branch ancestors are already folded into a fork's file and need no
  carrying.
- **Secrets in transcripts.** Sealed in transit and at rest on nodes the
  operator owns; a bundle *file* is different — export warns and offers
  convergence's secret scan idea.
- **Memory merge conflicts** need a home in the console: a needs card
  with the two versions and a resolve action.

### 5.6 Cost and phasing

1. Bundle format, canonicalize/localize with tests across the three
   OSes, `export`/`import` as files, import as adoption. ~1 week.
2. Mesh transfer (`session_export`, chunked `file_read`, `session_import`),
   `move` and `copy` with tombstones and re-homed bus rows, preflight,
   console picker on the session page and in Now. ~1 week.
3. Memory convergence over the bus with conflict cards. ~3–4 days.
4. Transcript replication and instant move. ~1 week, after 1–3 have
   settled the formats.

---

## Recommendation for the next version

Two versions, not one:

- **v0.10 — the console round**: §1, §2, §3, §4 together. They share
  one piece of infrastructure (the provenance index and the file ops
  over the mesh) and are all visible every day. About a week and a half.
- **v0.11 — sessions that move**: §5 phases 1 and 2. Phases 3 and 4 as
  the version after, once move has been used for real.

---

## 6. Boards: layouts of sessions across the estate (B-6)

**Ask.** The terminal habit: two sessions side by side, or one over two;
arbitrary grids. In Aspen, let the operator define layouts of sessions
from any node, keep several, and put a session in as many as they like.

**Today.** One session per page. Now shows the fleet as rows; switching
sessions is navigation. Nothing holds two conversations on screen.

### 6.1 The object: a board

A **board** is a named arrangement of **panes**. Its shape is a split
tree, the model tmux and tiling window managers converge on because it
composes: a node is either a split (horizontal or vertical, with sizes)
or a pane. `1|2` is one split; `1|2/3` is a split whose right child is a
split; a 2×2 grid is a split of two splits. Any layout the operator can
draw with a divider is representable, and dividers are how it is edited.

```
Board { id, name, updated_at, root }
root  = Split { dir: "row"|"col", sizes: [..], children: [node, ...] }
      | Pane  { kind: "session", agent }            // @main@hub@anindor-wsl
      | Pane  { kind: "view",    agent, path }      // an artifact, live
      | Pane  { kind: "dynamic", query }            // §6.4
```

A pane holds a **reference** to a session, not the session. The same
session may sit in any number of boards, and twice in one board if
someone wants that; each pane opens its own event stream and the
transcript reducer is already per-mount. Sessions on other nodes work
as they do on the session page today: the console's node proxies the
stream over the mesh.

Boards are the operator's, not a browser's. They are stored on the node
(`boards` table) and **ride the mesh**: the roster carries a digest of a
node's boards, a peer whose digest differs pulls them (`boards` op), and
last-writer-wins by `updated_at`. Open any console on any node and the
same boards are there — the estate has one set of desks. A board's URL
is `/board/<id>`; boards export and import as JSON.

### 6.2 Presets and editing

- **Presets** on the new-board menu: `1|2`, `1/2`, `1|2/3`, `2×2`,
  `main + stack` (a 60% main pane with a column of smaller ones). Pick
  one, then drop sessions into the empty panes from a picker that lists
  the fleet across nodes with their state.
- **Dividers drag.** Sizes persist. Double-click a divider to equalize.
- **Split this pane** right or down; **close** a pane; **swap** two
  panes by dragging a pane's header onto another; **zoom** a pane to
  fill the board and back (tmux's `prefix z`), which is how a two-up
  becomes a focused view without leaving the board.
- **Add to board** from any session page: a menu of boards, "split
  right" or "split down" of the focused pane.
- On a narrow screen a board degrades to tabs across the top, one pane
  visible; the layout is kept for wide screens.

### 6.3 Working in a board

- **Focus.** One pane has focus; its border says so; its composer takes
  the keyboard. Click, or `ctrl+1…9` by pane order, or `ctrl+arrows`
  to move focus spatially. The session page's hotkeys become pane-scoped
  (the escape-to-page-keys behavior already exists and generalizes).
- **Compact panes.** Below a width threshold a pane drops to a compact
  header (name, node, state dot, working timer) and summary-only tool
  cards; the transcript stays readable at two or three columns.
- **Attention.** A pane whose agent raised a permission or a question
  gets a marked border; `ctrl+.` jumps focus to the next pane needing
  attention. This is what a two-up is *for*: watch one, answer the
  other.
- **Broadcast** (tmux's synchronize-panes): a toggle on the board that
  sends the focused composer's message to every session pane on it,
  with each pane's header showing a broadcast badge while it is on. For
  driving parallel agents: "run the tests", "report status", "stop".
  Off by default; confirm on first use per board.
- **Drafts** are per pane (the composer draft key gains the board and
  pane id), so two panes of one session keep separate drafts.
- **Pane kinds beyond sessions**: an artifact viewer pane (a log or
  report the agent keeps updating, next to the session writing it — the
  viewer re-fetches on the session's turn end); a Flow pane filtered to
  the agents on the board (their bus traffic beside them); a Now pane.
  Each is the existing page in a frame, so they cost little.

### 6.4 Dynamic boards

A board whose panes are a **query** rather than a list: every session
matching a filter, laid out automatically (grid by count, or main +
stack with the busiest as main). Filters: node, channel/repo, state
(`busy`, `needs attention`), name pattern, tag. Examples the estate
would use at once:

- *everything busy right now* — the operator's overview during a push;
- *#hub on any node* — one repo's agents wherever they run;
- *needs attention* — a board that is empty when all is well;
- *macbook* — everything on the laptop before it leaves (a natural place
  for "evacuate", B-5b).

A dynamic board refreshes from the roster (the same poll Now uses);
panes come and go without the operator arranging them. Pinning a
dynamic board's current membership turns it into an ordinary one.

### 6.5 What it implies

- **Boards are the natural home for cross-node work** shipped in v0.11:
  a session and its cross-machine fork side by side, or a session before
  and after a move (the tombstone pane shows the pointer).
- **Per-repo default board**: opening a repo's channel from Flow could
  open its board rather than a list.
- **Recording a board** as a template ("my review desk") to instantiate
  with different sessions — the presets are just templates with empty
  panes.
- Later: pane-to-pane drag of text (a result from one agent into
  another's composer), and a "pair" mode where two panes' sessions are
  put on a bus thread together with the operator watching both — the
  duo command as a board.

### 6.6 Cost and phasing

1. **Session as a pane.** Refactor the session page's view into a
   component that can be mounted several times: pane-scoped hotkeys,
   focus, compact mode, per-pane drafts. The transcript/event plumbing
   is already per-mount. ~2 days.
2. **Board model, renderer, persistence.** Split tree, dividers, presets,
   split/close/swap/zoom, the picker, `boards` API and table, `/board/:id`,
   sidebar switcher, add-to-board from a session. ~2–3 days.
3. **Mesh sync** of boards via roster digest + `boards` op. ~1 day.
4. **Attention, broadcast, tabs on narrow screens, view/Flow panes.**
   ~1–2 days.
5. **Dynamic boards.** ~1 day.

About a week and a half; phases 1–3 are the version, 4–5 the polish
after using it.

**Open.** Whether a board should also capture per-pane render mode
(chat/console/source) — probably yes, it is part of "how I look at this
one". Whether `ctrl+number` collides with browser tabs (it does in most
browsers; `alt+number` or a leader key like `g` then a digit is safer —
the hotkey layer already has chords).

---

## 7. Plugins: a mesh-managed library, activated by scope (B-7)

**Ask.** Let Aspen manage the plugins its sessions run with, rather than
driving the harness's own install state: a mesh-wide registry of
marketplaces that survives disconnects and rejoins; activate a plugin for
the mesh, a node, a repo, or a single session; see where a plugin is
active and what a scope has; keep marketplaces updated on a timer and on
demand; cache versions so a running session is never changed under it,
and nag rather than restart when a newer version is waiting.

**What the harness gives us.** `--plugin-dir <path>` (repeatable) loads
a plugin for that session only. A marketplace is a git repo (or a
directory) with `.claude-plugin/marketplace.json`: `plugins[]` each with
a `source` — a relative path in the marketplace repo (`./plugins/x`), or
`{source: git-subdir|github|url, url|repo, path?, ref?}` — and sometimes
a `version`. A plugin is a directory with `.claude-plugin/plugin.json`.
The harness itself caches under `~/.claude/plugins/cache/<market>/<plugin>/
<version>/`, keying an unversioned plugin by the marketplace commit. We
do the same under our own data dir and never touch the harness's tree.

### 7.1 The registry (mesh-synced)

Two small tables, synced the way boards are (roster digest, last writer
wins, tombstones): **marketplaces** `{name, source, added_at,
updated_at}` and **rules** `{id, marketplace, plugin, scope_kind: mesh |
node | repo | session, scope, enabled, pin?, updated_at}`. Add a
marketplace on any console and every node has it within a roster tick.

Everything else is per node and derived: the marketplace **checkout**
(`<data>/plugins/marketplaces/<name>/`, a clone or the directory
itself), the parsed **catalog** (`<data>/plugins/catalog.json`), and the
**cache** (`<data>/plugins/cache/<market>/<plugin>/<version>/`). Sync
runs at start, on a timer (`plugins.sync_minutes`, default 60), and on
demand; it pulls each marketplace, re-reads its catalog, and
materializes every *activated* plugin's current version — relative
sources copied out of the checkout, git sources shallow-cloned at their
ref — so the next spawn has it on disk.

### 7.2 Scope and the effective set

A rule's scope encloses a session when it is `mesh`; `node` and the
session's node matches; `repo` and the session's repo matches by git
origin, else by basename; `session` and the agent's name matches. The
**effective set** is the union of enclosing rules, with the most
specific rule for a plugin deciding (a session-level *disabled* beats a
mesh-level *enabled*). At spawn the node resolves each plugin to its
cached version (the rule's pin, else newest) and appends `--plugin-dir
<path>`; the session records what it started with.

This is the micro-management the harness lacks made manageable: the
default is "every session in this repo", and the exceptions are one
toggle each, visible from both sides.

### 7.3 Surfaces

- **Plugins page** (`/plugins`): marketplaces (add by GitHub `owner/repo`,
  git URL, or local path; sync now; remove), the catalog across
  marketplaces with search, cached versions, and for each plugin an
  **activation matrix**: mesh · each node · each repo · each session, with
  enable/disable toggles and a pin. From the plugin, its application.
- **From the scopes**: the session page gets a *plugins ▾* menu listing
  the effective set with versions and, when a newer cached version
  exists, a **restart nag** ("a newer version of X is cached — restart
  to pick it up"); *reload* stays for hot-reloadable parts. Node and
  repo rows in Mesh show their plugin counts and open the same matrix
  filtered.
- **Updates**: the sync timer; *sync now*; and a sync before a spawn when
  the last one is older than the timer.

### 7.4 What it implies

- **A mesh has one plugin library**, so a session moved or copied to
  another node (v0.11) keeps its plugins: the rules travel with the
  registry, the target caches what it needs at revive.
- **Repo-scoped rules are the natural home for the trust review** (the
  autorun surface already inventories plugins the repo would load).
- **Private marketplaces** are just a git repo the mesh can reach; a
  team's plugins live in one and reach every node without any node
  installing anything by hand.
- Later: per-plugin **usage** (which sessions invoked its skills, from
  the transcripts), and pinning by rule for reproducibility.

### 7.5 Cost

Registry + sync + materialization + spawn integration: ~3 days. Page,
matrix, session menu, nag: ~2 days. One version (v0.13).

---

## 8. Activity: tasks, monitors, subagents, workflows (B-8)

**Ask.** The console has no signal for what a session is running *beside*
its main turn: background shell tasks, monitors and scheduled wakeups,
subagents, workflows. Recognize them, signal them where the session is
shown, and let the operator see them — including a subagent's own
transcript — the way the TUI does.

**Where the facts are.** Everything is in the stream: a background task
is a `Bash` call with `run_in_background` and its later notification; a
subagent is an `Agent` call (or `Task`) whose work lands in a sidechain
transcript under `<project>/<session>/subagents/agent-<id>.jsonl`; a
workflow is a `Workflow` call with its run id; a monitor or loop is a
scheduling call. Each has a start (the tool use), an end (the tool
result or a task-notification line), and, for agents and workflows, a
body of its own on disk.

### 8.1 The model: activities

The node derives an **activity ledger** per session from its event
stream: `{id, kind: task | agent | workflow | monitor, label, started_at,
ended_at?, status, detail}`. Live sessions maintain it from events;
rehydration rebuilds it from the transcript. It rides the roster
summary as counts (`activities: {running: n, agents: n, tasks: n}`) so
every surface can signal without asking.

### 8.2 Surfaces

- **Signal everywhere**: the session row in Now, the board pane bar, and
  the rail glyph gain a small activity chip (`2 agents · 1 task`), pulsing
  while any is running.
- **Activity drawer** on the session page (and in a pane): the ledger,
  newest first — running first — each with elapsed time and its detail;
  a background task shows its command and last output lines; a workflow
  its phases and agents; a monitor its schedule.
- **Subagent view**: clicking an agent activity opens its sidechain
  transcript rendered with the same tool cards and markdown as the main
  view, live while it runs (the file is tailed). Served over the mesh by
  the same transcript ops with a `subagent` parameter.
- **Dynamic boards** (§6.4) gain `has activity` as a state.

### 8.3 What it implies

- A **fleet-wide activity view** in Now: everything running in the
  background across the estate, which is what "is anything still going?"
  asks before closing a laptop; pairs with evacuate (B-5b).
- Subagent transcripts become **artifacts** (§3): linkable, viewable,
  carried by a move (tier B already carries the subfolder).
- Cost estimates per activity (a subagent's spend from its transcript)
  roll up to the session's meter.

### 8.4 Cost

Ledger from events and rehydration, roster counts, chips: ~2 days.
Drawer and subagent view (live tail over the mesh): ~2 days. One version
(v0.14).
