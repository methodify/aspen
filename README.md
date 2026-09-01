# Aspen

**A control plane for a fleet of coding agents across all your repos and all
your machines.** One daemon per machine runs agent sessions in the repos they
work on; a bus connects them; a mesh connects the machines.

- One **node daemon** (`aspen`) per machine runs agent sessions over their
  native headless protocols, in the repo each works on.
- One **bus** lets agents (and you) talk — automatically within a repo,
  deliberately across repos and machines — with three delivery classes and a
  passive, honest trail.
- One minimal **rendezvous** relay stitches nodes together across any network
  topology, holding nothing it can read.
- One **operator console** (web) is the window into all of it — designed as a
  switchboard, not a dashboard (see [`docs/V2.md`](docs/V2.md)): a Command
  triage home, first-class **Conversations** (channels that span repos and
  nodes, with delivery/ingest ticks and a Route gesture), interactive
  Sessions, a spatial **Map** of the mesh, and a Library of repos and skills.

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
| `aspen` | The `aspen` daemon + CLI (`up`, `down`, `update`, `dev`, `bus`, `mesh`) |
| `ui/` | The operator console (React + TypeScript) |
| `rendezvous/cloudflare/` | The Workers + Durable Objects rendezvous port |

## Install

One binary, console included, installed to `~/.local/bin` on every platform
(`%USERPROFILE%\.local\bin` on Windows) — the same place `claude` lives.

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/methodify/aspen/main/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/methodify/aspen/main/install.ps1 | iex
```

While the repo is private, set `GITHUB_TOKEN` (a PAT with repo read access)
before running the installer. `ASPEN_VERSION` pins a tag; `ASPEN_INSTALL_DIR`
overrides the destination.

Updating in place, from the daemon's own machine:

```bash
aspen update             # install the latest release over this binary
aspen update --restart   # …and bounce a running daemon onto it: sessions
                         # are stopped cleanly and revived on the new binary
                         # with their context intact
aspen update --version v0.2.0 --force
```

All node state lives in `~/.aspen` (override with `--data-dir`): the bus
store, mesh identity and certificates, trusted-repo decisions, the API token,
daemon state, and logs. Updates never touch it.

### Releasing (maintainers)

Push a tag `v*` matching the workspace version. CI builds the console, embeds
it, compiles `aspen` + `aspen-relay` for Linux (x86_64, aarch64), Windows, and
macOS, and publishes a GitHub Release with per-target binaries and a
`SHA256SUMS` the installers and `aspen update` verify against.

## Build from source (one machine)

```bash
cargo build --release
( cd ui && npm ci && npm run build )      # builds ui/dist, embedded by the release build
./target/release/aspen up                 # http://127.0.0.1:7420
```

Open the console, start a session in a repo, and step into it. Two sessions
in the same repo share a `#repo` channel automatically. The daemon remembers
repos you use and can list and resume existing sessions found on disk.

Start the daemon in the background instead:

```bash
aspen up -d      # detached; logs to <data-dir>/aspen.log
aspen down       # stop it (clean shutdown)
```

A clean shutdown records which sessions were live; the next `aspen up`
revives them automatically (`--no-resume` to skip). In debug builds the
console is read live from `ui/dist`; release builds embed it, and `--ui DIR`
serves a directory from disk in either case.

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
