# Memory convergence (opt-in)

**Status:** reference for what is built (2026-09-07, v0.16). Proposal:
PROPOSALS-2026-09-B.md §5. Code: `crates/aspen-node/src/memory.rs`,
`store.rs` (`memory_base`), the roster field and `memory_files` /
`memory_conflicts` / `memory_resolve` ops in `federation.rs`, `needs.
memory` and `POST /api/memory/resolve` in `api.rs`, the Now card and the
Mesh-panel toggle.

## 1. What converges

Project memory: the text files under `<claude home>/projects/<slug>/
memory/` for a repo (`.md`, `.txt`, `.json`, `.yaml`, `.toml`, `.csv`,
`.jsonl`; up to 512 KB each). Two nodes take part when both have
`memory.sync` on (`aspen config memory-sync on`, or the *memory sync*
box on this node's row in the Mesh panel) and both hold a counterpart
of the repo: the same git origin, else the same directory name.
Conflict copies (`*.from-<node>.*`) are never synced.

## 2. Mechanism

The roster carries `memory: {key: digest}` — one digest per registered
repo, keyed `origin:<url>` or `name:<basename>`, a hash over every
file's path and content hash plus tombstones. A peer that holds the
same key with a different digest pulls `memory_files {key}`: every
file, canonicalized with the sender's `PathCtx` (so paths translate
across homes and OSes, as in MIGRATION.md), plus tombstones.

Each file merges against a **base** — the last content both sides
agreed on, kept in `memory_base(repo, rel, content, deleted)`:

| remote | local | base | result |
|---|---|---|---|
| = local | | | agree; base := local |
| = base | ≠ base | | keep local (they have not moved) |
| ≠ base | = base | | take remote; base := remote |
| ≠ base | ≠ base | held | line-level 3-way merge (`diffy`); clean → write, base := merged; conflict → keep local, write `<name>.from-<node><ext>` beside, raise a `memory_conflict` notice |
| present | absent | absent, or tombstone ≠ remote | create; base := remote |
| present | absent | tombstone = remote | stay deleted (we removed it) |
| deleted | present | = local | delete; base := tombstone |
| deleted | present | ≠ local | keep (an edit beats a delete) |
| both absent | | | nothing |

A tracked file that vanishes locally becomes a tombstone the next time
the digest is computed. Nothing is injected into a running session: a
changed file is picked up whenever the harness next reads memory.

## 3. Conflicts

Now's *Needs you* band lists every conflict on every node (`needs.
memory`, gathered like prompts) with **keep mine** (drops the incoming
copy) and **take theirs** (replaces the file with it); either way the
result becomes the new base and the roster re-broadcasts so the peer
converges to it. The copy can also be opened beside the original in the
viewer, or resolved by hand — deleting the `.from-` file resolves it.

## 4. Verified (rig, 2026-09-07)

Two nodes with counterpart repos (same origin) and the setting on: a
file written on one appeared on the other within a roster tick; edits
to different lines on both merged cleanly; edits to the same line
produced a `.from-<node>` copy, a notice, and a Now card whose *take
theirs* replaced the file and converged the peer.

## 5. Not built

A CRDT (SYNC.md §3: only if line conflicts turn out common); syncing
`CLAUDE.md` or user-level memory; more than pairwise convergence
(with three nodes every pair converges, which is enough for one
operator).
