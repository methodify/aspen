# Proposal R — Files: browse what an agent can see, and take it anywhere

**Status:** accepted 2026-10-09 (all three questions: yes; the name is
"Files"); shipped as v0.50.0. As built: the path linker already covered
prose, so R-4 adds what it skipped, paths in backticks (`codePathTarget`);
copy, share and drag live in `ui/src/fileActions.ts`, used by the Files
page and the viewer.

---

## 1. The problem

Artifacts were meant to be how an agent's output reached the operator. In
practice agents write files with shell commands (`cat > x.md <<EOF`,
`… > report.csv`, `tee`), not with the harness's write tools, so the
Artifacts list (built from Write/Edit tool calls) misses most of what
they make. The operator ends up doing one of two things:

- going to the machine to fetch the file, or
- opening some other artifact in a browser console and editing the
  `?path=` query to the file they actually want.

The node already has a well-defined file space it will serve for an
agent (`artifacts::resolve`): the agent's **repo**, the session's own
data (transcript dir, plans, image cache, Codex sessions), **temp**, the
session's **attachments**, and **any file its tool calls named**, with
symlinks resolved before the check. What is missing is a way to **see**
that space, and to get a file **out** in a form other tools accept.

## 2. The design

### R-1 Listing (node)

- `GET /api/agents/{name}/files?dir=<path>` returns the entries of one
  directory: `{dir, parent, entries: [{name, kind: file|dir|link, size,
  mtime, served}], truncated}`. With no `dir`, it returns the **roots**:
  repo, attachments, plans, temp, and "named by tool calls outside these"
  as a virtual list.
- The same check as reading: a directory is listed only if it lies in the
  file space, and an entry whose resolved target leaves it (a symlink out)
  is marked `served: false` and cannot be opened.
- `.git` is hidden; dotfiles and gitignored paths are shown dimmed, with a
  toggle to hide them. At most 2,000 entries per directory (`truncated`
  says so; the filter narrows it).
- `GET …/files/recent?limit=50` returns the newest-modified files in the
  repo (skipping `.git`, `node_modules`, `target`, `dist`). This is the
  answer to "the agent just wrote something with bash": it appears at
  the top, however it was written.
- A mesh op `file_list` (observe) serves remote agents the same way,
  through the relay too.

### R-2 The Files page (console)

- `/session/{name}/files?dir=` opens from the session's ⋯ menu (**files**),
  from a board pane's menu, and from the palette.
- Layout: a root switcher (repo · attachments · plans · temp · named
  elsewhere), breadcrumbs, a filter box, and a list of name, size and
  modified time, sortable. A **Recent** tab lists the newest files first.
- Clicking a file opens the existing viewer (`/view`). Each row also has:
  **download**, **copy path**, **copy** (R-3), and **share** where the
  platform supports it.
- Phone width works as in the rest of the console: rows wrap, and the
  actions sit behind one button.

### R-3 Getting a file out: copy, share, drag

A web page cannot put an arbitrary file on the system clipboard: browsers
allow text, HTML and PNG images, nothing else. So **copy** does the most
useful thing per kind, and the other routes cover the rest:

| What | How it leaves |
|---|---|
| An image | **Copy** puts it on the clipboard as a PNG; it pastes as an image attachment in any chat (Aspen, claude.ai, Slack…). |
| A text file (markdown, code, CSV, ≤ 1 MB) | **Copy** puts its contents on the clipboard as text. |
| Any file, into another Aspen session | **Copy** also carries an Aspen file reference. Pasting into any Aspen composer (another session, another board pane, another mesh's console) fetches the file and adds it as an attachment, exactly as if it had been dropped there. Dragging a row onto a composer does the same. |
| Any file, to another app on a phone | **Share** opens the system share sheet with the file (`navigator.share({files})`): send it to the Claude app, Mail, Files, AirDrop. Also offered on desktop where the browser supports it (Chrome on Windows and ChromeOS, Safari on macOS). |
| Any file, to the desktop | **Download**, as today. In Chrome, dragging a row out of the page onto the desktop also saves it. |

### R-4 Paths in the transcript become links

When an agent's reply names a file ("I wrote it to `docs/report.md`",
`/tmp/out.csv`), the path becomes a link to that file in the viewer,
whenever the node would serve it. This is the shortest path from "the
agent points me to a doc" to the doc. Both renderers (chat and console)
link the same way.

### Built on what exists (the operator's guidance: where a piece maps onto existing code, reuse it; where it doesn't, add what's needed)

| Piece | Reuses |
|---|---|
| What may be listed | `artifacts::resolve`: the same roots and the same symlink-safe check that `file_stat`/`file_read` use. The listing is the third verb beside them, in `artifacts.rs`. |
| Entry metadata | `artifacts::stat` (size, media type, kind) per entry. |
| Remote agents | The existing `remote_parts` → mesh-op path the file routes use (`file_stat`, `file_read`), with one more op name, `file_list`. |
| Opening a file | The existing viewer (`/view`, `View.tsx`) and `api.fileBlob` / `api.fileUrl`, with the relay and direct paths as they are. |
| Download | The viewer's download (`?download=1`), unchanged. |
| Attaching into a composer | The composer's existing `addFiles` (the paste and drop path): an Aspen file reference fetches the blob with `api.fileBlob` and hands it to `addFiles` as a `File`. |
| The page | A sibling of the Artifacts menu: its rows become links into the Files page, and the Files page reuses the viewer's row styling. |
| Recent | The `touched_paths_for` list Artifacts already reads, plus newest-modified files from the same roots. |

Where something new is needed (the directory listing itself, the Files
page, the clipboard and share handling), it is added beside these rather
than in place of them. Listing is gated exactly like reading: no second
file-serving path, storage or auth.

## 3. Order and size

| Item | What | Size |
|---|---|---|
| R-1 | Listing, roots, recent; mesh op | M |
| R-2 | Files page, menu and palette entries | M |
| R-3 | Copy (PNG, text, Aspen reference), share, drag | S–M |
| R-4 | Linkify served paths in replies | S |

Proposed: all four in one release.

## 4. Questions for the operator

1. **Temp in the roots?** `/tmp` is shared by everything on the machine
   and is already servable. Proposed: list it as a root, filtered to
   files modified since the session started, so it shows what this
   session made rather than everything in /tmp.
2. **Name:** "Files" (proposed), or Explorer or Navigator.
3. **The Aspen file reference on copy:** pasting into an Aspen composer
   attaches the file (proposed). Should pasting into a non-Aspen app also
   get the file's text (for text files) or its path (otherwise)? Proposed:
   yes, since the clipboard can carry both.
