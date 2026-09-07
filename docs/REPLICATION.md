# Transcript replication (opt-in)

**Status:** reference for what is built (2026-09-07, v0.16). Proposal:
PROPOSALS-2026-09-B.md §1. Code: `crates/aspen-node/src/replicate.rs`
(the replicator, the target ops, staging a bundle), `store.rs`
(`replicas` table), `node::pull_from_replica`, the transcript fallback
and `/api/replicas` in `api.rs`, the banner and picker in the console.

Not the default mode of operation. A node replicates only when its
operator points it at a peer.

## 1. Opting in

- `aspen config replicate <peer>` (`off` to stop), or the **replicate
  to** picker on this node's row in the Mesh panel. Settings:

  ```json
  "replication": { "to": "j2", "repos": ["hub"], "accept": true, "keep_days": 30 }
  ```
  `repos` limits which handles replicate (default all); `accept: false`
  makes a node refuse replicas; `keep_days` prunes replicas nobody has
  written to.

## 2. Mechanism

Every five seconds the source walks its agents (skipping moved ones and
repos not in the list) and, for each session's files — the main
`<sid>.jsonl` and everything under `<sid>/` (subagent transcripts, tool
results) — compares the file size with what the target has acknowledged.
Anything longer is sent in 256 KB chunks over the sealed link as
`replica_append {session_id, repo, title, ctx, rel, offset, data,
truncate}`; the target appends and answers with the bytes it now holds.
A file that shrank on the source (a rewrite) is re-sent from zero with
`truncate`. Six chunks per tick keep a big backlog from starving the
roster.

Offsets are learned, not assumed: once per agent per link-life the
source asks `replica_offsets {agent}` and resumes from the answer, so a
restart, a link outage, or a prune on the target all resolve to
"send what is missing". The target trusts the file on disk over its row
if they disagree.

The target keeps files under `<data>/replicas/<node>/<agent>/<rel>` and
a `replicas` row per file (node, agent, rel, session id, repo handle,
title, the source's `PathCtx`, bytes, updated_at). `ctx` is what lets a
replica become a session elsewhere: paths are canonicalized with it at
import time exactly as an export would have done.

## 3. Reading a replica

`GET /api/agents/{name}/transcript` for `bare@node`, when the link to
`node` is down and a replica is held here, serves the replica (the
delta form too). `GET /api/agents/{name}/replica` says whether one is
held and whether the home is up; the session page shows a banner —
"*node* is down — showing the replica held on *me* as of *t*" — with
the composer left alone (messages queue on the bus as before). The Mesh
list has a **Replicas held here** section; `GET /api/replicas` lists them.

## 4. Starting from a replica

`POST /api/agents/{name}/move {to: me, from_replica: true}` — or the
banner's *start from the replica here*, or the move dialog when the
source is down — stages the replica as a bundle (tiers A and B,
canonicalized with the stored ctx), imports it as a **copy**, and spawns
it as a fork. It cannot tombstone the original: the report notes that
the source may still run it, and when the source returns the operator
keeps one (the adoption verbs apply). A replica without a main
transcript yet cannot be started.

## 5. Verified (rig, 2026-09-07)

j1 pointed at j2: a 600 KB transcript arrived within a tick; a new turn
on j1 appeared at the tail of j2's copy; with j1 down, j2's console
rendered the session from the replica with the banner, and *start from
the replica here* produced a live fork on j2.

## 6. Not built

Replicating to more than one peer; replicating memory (S-5 converges it
instead); History's session lists for a down node; a per-session
opt-out.
