# Proposal: a node in more than one mesh

**Status:** proposal — not scheduled. Written 2026-09-03 to capture current
thinking; revisit when the operator model for *other people* is clearer
(see §7). Nothing here is built.

**One-line version:** the crypto already allows it (one keypair, one cert
per mesh); the product question is *exposure* — which repos and agents a
given mesh is allowed to see — and the prerequisite is the per-peer
capability layer, because a second mesh with other people in it is the
first multi-operator mesh.

---

## 1. The scenario

One laptop. A **personal mesh** joining the operator's own machines (WSL,
Windows, MacBook). A **work mesh** joining nodes across an employer's
estate, whose root key is held by someone else. The laptop should be a
member of both, so its operator has one console over everything they run —
without the work mesh learning anything about the personal side, and
without work peers gaining authority over the laptop.

## 2. What already works (the identity layer)

- A node is **one keypair** (`identity.json`: ed25519 + x25519). A cert is a
  *root's* signature binding that keypair to a **name in that mesh**.
  Nothing is single-mesh about that: the same keypair can carry N certs
  from N roots, and the name may differ per mesh (`laptop` at home,
  `bwilliams-laptop` at work).
- The federation hello carries the peer's cert, which names its mesh;
  verifying it against *that mesh's* root (rather than "our root") is a
  small change.
- Per-mesh peers, relay, links, rosters, and the join bundle follow from
  giving each mesh its own state. Sealed envelopes are per-link and don't
  care.

So the crypto and transport are a refactor, not a design problem.

## 3. The design problem: exposure

Today "in the mesh" means *every agent on this node is visible to every
peer*, and every peer may act on them (§8.1 of DESIGN.md lists the full
table: message, interrupt, stop, revive, answer prompts, spawn,
acknowledge trust). With two meshes on one laptop that is exactly wrong:
work peers would see personal repos and agents.

Membership therefore moves down a level:

> **A repo is exposed to zero or more meshes.** The roster a peer receives
> contains only agents in repos exposed to *its* mesh. Bus delivery, links,
> channels, needs aggregation, history aggregation, and every node-scoped
> op (`node_repos`, `node_sessions`, `spawn`…) filter by the same rule.

### 3.1 Defaults (the part that decides whether this is safe)

- **Exactly one mesh:** everything is exposed — today's behavior, no
  friction, no new concept for the single-mesh user.
- **Joining a second mesh:** every *existing* repo is pinned explicitly to
  the first mesh at that moment (so the rule change leaks nothing), and
  **new repos default to no mesh** until the operator sets them. The Mesh
  list shows an "exposed to" chip per repo (`personal · work`); one click.
- **Discovery** registers repos as unexposed on a multi-mesh node.
- Exposure lives on the node that owns the repo. A peer never learns a repo
  exists unless it is exposed to the peer's mesh.

### 3.2 Node names

`name@repo@node` stays three segments only if node names are unique
across every mesh a node belongs to. Enforce it locally: refuse to join a
mesh whose peer names collide with any existing peer (same guard family as
the duplicate-name check at certify). Rare, cheap, and it keeps addresses
from growing a fourth segment.

### 3.3 Bridges

A custom channel whose members span two meshes is a **bridge** through
the laptop: a message posted there fans out to both. The operator owns
both ends, so it should be *allowed* — but it is the one place data
crosses meshes, so the UI must label it (`bridge · personal ↔ work`) and
`bus_status` should say so to the agents in it. Links across meshes are
the same and get the same label. Open question: allowed by default, or
opt-in per channel? (Lean: allowed, labeled; revisit if a non-owner
operator model appears.)

### 3.4 Operator and console

One human. The console groups membership by mesh; the fleet view marks
which mesh each remote agent arrived through; the ceremony panel asks
which mesh a step concerns. Needs-you aggregates across both, tagged.

## 4. The trap: a work mesh is multi-operator

Everything built so far assumes **one operator**: peers are the same person
wearing another hostname. A work mesh whose root is held by someone else
has *other people's nodes* as peers. Joining it under today's semantics
hands them the full §8.1 table over the laptop — including spawning
sessions on it and acknowledging trust gates on its behalf.

So "a node in a work mesh" is not just "two meshes"; it is the first
**multi-operator** mesh, and it needs the **per-peer capability layer**
§8.1 already sketches:

| capability | grants |
|---|---|
| `observe` | roster, transcripts, runtime info, history, repo listings — of exposed repos only |
| `control` | message, interrupt, stop, revive, answer prompts, set model/mode/title/charter |
| `spawn` | start/resume sessions here (in exposed repos) |
| `trust` | acknowledge the trust gate on this node's behalf |

- Own-mesh peers (certified by *my* root): all four, as today.
- Foreign-mesh peers: **`observe` by default**, per exposed repo; anything
  more is an explicit grant per peer or per mesh, stored on the node that
  grants it, enforced in `serve_api_req` before dispatch.
- Certs could carry a root-signed *role* claim so a mesh's policy travels
  with identity rather than living only on each node; that's a later
  refinement. First version: local allowlist in the mesh's config.
- Every remote `spawn` / `trust` is recorded in the fleet event log with
  the originating node and mesh (the audit trail §8 calls for).

**This layer is the prerequisite.** It is also worth building on its own,
before any multi-mesh work, because it is the difference between "a leaked
token commands my sessions" and "a leaked token commands my sessions *and*
whatever a foreign peer was granted".

## 5. The alternative that works today

For pure isolation, run **two daemons** on the laptop:

```
aspen --data-dir ~/.aspen-work up --listen 127.0.0.1:7421
```

joined to the work mesh. Separate store, separate repo registry, separate
console, separate token; the work mesh never learns the personal node
exists; nothing to scope or expose. Costs: two consoles and no bridging.

This is not a workaround — it is the strongest isolation story available,
and for "personal vs work" it may simply be the right answer. Single-daemon
multi-mesh earns its complexity only when the operator wants **one console
over both** and **deliberate bridges** between them. The capability layer
(§4) is needed even for the two-daemon route the moment the work mesh has
other operators in it.

## 6. Mechanics, if built

**Storage.** `identity.json` keeps one keypair and gains `certs: {mesh:
NodeCert}` (the current single `cert` migrates to a one-entry map).
`mesh.json` becomes `meshes.json`: a list of `{mesh, root_public, peers,
relay, policy}`; the single-mesh file migrates to a list of one. Repo
exposure is a `repo_meshes(path, mesh)` table in the store.

**Daemon.** `NodeInner.meshes: RwLock<Vec<Arc<MeshState>>>`; each
`MeshState` as today (identity cert for *that* mesh, config, links, remote
rosters, health), plus a `policy`. The federation listener accepts hellos
for any mesh the node belongs to and verifies against that mesh's root.
Dialers, relay clients, and roster broadcasts run per mesh; roster payloads
filter by exposure. `remote_parts` and the resolver look up peers across
all meshes (names are unique across them, §3.2). Hot reload extends to
"joined a second mesh live".

**Ceremony.** Unchanged shape — enroll (one keypair, so `enroll` reuses the
identity and only needs the mesh's root holder to certify), certify, join,
bundle, deep links — with the mesh named in every artifact (it already is,
in the cert). `aspen mesh apply` and the console panel gain a mesh
selector. Leaving a mesh: `aspen mesh leave <mesh>` drops its cert, peers,
links, exposures, and relay; the daemon reloads.

**Console.** Membership grouped by mesh; repo rows carry the exposure chip;
remote agents/repos carry a mesh tag; bridges labeled on the map and in
Flow; ceremony panel per mesh; needs/history aggregation tagged by mesh.

**Bus semantics that do not change.** Urgency classes, delivery physics,
send-order, the trail, name resolution order, topology (links/channels)
and the open/closed dial. Exposure and capabilities are *filters in front
of* the existing machinery, not new machinery.

## 7. Open decisions (why this is parked)

1. **What does the product afford operators other than me?** Everything
   above treats a foreign mesh as "other people I must be protected from".
   That is the safe default, but a real work mesh will want *collaboration*
   affordances: a shared channel with a colleague's agent, a link into a
   team's triage agent, a colleague answering a prompt on a shared repo.
   Which of those are grants an operator makes, and which are mesh-level
   conventions? Unclear until the operator has lived in one.
2. **Foreign-mesh default capability:** `observe` (lean) vs nothing.
3. **Bridges:** allowed-and-labeled (lean) vs opt-in.
4. **One daemon vs two:** whether the single-console benefit is worth the
   exposure model at all, for this operator's actual life.
5. **Role claims in certs** vs local allowlists — decide when there is a
   second root holder to talk to.

## 8. Recommended order, when resumed

1. Per-peer capability layer with audit events (worth doing regardless).
2. Per-mesh daemon state + `meshes.json` + certs map + name-uniqueness
   guard + hot join/leave.
3. Repo exposure with the pinning rule, filtering everywhere, the chip.
4. Bridge labeling; console grouping; ceremony per mesh.

Until then: the two-daemon pattern (§5) covers isolation with no code,
and this document is the IOU.
