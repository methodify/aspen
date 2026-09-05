// Aspen rendezvous — Cloudflare Workers + Durable Objects.
//
// Speaks the identical protocol to the standalone `aspen-relay` binary and
// the relay embedded in every node (aspen-wire::relay): challenge →
// register (cert verified against the mesh root public key + an ed25519
// challenge signature) → route sealed frames by node name → presence →
// mailbox. It holds only the mesh ROOT PUBLIC key and reads nothing it
// routes; every routed frame is a SealedEnvelope.
//
// One Durable Object per mesh owns that mesh's live sockets and mailbox.
// Sockets use the WebSocket Hibernation API: the object is evicted while
// idle (no duration billed), sockets stay open, and per-socket state
// (nonce, node name) rides in the socket attachment so it survives.
// Mail waiting for offline nodes lives in DO storage, bounded per
// recipient and swept by alarm.
//
// Deploy: `wrangler deploy` (see wrangler.toml). Mesh root public keys go
// in the MESH_ROOTS secret: JSON { meshName: base64RootPubkey }.

import { verifyAsync } from '@noble/ed25519';

// Mirrors aspen-wire::relay MAILBOX_* — keep in step.
const MAILBOX_MAX_ITEMS = 200;
const MAILBOX_MAX_BYTES = 2 * 1024 * 1024;
const MAILBOX_TTL_MS = 7 * 24 * 3600 * 1000;
const SWEEP_EVERY_MS = 6 * 3600 * 1000;

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

// ---- encoding helpers ---------------------------------------------------

function b64(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
function b64encode(bytes) {
  let s = '';
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

// The signed-challenge context — must byte-match aspen-wire::relay::
// challenge_context (v1): "aspen-relay-challenge-v1\0" mesh \0 node \0 nonce.
function challengeContext(mesh, node, nonce) {
  const head = new TextEncoder().encode('aspen-relay-challenge-v1\0' + mesh + '\0' + node + '\0');
  const out = new Uint8Array(head.length + nonce.length);
  out.set(head, 0);
  out.set(nonce, head.length);
  return out;
}

// The cert-signing context — must byte-match aspen-wire::identity::
// cert_signing_bytes (v1): "aspen-cert-v1\0" mesh \0 node \0 ed_pub x_pub.
function certContext(mesh, node, edPub, xPub) {
  const head = new TextEncoder().encode('aspen-cert-v1\0' + mesh + '\0' + node + '\0');
  const out = new Uint8Array(head.length + edPub.length + xPub.length);
  out.set(head, 0);
  out.set(edPub, head.length);
  out.set(xPub, head.length + edPub.length);
  return out;
}

async function verifyRegister(msg, mesh, rootPub, nonce) {
  if (!msg || typeof msg !== 'object') return 'bad register';
  if (msg.mesh !== mesh) return `wrong mesh: relay serves '${mesh}', node claims '${msg.mesh}'`;
  const cert = msg.cert;
  if (!cert || cert.node !== msg.node) return 'cert node name does not match register';
  if (cert.mesh !== mesh) return 'cert is for another mesh';
  let edPub, xPub, rootSig, chalSig;
  try {
    edPub = b64(cert.ed_public);
    xPub = b64(cert.x_public);
    rootSig = b64(cert.root_sig);
    chalSig = b64(msg.challenge_sig);
  } catch {
    return 'malformed base64 in register';
  }
  const certOk = await verifyAsync(rootSig, certContext(mesh, cert.node, edPub, xPub), rootPub);
  if (!certOk) return 'cert not valid for this mesh';
  const chalOk = await verifyAsync(chalSig, challengeContext(mesh, msg.node, nonce), edPub);
  if (!chalOk) return 'challenge signature invalid';
  return true;
}

// ---- the mesh object ----------------------------------------------------

export class MeshRelay {
  constructor(state, env) {
    this.state = state;
    this.env = env;
  }

  rootFor(mesh) {
    const roots = JSON.parse(this.env.MESH_ROOTS || '{}');
    return roots[mesh] ? b64(roots[mesh]) : null;
  }

  // Every live socket with its attachment { mesh, node|null, nonce }.
  sockets() {
    return this.state.getWebSockets().map((ws) => [ws, ws.deserializeAttachment() || {}]);
  }
  socketFor(node) {
    for (const [ws, a] of this.sockets()) if (a.node === node) return ws;
    return null;
  }
  presentNodes() {
    return this.sockets().map(([, a]) => a.node).filter(Boolean);
  }

  async fetch(request) {
    const url = new URL(request.url);
    const mesh = url.searchParams.get('mesh');
    const rootPub = this.rootFor(mesh);
    if (!rootPub) return new Response('unknown mesh', { status: 403 });
    if (request.headers.get('Upgrade') !== 'websocket') {
      return new Response('websocket only', { status: 426 });
    }

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    // Hibernation API: the runtime owns the socket; we get callbacks.
    this.state.acceptWebSocket(server);

    // 1. Challenge. The nonce rides in the attachment until Register.
    const nonce = crypto.getRandomValues(new Uint8Array(32));
    server.serializeAttachment({ mesh, node: null, nonce: b64encode(nonce) });
    server.send(JSON.stringify({ nonce: b64encode(nonce) }));

    if (!(await this.state.storage.getAlarm())) {
      await this.state.storage.setAlarm(Date.now() + SWEEP_EVERY_MS);
    }
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws, data) {
    let msg;
    try {
      msg = JSON.parse(typeof data === 'string' ? data : new TextDecoder().decode(data));
    } catch {
      return;
    }
    const a = ws.deserializeAttachment() || {};

    // 2. Register (first message on a socket that hasn't).
    if (!a.node) {
      const rootPub = this.rootFor(a.mesh);
      let ok;
      try {
        ok = await verifyRegister(msg, a.mesh, rootPub, b64(a.nonce));
      } catch (e) {
        ok = 'verify error: ' + e;
      }
      if (ok !== true) {
        ws.send(JSON.stringify({ t: 'rejected', reason: ok }));
        ws.close(1008, 'rejected');
        return;
      }
      // One socket per node name: a newer registration replaces an older.
      const stale = this.socketFor(msg.node);
      if (stale && stale !== ws) {
        try { stale.close(1000, 'replaced'); } catch {}
      }
      const peers = this.presentNodes().filter((n) => n !== msg.node);
      ws.serializeAttachment({ mesh: a.mesh, node: msg.node, nonce: null });
      ws.send(JSON.stringify({ t: 'welcome', peers }));
      this.broadcastPresence(msg.node, true);
      await this.drainMail(ws, msg.node);
      return;
    }

    // 3. Routing.
    if (msg.t === 'route' && msg.to) {
      const dest = this.socketFor(msg.to);
      if (dest) {
        dest.send(JSON.stringify({ t: 'route', from: a.node, data: msg.data }));
      } else {
        ws.send(JSON.stringify({ t: 'undeliverable', to: msg.to }));
      }
      return;
    }

    // 4. Mailbox: hand over now if present, else keep (bounded).
    if (msg.t === 'store' && msg.to && msg.id != null) {
      const dest = this.socketFor(msg.to);
      if (dest) {
        dest.send(JSON.stringify({ t: 'mail', from: a.node, id: msg.id, data: msg.data }));
        return;
      }
      const stored = await this.storeMail(msg.to, a.node, msg.id, msg.data);
      if (!stored) ws.send(JSON.stringify({ t: 'mailbox_full', to: msg.to }));
    }
  }

  async webSocketClose(ws) {
    this.onGone(ws);
  }
  async webSocketError(ws) {
    this.onGone(ws);
  }
  onGone(ws) {
    const a = ws.deserializeAttachment() || {};
    if (a.node && this.socketFor(a.node) === null) this.broadcastPresence(a.node, false);
  }

  broadcastPresence(node, online) {
    const msg = JSON.stringify({ t: 'presence', node, online });
    for (const [ws, a] of this.sockets()) {
      if (a.node && a.node !== node) {
        try { ws.send(msg); } catch {}
      }
    }
  }

  // ---- mailbox in DO storage: key "mail:<to>:<from>:<id>" -------------

  async storeMail(to, from, id, data) {
    const prefix = `mail:${to}:`;
    const existing = await this.state.storage.list({ prefix });
    let count = 0;
    let bytes = 0;
    for (const [k, v] of existing) {
      if (k === `${prefix}${from}:${id}`) continue; // replaced below
      count++;
      bytes += v.data.length;
    }
    if (count >= MAILBOX_MAX_ITEMS || bytes + data.length > MAILBOX_MAX_BYTES) return false;
    await this.state.storage.put(`${prefix}${from}:${id}`, { from, id, data, at: Date.now() });
    return true;
  }

  async drainMail(ws, node) {
    const prefix = `mail:${node}:`;
    const items = await this.state.storage.list({ prefix });
    if (items.size === 0) return;
    const cutoff = Date.now() - MAILBOX_TTL_MS;
    const sorted = [...items.entries()].sort((x, y) => x[1].at - y[1].at);
    for (const [k, v] of sorted) {
      if (v.at >= cutoff) {
        try {
          ws.send(JSON.stringify({ t: 'mail', from: v.from, id: v.id, data: v.data }));
        } catch {
          break;
        }
      }
      await this.state.storage.delete(k);
    }
  }

  // Alarm: drop expired mail; re-arm while any remains.
  async alarm() {
    const all = await this.state.storage.list({ prefix: 'mail:' });
    const cutoff = Date.now() - MAILBOX_TTL_MS;
    let remaining = 0;
    for (const [k, v] of all) {
      if (v.at < cutoff) await this.state.storage.delete(k);
      else remaining++;
    }
    if (remaining > 0) await this.state.storage.setAlarm(Date.now() + SWEEP_EVERY_MS);
  }
}
