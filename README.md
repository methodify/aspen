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
- One **operator console** (web) organized around the operator's questions,
  not data types: **Now** (what needs you, and what every agent is doing —
  asks, current tool, files touched, context, cost), **Flow** (the bus as a
  timeline: channels, DMs, receipts, who is waiting on whom, what is stuck),
  **Mesh** (nodes → repos → agents with their channels, editable in place,
  as a map or a list — where you also declare **links**: directed pathways
  with a purpose that agents are told about), **History** (what happened
  across the fleet: lanes per agent, turns, tools, prompts, messages, with
  a brushable day), and the interactive **Session** view. The rail is
  your working set — pinned and recent sessions — not a copy of the fleet.

Built on Claude Code's headless NDJSON protocol; designed so a second agent
runtime is an adapter, not a rewrite.

- Product design: [`docs/DESIGN.md`](docs/DESIGN.md)
- Node API: [`docs/API.md`](docs/API.md)
- Proposals (parked, revisit): [`docs/proposals/`](docs/proposals/)
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
| `aspen` | The `aspen` daemon + CLI (`up`, `down`, `status`, `update`, `dev`, `bus`, `mesh`) |
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

No token needed (the repo is public; `GITHUB_TOKEN` is still honored to lift
the API rate limit). `ASPEN_VERSION` pins a tag; `ASPEN_INSTALL_DIR`
overrides the destination.

Linux binaries need glibc 2.28 or newer (Ubuntu 20.04+, Debian 10+, RHEL 8+).

Updating in place, from the daemon's own machine:

```bash
aspen update             # install the latest release over this binary
aspen update --restart   # …and bounce a running daemon onto it: sessions
                         # are stopped cleanly and revived on the new binary
                         # with their context intact
aspen update --version v0.2.0 --force
```

On Windows a running daemon holds `aspen.exe` open, so `aspen update` alone
can't replace it in place — use `aspen update --restart` (it stops the
daemon, swaps the binary, and brings it back), or `aspen down` first. On
Linux/macOS a plain `aspen update` works with the daemon up; `--restart`
then bounces it onto the new binary.

`--restart` keeps the previous binary in a rollback slot (`aspen.prev` /
`aspen.exe.old`), health-checks the new daemon, and puts the old one back if
it doesn't come up; `aspen update --rollback` does that by hand, and
`aspen update --check` only reports.

### Auto-update, drain, and the fleet

Nodes check the release channel themselves (at start, every 6 hours, and
when a peer hints) and show what they find in the console (the version
badge, a card under *Needs you*, and *Mesh → list → Nodes*). Whether they
*apply* it is a per-node policy:

```bash
aspen config update auto              # notify (default) | auto
aspen config update-window 02:00-06:00
aspen config update-soak 24h          # don't take a release younger than this
aspen config update-skip 0.4.0        # snooze one version
```

`auto` never interrupts work: the node *drains* — refuses new sessions and
waits until every session has been idle five minutes with nothing pending —
then runs the equivalent of `aspen update --restart` and revives everything.
The same drain is what the console's **update** button does, for this node
or any peer; **update now** restarts through whatever is running, after
naming the sessions it will interrupt. **update fleet** rolls the mesh one
node at a time, this node last, and stops at the first node that doesn't
come back. `aspen status` and `aspen logs` show the state from a shell;
the node rows in the console show version, harness version, uptime, and
each node's log. Design reference: [`docs/SERVICING.md`](docs/SERVICING.md).

All node state lives in `~/.aspen` (override with `--data-dir`): the bus
store, mesh identity and certificates, trusted-repo decisions, the API token,
daemon state, and logs. Updates never touch it.

### Releasing (maintainers)

Push a tag `v*` matching the workspace version. CI builds the console, embeds
it, compiles `aspen` + `aspen-relay` for Linux (x86_64, aarch64 — linked
against glibc 2.28 via `cargo-zigbuild`, so the binaries run on any distro
from Ubuntu 20.04 / Debian 10 / RHEL 8 on) and Windows, and publishes a
GitHub Release with per-target binaries and a `SHA256SUMS` the installers
and `aspen update` verify against. (macOS targets are paused
in the workflows until the mac is in scope — uncomment to re-enable.)

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

The **Library** shows repositories grouped by node — this node and every
reachable peer. You can trigger repo discovery on a peer, browse its
sessions, and start or resume a session there; the work runs on the owning
node, so leaving the mesh simply hides that node's content (it was never
copied locally). `aspen config` sets daemon start defaults (`headless`,
`listen`) and default harness args; `aspen restart` bounces the daemon in
its current mode.

`aspen up -d --headless` runs an API-only node (no console) on an
**ephemeral port** — the OS picks a free one, and `aspen status` (and the
CLI generally) find it via the daemon state file. That makes a second node
on the same machine zero-config: e.g. the Windows and WSL2 sides of one box,
where WSL2 forwards Linux listeners onto Windows localhost and fixed ports
collide. A headless node dials its peers (or a relay); a node that must be
dialed at a stable URL passes `--listen` explicitly. `aspen repos discover` recovers repos from Claude
Code's session store and registers them (also a button in Library, which
additionally reads a repo's `.mcc/sessions` register to carry mcc session
names and args over on resume). Per-harness default CLI args (e.g. always
pass `--chrome` to claude) live in settings — the Claude defaults strip in
Library, or `settings.json` in the data dir; per-session args stack on top.

`aspen status` reads out the whole node: binary and daemon versions, pid and
uptime, the session roster with turn state, pending revives, and mesh
membership with live link health (from disk when the daemon is down). A clean
shutdown records which sessions were live; the next `aspen up` revives them
automatically (`--no-resume` to skip). In debug builds the
console is read live from `ui/dist`; release builds embed it, and `--ui DIR`
serves a directory from disk in either case.

Dev harness without the UI:

```bash
aspen dev oneshot --repo /path/to/repo --prompt "hello"
aspen dev chat    --repo /path/to/repo            # interactive REPL
aspen dev duo     --repo /path/to/repo            # two agents talk over the bus
aspen bus log
```

## Names, branches, bookmarks

Agents are named **per repo**: `arch@nonlinear`, and `arch@nonlinear@anindor`
only when the same repo handle exists on two nodes. A repo's handle defaults
to its directory name (renamable in Library; unique per node). Inside a
repo, a bare `arch` reaches that repo's arch; across repos, say `arch@repo`.
Ambiguous names are refused with the candidates, never guessed.

An agent name points at a **head** — the session being continued. **branch**
(session page, or `/branch label` in the composer) bookmarks the current tip
and forks; the name continues on the fork, and revive/restart/update follow
it. **history** lists the lineage and bookmarks; *resume here* on a bookmark
forks from that point and makes it the head, bookmarking the line you were
on. Nothing is ever lost or overwritten.

### Branches made outside Aspen

Branching is not a wire operation in the runtime — a fork is a relaunch
(`claude -r <id> --fork-session`) — so there is nothing to intercept; but
the transcript on disk says what happened, and Aspen asks you what it
means. A fork of an agent's session made from a terminal, or an agent's
session driven from a terminal while the agent is down, shows up under
*Needs you* with the identity question: **carry** (the name moves to the
branch, its old tip bookmarked), **new agent** (the branch gets its own
name; the original keeps its session), or **ignore** — nothing moves until
you answer. The same choice sits in the session's **branch** control
("continue as @…") and on every bookmark ("as new agent"), and as
`/branch [label] [as name]`.

Detection runs from the transcripts every 15 seconds. For a second's
latency instead, install Claude Code's SessionStart/SessionEnd hooks:

```bash
aspen hooks install      # adds two entries to ~/.claude/settings.json
aspen hooks status
aspen hooks uninstall
```

The hook (`aspen hook`) only nudges the local daemon; Aspen's own sessions
are filtered out by their entrypoint.

## Joining a second machine (the mesh)

Three commands, two pastes, no restarts — the daemons can be running the
whole time (mesh commands apply to a running daemon live).

```bash
# On machine A (creates the mesh + its root key; A's daemon joins it live):
aspen mesh init --mesh mymesh --node alpha

# On machine B:
aspen mesh enroll --node beta            # prints an enroll blob → paste on A
# → on A: certify it, telling B how to reach A:
aspen mesh certify aspen:enroll:… --url ws://<A-host>:7420/api/federation/ws
#   prints a join BUNDLE (B's cert + A's cert + that URL + A's relay, if any)
# → on B:
aspen mesh join aspen:bundle:…           # installs the cert, registers A as
                                         # a peer, dials it — link comes up
```

The console can drive this too, without ever holding the keys: the Mesh
page shows membership and link health, inspects pasted blobs (with a
duplicate-name warning before anything runs), and **queues** each step;
`aspen mesh apply` in a shell reviews and executes the queue, and the
resulting blob shows up in the console with a **deep link** you open on
the other node's console to prefill the next step. Omit `--url` if A will dial B instead (then `aspen mesh peers-add … --url`
on A). Names must be distinct per node — the hostname default collides on a
Windows+WSL box, so name them (`anindor`, `anindor-win`). For peers with no
direct path, a rendezvous relay:

```bash
aspen mesh relay wss://relay.example.com/relay   # on each node; rides the
                                                 # bundle for later joiners
```

Either console can now see and drive agents on the other node
(`@agent@node` addressing), and the Library shows both nodes' repos. Bus traffic and remote sessions travel
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
