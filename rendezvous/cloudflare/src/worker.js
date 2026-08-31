// Aspen rendezvous — Cloudflare Workers + Durable Objects port.
//
// Speaks the identical protocol to the standalone `aspen-relay` binary
// (aspen-wire::relay): challenge → register (cert verified against the mesh
// root public key + an ed25519 challenge signature) → route sealed frames
// by node name → presence. It holds only the mesh ROOT PUBLIC key and reads
// nothing it routes; every routed frame is a SealedEnvelope.
//
// One Durable Object instance per mesh (named by mesh id) owns the set of
// live node sockets. WebSocket hibernation keeps idle meshes ~free.
//
// Deploy: `wrangler deploy` (see wrangler.toml). Configure the mesh root
// public key(s) via the MESH_ROOTS secret: a JSON object { meshName:
// base64RootPubkey }.

import { verifyAsync } from '@noble/ed25519';

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === '/healthz') return new Response('ok');
    if (url.pathname !== '/relay') return new Response('not found', { status: 404 });

    const mesh = url.searchParams.get('mesh');
    if (!mesh) return new Response('missing ?mesh', { status: 400 });

    // One DO per mesh.
    const id = env.MESH.idFromName(mesh);
    return env.MESH.get(id).fetch(request);
  },
};

// base64 → Uint8Array
function b64(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// The signed-challenge context — must byte-match aspen-wire::relay::
// challenge_context (v1). Layout: "aspen-relay-challenge-v1\0" mesh \0 node
// \0 nonce.
function challengeContext(mesh, node, nonce) {
  const enc = new TextEncoder();
  const head = enc.encode('aspen-relay-challenge-v1\0' + mesh + '\0' + node + '\0');
  const out = new Uint8Array(head.length + nonce.length);
  out.set(head, 0);
  out.set(nonce, head.length);
  return out;
}

// The cert-signing context — must byte-match aspen-wire::identity::
// cert_signing_bytes (v1): "aspen-cert-v1\0" mesh \0 node \0 ed_pub x_pub.
function certContext(mesh, node, edPub, xPub) {
  const enc = new TextEncoder();
  const head = enc.encode('aspen-cert-v1\0' + mesh + '\0' + node + '\0');
  const out = new Uint8Array(head.length + edPub.length + xPub.length);
  out.set(head, 0);
  out.set(edPub, head.length);
  out.set(xPub, head.length + edPub.length);
  return out;
}

export class MeshRelay {
  constructor(state, env) {
    this.state = state;
    this.env = env;
    this.nodes = new Map(); // node name -> WebSocket
  }

  rootFor(mesh) {
    const roots = JSON.parse(this.env.MESH_ROOTS || '{}');
    return roots[mesh] ? b64(roots[mesh]) : null;
  }

  async fetch(request) {
    const url = new URL(request.url);
    const mesh = url.searchParams.get('mesh');
    const rootPub = this.rootFor(mesh);
    if (!rootPub) return new Response('unknown mesh', { status: 403 });

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    server.accept();
    this.handle(server, mesh, rootPub);
    return new Response(null, { status: 101, webSocket: client });
  }

  async handle(ws, mesh, rootPub) {
    // 1. Challenge.
    const nonce = crypto.getRandomValues(new Uint8Array(32));
    ws.send(JSON.stringify({ nonce: b64encode(nonce) }));

    let node = null;
    ws.addEventListener('message', async (evt) => {
      let msg;
      try { msg = JSON.parse(evt.data); } catch { return; }

      // Registration (first authenticated message).
      if (node === null) {
        try {
          const ok = await this.verifyRegister(msg, mesh, rootPub, nonce);
          if (ok !== true) {
            ws.send(JSON.stringify({ t: 'rejected', reason: ok }));
            ws.close();
            return;
          }
        } catch (e) {
          ws.send(JSON.stringify({ t: 'rejected', reason: 'verify error: ' + e }));
          ws.close();
          return;
        }
        node = msg.node;
        const peers = [...this.nodes.keys()];
        this.nodes.set(node, ws);
        ws.send(JSON.stringify({ t: 'welcome', peers }));
        this.broadcastPresence(node, true);
        return;
      }

      // Routing.
      if (msg.t === 'route' && msg.to) {
        const dest = this.nodes.get(msg.to);
        if (dest) {
          dest.send(JSON.stringify({ t: 'route', from: node, data: msg.data }));
        } else {
          ws.send(JSON.stringify({ t: 'undeliverable', to: msg.to }));
        }
      }
    });

    const close = () => {
      if (node && this.nodes.get(node) === ws) {
        this.nodes.delete(node);
        this.broadcastPresence(node, false);
      }
    };
    ws.addEventListener('close', close);
    ws.addEventListener('error', close);
  }

  async verifyRegister(msg, mesh, rootPub, nonce) {
    if (msg.mesh !== mesh) return 'wrong mesh';
    const cert = msg.cert;
    if (!cert || cert.node !== msg.node) return 'cert/name mismatch';
    const edPub = b64(cert.ed_public);
    const xPub = b64(cert.x_public);
    // Membership: cert signed by the mesh root.
    const certOk = await verifyAsync(
      b64(cert.root_sig),
      certContext(cert.mesh, cert.node, edPub, xPub),
      rootPub,
    );
    if (!certOk) return 'cert not signed by mesh root';
    // Identity: challenge signed by the node's ed key.
    const chalOk = await verifyAsync(
      b64(msg.challenge_sig),
      challengeContext(mesh, msg.node, nonce),
      edPub,
    );
    if (!chalOk) return 'challenge signature invalid';
    return true;
  }

  broadcastPresence(node, online) {
    const msg = JSON.stringify({ t: 'presence', node, online });
    for (const [name, sock] of this.nodes) {
      if (name !== node) {
        try { sock.send(msg); } catch { /* dropped */ }
      }
    }
  }
}

function b64encode(bytes) {
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}
