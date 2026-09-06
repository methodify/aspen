# The relay: rendezvous, mailbox, and paths between nodes

**Status:** design reference for what is built (2026-09-05). Companion to
DESIGN.md §7 (transports) and §8 (security); the protocol lives in
`crates/aspen-wire/src/relay.rs`; hosting notes in `rendezvous/README.md`.

A node reaches a peer three ways, tried in this order: **loopback** (same
node), **direct** (a dial URL — LAN, tailnet), **relay** (a rendezvous both
can reach). This document is about the third, and about what a relay does
beyond routing.

---

## 1. What a relay is

A rendezvous point that does four things and knows nothing else:

1. **Admit** a node to a mesh. On connect it sends a nonce; the node answers
   `Register { mesh, node, cert, challenge_sig }`. The relay verifies the
   cert against the mesh **root public key it was configured with** — never
   the copy embedded in the cert — and the challenge signature against the
   cert's ed25519 key. Membership and identity proven; nothing else learned.
2. **Route** frames by node name: `Route { to, data }` in, `Route { from,
   data }` out, `Undeliverable { to }` if the target isn't present.
3. **Presence**: `Welcome { peers }` on registration, `Presence { node,
   online }` after.
4. **Mailbox** (§4): keep sealed bus envelopes for nodes that are absent.

`data` is opaque. Two peers meeting through a relay run the ordinary
federation handshake and then sealed envelopes *over* it; they authenticate
each other end to end and never trust the relay. A fully compromised relay
yields metadata (mesh and node names, public keys, who-talks-to-whom-when,
frame sizes) and denial of service — not contents, not the ability to
inject, not command.

## 2. Three hosts, one protocol

| host | where | tenancy |
|---|---|---|
| **embedded** (`/api/federation/relay` on every node, v0.7.0) | the daemon itself | that node's mesh only; needs only the root public key it already holds |
| **standalone `aspen-relay`** | a small container/VPS; TLS in front | one mesh: `--mesh --root-pubkey` |
| **Cloudflare Worker + Durable Object** | `rendezvous/cloudflare` | allowlisted meshes via the `MESH_ROOTS` secret; one DO per mesh; `?mesh=NAME` |

A node cares only about the URL. The embedded host makes the common case
free: a reachable node — the root, listening beyond loopback — is the
rendezvous for everyone that only dials out, with no extra process. The
other two are for when no node is reachable from everywhere.

**Tenancy is closed by configuration**, on purpose. Verifying a cert against
its own embedded root proves nothing (anyone can mint a root and certify
themselves). A genuinely public relay is a policy decision, not a crypto
one: key tenants by root-public-key fingerprint (two strangers will both
name a mesh `home`), and answer the abuse questions (who pays, rate
limits, no mailbox for strangers). Not built.

## 3. Several relays per node

`mesh.json` carries `relays: [...]` (the legacy single `relay` field is
folded in on read). `aspen mesh relay <url>` adds one (idempotent),
`--remove` drops one, no argument clears all; the console lists them with
state and add/remove. A node keeps a client on every relay it lists and
opens **one link per peer**, through whichever path presents the peer
first. A join bundle made with `--url` carries the certifier's relay, so
new nodes inherit one.

Typical shape: the root's embedded relay on the LAN plus a Cloudflare
relay for when the machines are apart. Cost: one idle WebSocket per relay
per node.

When a relay reports a peer offline, the link riding that relay is torn
down immediately — so pending mail takes the mailbox rather than a dead
session, and so a peer reachable through another relay gets a fresh link.

## 4. The mailbox — why it is at the bus layer

The naive spool ("keep any frame for an offline node") is wrong. Frames
after the handshake belong to a *link session*: nonces, the peer's cert
from *that* hello. Replaying them into a later session is garbage. Bus
envelopes are different: they are sealed to the recipient's **static**
keys (identity x25519), so they open without any session.

So the mailbox is a second path alongside links, for bus traffic only:

- A node with pending rows for a peer it has **no live link to** seals
  each row's bus frame to the peer's cert and sends `Store { to, id, data }`
  to a connected relay (`id` = the row's uuid; same sender+id replaces).
- The relay delivers `Mail { from, id, data }` when the recipient registers
  — or immediately if it is present.
- The recipient opens it with the sender's cert (on file — a peer it has
  met; certs learned over verified links are recorded, §6), inserts the
  row, and acks: by link if one is up, else `Store` back through the
  mailbox.
- The origin keeps its row **pending until that ack** — the same
  at-least-once rule as links (`bus_ack`), so a lost mail is simply
  re-handed. Hand-offs repeat at most every 10 minutes per row while it
  stays pending; a link going down re-ticks that peer's pending rows at
  once.

This is what lets a message sent at 11pm to a machine that is off arrive
when it boots at 8am, with the sender closed in between: the relay holds
the envelope, the mini drains it, the ack waits in the mailbox for the
laptop.

**Bounds** (`aspen-wire::relay::MAILBOX_*`, mirrored in the worker): 200
items and 2 MB per recipient, 7-day TTL, `MailboxFull { to }` refuses and
the origin retries later. The embedded and standalone hosts keep the
mailbox in memory (lost on restart — the origin re-hands); the worker
keeps it in Durable Object storage and sweeps by alarm.

## 5. The Cloudflare worker

Rewritten (2026-09-05) on the **WebSocket hibernation API**: the object is
evicted while idle and billed nothing, sockets stay open, and per-socket
state (challenge nonce, node name) rides in the socket attachment so it
survives eviction. One socket per node name (a newer registration replaces
an older). Mail lives in DO storage under `mail:<to>:<from>:<id>`; a
6-hourly alarm drops expired items while any remain. Exercised locally
with `wrangler dev` against real nodes: two loopback nodes linked through
it, and a message stored while the peer was down landed on its return and
was acked back.

## 6. Certs learned over links

The join bundle only ever carried the certifier's cert. A peer met through
a relay presents a root-signed cert in its hello; that used to be trusted
"for the session" and forgotten — and since envelopes are sealed *to* the
recipient's cert, the peer's roster was silently undeliverable. Now a
valid root-signed cert not on file is **recorded**: in memory for
`send_to`, and in `mesh.json` with no dial URL, so the peer is a known
member from then on (the console shows it as reached via relay/inbound).
Certs are public facts; recording one grants nothing the root signature
hadn't already.

## 7. Mesh where it can be, spokes where it must be (built 2026-09-05)

Reachability decides the shape of a mesh: with one node listening beyond
loopback you get hub-and-spoke through it. Three mechanisms turn that into
a mesh wherever the network allows, without touching config:

**Advertise.** Every roster carries `advertised { dial_urls, relay_urls }`:
when the node listens beyond loopback, its federation endpoint as its
hostname and as every non-loopback IPv4, and the relay it hosts at the
same addresses; plus anything the operator set with `aspen config
advertise <url>[,<url>]` (a tailnet name, a port-forward). A loopback-only
node advertises nothing — it is a **spoke by its own choice**, and the
console says so on its row.

**Direct first.** A dialer tries every candidate for a peer — the
configured URL, then what the peer advertises — round-robin every 5s while
there is no link, every 30s while a relay link carries the peer. A relay
link no longer stops the dialer: when the direct hello completes it
**supersedes** the relay link (the relay-side channel is dropped; that
session ends without disturbing the live one). `link_kind` records how
each peer is reached (`direct` / `relay:<url>`); the console and `aspen
status` show it.

**Fall back.** When a direct link drops, the lower-named side starts a
relay link at once on any relay where the peer is present (rosters and
`Welcome`/`Presence` keep a present-set per relay session); the peer's
side does the same. Relays a peer *hosts* are **discovered** from its
advertisement and joined automatically — as fallback paths, listed in the
console as *discovered from X*, never persisted — but only from peers we
have no configured dial URL to (a dialed peer's relay is at the same
address; set it with `aspen mesh relay` if wanted), one per peer, and
pruned when the peer stops advertising it.

**One relay, many names.** The root advertises its relay as
`ws://anindor:7693/…` and `ws://172.28.…:7693/…`; a client that already
sits on `ws://127.0.0.1:7693/…` must not open a second session to the same
relay — two sessions interleave one handshake and neither link forms
(observed). The embedded relay therefore says which node it is in
`Welcome { host }`; a client that already has a session to that host drops
the duplicate and forgets the discovered URL. The Rust hosts also fixed a
teardown bug on the way: an older socket closing for a node name that a
newer socket had replaced used to unregister the newer one.

Verified on three nodes: j1 (loopback) and j2 (beyond loopback) meet
through the root's relay; j1 learns j2's address from the roster, dials
it, and the direct link supersedes the relay one; j2 restarted
loopback-only → j1's direct link drops and falls back to the relay path
within seconds, and drives j2's agent over it.

## 8. Keepalive (2026-09-06)

A relay that restarts — a worker deploy, a host bounce — drops its sockets
without a close frame reaching the client; TCP alone never says so. Seen
live: a node reported its cloud relay *connected* for an hour after a
deploy had emptied the room, so a node that joined later found nobody
there. Now the client sends the text frame `ping` every 20s and treats 45s
of silence as a dead session (reconnect, re-register, re-link); the Rust
hosts answer `pong` in-loop, and the worker answers through
`setWebSocketAutoResponse` — the runtime replies without waking the
object, so hibernation still costs nothing. The relay row in the console
shows who else the relay says is present; that is the first thing to look
at when two nodes on one relay don't see each other.

**Restarts, precisely (2026-09-06).** Three more rules fell out of a
restart storm run against the deployed worker (a node restarted three
times, then both at once; every round must carry traffic afterwards):

- Only a **hello** may start a relay link. Any other frame from a peer we
  have no link with is dropped. Before, a stray mid-handshake or sealed
  frame from a session that had just died spawned a fresh link that choked
  on it — and its failure produced the next stray frame: a cascade of dead
  links every 100 ms, seen live between a laptop and a WSL node.
- A hello from a peer that dials us **replaces** the link we had with it
  (it is telling us it started over). A hello from a peer *we* dial is
  the reply to our attempt when one is in flight, and ignored otherwise.
- A relay that sees a node **re-register while its old socket is still
  there** announces `offline` for it before `online`, so peers linked over
  the old socket drop that link at once instead of sending into it.
  Handshakes time out after 15s so a crossed attempt never holds a peer's
  slot.

## 9. Reach memory: backoff, timeouts, alternates (2026-09-06)

An audit on the live mesh found three kinds of waste, all from dialing
addresses that could never work. A WSL node carries the Windows host's
name, which from the mac resolves to the Windows box (so `anindor:7421`
black-holed: a 75s OS connect timeout, every ~80s, all day), and from the
WSL side resolves only to IPv6 while the Windows node listens on IPv4 (so
the configured relay `ws://anindor:7420/…` was refused every 5s, forever,
at INFO — while the direct link to the same node was up by IP). And a
black-holed candidate held the peer's whole dialer for the OS timeout,
so the working candidates behind it waited.

Now every URL a node dials — a peer's configured URL, its advertised
addresses, a relay and its aliases — has **reach memory**
(`MeshState::reach`): consecutive failures, the moment it may be tried
again, when it last worked, the last error.

- **Timeout.** Every dial is bounded by `DIAL_TIMEOUT` (10s). A black
  hole costs ten seconds, once, then backs off.
- **Backoff.** Failures double the wait from 5s, capped at 60s for a URL
  the operator configured and 10 minutes for one merely learned
  (advertised, discovered). Success resets it. The first failure of a
  URL logs at INFO; the rest at DEBUG, so a dead address is one line,
  not a heartbeat.
- **Order.** Candidates are tried best first: the one that last worked,
  then the never-tried, then the rest by fewest failures — and a URL in
  its backoff window is skipped. A peer carried by a relay link is
  probed for a direct path every 30s, unless an untried candidate exists,
  which is probed at once. The list is recomputed for that decision,
  because the peer's advertisement usually lands during the first dial.
- **Relay aliases.** A configured relay URL that a peer advertises among
  its `relay_urls` is that peer's relay; the other URLs in that set are
  aliases for the same host, and the relay client dials whichever is
  reachable, keeping the configured URL as the session's identity
  (`Welcome.host` dedupe still applies).
- **Hostname honesty.** A node advertises its hostname only when that
  name resolves to one of its own non-loopback IPv4 addresses (cached
  five minutes). The WSL node no longer sends peers to the Windows box.

The console shows it: a peer row with several paths reads "N paths · M
backing off", and its tooltip lists each — proven, untried, or failing
with the error and the seconds until the next try (`candidates` in
`GET /api/mesh`).

Verified on the rig: a relay configured under an IPv6-only alias is
refused once, then reached under its IPv4 alias five seconds later, and
peers link over it; a black-holed configured dial URL fails in 10s and
the peer's advertised working address connects one second later,
superseding the relay link; a hostname that does not resolve to the node
is dropped from its advertisement.

Two more things came out of the audit. The daemon leaked `ASPEN_DETACHED`
into the sessions it spawns, so `aspen up -d` run inside a session ran the
node in the foreground; sessions no longer inherit it. And a daemon wedged
during shutdown (a lock held across the session ladder) ignored SIGTERM;
a second signal, or 60s, now ends the process outright — the live marks
are already on disk.

## 10. Not built

Console-through-relay (DESIGN §7 mentions it; the relay routes node↔node
only), a public multi-tenant relay (§2), rate limiting beyond the
platform's, a persistent mailbox for the Rust hosts, relay preference
order (the list is nominally ordered; nothing consumes the order yet —
the console offers no reordering for that reason), and IPv6 in
advertisements.
