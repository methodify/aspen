# Proposal 2026-11-C: Where a session works, not only where it started

**Status:** accepted 2026-11-22: C-2 automatic, C-3 for agent, repo and project (answers in §4); shipped as v0.56.0.

## 1. The situation

More and more sessions start in a pilot repo and do their work elsewhere: a
git worktree, a peer folder beside the repo, a scratch clone. On this node,
`@main` of `~/src/cyberdeck` grew a second track, and its agents agreed that
the second one works in `~/src/cyberdeck-hw`, next to the original. The
operator could not open what it wrote for them.

The **serving rule** (artifacts.rs; ACTIVITY.md, PROPOSALS-2026-09 §3) lets
the console open a path for an agent when it is under the agent's repo, the
session's own data, temp or attachments, or **named by a tool call in its
transcript**. Reading the cyberdeck transcripts shows three gaps:

| What happened | Why it was refused |
|---|---|
| A subagent wrote `cyberdeck-hw/hardware/sourcing/prototype-buylist.md` (Write, Edit) | Only the main transcript is scanned; subagents' (`subagents/agent-*.jsonl`) never were. |
| The main agent changed `params.py`, `case.py`, `plan.py` with `cd …/cyberdeck-hw/hardware/cad && sed -i …` / `python3 - <<EOF` | Shell work names no file to the tool-call scan. |
| It then wrote "see `~/src/cyberdeck-hw/hardware/sourcing/prototype-buylist.md`" | A path in prose is not a reason to serve (rightly, alone). |

## 2. The design

The rule stays a rule: it never becomes "the whole disk". It learns where the
session **works**.

### C-1 Subagents count

Files named by tool calls in the session's subagent and workflow transcripts
(`<session>/subagents/**/agent-*.jsonl`) count like the main transcript's.
They are the session's work.

### C-2 Work trees the session worked in

A **work tree** is the top of a git checkout (a folder holding `.git`, a
directory or a worktree's file). A session **worked in** one when:

- a Write, Edit or NotebookEdit call (main or subagent) named a file in it; or
- a Bash call ran in it: `cd <dir>`, `git -C <dir>`, `pushd <dir>` at the
  start of a command, or the line's recorded `cwd`; or
- it is a **worktree of the agent's repo** (`git worktree list`), wherever it
  lives.

Every such work tree is served whole, as the repo is. A path that is in no
work tree (a home folder, `/etc`) never becomes a root this way: a work tree
must be a git checkout, and never the home folder itself or above it.

This is the "it just works" part: cyberdeck-hw is a checkout, the agent
`cd`'d into it and its subagent wrote there, so the whole folder opens, with
the files it made by shell as well.

### C-3 Roots the operator adds

Some work is not in git (a data folder, a `~/scratch/x`). The operator can
add **extra roots** to a repo, a project, or one agent:

- In a refused file's message, *serve this folder for @agent* (the folder
  the file is in, or a parent the operator picks).
- On the Files page, *+ add a folder* beside the roots.
- On a repo's row (Mesh view) and a project's Files view: the folders every
  agent of that repo, or every member of that project, may be served from.

Extra roots are stored on the node that serves them; a project's are synced
with the project (a field, merged like the rest) and apply on each member's
node.

### C-4 Seeing it

The Files page lists every root with its reason: *repo*, *worktree of the
repo*, *worked in (subagent wrote …)*, *added by you*, and the session data,
temp and attachments. A refusal says which rule would serve the file, and
offers C-3 in one click.

### Not now

- Following symlinks out of a served root: a link inside a root to outside
  it is still refused (as now).
- Serving a path only because a reply names it.

## 3. Order and size

One release (v0.56): C-1 (S), C-2 (M), C-3 (M), C-4 (S).

## 4. Questions for the operator (answered 2026-11-22)

1. C-2 serves a whole work tree once the session wrote or ran a command
   there: **yes, automatically**.
2. C-3: **agent, repo and project**, all three.

## 5. As built

- `artifacts::space` builds the roots: the fixed ones, `git worktree list`
  for the repo (trusted only when the answer names the repo itself; a folder
  whose `.git` git does not take otherwise got the enclosing checkout's
  list), the work trees from the session's and its subagents' writes and
  shell directories, and the added folders. What a session named and where
  it worked is cached until any of its transcripts changes.
- Repo-scope and agent-scope folders live in the node's `served_roots`
  table; a project's in its synced `roots` field (`[{node, path}]`).
- The C-3 choices sit on the refused file (*serve <folder> for @agent ·
  #repo · each project*) and on the Files page (*+ add a folder*), not on
  repo rows: one place to decide, where the need shows.
- On the real `~/src/cyberdeck` session: `cyberdeck-hw` (and
  `cyberdeck-live`) are git worktrees of the repo, now served; the
  subagent's buy list and the shell-edited `params.py` open; `~/.ssh/config`
  stays refused.
