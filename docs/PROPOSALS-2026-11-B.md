# Proposal 2026-11-B: Starting agents where the work is

**Status:** accepted 2026-11-13 (the operator asked for it as the next slate, design and build); shipped as v0.55.0, with the Team view desk checkbox asked for during the build.

## 1. The situation

The operator works a project and needs a specialist on another node: a D365
developer on the node that has the tooling, say. That agent does not share
the project's repo. It needs a folder of its own, often one that does not
exist yet. It starts the same way every time: the same two plugins, the same
charter, the same opening words ("we are part of a bigger project; read
these skills; then wait for me").

Today that takes four trips:

1. Make the folder on that node, outside Aspen (a shell on the machine).
2. Add it as a repository (Mesh → Add repository; the path must exist).
3. Start the agent (the repo row's *new session*: a name, nothing else).
4. Type the opening message, and put the agent on the project.

Session templates (PLUGINS.md §7b) already carry plugins, model, charter,
runtime, permission and a board. But they are mesh-wide only, they have no
opening message, and the Mesh view's start does not offer them.

## 2. The design

### B-1 A folder that is not there yet

- **Add repository** (Mesh → repositories, any node) takes a path that
  does not exist: it says *not there yet*, and *create it* (default on)
  makes it, parents included, on that node. *Make it a git repo* (off by
  default) runs `git init` there. The specialist's folder is a workplace,
  not necessarily a repo.
- **The path picker** gains *new folder…*: a name, made inside the folder
  shown, then opened. On any node (`fs_mkdir`, the same trust as adding a
  repo).
- Both refuse a path that exists as a file, and never touch an existing
  folder beyond registering it.

### B-2 Templates that know where they belong, and what to say first

A session template gains three fields:

| Field | What it does |
|---|---|
| `node` | The node it is for. Empty: anywhere (mesh-wide, as now). Set: offered only there, and the node refuses to start it elsewhere. |
| `prompt` | The **starter prompt**: sent as the operator's first message once the session is up. |
| `effort` | The effort it starts at (Claude), as a project role already can. |

- The starter prompt fills in `{{name}}` (the agent), `{{node}}`, `{{repo}}`
  (the handle), `{{project}}` (the project's name) and `{{charter}}` (the
  project's charter), when it is started into a project.
- Templates still sync mesh-wide (one place to edit them, any console); the
  node scope is a property, not a storage place.
- **Eligible here** means: its `node` is empty or this node, and its `repo`
  is empty or names this repo (handle, basename, path).

### B-3 One start dialog where you start things

The Mesh view's *new session* on a repo becomes a small dialog:

- **name** (as now);
- **template**: the ones eligible for this node and repo, *none* first; one
  scoped to this node is marked *this node's*;
- **starter prompt**: filled from the template, editable, and usable with
  no template at all;
- **join a project**: optional; the new agent is put on the project's desk
  (and so on its team, with the joining briefing) and `{{project}}` is
  filled;
- runtime and effort, folded under *more*, filled from the template.

The plain start (`POST /api/agents`, here or on a peer) takes `prompt` and
`project` too, so the dialog works with or without a template.

### B-4 What it made us reach for

- **Add, then start.** After adding a repository, its start dialog opens
  on it: make the folder, register it and start the specialist in one go.
- **Save as template** from a running agent's ⋯ menu: its runtime, model,
  effort, charter and the plugins it runs, scoped to its node, with a
  starter prompt to fill in. The template editor opens on it. A good
  agent becomes a starter in one step.
- **Templates in the editor** show and set the node scope, the starter
  prompt and effort; the list says where each belongs.
- Now's new-session panel and the palette list a node-scoped template with
  its node, and start it there.

### Not now

- Per-node plugin availability checks (a template naming a plugin a node
  does not have): the spawn already reports a plugin it cannot resolve.
- Templates that create their own folder by pattern (`~/work/{{name}}`):
  worth it once B-1 is in hand; backlog.

## 3. Order

One release (v0.55): B-1, B-2, B-3, B-4 together. They are one flow.
