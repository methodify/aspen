# Proposal U — Moving repos around the mesh: pick a path, send a file, transfer a repo

**Status:** accepted 2026-10-27; shipped as v0.52.0 (BUNDLES.md §5, BOARDS.md tab order). The operator's calls: the picker spans
the whole filesystem on a full-policy mesh; a transfer shows the plan
and waits for a confirm; the source's bundle is deleted after a
transfer; chunked download is in. U-4 (reordering a board's tabs) was
added to the slate.

---

## 1. What prompted it

The import form asks for "a path on the node (the relay cannot carry the
file)". To import, the operator has to know the bundle's full path on the
target node, type it, and first get the file there by other means (scp, a
sync folder, a drive). Export has the same shape on the other side: it
writes into the node's exports folder, and the download link works only
on a direct connection.

Everything needed to do better is already built:
- the Files page lists directories, over the relay too (`file_list`, R-1);
- session move pulls a staged bundle from a peer in 256 KB pieces
  (`bundle_read`, `pull_session`);
- big frames travel in pieces through the relay, both ways (v0.27.3,
  v0.50.7);
- export and import already run on any node of the mesh (`bundle_verb`
  proxies them).

## 2. The design

### U-1 Pick a path on a node

A **path picker**: a small browser of a node's filesystem that lists
directories and files (name, size, modified time) and returns a full
path. It never opens a file.

- **Where it appears:** every place that asks for a path on a node.
  - Import's *bundle* field. Files ending `.aspen-repo` are shown first;
    other files are dimmed.
  - Import's *into* (a folder).
  - Export's *write to* (a folder).
  - *Add repository* on the Mesh page (a folder).
- **Starting points:** home, the node's exports and imports folders, the
  repos it knows, and the filesystem roots (`/`, or each drive on
  Windows). Breadcrumbs and a filter work as on the Files page. Dotfiles
  are hidden by default, with a toggle to show them.
- **Wire:** `GET /api/fs/list?node=&dir=` returns
  `{dir, parent, entries: [{name, kind, size, mtime}], truncated}`. The
  mesh op is `fs_list`. At most 2,000 entries per directory, like
  `file_list`.
- **Who may list:** a peer whose mesh grants it **full** policy, plus the
  node's own console. The op is classed *control*, not observe, so an
  observe-only mesh is refused.
  - The reasoning: a full-policy peer can already start a session on this
    node, and a session reads anything the node can. Knowing which files
    exist adds nothing that peer could not already learn.
  - What the picker withholds is the contents: it never reads a file. On a
    node you only let others observe, it is refused outright.

### U-2 Send a file from the console, and fetch one back

- **Upload in pieces:**
  - `POST /api/repos/import/upload/{id}?offset=` appends raw bytes, 4 MB
    per request.
  - `GET` on the same path answers the size so far, so a dropped upload
    resumes where it stopped.
  - `POST …/{id}/done` moves the finished file into the node's imports
    folder.
  - Pieces of that size ride the relay (fragmented, as since v0.50.7), so
    *choose a file* works over the relay and on any node. A node that is
    not the console's own gets each piece through the mesh op
    `upload_chunk`.
- **Download in pieces:** `GET /api/repos/export/download?file&offset&len`
  serves a range. The console joins the ranges into a file it hands to the
  browser, so *download* works over the relay too. The current
  single-request download stays for direct connections.
- The import form gains a **choose a file** button, beside the path field
  and its picker. It shows progress, then fills the bundle path.

### U-3 Transfer a repo to another node

The export panel on a repo row gains a destination: **write to** (as
today) or **transfer to**: a node, a path there (the picker), *a new
repo* or *top up*, and *register the names*. Both nodes must be up at
that moment; the console says so when one is not.

The target node runs it, as it runs a session move:

1. **Export.** The target asks the source to export with the panel's
   choices (op `repo_export`, as today).
2. **Pull.** The target pulls the file from the source's exports folder
   in pieces (new op `export_read`: by file name, exports folder only)
   into its own imports folder.
3. **Plan.** The target stages the bundle and makes the import plan
   (`repo_import_preflight`).
4. **Confirm.** The console shows the plan, the same view import uses:
   sessions, memory, names, warnings. The operator confirms.
5. **Install** (`repo_import`). The source's copy of the bundle is then
   deleted; the target's stays in its imports folder, like an upload.

- **A transfer is a job.** `POST /api/repos/transfers` starts it, and
  `GET /api/repos/transfers/{id}` reports its step, bytes so far and the
  plan, or the error. The console polls it, so the transfer survives a
  closed panel or a reload.
- **Nothing is installed without the confirm.** A failed step leaves the
  source repo untouched and the target with at most a staged bundle,
  which expires like any staging.
- **Sealed transfers** are optional. Across the operator's own mesh the
  bundle travels inside the mesh's sealed envelopes either way, so the
  passphrase field is offered but not required.

### U-4 Reorder a board's tabs

A board gets its own **tab order** (`Board.tab_order: [agent…]`,
synced with the board like the bench). Tabs follow it, and members it
does not name follow in layout order. The layout itself does not move:
reordering a split tree by dragging tabs has no single right answer
(where does a pane go?), and the operator was fine with tabs and layout
ordering independently.

- **Drag a tab** along the strip. An insertion bar shows where it will
  land; drop to place it. Stage tabs reorder among stage tabs, benched
  ones among the bench (the bench keeps its own section).
- **Keyboard:** `alt+shift+[` and `alt+shift+]` move the active tab left
  and right.
- `alt+1…9` follow the new order. A board saved by an older console or
  node keeps the stored order, as the bench does.

## 3. Built on what exists

| Piece | Reuses |
|---|---|
| The picker | the Files page's list, breadcrumbs, filter and row styling; `artifacts::list_dir`'s entry shape and caps |
| Who may list | `op_capability` (control) and the mesh policy, as every op |
| Upload pieces | `post_bundle_upload`'s streaming write, the tunnel's request fragments |
| Download pieces | `get_bundle_download`'s file checks, `file_read`'s ranged reads |
| Transfer pull | `pull_session`'s chunk loop and size checks, `bundle_read`'s safe paths |
| Plan and install | `repo_import_preflight` and `repo_import` and their console view, unchanged |

New: `fs_list`, the chunked upload and download routes, `export_read`,
the transfer job, the picker component, and the *transfer to* option.

## 4. Order and size

| Item | What | Size |
|---|---|---|
| U-1 | Path picker: `fs_list`, component, four call sites | M |
| U-2 | Chunked upload and download, *choose a file* | M |
| U-3 | Transfer job, `export_read`, *transfer to*, confirm in the plan view | M–L |
| U-4 | Tab order: field, drag with an insertion bar, keys | S |

Proposed: one slate, one release (v0.52), in that order.

## 5. Questions for the operator (answered 2026-10-27)

1. Picker scope: **the whole filesystem** on a full-policy mesh.
2. Transfer: **show the plan and wait for a confirm**.
3. The source's bundle after a transfer: **delete it**.
4. Chunked download to the browser: **include it**.
