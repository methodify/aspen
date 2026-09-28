# Proposal O — A repo with its context, as a file; and workflows you can see into

**Status:** exploration and slate, 2026-09-28. Nothing here is built.
Two asks from the operator; decisions for him in §3.

---

## 1. Repo bundles: export a repo with its sessions, import it anywhere

### 1.1 The ask, and what happened without it

A project researched and designed on the work mesh was "cleansed" and
moved to the personal mesh by tarring `~/src/pick-when` and
`~/.claude/projects/-…-pick-when` and untarring them on the other box.
The code travelled; the context mostly did — but the evidence shows the
seam: the personal box's project dir still holds session `2d74a9ce…`
whose every line says `/home/bryon/src/pick-when` (the work box's user),
and `3e9d371b…` (the live build session) carries the same paths in its
first days. A transcript copied by hand is a transcript keyed and
written for another machine. Meshes stay separate (no inter-mesh
features, by decision); the ask is a **file** that carries a repo *with
its context* across any gap — two meshes, an air gap, a colleague.

Session transcripts do not belong in git: they are large (pick-when:
154 MB of harness context against 8.5 MB of git history) and more
sensitive than code (tool outputs, file contents, secrets that passed
through a shell). So the bundle is its own artifact, beside git, not in
it.

### 1.2 What already exists

MIGRATION.md's bundle moves **one session**: `migrate.rs` canonicalizes
every path anchor (`{{REPO}}`, `{{CLAUDE_PROJECT_DIR}}`, `{{HOME}}`,
`{{TMP}}`, `{{ENCODED_DIR}}`; boundary-anchored, JSON leaves and keys,
Windows dialects, round-trip guard) and localizes on install; tiers A–E
cover the transcript, its sidecar folder (tool results, subagents,
workflows), project memory, artifacts written outside the repo, and an
optional `repo.patch`. `aspen session export/import` already write and
read it as a tar. The repo bundle is that, **for every session of a
repo at once, plus the repo itself.**

### 1.3 The bundle (`.aspen-repo`, a tar.zst)

```
manifest.json            source node, PathCtx, repo basename, origin URL,
                         branch + HEAD, harness versions, what was chosen,
                         sizes, sha256 per file, created_at, notes
repo/                    the working tree, per the chosen mode (below)
context/claude/          every transcript of the repo, canonicalized;
                         each sidecar folder (tool-results, subagents,
                         workflows); memory/
context/codex/           every rollout whose cwd is the repo, canonicalized
aspen/agents.json        the repo's names: agent rows (name, charter,
                         title, harness args, model), lineage, bookmarks,
                         which transcript each name is on
aspen/boards.json        (opt-in) boards whose panes are all in this repo
```

**Repo modes** (one choice):

- **tracked** — `git ls-files` plus `.git` (history and branches travel;
  `node_modules`, build output and ignored secrets stay home). Default.
- **tracked + untracked, honoring .gitignore** — adds new files not yet
  committed (`git ls-files --others --exclude-standard`).
- **everything** — the whole directory, dotfiles and ignored files
  included, with a warning that `.env`-style files come along, and the
  largest directories listed in the preflight so `node_modules` is a
  visible choice, not an accident.
- **none** — context only, for a repo that already exists at the far
  end via git (the common case once both sides can reach the remote).

**Context choices** (checkboxes, each with its size in the preflight):
transcripts (all, or a picked list — the Mesh list's rows, with names),
sidecar folders (tool results, subagent and workflow transcripts — the
bulk), memory, Aspen names and lineage.

**Sensitivity.** The export dialog says what a bundle holds in plain
words; an optional **passphrase** encrypts the tar (age-style: scrypt +
XChaCha20-Poly1305, which the wire crate already has), so a bundle can
sit in a shared folder. Import asks for it.

### 1.4 Import: new repo, or top up

On the Mesh list: **import…** takes the file, a target node, and a path.

- **New** — the path does not exist (or is empty): the tree is written,
  `.git` restored (then `git checkout` of the recorded branch), the
  repo registered on the node, every transcript localized to the new
  path into that node's harness store, memory written, names
  registered (not started — revive from the list, as `session import`
  does today).
- **Top up** — the repo exists there (matched by path, or by origin URL
  as `move` does): repo files are **not** written unless asked (git is
  the tool for code; an optional "write files that are missing here"),
  and context **merges** under convergence's rules:
  - a transcript this node lacks → installed;
  - the same session id on both → if one is a prefix of the other
    (the same conversation, further along), the longer wins; if they
    diverged, the incoming one is installed as a **fork** with a new
    id and lineage to the existing one (never two writers on one id);
  - memory → the existing merge (`<name>.from-<node>.<ext>` on
    conflict);
  - names → a name already on the node keeps its transcript; the
    incoming one lands as `name-2` pointing at its own.
- **Preflight** before anything is written: what is new, what merges,
  what forks, what collides, the residue report (source paths the
  rewrite could not place), sizes.

Nothing about this needs a mesh link between the two sides: export
downloads a file, import uploads one. Within one mesh, *export to node*
could skip the file (stream bundle → peer), but that is `move` for a
whole repo and waits for demand.

### 1.5 Transport and size

Bundles are big (pick-when, tracked mode, all context: ~160 MB before
compression). A direct connection streams it both ways. Through the
relay, a browser download of that size is the wrong road (sealed
frames, base64, a 64 MB gateway cap), so over the relay the console
offers **write to a path on the node** and the CLI verb:

```
aspen repo export <path|handle> -o pick-when.aspen-repo [--repo tracked|untracked|all|none] [--no-sidecars] [--sessions id,id] [--passphrase]
aspen repo import pick-when.aspen-repo --to ~/src/pick-when [--node <n>] [--top-up] [--dry-run]
```

### 1.6 Rows

| id | row |
|---|---|
| X-1 | `repo_export` / `repo_import` in `migrate.rs`: the manifest, the tar.zst stream, canonicalization of every session of the repo (Claude and Codex), sha256 per file, round-trip guard per transcript |
| X-2 | Import merge rules (new / top-up; longer-prefix-wins, fork on divergence, name collisions), preflight + dry-run |
| X-3 | CLI verbs; `POST /api/repos/export` (streams, or writes to a node path) and `POST /api/repos/import` (upload stream, or a node path), preflight `GET /api/repos/export/preflight` |
| X-4 | Console: repo row ⋯ → *export…* (modes, checkboxes with sizes, passphrase, download or write-to-path); Mesh list → *import…* (file, node, path, preflight readout, go) |
| X-5 | Optional passphrase encryption |

---

## 2. Workflows in the Activity viewer

### 2.1 What is broken today (confirmed on @build@pick-when)

1. **They never finish.** The ledger (`aspen-claude/src/activity.rs`)
   sets a workflow's id to the `wf_…` run id it finds in the launch
   result, but the completion notice names the **Task ID**
   (`<task-id>wsjwtbrwy</task-id>`); the two never match, so every
   workflow stays *running* until the process restarts and `settle_before`
   marks it *unknown*. All six of the session's runs completed hours ago.
2. **They are all called "workflow".** The name is read from the script's
   `name: 'pick-when-wave4'`, but the token reader stops at the quote and
   returns nothing. The launch result's `Summary:` line is never used.
3. **Resumes look like new runs.** `resumeFromRunId` relaunches the same
   run under a new Task ID; wave 3 appears three times.
4. **No way in.** The row expands to "output: no output yet".

### 2.2 What the harness gives us (Claude Code 2.1.281)

- **Live, on the stream:** `{type:"system", subtype:"task_progress",
  task_id, tool_use_id, description, usage:{total_tokens, tool_uses,
  duration_ms}, last_tool_name, summary, workflow_progress:[…]}` — Aspen
  receives these today and drops them as generic status frames. Also
  `task_started` and `task_notification`.
- **On disk, per run:**
  - `<session>/workflows/wf_<run>.json` — written at the end: `status`,
    `workflowName`, `phases[{title, detail}]`, `agentCount`,
    `totalTokens`, `totalToolCalls`, `durationMs`, `logs` (retries,
    stalls), `result[{key, report}]`, `script`, `scriptPath`, and
    `workflowProgress` — the same array the stream carries: a
    `workflow_phase` entry per phase and a `workflow_agent` entry per
    agent with `label`, `phaseTitle`, `agentId`, `model`, `state`
    (queued / running / done / failed), `attempt` + `lastAttemptReason`,
    `startedAt`, `lastProgressAt`, `tokens`, `toolCalls`, `durationMs`,
    `lastToolName`, `lastToolSummary`, `promptPreview`, `resultPreview`.
  - `<session>/subagents/workflows/wf_<run>/journal.jsonl` — appended
    live: `started {agentId, label, phase}` and `result {agentId, result}`.
  - `…/wf_<run>/agent-<id>.jsonl` + `.meta.json` — each agent's full
    transcript, live.
  - `<session>/workflows/scripts/<name>-wf_<run>.js` — the script.

That is everything the TUI shows: the run list, its phases, the agents
in a phase with model, time, tokens and last tool, and each agent's
transcript.

### 2.3 Design

**Ledger fixes (W-1).** Key a workflow by its Task ID (`Task ID:` in the
launch result); keep the run id (`Transcript dir: …/wf_<run>`, or the
`resumeFromRunId` input) as `detail.run_id`; name from the script's
`meta.name` (quoted) or the result's `Summary:`; description from
`Summary:`. A resume of a run already in the ledger folds into it
(`attempts: n`) instead of adding a row. **Second source for the end:**
when the run's `workflows/wf_<run>.json` exists, its `status`, totals and
duration are authoritative — so a run that finished while Aspen was
down, or across a restart, reads *completed*, not *unknown*.

**Live progress (W-2).** The node keeps the latest `task_progress` frame
per task id on the live session (in memory; small), so a running
workflow's phases and agents are current to the second without reading
files. When the process is gone, the state file and the journal answer
instead.

**One read for a run (W-3).** `GET /api/agents/{name}/workflows/{run}`
(and the `workflow` op for a peer's session) returns:

```
{ run_id, task_id, name, description, status, started_at, ended_at,
  duration_ms, total_tokens, total_tool_calls, agent_count, logs,
  phases: [{ index, title, detail, agents: [{ agent_id, label, model,
    state, attempt, retry_reason, started_at, last_progress_at,
    duration_ms, tokens, tool_calls, last_tool, last_tool_summary,
    prompt_preview, result_preview, has_transcript }] }],
  results: [{ key, report }], script_path, source: "live" | "state" | "journal" }
```

built from the live frame, else the state file, else the journal plus
the agents' meta files (labels, phases) and their transcripts' mtimes
(activity). The subagent transcript route learns the workflow layout
(`subagents/workflows/<run>/agent-<id>.jsonl`), so every workflow agent
opens in the existing subagent view.

**The view (W-4).** The Activity row for a workflow shows its name, its
phase ("Build · 7 of 9 done"), tokens and elapsed, and opens a
**workflow page** (`/session/<name>/workflow/<run>`, a sibling of the
subagent route):

```
pick-when-wave4                         completed · 53m · 9 agents · 4.37M tok · 1,238 tools
Wave 4: estimator+analytics, associates+clients UI, …
[ Build 9/9 ]                           ← phase rail; the selected phase below
─────────────────────────────────────────────────────────────────────────
● done   build:analytics-estimator        opus-5.5   26m  612k tok  144 tools  Bash · SP=/tmp/…
● done   build:associates-clients-ui      opus-5.5   26m  420k tok  137 tools  Bash · …
↻ done   build:offline-print-notify (2)   opus-5.5   23m  …         retried: stalled 1364s
…
logs     [stall] agent "build:offline-print-notify" stalled (no progress) after 1364s — retrying (1/5)
results  ▸ analytics-estimator   ▸ associates-clients-ui   …     (each report, markdown)
script   ▸ pick-when-wave4-wf_1e0966ef-8da.js
```

Running agents tick live (state, elapsed, tokens, last tool); a row
opens that agent's transcript; a result expands its report. On a phone
the phase rail becomes chips and rows stack. The fleet's Activity page
(`/activities`) lists workflows the same way.

**Controls (W-5, later).** The harness has `workflow_kill` and
`workflow_steer`; a *stop run* verb (through the same stop path as a
task) is cheap; steering waits for a harness API we can call.

### 2.4 Rows

| id | row |
|---|---|
| W-1 | Ledger: Task ID as the id, run id in detail, real name and description, resumes folded, the state file as the second source for the end |
| W-2 | Node keeps the latest `task_progress` per task on the live session |
| W-3 | `GET /api/agents/{name}/workflows/{run}` + `workflow` op; subagent route finds workflow agents |
| W-4 | Workflow page (phase rail, agents table, logs, results, script), Activity row summary, live refresh |
| W-5 | Stop a running workflow |

---

## 3. Decisions for the operator

- **(a) Order.** Recommended: **W-1 now** (it is a bug: the six runs
  you saw finished hours ago), then **W-2..W-4** as one release, then
  **X-1..X-4** as the next, X-5 after. Alternatively bundles first.
- **(b) Bundle repo default:** *tracked + .git* (recommended), or
  *tracked + untracked*?
- **(c) Top-up and repo files:** never write repo files on top-up unless
  asked (recommended — git carries code), or always write missing ones?
- **(d) Encryption** in the first cut, or after?
- **(e) Relay-attached consoles:** export/import through a node path
  and the CLI (recommended), or chunked download/upload over the tunnel
  (slower, but one road everywhere)?

A note on @build@pick-when: none of this needs it stopped. W-1 ships as
a daemon change, and relaunching the daemon revives sessions but
interrupts a running turn — I will not relaunch this node while that
session is mid-turn; the fix applies at the next relaunch you choose.
