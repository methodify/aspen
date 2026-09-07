# A node in more than one mesh: membership, exposure, capabilities

**Status:** reference for what is built (2026-09-07, v0.18). Proposal:
docs/proposals/multi-mesh.md (§8 order, built in one round). Code:
`aspen-wire::identity` (extra certs), `crates/aspen-node/src/mesh.rs`
(per-mesh files, policy), `federation.rs` (union `MeshState`, hello
negotiation, per-mesh rosters and relays, the capability guard, exposure
filters), `store.rs` (`repo_meshes`), `meshops.rs` (join a second mesh,
leave one, policy), `api.rs` (`meshes`, `consoles`, `/api/repos/expose`),
the Mesh panel and list.

## 1. Membership

A node is **one keypair and one name** in every mesh it belongs to. The
first mesh is the **primary** (`mesh.json`, `identity.cert`); each
additional mesh is `meshes/<name>.json` with its own root, peers, relays
and policy, and its cert in `identity.certs`. `aspen mesh enroll` on a
certified node prints the same enroll blob it always did; the other
mesh's root holder runs `aspen mesh certify`, and `aspen mesh join
<bundle> [--policy full|observe]` installs the second membership. `aspen
mesh leave --mesh <name>` drops it; `aspen mesh status` lists all; `GET
/api/mesh` carries `meshes: [{mesh, primary, policy, peers, relays,
root_here}]` and `multi_mesh`.

**Names are unique across every mesh a node is in** — a join that would
collide (the certifier's name, or the node's own) is refused — so
`name@repo@node` keeps three segments and links, rosters and health stay
keyed by node name.

## 2. Links

A hello carries the primary cert plus every extra cert (`certs`). The
receiver picks the first it can verify against a root it holds; that
mesh is the link's mesh (`MeshState.link_mesh`), remembered until the
link drops. A peer met through a relay before any config listed it is
recorded in the config of the mesh its cert names. Relays belong to the
mesh whose config lists them (a discovered relay, to the mesh of the
peer that advertised it); the relay client registers with that mesh's
name and cert. The embedded relay host admits members of any mesh its
node is in. Peer lookups for sealing, dialing and routing span every
mesh.

## 3. Exposure

While a node is in one mesh, everything is exposed — the single-mesh
behavior, unchanged. Joining a second mesh **pins every existing repo to
the primary** (`repo_meshes`), so the rule change leaks nothing; a repo
added afterwards is exposed to no mesh until the operator says
(`POST /api/repos/expose {path, meshes}`; the *exposed to* chips on a
repo's row in the Mesh list). Rosters to a peer carry only agents in
repos exposed to its mesh; ops from a peer are refused for an unexposed
agent or repo before dispatch and filtered afterwards (`node_repos`,
`node_discover`, `history`, `needs`, `fleet_activities`, `usage`,
`notices`, `adoptions`).

**Mesh-wide state stays in the primary mesh.** Boards, the plugin
registry, templates and memory digests ride only rosters to primary-mesh
peers, and the ops behind them are refused to others — so a work mesh
never learns a personal mesh's boards, and never writes into them.
Operator mail, memory conflicts and node servicing (update, evacuate,
logs, links, adoptions) are likewise the primary mesh's.

## 4. Capabilities

Every op is classed `observe` (transcripts, activities, artifacts,
files, runtime, boards, registry, templates, needs, repo and session
listings, history, usage, notices, replica offsets, preflight, event
subscriptions, `http`), `spawn` (`spawn`, `template_spawn`), `trust`
(`adoption`, `node_repo_skip`), or `control` (everything else). A mesh's
**policy** says what its peers get here: `full` — all four, the default
for the primary mesh; `observe` — reads only, the default for a mesh
joined second. `aspen mesh policy <mesh> full|observe` changes it. The
guard runs before dispatch (`forbidden: Control not granted to mesh
'work' here (policy observe)`), bus rows from an observe-only peer are
dropped, and every `spawn`/`trust` op from any peer is recorded on the
fleet trail as `remote_control {peer, mesh, op, agent}`. A console peer
(RELAY.md §11) gets observe and control, never spawn or trust.

## 5. Console

The Mesh panel groups peers by mesh with the policy, `primary` and
`root key` chips; remote agents carry their mesh as a chip in Now; the
Mesh list's repo rows show and toggle exposure while in more than one
mesh.

## 6. Verified (rig, 2026-09-07)

A fourth node created a second mesh and certified an existing member's
enroll blob; the member joined it (policy observe), pinned its repo to
the first mesh, and linked to both; the new peer saw nothing until the
repo was exposed, then saw its sessions, could read a transcript and
history, was refused a message until the policy became `full`, and then
got a reply across meshes. The first mesh's peers saw the same sessions
throughout.

## 7. Not built

Different names per mesh; root-signed role claims in certs (policies
are local); bridges labeled in Flow (a channel spanning meshes is
possible only through a node in both, and is unlabeled); the proposal's
per-peer grants finer than the mesh policy.
