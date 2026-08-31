# Aspen rendezvous

The minimal cloud piece: authenticate nodes to a mesh, route sealed frames
between them by name, report presence. It holds only the mesh **root public
key** — enough to verify membership, never to forge it — and every routed
frame is a `SealedEnvelope` it cannot read. A fully compromised rendezvous
yields metadata and denial of service, not command and control.

Two interchangeable implementations speak the identical protocol
(`aspen-wire::relay`):

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
wrangler secret put MESH_ROOTS   # JSON: { "mymesh": "<base64 root pubkey>" }
wrangler deploy
```

Nodes point at the Worker with the mesh in the query string:

```bash
aspen mesh relay wss://aspen-rendezvous.<subdomain>.workers.dev/relay?mesh=mymesh
```

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

The relay never inspects `data`. Node pairs run the full mutually-
authenticating federation handshake end-to-end over this routing, so the
relay is a dumb pipe between two peers who trust each other, not it.
