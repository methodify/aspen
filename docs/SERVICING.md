# Servicing: updates, drain, inventory, rollout

**Status:** design reference for what is built (2026-09-03). Companion to
DESIGN.md §8.2 (release authenticity) and §8.1 (what a peer may do).

A fleet of daemons on several machines has to be *serviced*: told a new
version exists, moved onto it without losing work, watched while it comes
back, and put back if it doesn't — from one console, without a shell on
every box. This document is the model for that. The auto-updater is its
first consumer; drain, inventory, and remote logs are the primitives it
introduced, and they are reused by anything that has to take a node out of
service for a moment.

---

## 1. Knowing: the release check

Each node checks the release channel **for itself**: at daemon start (after
a short delay), every 6 hours, on an operator request, and on a *hint* from
a peer (§6). The check fetches release metadata (`releases/latest`, or a
pinned tag) and records:

```
ReleaseInfo { version, tag, published_at, notes }      // the newest release seen
CheckResult { at, ok, error }                          // the last attempt
```

`notes` is the release body — the console shows it, so "v0.4.0 available"
carries what changed. `behind` is true when the release is newer than the
running binary (numeric semver compare; prereleases are not fetched).

While the repo is private the daemon inherits `GITHUB_TOKEN`/`GH_TOKEN`
from the shell that ran `aspen up`. Public repos need no token. Overrides
(`ASPEN_RELEASE_REPO`, `ASPEN_GITHUB_API`) are honored exactly as by
`aspen update`, so tests can point a daemon at a fake server.

## 2. The policy

Three knobs, per node, in `settings.json` under `update`:

| knob | values | meaning |
|---|---|---|
| `mode` | `notify` (default) · `auto` | notify = check and badge, never apply. auto = apply when the gates in §4 pass |
| `window` | none · `"HH:MM-HH:MM"` (node-local time) | if set, `auto` only fires inside it; wrapping windows (`22:00-06:00`) are fine |
| `soak` | none · a duration (`24h`, `90m`) | don't auto-apply a release younger than this — lets a bad tag be pulled before the fleet takes it |

Plus `skip`: a version to snooze (the badge goes quiet for that version;
`auto` will not apply it). And `check: false` turns the check off entirely
for a node that must never touch the network for this.

Why not "immediate"? A restart interrupts any turn in flight — the tool
call is lost, the session resumes at its last completed message. "Install
regardless" is therefore never what an unattended policy should do; it
exists only as a human verb (§5, *update now*).

Why no "max wait"? If `auto` is on and the node has not been quiet for 24
hours, the node **escalates** instead of forcing: the console shows the
pending update in *Needs you* with what it is waiting on. Policy never
interrupts work; it asks.

The policy is a settings mutation, not a trust mutation: `aspen config
update auto`, `PUT /api/settings` (merge), and the console all set it. The
console can also push one node's policy to every node in the mesh
(`PUT /api/update/policy` fans out over the `node_update_policy` op).

## 3. Node state: ready → draining → updating

A node is in exactly one servicing state:

```
ready                        normal operation
draining { since, by, when, waiting_on[] }
                             an update is requested; new spawns are refused
                             (409 "node is updating"); waiting for quiet
updating { since }           the updater child has been launched; the daemon
                             expects to be stopped any moment
```

`by` is who asked (`policy`, `operator`, or a peer node's name); `when` is
`quiet` or `now`. The state is visible in `/api/update`, `/api/node`, the
roster (so peers and their consoles see it), and `aspen status`.

Drain is a primitive, not an updater detail: "refuse new work, wait until
quiet, then act" is what `aspen down --drain` and a host reboot want too.
The updater is one reason to drain.

## 4. The quiet gate

Safe to restart means, **all of**:

- every live session's turn state is idle, and has been for at least the
  quiet interval (5 minutes; `ASPEN_QUIET_SECS` overrides for tests);
- no open permission prompt or question on any session (the human's answer
  would be lost across a restart);
- no session spawned in the last quiet interval (someone is clearly about
  to use it);
- if a window is set: now is inside it;
- for policy-driven drains: the release has soaked (§2).

`when: now` skips the first three gates and restarts through whatever is
running (sessions are revived; in-flight turns are lost). A drain that has
waited longer than 24 hours is flagged `overdue` and surfaces in the
console's *Needs you* band.

## 5. Applying: the unattended updater

The daemon never replaces its own binary in-process. When the gate passes it
launches a detached child:

```
aspen update --restart --unattended --trigger <policy|operator|peer:NAME>
```

with stdout/stderr appended to `aspen.log`, and moves to `updating`. The
child is the same code path as an operator typing `aspen update --restart`,
which is already proven on every platform (on Windows the updater is itself
`aspen.exe`, so the rename-aside trick is exercised every time). Three
things the unattended case adds, and the attended case now gets for free:

1. **A rollback slot.** Before replacing, the previous binary is kept as
   `aspen.prev` (unix) / `aspen.exe.old` (Windows; the rename target). The
   slot is overwritten only by the *next* successful update.
2. **A health check.** After starting the new daemon the updater waits (30s)
   for `daemon.json` to carry the child's pid **and** `/api/node` to answer
   with the expected version. If either fails: stop whatever started,
   restore the slot over the binary, start the previous daemon, and report
   `rolled_back`. Nobody is at a shell for an unattended update on a remote
   node, so this is not optional. `aspen update --rollback` performs the
   same swap by hand.
3. **An outcome record.** `update-outcome.json` in the data dir:
   `{ from, to, ok, rolled_back, error, trigger, started_at, finished_at }`.
   The next daemon to start reads it, records a fleet event
   (`update_applied` / `update_failed` / `update_rolled_back`, agent
   `node`), keeps it for `/api/update`, and marks it consumed.

Fleet events for the whole lifecycle: `update_available` (once per
version), `update_requested`, `update_started`, `update_applied`,
`update_failed`, `update_rolled_back`, `update_cancelled`. History shows
them like any other event; a remote node's failure is readable from here.

`aspen up` no longer deletes `aspen.exe.old` at start — it is the slot.

## 6. The mesh: hints, not authority

When a node learns of a newer release it sends `{ t: "update_hint",
version, tag }` to every linked peer. A peer treats a hint **only as a
reason to check now** (rate-limited to once a minute). It never trusts the
peer's claim of what the version is, and it never accepts bytes from a
peer: every node fetches from the release channel and verifies the
checksum itself. A compromised or stale peer can make a node *look*, not
*install*.

The roster carries each node's servicing summary (`available`, `state`,
`state_detail`, `policy mode`, and the inventory of §8), so every console
in the mesh sees the same picture without a round trip.

## 7. Remote update and rolling rollout

`node_update { when }`, `node_update_cancel`, `node_update_check`,
`node_update_policy { policy }`, `node_update_status`, `node_logs { lines }`
are node-scoped `api_req` ops, like `spawn` and `node_repo_skip`. They are
control-class — a peer makes another machine fetch and execute a binary —
but from a source that machine verifies for itself, so under the
single-operator table (DESIGN §8.1) they are acceptable, and under the
capability layer they belong to an own-mesh-only capability, `service`,
that a foreign-mesh peer never receives. That row is added to §8.1.

**Update fleet** (`POST /api/update/fleet { when }`) is a rolling rollout run
by the node whose console asked:

- one node at a time, in roster order, **this node last** so the console
  you are watching from stays up until the end;
- each node is sent `node_update { when }` and then watched (its roster
  version, or `/api/node` for self) until it reports the target version, or
  15 minutes pass, in which case the rollout **stops** and reports which
  node failed — a bad release costs one node, not the mesh;
- `DELETE /api/update/fleet` stops after the node in progress;
- progress (`order`, `done`, `current`, `failed`, `stopped`) is in
  `/api/update` under `rollout`.

Serial is the default because the point of a rollout is to notice. A
parallel flag can come later.

## 8. Inventory

Servicing needs to know what is on each machine. Every node reports, in
its roster and `/api/update`:

```
inventory { os, arch, claude_version, started_at, pid }
```

`claude_version` is `claude --version` run once at daemon start (quietly,
in the background) — the harness is the thing agents actually depend on,
and skew *there* changes behavior across the fleet silently. Uptime is
`started_at`. The console's node rows show version, harness version, state,
uptime; skew in either version is flagged.

## 9. Version skew and compatibility

During a rollout the mesh runs mixed versions for hours; that is normal
and must not fail strangely. The federation hello now carries a **protocol
number** (`proto`, currently 1). A peer whose number differs is refused at
hello with a health error that says so (`peer speaks protocol 2, this node
speaks 1 — update`), instead of silently mis-parsing frames later. Absent
`proto` (pre-servicing daemons) is treated as 1. Bump the number only when
a frame format actually changes incompatibly.

## 10. Remote logs

`GET /api/logs?node=&lines=` tails `aspen.log` on this node or a peer (the
`node_logs` op). The first failed remote update sends the operator to the
log on another machine; this keeps them in the console. Read-only, capped
at 2000 lines.

## 11. Console

- **Version badge** (status bar): a dot and tooltip when a newer release is
  known — *v0.4.0 available · notes*; the existing stale-UI reload button
  is unchanged.
- **Now → Needs you**: one card when any node is behind, draining, updating,
  overdue, or the last update failed: *v0.4.0 · 2 of 3 nodes behind ·
  [Update fleet] [Notes] [Snooze]*; when a rollout is running, its progress.
- **Mesh → list → Mesh panel → Nodes**: one row per node (self first):
  version (skew flagged), harness version, uptime, servicing state with
  what it is waiting on, **Update** (as soon as quiet), **Update now**
  (confirm names the sessions it will interrupt), **Cancel**, **Check**,
  **Logs** (drawer), and the policy editor for that node with *apply to all
  nodes*.

Two verbs on purpose: *Update* waits for quiet; *Update now* interrupts.
"Now" is always a human's decision — no policy value produces it.

## 12. Security consequences, stated plainly

- **Signing is the precondition for `auto` in public.** Auto-update is the
  threat model where whoever can write a release to GitHub gets code onto
  every node with no human in the loop. Checksums prove the download
  matched the release; only a signature (DESIGN §8.2, minisign) proves the
  operator cut it. `mode: auto` ships now for the private phase; signing is
  the blocker for going public, as already planned.
- **Peers can trigger, never supply.** No frame carries a binary or a URL to
  fetch from; the release source is compiled in (overridable only by the
  daemon's own environment).
- **Every remote update request is a fleet event** with the requesting node
  in `detail`, the audit trail §8 asks for.

## 13. Retracting a release

Deleting a tag on GitHub makes `latest` go backwards. A node running a
version newer than `latest` reports `withdrawn: true`; the console says
*you're on 0.4.0, which is no longer published; latest is 0.3.1* and
offers `aspen update --version v0.3.1` (auto never downgrades).

## 14. Not built (yet), and why

- **Peer-to-peer binary distribution** for air-gapped nodes — breaks §6's
  "peers never supply"; would need signed artifacts first.
- **Parallel rollout** — serial is the point until releases are boring.
- **A tunable quiet interval** — one constant until someone needs it.
- **Harness update orchestration** — Claude Code updates itself; we report
  its version and skew, nothing more.
