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

## 8. Not built

Console-through-relay (DESIGN §7 mentions it; the relay routes node↔node
only), a public multi-tenant relay (§2), rate limiting beyond the
platform's, a persistent mailbox for the Rust hosts, relay preference
order (the list is nominally ordered; nothing consumes the order yet —
the console offers no reordering for that reason), and IPv6 in
advertisements.
