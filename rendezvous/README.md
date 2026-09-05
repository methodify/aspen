# Aspen rendezvous

The minimal cloud piece: authenticate nodes to a mesh, route sealed frames
between them by name, report presence. It holds only the mesh **root public
key** — enough to verify membership, never to forge it — and every routed
frame is a `SealedEnvelope` it cannot read. A fully compromised rendezvous
yields metadata and denial of service, not command and control.

Three interchangeable implementations speak the identical protocol
(`aspen-wire::relay`). Every **node** hosts one at
`ws://<host>:<port>/api/federation/relay` (v0.7.0+) — a reachable node such
as the root is usually all a mesh needs. The two below are for when no node
is reachable from everywhere (nodes behind different NATs, laptops on the
road):

## 1. Standalone binary (`aspen-relay`)

The self-hostable reference — a single Rust binary, ideal for a tiny
container (Fly, Hetzner, a VPS).

```bash
cargo build --release -p aspen-relay
aspen-relay --listen 0.0.0.0:7440 \
            --mesh mymesh \
            --root-pubkey "$(aspen mesh root-pubkey)"   # run on a mesh node
```

Nodes then point at it:

```bash
aspen mesh relay wss://relay.example.com/relay   # on each node
```

(Terminate TLS at a reverse proxy, or front it with anything that speaks
WSS. The relay itself serves plain ws.)

## 2. Cloudflare Workers + Durable Objects (`cloudflare/`)

The nearly-free hosted option — WebSocket hibernation keeps idle meshes off
the meter. One Durable Object instance per mesh owns its live node sockets.

```bash
cd cloudflare
npm install
npx wrangler login               # OAuth in the browser
npx wrangler deploy              # prints https://aspen-rendezvous.<subdomain>.workers.dev
echo '{"mymesh":"<base64 root pubkey>"}' | npx wrangler secret put MESH_ROOTS
```

The Durable Object class is declared with `new_sqlite_classes`, which the
free plan requires (and which is what alarms and storage need anyway).
Re-run `secret put` with the whole JSON to add or remove a mesh.

Nodes point at the Worker with the mesh in the query string:

```bash
aspen mesh relay wss://aspen-rendezvous.<subdomain>.workers.dev/relay?mesh=mymesh
```

Sockets use the Workers **hibernation API**: the object sleeps while idle
(no duration billed) and per-socket state rides in the socket attachment;
mail waits in Durable Object storage and is swept by alarm. Local test:
`echo 'MESH_ROOTS={"mymesh":"<root pubkey>"}' > .dev.vars && npx wrangler dev`,
then `aspen mesh relay ws://127.0.0.1:8787/relay?mesh=mymesh` on a node.

## Several relays per node

`aspen mesh relay <url>` **adds** a relay (idempotent); `--remove` drops one;
no argument clears all. A node keeps a client on every relay it lists and
reaches a peer through whichever presents it first — the root's embedded
relay on the LAN and a Cloudflare one for the road can both be set. A join
bundle carries the certifier's relay, so new nodes inherit one.

## The mailbox

Live links are sessions; a frame from one cannot be replayed into a later
one. So the relay also keeps a **mailbox at the bus layer**: a node with
pending mail for a peer it has no live link to hands the relay a sealed
envelope (`store`); the relay delivers it (`mail`) the moment the peer
registers — or immediately if the peer is present. Envelopes are sealed
to the recipient's static keys, so they open without a session. The
recipient acks through the same path; the origin keeps its row pending
until that ack — at-least-once, end to end. Bounded per recipient (200
items / 2 MB), 7-day TTL; a full box answers `mailbox_full` and the origin
retries later.

Both implementations are modular by design — a node cares only about the
`wss://…/relay` URL, never which one answers. An Azure Functions port can be
added later against the same `aspen-wire::relay` contract.

## Protocol (for a third implementation)

1. On connect the relay sends `Challenge { nonce }`.
2. The node replies `Register { mesh, node, cert, challenge_sig }`. The
   relay verifies `cert` against its configured mesh root public key
   (`aspen-cert-v1` context) and `challenge_sig` against the cert's ed25519
   key (`aspen-relay-challenge-v1` context).
3. The relay replies `Welcome { peers }` and thereafter pushes
   `Presence { node, online }`.
4. Frames route by name: a node sends `Route { to, data }`; the relay
   delivers `Route { from, data }` to the target, or `Undeliverable { to }`
   back. `data` is an opaque federation frame (a sealed envelope after the
   node-to-node handshake completes).
5. Mailbox: `Store { to, id, data }` keeps a sealed bus envelope for an
   absent node (same `from`+`id` replaces); `Mail { from, id, data }` delivers
   it on registration or at once; `MailboxFull { to }` refuses.

The relay never inspects `data`. Node pairs run the full mutually-
authenticating federation handshake end-to-end over this routing, so the
relay is a dumb pipe between two peers who trust each other, not it.
