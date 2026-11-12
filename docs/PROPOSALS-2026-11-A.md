# Proposal 2026-11-A — Projects: a place for work, not a split screen

**Status:** accepted 2026-11-11 (answers in §4); tranche 1 shipped as v0.53.0 (BOARDS.md §12, SESSION_BAR.md, API.md). As built: rail ordering is drag on the Projects page; effort is Claude-only for now (Codex sets reasoning effort differently). The hero is
**projects**; riding along are effort, the workflow viewer, and which
projects the rail shows. A-12 is off the slate (backlog): what the
operator wants is a durable pause that keeps unfinished agents' state,
which the harness does not offer yet.

---

## 1. Where boards ended up

Boards began as split screens: a layout of panes. In use they became
something else.
- **The team:** the bench and *a board is a team* made a board's members
  aware of each other.
- **The roster:** tabs made a board a list of people, not a picture.
- **The project area:** the operator now opens a board to work on a
  project, not to arrange windows.

What a project needs and a board does not have:
- **Something it is about.** A name is not a mission. Members join without
  knowing what the work is, unless each one's charter repeats it.
- **Shared things.** A spec, a decision, a file the whole team needs: today
  it lives in one agent's repo, on one node, and reaches the others by
  hand.
- **A quiet way to say who joined.** A board notice wakes every member for
  one turn, to read news that could have waited for its next turn.
- **A way to step back from one.** Every board sits in the rail forever.

## 2. The design

### A-1 A project is a board, grown up

Every board becomes a **project**, keeping its id, its sync (last writer
wins per field, §2 A-9) and everything it has now. The split layout
becomes the project's **desk**: one way of looking at its people, beside
the others below.

```
Project {
  id, name, updated_at, deleted,
  status: active | paused | archived,
  charter?:  text   // to every member (A-2)
  notes?:    text   // the operator's; never shown to agents
  desk:      the layout, bench, tab order, pairs   (as boards have now)
  query?:    a dynamic project's fleet query (as now)
}
```

- **Members** are the desk's panes and its bench, as today. The bench is
  how a member steps back without leaving.
- **Status:**
  - *active* is the default.
  - *paused* keeps the members, sends no lifecycle notices, and drops the
    project from the rail.
  - *archived* also makes it read-only.
- **The Boards page becomes Projects.** The word *board* stays for the
  desk, so nothing the operator knows is lost.

### A-2 A charter for the whole project, in layers

A project's **charter** sets the tone for everyone on it: what the work
is, how the team works, what *done* means. It layers with what exists:

```
the harness's system prompt
  + each project's charter       (one block per project the agent is on)
  + the agent's own charter      (its role on the team)
```

- At spawn, each project an agent belongs to adds a block: the project's
  name, its charter, and the team (names, roles, nodes).
- A charter change reaches running members as an **ambient** notice (A-3).
  They read it on their next turn; no turn is spent on it.
- The blocks are capped (8 KB per project) so a long charter cannot crowd
  out the agent's own.

### A-3 Ambient notices: news that waits for the next turn

Today every bus row wakes its recipient: the delivery engine writes it at
once, and an idle agent takes a turn to read it. A new class of row,
**ambient**, never wakes anyone:

- **Delivery:** an ambient row stays pending until something else is
  written to the agent: a bus message, an operator message, its next
  start. It then goes out first in the same write, in order, as pending
  rows already do. A busy agent gets it mid-turn, as it would any message.
- **What is ambient:**
  - project lifecycle notices: joined, left, benched, charter changed,
    files changed, paused, archived;
  - the K-3 board notices, which become ambient.

  The operator can still send an announcement that wakes.
- **Digests:** several ambient notices for one agent fold into one
  block, so a busy week is one paragraph, not twenty.
- **Wire:** `messages.urgency = "ambient"`. `delivery::attempt` writes
  nothing while everything pending is ambient.
- **Aspen's only.** Agents cannot send ambient (the operator's call):
  agents are polite to each other and would choose the quiet way when
  the recipient needs interrupting. Aspen knows by design which of its
  own notices can wait.

### A-4 What an agent needs to know, and how it asks

Thinking from the member's side, it **needs**:
- what the project is (the charter);
- who else is on it, what each does, and where they run;
- what changed while it was away;
- where the shared things are.

It **wants** to:
- tell the team something once, not five times;
- leave a file where the others will find it;
- know when it is being asked for, not only spoken to.

The Aspen MCP server gains project tools. They follow the bus tools'
style, and each answers only for projects the agent is on:

| Tool | What it gives |
|---|---|
| `project_list` | the agent's projects: name, status, charter (short), team size |
| `project_get` | one project: charter, team (name, role from its charter's first line, node, state: running · idle · stopped · benched), recent activity |
| `project_post` | a message to the whole team on the project's thread (`project:<id>`), delivered like any bus message (ambient is Aspen's, §A-3) |
| `project_files` / `project_read` / `project_write` | the project's files (A-5) |

- **Team view:** `bus_status` gains a *projects* section, as it shows
  board-mates now.
- **Addressing:** the project's thread is a channel on the bus, so a team
  message shows in Flow and in the project's activity (A-6).

### A-5 Project files

A small shared space per project for things the whole team needs: specs,
decisions, a glossary, a screenshot, a hand-off note.

- **Who can use them:**
  - The operator reads and writes them from the console.
  - Members read and write them through the tools.
  - Paths are relative; no executables run from here; 64 MB per project.
- **Where agents find them:** a link in each member repo,
  `<repo>/.aspen/projects/<slug>/` (a symlink, or a directory junction on
  Windows, which needs no admin or developer mode), kept out of git by
  `.git/info/exclude` (local to the clone; no tracked file is touched).
  Agents use their own file tools on it. The project notice names the
  folder too, for an agent whose repo has no link.
- **Watched:** writes through the link bypass Aspen, so each node watches
  its project folders; a change becomes a new version, a sync and an
  ambient digest.
- **Where they live:** every member node keeps a copy.
  - The store holds **content-addressed blobs** (sha256) and a
    **manifest** of path → blob, author, time and previous versions.
  - Nodes sync the manifest with the project and pull the blobs they
    lack, in pieces, as transfers do (U-2).
  - A member on a node that is offline still reads what it had. A write
    while offline lands when the node is back.
- **Conflicts:** two writes to one path while apart are both kept. The
  later one takes the name, the other becomes `name (from @x).ext`, and
  the operator sees a *both kept* mark. No silent loss.
- **History:** the last 10 versions of a path, and restore.
- Writes reach members as one ambient digest: *"specs/api.md changed
  (@arch)"*.

### A-6 The project's home page

`/project/{id}` opens to the **desk**, as a board does now (layout or
tabs). Beside it are four more views, one click away:

| View | What it shows |
|---|---|
| **Desk** | the board as today: layout or tabs, bench, pairs, broadcast |
| **Team** | the members, stage and bench: role (the first line of each charter), node, state, last worked, waiting pips; *add a member* (the + picker), *bench*, *remove* |
| **Charter & notes** | the project charter (what agents see) and the operator's notes (what only the operator sees), edited in place |
| **Files** | the project's files (A-5), as the agent Files page shows a repo |
| **Activity** | one timeline: team-thread messages, joins and leaves, charter edits, file changes, members' workflow runs and their outcomes |

- **The header:** the project's name, status, meter and counts, and
  *start the stopped*.
- **Usage:** what the members spent, from Usage, for the project as a
  whole.

### A-7 Which projects the rail shows

- **Default:** the rail shows the projects the operator *keeps there*. A
  new project goes in; *hide from the rail* takes one out. Paused and
  archived ones never show.
- **Where it is set:** the Projects page lists every project with a
  *show in rail* toggle and drag to order. The same toggle sits in each
  project's ⋯ menu.
- **Where it is kept:** synced with the project (shown, order), so the
  rail is the same on every console and the phone (the operator's call).
- **Waiting is never hidden:** a hidden project with a member waiting on
  the operator still shows its red pip, in a *hidden · 1 waiting* line at
  the foot of the section.

### A-8 More to reach for (proposed for the second tranche)

- **Start a team from a template.** Templates already spawn agents; a
  project template carries a charter, roles (agent charters) and the
  desk.
- **A briefing for a new member.** On joining, its first ambient notice
  carries the charter, the team, the file list and the last few
  decisions from the activity: up to speed without asking anyone.
- **Decisions.** `project_post` with `kind: decision` pins a post to the
  top of Activity and into the briefing.
- **Search within a project:** its members' transcripts, files and team
  thread.

### A-9 Sync: field-level last-writer-wins now, Automerge only if text demands it

Who writes a project: the operator (one console at a time, in practice),
and **nodes, automatically**: a rename or a name fold rewrites the desk,
the bench and the tab order on the node where it happens; a template
spawn places agents. Agents write no metadata (their posts are
append-only, their files have A-5's rule). So concurrent writes to one
project do happen, between the operator and a node, and a whole-row
last writer loses one of them.

Boards sync last-writer-wins per **row**: an edit to the layout on one
node and to the name on another, close together, lose one of them.

- **Fields:** projects add fields people edit apart (charter, notes,
  status), so this moves to last-writer-wins per **field**, each with its
  own hybrid-clock stamp. This is a small change to the row's shape, and
  merging stays a comparison.
- **Text:** the case Automerge solves is **two people editing one text at
  once**. Here the operator writes the charter and notes alone; agents
  write files, not the charter.
- **Recommendation:** field-level LWW now. Files have their own rule
  (A-5). Automerge (`automerge` for Rust, `@automerge/automerge` for the
  console) waits until a text is truly co-edited. Choosing it then
  changes only that one field's type.

### A-10 Effort

Claude Code takes `--effort low|medium|high|xhigh|max` at start and
`/effort` while running (it is on the headless list). Codex has a
reasoning effort per turn.

- **Seeing it:** the model readout in the session bar becomes
  *opus-4-8 · high*. Hover says which default it came from: the model's,
  the repo's, the node's.
- **Setting it:** the ⋯ menu's *model* row becomes *model & effort*.
  - It is a choice of the model's levels plus *default*.
  - On a running Claude session the choice is sent as `/effort <level>`.
  - On a stopped one it is kept for the next start (`--effort`).
  - Runtime defaults (the Mesh page) gain an effort per runtime and node,
    as they hold CLI arguments now.
- Workflows' agents keep their own (the script's `effort`). The workflow
  view shows each agent's effort beside its model.

### A-11 The workflow viewer, for big runs

- **It remembers where you were.**
  - The selected phase and the page's scroll go in the URL
    (`?phase=Review#agent-a1b2`).
  - A reload, a link, or coming back from an agent's transcript lands on
    the same phase and the same row.
- **The way back:**
  - An agent's transcript, opened from a workflow, carries
    *← workflow tiller-build-v2 · Review*. It returns to that phase,
    scrolled to that agent.
  - The browser's back works too, now that the state is in the URL.
- **The script while it runs:** the node finds a running workflow's
  script path from the launch result, which names the script
  (`Script: …`), or from the call's `scriptPath`. It no longer waits for
  the run's state file.
- **Big runs:**
  - A phase with many agents gets a filter (*running · failed · done ·
    text*). The same filter keeps the page short for a phase of 50.
  - The counts on each phase tab become links to that filter.

### A-12 Pausing a workflow (from the 2026-11-06 notes)

Claude Code's pause is terminal-only; the headless protocol has none.
What Aspen can offer is **stop for later** and **resume**:
- *Stop for later* stops the run's task.
- *Resume* relaunches it with `resumeFromRunId` and the original inputs.
  Finished agents return from the journal; agents that were mid-task
  start over.
- After a reboot, the viewer offers *resume* on a run cut off midway.

Verifying the stop on a real run comes first; the operator's call whether
this rides in this slate.

## 3. Order and size

**Tranche 1 (v0.53): a project you can work in.**

| Item | What | Size |
|---|---|---|
| A-1 | Boards become projects: status, charter, notes, the Projects page | M |
| A-2 | Project charter layered into spawn; ambient updates | M |
| A-3 | Ambient notices; K-3 notices become ambient | S–M |
| A-6 | Project home: desk, team, charter & notes, activity (thread and lifecycle) | L |
| A-7 | Rail: show/hide and order per console, waiting never hidden | S |
| A-9 | Field-level LWW for projects | M |
| A-10 | Effort: shown, set live or for the next start, runtime defaults | M |
| A-11 | Workflow viewer: URL state, the way back, the script while running, filters | M |

**Tranche 2 (v0.54): a project the team shares.**

| Item | What | Size |
|---|---|---|
| A-4 | Project tools for agents; the team thread | M |
| A-5 | Project files: blobs, manifest, sync, versions, both-kept | L |
| A-8 | Templates, briefing, decisions, project search | M |
| A-12 | Stop for later / resume (if the operator wants it here) | M |

## 4. Questions for the operator (answered 2026-11-11)

1. Boards become projects, the semantics recast as described: **yes**.
2. Ambient by default for lifecycle notices: **yes**, and ambient is
   **Aspen's alone**; agents' messages wake as now.
3. Files on every member node: **yes**, linked into member repos
   (`.aspen/projects/<slug>/`, excluded locally), the notice naming the
   folder as well.
4. Field-level LWW now, Automerge later if text is co-edited: **the
   architect's call**, after the operator's challenge (§A-9: nodes do
   write projects concurrently; agents do not).
5. The rail's shown projects and order: **synced**, as boards.
6. The operator's notes never reach agents: **yes**, for now. Beyond
   this slate the operator wants **project memory**: a memory the members
   share across nodes and repos, as native-feeling as each harness's own,
   visible to and authorable by the operator (backlog M-1).
7. Workflow stop/resume: **backlog** (W-1), until the harness can pause
   with unfinished agents' state kept.
