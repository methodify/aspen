# Repo bundles: a repo with its context, as one file

**Status:** reference for what is built (2026-09-28, v0.47: X-1..X-5 of
PROPOSALS-2026-09-O.md §1). Code: `crates/aspen-node/src/repobundle.rs`
(format, export, stage, plan, install, sealing), the `repo_export*` /
`repo_import*` mesh ops in `federation.rs`, the routes in
`crates/aspen/src/api.rs`, `aspen repos export|import` in `main.rs`,
`ui/src/bundles.tsx`.

## 1. Why

Code travels by git; the context an agent built — its transcripts, their
tool results and subagent/workflow transcripts, project memory, and the
Aspen names on them — is keyed by absolute paths and trapped on its
machine. Meshes stay separate by design, so this is a **file**: export
one on one side of any gap (two meshes, an air gap, a colleague), import
it on the other. It is deliberately not git: transcripts are large and
more sensitive than code.

## 2. The file (`.aspen-repo`)

A gzip'd tar:

| entry | what |
|---|---|
| `manifest.json` (first) | source node and its path anchors (`PathCtx`), repo basename, handle, origin, branch, HEAD, the options chosen, every session (id, harness, title, name, sizes), every file planned, notes, Aspen version |
| `repo/…` | the working tree per the chosen mode — not rewritten (it is code); symlinks kept |
| `context/claude/<id>.jsonl` | each transcript, **canonical** (the rules of MIGRATION.md §2: `{{REPO}}`, `{{CLAUDE_PROJECT_DIR}}`, `{{ENCODED_DIR}}`, `{{HOME}}`, `{{TMP}}`, boundary-anchored, JSON leaves and keys, Windows dialects) |
| `context/claude/<id>/…` | its sidecar folder: tool results, subagent and workflow transcripts (jsonl canonical, text canonical, binary as is) |
| `context/claude/memory/…` | project memory, canonical |
| `context/codex/…` | Codex rollouts whose cwd is the repo, under their date paths, canonical |
| `aspen/agents.json` | the repo's names on the chosen transcripts (not pending forks — one name per transcript): charter, title, harness, extra args, bookmarks; lineage between chosen sessions. On import, a transcript that already carries a name here keeps that name, and the bundle's is not registered (the preview says `already here under this name`) |
| `sums.json` (last) | size + sha256 of every file; the import verifies them all |

Text is rewritten as bytes: where a file is not UTF-8 throughout (Claude
cuts long tool output at a byte limit, sometimes inside a character), the
UTF-8 runs are canonicalized and the other bytes travel as they are, both
ways. Before v0.50.5 such a file was skipped as unreadable.

The manifest is written before any file is read, so it can list a file
the export then could not read (its notes say which). The import drops
such a file from the plan and says so as a warning before you confirm;
`sums.json` is the authority: a file it vouches for that the bundle lacks
is damage, and refused. A bundle made by v0.50.4 or earlier that lists a
skipped file imports this way too.

Canonicalized text entries carry the source file's modification time, so
an imported transcript keeps its age (the Mesh list sorts by it, and the
one-writer gate reads a just-written transcript as live elsewhere).

**Repo modes** (all offered; the default is the first):

- **tracked + history** — `git ls-files` plus `.git`.
- **+ untracked** — adds `git ls-files --others --exclude-standard`.
- **everything** — the whole directory, dotfiles and ignored files; the
  console warns that `.env`-style files come along and names the largest
  directories.
- **no files** — context only.

A directory that is not a git repository gets every file in the first
two modes too (noted in the manifest).

**Sealed** (optional): the gzip stream is encrypted —
`ASPENREPO-ENC1\n`, a 16-byte salt, scrypt parameters (log N 15, r 8,
p 1), a 16-byte nonce prefix, then chunks of the stream, each `u32
length ‖ XChaCha20-Poly1305(1 MiB)` under nonce `prefix ‖ u64 counter`
with the last chunk's associated data marked, so a truncated file is
refused. A wrong passphrase reads as *wrong passphrase, or the bundle is
damaged*.

## 3. Export

`POST /api/repos/export/preflight {path, node?}` answers the sizes of
each repo mode, the largest top-level directories, every session with
its name and sizes, memory, the number of names. `POST /api/repos/export
{path, node?, repo_mode, sessions?, sidecars, memory, names,
passphrase?, out?}` writes the bundle **on the repo's node** — to `out`
(a file or a directory there), else `<data-dir>/exports/<repo>-<stamp>.aspen-repo`
— and answers `{out, bytes, files, sessions, sealed, notes, file?}`;
fleet event `repo_exported`. `GET /api/repos/export/download?file=<name>`
streams a file from the exports dir.

Console: the repo row's **export…** — modes with file counts and sizes,
sessions (all ticked, with names), sidecars / memory / names, seal with
a passphrase, *write to*, a running size estimate; then **download** when
the console reaches the node directly (this node, not through the
relay), otherwise the path on the node and how to move it.

CLI: `aspen repos export <path> [-o file|dir] [--repo tracked|untracked|all|none]
[--sessions id,id] [--no-sidecars] [--no-memory] [--no-names] [--seal |
--passphrase-env VAR]`.

## 4. Import

`POST /api/repos/import/upload` (raw body, direct connections) stores a
file in `<data-dir>/imports/` and answers its name.
`POST /api/repos/import/preflight {file, node?, passphrase?, target, mode,
repo_files, names}` unpacks it into staging (verifying every checksum)
and answers the **plan**; `POST /api/repos/import {staging_id, …}`
carries it out and answers the report; fleet event `repo_imported`.
Staged bundles nobody confirms are swept after a day.

- **New** (`mode: new`): the target must be absent or empty. The tree is
  written (with `.git`, so history and branches are there), the repo
  registered, every transcript localized to the new path.
- **Top up** (`mode: top_up`): the repo must exist. Its files are left
  alone unless `repo_files: missing` (write files it lacks; never its own
  `.git`). Context merges:

| the transcript here | what happens |
|---|---|
| absent | installed |
| absent, but a name on this node holds that session id in another repo (a copy of a repo on the same node) | installed **under a new id**, with lineage — the one-writer rule never sees two copies of one id |
| the same conversation, further along in the bundle | replaced; the local copy kept as `<id>.jsonl.bak-<time>` |
| the same, further along here | kept |
| diverged (neither is a prefix of the other) | the incoming one installed **as a fork** under a new id, lineage to the local one at the last common message |

  Transcripts are compared by line identity (Claude's `uuid`, else a
  line's timestamp and type) with the line count as the tie-break, so
  path differences between the copies do not matter. Sidecar files keep
  the bigger copy; memory keeps both on a difference
  (`<name>.from-<node>.<ext>`); Codex rollouts already here are kept.
- **Names** (optional): each lands as `bare@<target handle>`; one already
  here on the same transcript is left; one taken by another transcript
  lands as `bare-2`. Registered, **not started** — resume from the repo's
  row. A revive may meet the live-elsewhere question when the source was
  writing that transcript minutes before the export; *in place* is the
  answer for an import.
- The report lists files written, sessions installed / replaced / forked /
  kept, memory written and conflicts, names, and the **residue** — source
  paths the rewrite could not place (advisory).

Console: the Mesh list's **import a repo…** — node, the bundle (a file to
upload when direct, or a path on the node), passphrase, *a new repo* or
*top up a repo that is here* with the path, *write files this repo
lacks*, *register the names*; **preview** shows the plan (each session's
action, names, warnings, blockers); **import** carries it out.

CLI: `aspen repos import <file> --to <path> [--top-up] [--missing-files]
[--no-names] [--dry-run] [-y] [--passphrase-env VAR]` (asks for the
passphrase when the file is sealed).

## 5. Over the relay, a peer's repo, and from node to node (v0.52)

Every verb runs on the repo's node (`repo_export_preflight` observe,
`repo_export` control, `repo_import_preflight` / `repo_import` trust),
with a 30-minute wait. Since v0.52 (PROPOSALS-2026-10-U) the file itself
moves too, in 1 MB pieces, so none of it depends on a direct connection:

- **Choose a file** in import uploads from the browser to any node, over
  the relay too (`POST /api/repos/import/upload/{id}` `{offset, data,
  node?}`; `GET` answers the size so far, so an upload resumes; `…/done`
  makes it `upload-<id>.aspen-repo` in imports). A piece at the wrong
  offset is refused with where the upload stands.
- **Download** after an export fetches the bundle in pieces
  (`GET /api/repos/export/read?file=&offset=&len=&node=`, the exports
  folder by bare name only) and hands it to the browser. A direct
  connection to the console's own node keeps the one-request link.
- **browse…** next to every path on a node (the bundle, *into*, *write
  to*, and *add repository* on Mesh) opens the path picker: the node's
  folders, names only, never a file's contents (`GET /api/fs/list?node=
  &dir=`, op `fs_list`, classed control so only a full-policy peer or the
  node's own console may list). It starts from home, the node's exports
  and imports, its repos and the filesystem roots (each drive on Windows).
- **Transfer** (the export panel's *to: another node*) runs on the target
  node as a job (`POST /api/repos/transfers` `{to, from, path, …export
  choices, import: {target, mode, repo_files, names}}`, then
  `GET /api/repos/transfers/{id}?node=`): the source exports into its
  exports folder, the target pulls it in pieces (op `export_read`),
  stages it and makes the plan. The console shows the plan; **install**
  (`POST …/{id}/install`) runs the import, then the source's copy is
  deleted. Nothing is installed without that confirm; a failed step
  leaves the source untouched. `transfer_start` and `transfer_install`
  are trust, like import.

## 6. Verified (rig, 2026-09-28)

repo1 on the rig root (4 Claude sessions, 3 names): exported plain and
sealed from the CLI (no absolute repo path left in the bundle: 61
`{{REPO}}`); imported on the member as a new repo at another path — git
history intact, 59 paths localized to the new location, none of the old,
no sentinels, names registered; a wrong passphrase refused; a sealed
top-up of the same bundle: every session *nothing to do*, names *already
here*; a shortened transcript replaced (with `.bak`), a diverged one
installed as a fork with lineage and its own name; the console's export
(download byte-identical, a traversal attempt refused) and import (upload,
preview, import); an import on the **same** node installed the named
sessions under new ids, and an imported name resumed on its own
transcript in the new repo.
