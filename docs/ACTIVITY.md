# Activity: tasks, subagents, workflows, monitors

**Status:** design reference for what is built (2026-09-07, v0.14).
Proposal: PROPOSALS-2026-09.md §8. Code: `crates/aspen-claude/src/
activity.rs` (the ledger), `transcript.rs::rehydrate_file` (subagent
transcripts), counts in `node.rs`, routes and ops in `api.rs` /
`federation.rs`, the drawer and subagent mode in `ui/src/pages/Session.tsx`,
chips in Now, the board pane bar, and the rail.

## 1. Where the facts are

A session's side work leaves a complete trail in its own transcript:

| activity | start | id | end |
|---|---|---|---|
| task | `Bash` tool_use with `run_in_background: true` | its tool_result: "Command running in background with ID: X" | a `<task-notification>` user line with `<task-id>X</task-id>` and `<status>` |
| agent | `Agent` (or `Task`) tool_use | its tool_result: "agentId: X" (async); a synchronous agent returns its result in the same call and is *done* at once | the notification, as above |
| workflow | `Workflow` tool_use | "wf_…" in the result | the notification |
| monitor | `ScheduleWakeup` / `CronCreate` / `Monitor` tool_use | the tool_use id | (a wakeup fires once; shown as scheduled) |

A subagent's own transcript lives beside the session's: `<project>/
<session>/subagents/agent-<id>.jsonl`, with `agent-<id>.meta.json`
carrying the tool_use id — which is how a synchronous agent's transcript
is found too.

## 2. The ledger

`activity::derive` replays lines into `Activity {id, kind, label,
tool_use_id, started_at, ended_at, status, detail, has_transcript}`;
`activities(repo, session)` caches the result by the transcript file's
size and mtime, so the roster can carry counts every tick without
re-parsing a large file, and a live session's ledger is always as fresh
as its last write. **The stale rule**: an activity still "running" that
started before the current process was spawned (`agents.last_spawned_at`)
is marked `unknown` — the harness that owned it is gone and its
notification will never come. Counts (`running`, `agents`, `tasks`,
`workflows`, `monitors`) ride the agent JSON and the roster as
`activities`, for live sessions only.

## 3. Surfaces

- **Signal**: a Now card, a board pane bar, and the session header show
  an activity chip (`2 agents · 1 task`) while anything runs; the chip
  pulses. Dynamic boards accept `has activity` as a state.
- **Drawer** (*activity ▾* on the session page, also in a pane): the
  ledger newest first, running first, each with kind, label, elapsed or
  duration, status, and detail — a task's command and summary, an
  agent's type and prompt, a workflow's run id, a monitor's schedule.
  Polled every 3 s while open.
- **Subagent view**: an agent with a transcript opens at
  `/session/<name>/agent/<id>` — the session view in read-only mode over
  the subagent's file, same tool cards and markdown, re-fetched every 3 s
  while the agent runs; served locally or over the mesh (`subagent` op).
- API: `GET /api/agents/{name}/activities`, `GET /api/agents/{name}/
  subagent/{id}`; mesh ops `activities`, `subagent`.
- **Fleet-wide (v0.15)**: `GET /api/activities` gathers every running
  item on this node and every up peer (`fleet_activities` op), tagged
  with the agent's full address; Now shows them as an **Activity** band
  between *Needs you* and *The fleet*, grouped by agent, present only
  while something runs. Remote agents' counts now survive the roster
  (`RemoteAgent.activities`), so chips show for peers' sessions too.
- **Task notifications (v0.15)**: a `<task-notification>` user line
  renders as a collapsed card (summary, status, task id) instead of a
  bubble; click to read it.

## 4. Verified (rig, 2026-09-07)

A session asked to start a 50 s background command and an Explore
subagent: the ledger showed the task running with its id and command and
the (synchronous) agent done with its result; counts reported one
running task; the subagent's transcript rendered in the subagent view.
Tasks from before a restart were settled as `unknown`.

## 5. Not built

Cost roll-up from subagent transcripts (USAGE.md folds their tokens);
a live tail of a subagent over a socket rather than a poll;
recognizing tasks the harness stopped without a notification (they age
out with the stale rule at the next restart).

## Details, stop, processes (v0.24, PROPOSALS-MCP.md §6.4)

The composer's status line carries the running count (`1 monitor ·`)
and opens the drawer; a row opens to status, runtime, script, purpose
and output; **stop** terminates the process running the script (a child
of the session's process matched by command line), after which the
harness reports the task ended and the ledger settles it — until it
does, the row reads `stopped (by you)`. Child processes no ledger row
explains (a hook-launched monitor) are listed under "processes under
this session" with a stop of their own. Monitors keep the whole script
and the tool's replies in `detail.command` / `detail.output`.
