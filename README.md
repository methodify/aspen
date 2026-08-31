# Aspen

**A control plane for a fleet of coding agents across all your repos and all
your machines.** Aspen is a rhizomatic organism: what looks like a forest of
separate agents is one living network.

- One **node daemon** (`aspen`) per machine runs agent sessions over their
  native headless protocols, in the repo each works on.
- One **bus** lets agents (and you) talk — automatically within a repo,
  deliberately across repos and machines — with three delivery classes and a
  passive, honest trail.
- One minimal **rendezvous** relay stitches nodes together across any network
  topology, holding nothing it can read.
- One **operator console** (web) is the window into all of it: the mesh, the
  sessions (fully interactive), the bus, the operator inbox, and skills.

Built on Claude Code's headless NDJSON protocol; designed so a second agent
runtime is an adapter, not a rewrite.

- Product design: [`docs/DESIGN.md`](docs/DESIGN.md)
- Node API: [`docs/API.md`](docs/API.md)
- Rendezvous: [`rendezvous/README.md`](rendezvous/README.md)
- Protocol ground truth: [`docs/reference/`](docs/reference/)

## Workspace

| Crate | What |
|---|---|
| `aspen-core` | Domain vocabulary: ids, bus semantics, normalized session events, the adapter seam |
| `aspen-claude` | The Claude Code adapter: process host, NDJSON protocol client, in-process MCP, transcript rehydration |
| `aspen-node` | The node: bus store, session manager, delivery engine, mesh federation, permission broker, skills |
| `aspen-wire` | Mesh identity, sealed envelopes, the relay protocol |
| `aspen-relay` | The standalone rendezvous relay binary |
| `aspen` | The `aspen` daemon + CLI (`up`, `dev`, `bus`, `mesh`) |
| `ui/` | The operator console (React + TypeScript) |
| `rendezvous/cloudflare/` | The Workers + Durable Objects rendezvous port |

## Quick start (one machine)

```bash
cargo build --release
( cd ui && npm ci && npm run build )      # builds ui/dist, served by `aspen up`
./target/release/aspen up                 # http://127.0.0.1:7420
```

Open the console, plant an agent in a repo, and step into its session. Two
agents in the same repo share a `#repo` channel automatically.

Dev harness without the UI:

```bash
aspen dev oneshot --repo /path/to/repo --prompt "hello"
aspen dev chat    --repo /path/to/repo            # interactive REPL
aspen dev duo     --repo /path/to/repo            # two agents talk over the bus
aspen bus log
```

## Joining a second machine (the mesh)

```bash
# On machine A (creates the mesh + its root key):
aspen mesh init --mesh mymesh --node alpha

# On machine B:
aspen mesh enroll --node beta            # prints an enroll blob
# → paste that blob on A:
aspen mesh certify aspen:enroll:…        # prints a cert blob
# → paste the cert blob on B:
aspen mesh join aspen:cert:…

# Tell each node how to reach the other — directly…
aspen mesh peers-add aspen:cert:… --url ws://<host>:7420/api/federation/ws
# …or through a rendezvous relay when there's no direct path:
aspen mesh relay wss://relay.example.com/relay
```

Now `aspen up` on both, and either console can see and drive agents on the
other node (`@agent@node` addressing). Bus traffic and remote sessions travel
end-to-end encrypted; the relay only routes.

See [`rendezvous/README.md`](rendezvous/README.md) to stand up a relay (a
one-binary container, or a Cloudflare Worker).

## Security posture

A mesh is rooted in a user-held keypair; nodes join by a root-signed
certificate. Every inter-node envelope is signed by its sender and encrypted
to its recipient, so neither a relay nor the network can read or forge mesh
traffic — a compromised relay yields metadata and denial of service, never
command and control. Non-loopback API listeners require a node token.

## Status

Verified live against Claude Code 2.1.251: single-node sessions, the
three-class bus, cross-node federation (direct and relayed), remote-session
control from the console, permission prompts answered from the UI, transcript
rehydration and resume, and live skill editing with reload. Platform focus is
Linux first, then Windows and macOS (the node code is written platform-aware;
tuning is a polish pass).

License: MIT.
