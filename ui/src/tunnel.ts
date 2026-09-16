// The console as a mesh peer (docs/RELAY.md §11, PROPOSALS-B §10).
//
// When the browser cannot reach any node's HTTP API directly, it can
// still reach a relay. This module makes the console a member: its own
// ed25519 + x25519 keypair (kept in localStorage), a root-signed cert
// minted by `aspen mesh certify` from the enroll blob it prints, and the
// federation protocol itself — challenge/register with the relay, hello +
// nonce proof with one node, then sealed envelopes carrying `api_req`
// (an `http` op the node dispatches into its own router) and `sub`/`ev`
// for a session's live events. The relay reads nothing; the node grants a
// console observe + control, never spawn or trust.
//
// Wire formats mirror aspen-wire exactly: base64 fields, the
// `aspen-env-v1` signing bytes, XChaCha20-Poly1305 over the raw x25519
// shared secret, the `aspen-relay-challenge-v1` context.

import { ed25519, x25519 } from "@noble/curves/ed25519";
import { xchacha20poly1305 } from "@noble/ciphers/chacha";
import { scoped } from "./profiles";

export interface NodeCert {
  mesh: string;
  node: string;
  ed_public: string;
  x_public: string;
  root_public: string;
  root_sig: string;
}

export interface ConsoleIdentity {
  node: string;
  ed_secret: string;
  ed_public: string;
  x_secret: string;
  x_public: string;
  cert?: NodeCert | null;
  /** Certs learned from bundles/hellos, by node name (sealing targets). */
  known?: Record<string, NodeCert>;
  /** The relay from the join bundle, if any. */
  relay?: string | null;
}

export interface TunnelConfig {
  enabled: boolean;
  relay: string;
  node: string;
}

const ID_KEY = "aspen.console.identity";
const CFG_KEY = "aspen.console.tunnel";

// ── base64 / bytes ──────────────────────────────────────────────────────

export function b64e(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}
export function b64d(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
const te = new TextEncoder();
const td = new TextDecoder();
function cat(...parts: Uint8Array[]): Uint8Array {
  const n = parts.reduce((a, p) => a + p.length, 0);
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}
const Z = new Uint8Array([0]);

// ── identity ────────────────────────────────────────────────────────────

export function loadIdentity(): ConsoleIdentity | null {
  try {
    const raw = localStorage.getItem(scoped(ID_KEY));
    return raw ? (JSON.parse(raw) as ConsoleIdentity) : null;
  } catch {
    return null;
  }
}
export function saveIdentity(id: ConsoleIdentity): void {
  // Never overwrite a certified identity for one mesh with one certified
  // for another: a save that would do that is a bug upstream, not a
  // change of mind (a new identity for a mesh starts uncertified).
  try {
    const cur = loadIdentity();
    if (cur?.cert && id.cert && cur.cert.mesh !== id.cert.mesh && cur.node !== id.node) {
      console.warn(`aspen: refusing to overwrite ${cur.node} (${cur.cert.mesh}) with ${id.node} (${id.cert.mesh})`);
      return;
    }
  } catch {
    /* fall through */
  }
  localStorage.setItem(scoped(ID_KEY), JSON.stringify(id));
}
/** A console name the mesh accepts: `console-` plus a slug. The prefix
 *  is what nodes key on (never dialed, observe + control only); the slug
 *  is the operator's ("bryons-phone"), else random. */
export function consoleNodeName(name?: string): string {
  const slug = (name ?? "")
    .toLowerCase()
    .replace(/^console-/, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  if (slug) return `console-${slug}`;
  const suffix = b64e(crypto.getRandomValues(new Uint8Array(6))).replace(/[^a-zA-Z0-9]/g, "").slice(0, 8).toLowerCase();
  return `console-${suffix}`;
}
export function createIdentity(name?: string): ConsoleIdentity {
  const edSecret = ed25519.utils.randomPrivateKey();
  const xSecret = x25519.utils.randomPrivateKey();
  const id: ConsoleIdentity = {
    node: consoleNodeName(name),
    ed_secret: b64e(edSecret),
    ed_public: b64e(ed25519.getPublicKey(edSecret)),
    x_secret: b64e(xSecret),
    x_public: b64e(x25519.getPublicKey(xSecret)),
    cert: null,
    known: {},
    relay: null,
  };
  saveIdentity(id);
  return id;
}
export function enrollBlob(id: ConsoleIdentity): string {
  return `aspen:enroll:${btoa(JSON.stringify({ node: id.node, ed_public: id.ed_public, x_public: id.x_public }))}`;
}

function certSigningBytes(c: { mesh: string; node: string; ed_public: string; x_public: string }): Uint8Array {
  return cat(te.encode("aspen-cert-v1\0"), te.encode(c.mesh), Z, te.encode(c.node), Z, b64d(c.ed_public), b64d(c.x_public));
}
export function verifyCert(c: NodeCert, rootPublic: string): boolean {
  try {
    return ed25519.verify(b64d(c.root_sig), certSigningBytes(c), b64d(rootPublic));
  } catch {
    return false;
  }
}

/** Install `aspen:bundle:…` (preferred: carries the certifier's cert and
 *  the relay) or `aspen:cert:…` for this identity. */
export function installBlob(id: ConsoleIdentity, blob: string): ConsoleIdentity {
  const t = blob.trim();
  const parse = (tag: string) => {
    const rest = t.startsWith(`aspen:${tag}:`) ? t.slice(`aspen:${tag}:`.length) : null;
    return rest ? (JSON.parse(atob(rest)) as Record<string, unknown>) : null;
  };
  const bundle = parse("bundle");
  const cert = (bundle ? bundle["cert"] : parse("cert")) as NodeCert | null;
  if (!cert) throw new Error("expected an aspen:bundle:… or aspen:cert:… blob");
  if (cert.node !== id.node) throw new Error(`that cert names ${cert.node}; this console is ${id.node}`);
  if (cert.ed_public !== id.ed_public || cert.x_public !== id.x_public) throw new Error("that cert covers different keys than this console holds");
  if (!verifyCert(cert, cert.root_public)) throw new Error("cert signature does not verify");
  const next: ConsoleIdentity = { ...id, cert, known: { ...(id.known ?? {}) } };
  if (bundle) {
    const certifier = bundle["certifier"] as NodeCert | undefined;
    if (certifier && verifyCert(certifier, cert.root_public)) next.known![certifier.node] = certifier;
    const relay = bundle["relay"];
    if (typeof relay === "string" && relay) next.relay = relay;
  }
  saveIdentity(next);
  return next;
}

export function loadConfig(): TunnelConfig {
  try {
    const raw = localStorage.getItem(scoped(CFG_KEY));
    if (raw) return JSON.parse(raw) as TunnelConfig;
  } catch {
    // fall through
  }
  return { enabled: false, relay: "", node: "" };
}
export function saveConfig(c: TunnelConfig): void {
  localStorage.setItem(scoped(CFG_KEY), JSON.stringify(c));
}

// ── sealed envelopes ────────────────────────────────────────────────────

interface Envelope {
  v: number;
  from: string;
  to: string;
  nonce: string;
  ciphertext: string;
  sig: string;
}

function envSigningBytes(e: Envelope): Uint8Array {
  return cat(te.encode("aspen-env-v1\0"), new Uint8Array([e.v]), te.encode(e.from), Z, te.encode(e.to), Z, b64d(e.nonce), b64d(e.ciphertext));
}

function seal(id: ConsoleIdentity, their: NodeCert, payload: string): string {
  const shared = x25519.getSharedSecret(b64d(id.x_secret), b64d(their.x_public));
  const nonce = crypto.getRandomValues(new Uint8Array(24));
  const ct = xchacha20poly1305(shared, nonce).encrypt(te.encode(payload));
  const env: Envelope = { v: 1, from: id.node, to: their.node, nonce: b64e(nonce), ciphertext: b64e(ct), sig: "" };
  env.sig = b64e(ed25519.sign(envSigningBytes(env), b64d(id.ed_secret)));
  return JSON.stringify(env);
}

function open(id: ConsoleIdentity, their: NodeCert, frame: string): string {
  const env = JSON.parse(frame) as Envelope;
  if (env.from !== their.node) throw new Error(`envelope from ${env.from}, expected ${their.node}`);
  if (!ed25519.verify(b64d(env.sig), envSigningBytes(env), b64d(their.ed_public))) throw new Error("envelope signature invalid");
  const shared = x25519.getSharedSecret(b64d(id.x_secret), b64d(their.x_public));
  const pt = xchacha20poly1305(shared, b64d(env.nonce)).decrypt(b64d(env.ciphertext));
  return td.decode(pt);
}

// ── the tunnel ──────────────────────────────────────────────────────────

export type TunnelState = "off" | "connecting" | "registered" | "linking" | "up" | "down";

interface HttpResult {
  status: number;
  content_type?: string;
  body?: string;
  body_b64?: string;
}

type Sub = { onEvent: (ev: unknown) => void; onEnd: () => void };

export class Tunnel {
  state: TunnelState = "off";
  error: string | null = null;
  present: string[] = [];
  config: TunnelConfig = loadConfig();
  private id: ConsoleIdentity | null = null;
  private ws: WebSocket | null = null;
  private target: NodeCert | null = null;
  private pending = new Map<string, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private subs = new Map<string, Sub>();
  private listeners = new Set<() => void>();
  private ready: Promise<void> | null = null;
  private readyResolve: (() => void) | null = null;
  private myNonce = "";
  private pingTimer = 0;
  private retryTimer = 0;
  private stopped = true;
  /** Whether learned certs are written back to the identity in storage.
   *  Only the console's main tunnel may: a peek tunnel (peek.ts) runs
   *  with *another* profile's identity, and `saveIdentity` writes to the
   *  active profile's slot — persisting from there overwrote one mesh's
   *  identity with another's (v0.33.1). */
  persist = true;

  get enabled(): boolean {
    return this.config.enabled && !!this.config.relay && !!this.config.node;
  }
  private frags = new Map<string, { n: number; pieces: (string | null)[]; at: number }>();
  /** Put a fragmented frame back together; null until every piece is in. */
  private reassemble(data: string): string | null {
    let f: { id: string; i: number; n: number; d: string };
    try {
      f = (JSON.parse(data) as { frag: { id: string; i: number; n: number; d: string } }).frag;
    } catch {
      return null;
    }
    if (!f || !f.n || f.i >= f.n) return null;
    const now = Date.now();
    for (const [k, v] of this.frags) if (now - v.at > 120000) this.frags.delete(k);
    let e = this.frags.get(f.id);
    if (!e) {
      e = { n: f.n, pieces: new Array<string | null>(f.n).fill(null), at: now };
      this.frags.set(f.id, e);
    }
    e.pieces[f.i] = f.d;
    if (e.pieces.some((p) => p === null)) return null;
    this.frags.delete(f.id);
    return e.pieces.join("");
  }
  onChange(f: () => void): () => void {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }
  private emit() {
    for (const f of this.listeners) f();
  }
  private set(state: TunnelState, error: string | null = null) {
    this.state = state;
    this.error = error;
    this.emit();
  }

  /** Start (or restart) with the saved config — or, for a peek tunnel
   *  into another profile's mesh (peek.ts), with the config and identity
   *  given. Resolves when linked. */
  start(config?: TunnelConfig, id?: ConsoleIdentity | null): Promise<void> {
    this.stop();
    this.config = config ?? loadConfig();
    this.id = id !== undefined ? id : loadIdentity();
    if (!this.enabled) return Promise.resolve();
    if (!this.id?.cert) {
      this.set("down", "this console has no cert yet — see /attach");
      return Promise.reject(new Error(this.error ?? ""));
    }
    this.stopped = false;
    this.ready = new Promise<void>((res) => {
      this.readyResolve = res;
    });
    this.connect();
    return this.ready;
  }

  stop(): void {
    this.stopped = true;
    window.clearTimeout(this.retryTimer);
    window.clearInterval(this.pingTimer);
    if (this.ws) {
      this.ws.onclose = null;
      this.ws.close();
      this.ws = null;
    }
    for (const [, p] of this.pending) p.reject(new Error("tunnel closed"));
    this.pending.clear();
    for (const [, s] of this.subs) s.onEnd();
    this.subs.clear();
    this.target = null;
    if (this.state !== "off") this.set("off");
  }

  private connect() {
    if (this.stopped || !this.id?.cert) return;
    const id = this.id;
    const cert = id.cert!;
    this.set("connecting");
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.config.relay);
    } catch (e) {
      this.set("down", e instanceof Error ? e.message : "bad relay URL");
      this.scheduleRetry();
      return;
    }
    this.ws = ws;
    let phase: "challenge" | "welcome" | "hello" | "auth" | "up" = "challenge";
    ws.onmessage = (m: MessageEvent) => {
      const text = String(m.data);
      if (text === "ping") {
        ws.send("pong");
        return;
      }
      if (text === "pong") return;
      let f: Record<string, unknown>;
      try {
        f = JSON.parse(text) as Record<string, unknown>;
      } catch {
        return;
      }
      try {
        if (phase === "challenge") {
          const nonce = b64d(String(f["nonce"] ?? ""));
          const ctx = cat(te.encode("aspen-relay-challenge-v1\0"), te.encode(cert.mesh), Z, te.encode(id.node), Z, nonce);
          const sig = ed25519.sign(ctx, b64d(id.ed_secret));
          ws.send(JSON.stringify({ mesh: cert.mesh, node: id.node, cert, challenge_sig: b64e(sig) }));
          phase = "welcome";
          return;
        }
        const t = f["t"];
        if (t === "rejected") {
          this.set("down", `relay rejected: ${String(f["reason"] ?? "")}`);
          ws.close();
          return;
        }
        if (t === "welcome") {
          this.present = (f["peers"] as string[]) ?? [];
          this.set("registered");
          if (!this.present.includes(this.config.node)) {
            this.set("registered", `${this.config.node} is not on this relay right now (present: ${this.present.join(", ") || "nobody"}); waiting`);
            phase = "hello";
            return;
          }
          this.sendHello();
          phase = "hello";
          return;
        }
        if (t === "presence") {
          const node = String(f["node"]);
          if (f["online"]) {
            if (!this.present.includes(node)) this.present.push(node);
            if (node === this.config.node && phase === "hello" && !this.target) this.sendHello();
          } else {
            this.present = this.present.filter((p) => p !== node);
            if (node === this.config.node && this.state === "up") {
              this.set("down", `${node} left the relay`);
              this.dropLink();
              phase = "hello";
            }
          }
          this.emit();
          return;
        }
        if (t === "undeliverable") {
          this.set("registered", `relay cannot reach ${String(f["to"])}`);
          return;
        }
        if (t === "route") {
          const from = String(f["from"] ?? "");
          let data = String(f["data"] ?? "");
          if (from !== this.config.node) return;
          // A piece of a bigger frame (RELAY.md §12): a transcript sealed
          // and base64'd is more than a relay passes in one message.
          if (data.startsWith('{"frag":')) {
            const whole = this.reassemble(data);
            if (whole === null) return;
            data = whole;
          }
          if (phase === "hello") {
            const h = JSON.parse(data) as { hello: NodeCert; nonce: string; certs?: NodeCert[] };
            const offered = [h.hello, ...(h.certs ?? [])];
            const theirs = offered.find((c) => c.mesh === cert.mesh && verifyCert(c, cert.root_public));
            if (!theirs) {
              this.set("down", `${from} presented no cert for mesh ${cert.mesh}`);
              return;
            }
            this.target = theirs;
            id.known = { ...(id.known ?? {}), [theirs.node]: theirs };
            if (this.persist) saveIdentity(id);
            ws.send(JSON.stringify({ t: "route", to: from, data: seal(id, theirs, JSON.stringify({ t: "auth", nonce: h.nonce })) }));
            phase = "auth";
            this.set("linking");
            return;
          }
          if (!this.target) return;
          let payload: Record<string, unknown>;
          try {
            payload = JSON.parse(open(id, this.target, data)) as Record<string, unknown>;
          } catch (e) {
            this.set("down", `bad frame from ${from}: ${e instanceof Error ? e.message : String(e)}`);
            return;
          }
          if (phase === "auth") {
            if (payload["t"] === "auth" && payload["nonce"] === this.myNonce) {
              phase = "up";
              this.set("up");
              this.readyResolve?.();
              this.pingTimer = window.setInterval(() => {
                if (ws.readyState === WebSocket.OPEN) ws.send("ping");
              }, 20000);
            } else {
              this.set("down", `${from} failed the nonce proof`);
            }
            return;
          }
          this.onFrame(payload);
        }
      } catch (e) {
        this.set("down", e instanceof Error ? e.message : String(e));
      }
    };
    ws.onclose = () => {
      window.clearInterval(this.pingTimer);
      this.dropLink();
      if (!this.stopped) {
        this.set("down", this.error ?? "relay connection closed");
        this.scheduleRetry();
      }
    };
    ws.onerror = () => {
      // onclose follows
    };
  }

  private sendHello() {
    if (!this.ws || !this.id?.cert) return;
    const nonce = crypto.getRandomValues(new Uint8Array(32));
    this.myNonce = b64e(nonce);
    const hello = { hello: this.id.cert, nonce: this.myNonce, proto: 1, certs: [] };
    this.ws.send(JSON.stringify({ t: "route", to: this.config.node, data: JSON.stringify(hello) }));
    this.set("linking");
  }

  private dropLink() {
    this.target = null;
    for (const [, p] of this.pending) p.reject(new Error("link dropped"));
    this.pending.clear();
    for (const [, s] of this.subs) s.onEnd();
    this.subs.clear();
  }

  private scheduleRetry() {
    window.clearTimeout(this.retryTimer);
    this.retryTimer = window.setTimeout(() => this.connect(), 4000);
  }

  private onFrame(p: Record<string, unknown>) {
    const t = p["t"];
    if (t === "api_res") {
      const id = String(p["id"]);
      const w = this.pending.get(id);
      if (!w) return;
      this.pending.delete(id);
      if (p["ok"] === true) w.resolve(p["body"]);
      else w.reject(new Error(String(p["error"] ?? "remote error")));
      return;
    }
    if (t === "ev" || t === "sub_end") {
      const id = String(p["id"]);
      const s = this.subs.get(id);
      if (!s) return;
      if (t === "ev") s.onEvent(p["ev"]);
      else {
        this.subs.delete(id);
        s.onEnd();
      }
      return;
    }
    // rosters and the like: nothing to do in a console
  }

  private sendSealed(payload: unknown) {
    if (!this.ws || !this.id || !this.target || this.state !== "up") throw new Error("tunnel is not up");
    this.ws.send(JSON.stringify({ t: "route", to: this.target.node, data: seal(this.id, this.target, JSON.stringify(payload)) }));
  }

  private async waitUp(): Promise<void> {
    if (this.state === "up") return;
    if (!this.ready) throw new Error("tunnel not started");
    await this.ready;
  }

  /** One request through the node's own router. */
  async http(method: string, path: string, body?: string, headers?: Record<string, string>): Promise<HttpResult> {
    await this.waitUp();
    const id = crypto.randomUUID();
    const p = new Promise<unknown>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      window.setTimeout(() => {
        if (this.pending.delete(id)) {
          // Say so where the operator looks: the link is up but the node
          // does not answer through it.
          this.error = "the node is not answering requests through the relay";
          this.emit();
          reject(new Error("request timed out: the node did not answer through the relay"));
        }
      }, 20000);
    });
    this.sendSealed({ t: "api_req", id, op: "http", agent: "", body: { method, path, body: body ?? null, headers: headers ?? {} } });
    return (await p) as HttpResult;
  }

  /** Subscribe to a session's live events (the node's `sub` frame). */
  subscribe(agent: string, onEvent: (ev: unknown) => void, onEnd: () => void): () => void {
    const id = crypto.randomUUID();
    this.subs.set(id, { onEvent, onEnd });
    void this.waitUp()
      .then(() => this.sendSealed({ t: "sub", id, agent }))
      .catch(() => {
        this.subs.delete(id);
        onEnd();
      });
    return () => {
      if (this.subs.delete(id)) {
        try {
          this.sendSealed({ t: "unsub", id });
        } catch {
          // link already gone
        }
      }
    };
  }
}

export const tunnel = new Tunnel();
