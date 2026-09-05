var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });

// node_modules/@noble/ed25519/index.js
var ed25519_CURVE = {
  p: 0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffedn,
  n: 0x1000000000000000000000000000000014def9dea2f79cd65812631a5cf5d3edn,
  h: 8n,
  a: 0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffecn,
  d: 0x52036cee2b6ffe738cc740797779e89800700a4d4141d8ab75eb4dca135978a3n,
  Gx: 0x216936d3cd6e53fec0a4e231fdd6dc5c692cc7609525a7b2c9562d608f25d51an,
  Gy: 0x6666666666666666666666666666666666666666666666666666666666666658n
};
var { p: P, n: N, Gx, Gy, a: _a, d: _d } = ed25519_CURVE;
var h = 8n;
var L = 32;
var L2 = 64;
var err = /* @__PURE__ */ __name((m = "") => {
  throw new Error(m);
}, "err");
var isBig = /* @__PURE__ */ __name((n) => typeof n === "bigint", "isBig");
var isStr = /* @__PURE__ */ __name((s) => typeof s === "string", "isStr");
var isBytes = /* @__PURE__ */ __name((a) => a instanceof Uint8Array || ArrayBuffer.isView(a) && a.constructor.name === "Uint8Array", "isBytes");
var abytes = /* @__PURE__ */ __name((a, l) => !isBytes(a) || typeof l === "number" && l > 0 && a.length !== l ? err("Uint8Array expected") : a, "abytes");
var u8n = /* @__PURE__ */ __name((len) => new Uint8Array(len), "u8n");
var u8fr = /* @__PURE__ */ __name((buf) => Uint8Array.from(buf), "u8fr");
var padh = /* @__PURE__ */ __name((n, pad) => n.toString(16).padStart(pad, "0"), "padh");
var bytesToHex = /* @__PURE__ */ __name((b) => Array.from(abytes(b)).map((e) => padh(e, 2)).join(""), "bytesToHex");
var C = { _0: 48, _9: 57, A: 65, F: 70, a: 97, f: 102 };
var _ch = /* @__PURE__ */ __name((ch) => {
  if (ch >= C._0 && ch <= C._9)
    return ch - C._0;
  if (ch >= C.A && ch <= C.F)
    return ch - (C.A - 10);
  if (ch >= C.a && ch <= C.f)
    return ch - (C.a - 10);
  return;
}, "_ch");
var hexToBytes = /* @__PURE__ */ __name((hex) => {
  const e = "hex invalid";
  if (!isStr(hex))
    return err(e);
  const hl = hex.length;
  const al = hl / 2;
  if (hl % 2)
    return err(e);
  const array = u8n(al);
  for (let ai = 0, hi = 0; ai < al; ai++, hi += 2) {
    const n1 = _ch(hex.charCodeAt(hi));
    const n2 = _ch(hex.charCodeAt(hi + 1));
    if (n1 === void 0 || n2 === void 0)
      return err(e);
    array[ai] = n1 * 16 + n2;
  }
  return array;
}, "hexToBytes");
var toU8 = /* @__PURE__ */ __name((a, len) => abytes(isStr(a) ? hexToBytes(a) : u8fr(abytes(a)), len), "toU8");
var cr = /* @__PURE__ */ __name(() => globalThis?.crypto, "cr");
var subtle = /* @__PURE__ */ __name(() => cr()?.subtle ?? err("crypto.subtle must be defined"), "subtle");
var concatBytes = /* @__PURE__ */ __name((...arrs) => {
  const r = u8n(arrs.reduce((sum, a) => sum + abytes(a).length, 0));
  let pad = 0;
  arrs.forEach((a) => {
    r.set(a, pad);
    pad += a.length;
  });
  return r;
}, "concatBytes");
var randomBytes = /* @__PURE__ */ __name((len = L) => {
  const c = cr();
  return c.getRandomValues(u8n(len));
}, "randomBytes");
var big = BigInt;
var arange = /* @__PURE__ */ __name((n, min, max, msg = "bad number: out of range") => isBig(n) && min <= n && n < max ? n : err(msg), "arange");
var M = /* @__PURE__ */ __name((a, b = P) => {
  const r = a % b;
  return r >= 0n ? r : b + r;
}, "M");
var modN = /* @__PURE__ */ __name((a) => M(a, N), "modN");
var invert = /* @__PURE__ */ __name((num, md) => {
  if (num === 0n || md <= 0n)
    err("no inverse n=" + num + " mod=" + md);
  let a = M(num, md), b = md, x = 0n, y = 1n, u = 1n, v = 0n;
  while (a !== 0n) {
    const q = b / a, r = b % a;
    const m = x - u * q, n = y - v * q;
    b = a, a = r, x = u, y = v, u = m, v = n;
  }
  return b === 1n ? M(x, md) : err("no inverse");
}, "invert");
var apoint = /* @__PURE__ */ __name((p) => p instanceof Point ? p : err("Point expected"), "apoint");
var B256 = 2n ** 256n;
var Point = class _Point {
  static {
    __name(this, "Point");
  }
  static BASE;
  static ZERO;
  ex;
  ey;
  ez;
  et;
  constructor(ex, ey, ez, et) {
    const max = B256;
    this.ex = arange(ex, 0n, max);
    this.ey = arange(ey, 0n, max);
    this.ez = arange(ez, 1n, max);
    this.et = arange(et, 0n, max);
    Object.freeze(this);
  }
  static fromAffine(p) {
    return new _Point(p.x, p.y, 1n, M(p.x * p.y));
  }
  /** RFC8032 5.1.3: Uint8Array to Point. */
  static fromBytes(hex, zip215 = false) {
    const d = _d;
    const normed = u8fr(abytes(hex, L));
    const lastByte = hex[31];
    normed[31] = lastByte & ~128;
    const y = bytesToNumLE(normed);
    const max = zip215 ? B256 : P;
    arange(y, 0n, max);
    const y2 = M(y * y);
    const u = M(y2 - 1n);
    const v = M(d * y2 + 1n);
    let { isValid, value: x } = uvRatio(u, v);
    if (!isValid)
      err("bad point: y not sqrt");
    const isXOdd = (x & 1n) === 1n;
    const isLastByteOdd = (lastByte & 128) !== 0;
    if (!zip215 && x === 0n && isLastByteOdd)
      err("bad point: x==0, isLastByteOdd");
    if (isLastByteOdd !== isXOdd)
      x = M(-x);
    return new _Point(x, y, 1n, M(x * y));
  }
  /** Checks if the point is valid and on-curve. */
  assertValidity() {
    const a = _a;
    const d = _d;
    const p = this;
    if (p.is0())
      throw new Error("bad point: ZERO");
    const { ex: X, ey: Y, ez: Z, et: T } = p;
    const X2 = M(X * X);
    const Y2 = M(Y * Y);
    const Z2 = M(Z * Z);
    const Z4 = M(Z2 * Z2);
    const aX2 = M(X2 * a);
    const left = M(Z2 * M(aX2 + Y2));
    const right = M(Z4 + M(d * M(X2 * Y2)));
    if (left !== right)
      throw new Error("bad point: equation left != right (1)");
    const XY = M(X * Y);
    const ZT = M(Z * T);
    if (XY !== ZT)
      throw new Error("bad point: equation left != right (2)");
    return this;
  }
  /** Equality check: compare points P&Q. */
  equals(other) {
    const { ex: X1, ey: Y1, ez: Z1 } = this;
    const { ex: X2, ey: Y2, ez: Z2 } = apoint(other);
    const X1Z2 = M(X1 * Z2);
    const X2Z1 = M(X2 * Z1);
    const Y1Z2 = M(Y1 * Z2);
    const Y2Z1 = M(Y2 * Z1);
    return X1Z2 === X2Z1 && Y1Z2 === Y2Z1;
  }
  is0() {
    return this.equals(I);
  }
  /** Flip point over y coordinate. */
  negate() {
    return new _Point(M(-this.ex), this.ey, this.ez, M(-this.et));
  }
  /** Point doubling. Complete formula. Cost: `4M + 4S + 1*a + 6add + 1*2`. */
  double() {
    const { ex: X1, ey: Y1, ez: Z1 } = this;
    const a = _a;
    const A = M(X1 * X1);
    const B = M(Y1 * Y1);
    const C2 = M(2n * M(Z1 * Z1));
    const D = M(a * A);
    const x1y1 = X1 + Y1;
    const E = M(M(x1y1 * x1y1) - A - B);
    const G2 = D + B;
    const F = G2 - C2;
    const H = D - B;
    const X3 = M(E * F);
    const Y3 = M(G2 * H);
    const T3 = M(E * H);
    const Z3 = M(F * G2);
    return new _Point(X3, Y3, Z3, T3);
  }
  /** Point addition. Complete formula. Cost: `8M + 1*k + 8add + 1*2`. */
  add(other) {
    const { ex: X1, ey: Y1, ez: Z1, et: T1 } = this;
    const { ex: X2, ey: Y2, ez: Z2, et: T2 } = apoint(other);
    const a = _a;
    const d = _d;
    const A = M(X1 * X2);
    const B = M(Y1 * Y2);
    const C2 = M(T1 * d * T2);
    const D = M(Z1 * Z2);
    const E = M((X1 + Y1) * (X2 + Y2) - A - B);
    const F = M(D - C2);
    const G2 = M(D + C2);
    const H = M(B - a * A);
    const X3 = M(E * F);
    const Y3 = M(G2 * H);
    const T3 = M(E * H);
    const Z3 = M(F * G2);
    return new _Point(X3, Y3, Z3, T3);
  }
  /**
   * Point-by-scalar multiplication. Scalar must be in range 1 <= n < CURVE.n.
   * Uses {@link wNAF} for base point.
   * Uses fake point to mitigate side-channel leakage.
   * @param n scalar by which point is multiplied
   * @param safe safe mode guards against timing attacks; unsafe mode is faster
   */
  multiply(n, safe = true) {
    if (!safe && (n === 0n || this.is0()))
      return I;
    arange(n, 1n, N);
    if (n === 1n)
      return this;
    if (this.equals(G))
      return wNAF(n).p;
    let p = I;
    let f = G;
    for (let d = this; n > 0n; d = d.double(), n >>= 1n) {
      if (n & 1n)
        p = p.add(d);
      else if (safe)
        f = f.add(d);
    }
    return p;
  }
  /** Convert point to 2d xy affine point. (X, Y, Z) ∋ (x=X/Z, y=Y/Z) */
  toAffine() {
    const { ex: x, ey: y, ez: z } = this;
    if (this.equals(I))
      return { x: 0n, y: 1n };
    const iz = invert(z, P);
    if (M(z * iz) !== 1n)
      err("invalid inverse");
    return { x: M(x * iz), y: M(y * iz) };
  }
  toBytes() {
    const { x, y } = this.assertValidity().toAffine();
    const b = numTo32bLE(y);
    b[31] |= x & 1n ? 128 : 0;
    return b;
  }
  toHex() {
    return bytesToHex(this.toBytes());
  }
  // encode to hex string
  clearCofactor() {
    return this.multiply(big(h), false);
  }
  isSmallOrder() {
    return this.clearCofactor().is0();
  }
  isTorsionFree() {
    let p = this.multiply(N / 2n, false).double();
    if (N % 2n)
      p = p.add(this);
    return p.is0();
  }
  static fromHex(hex, zip215) {
    return _Point.fromBytes(toU8(hex), zip215);
  }
  get x() {
    return this.toAffine().x;
  }
  get y() {
    return this.toAffine().y;
  }
  toRawBytes() {
    return this.toBytes();
  }
};
var G = new Point(Gx, Gy, 1n, M(Gx * Gy));
var I = new Point(0n, 1n, 1n, 0n);
Point.BASE = G;
Point.ZERO = I;
var numTo32bLE = /* @__PURE__ */ __name((num) => hexToBytes(padh(arange(num, 0n, B256), L2)).reverse(), "numTo32bLE");
var bytesToNumLE = /* @__PURE__ */ __name((b) => big("0x" + bytesToHex(u8fr(abytes(b)).reverse())), "bytesToNumLE");
var pow2 = /* @__PURE__ */ __name((x, power) => {
  let r = x;
  while (power-- > 0n) {
    r *= r;
    r %= P;
  }
  return r;
}, "pow2");
var pow_2_252_3 = /* @__PURE__ */ __name((x) => {
  const x2 = x * x % P;
  const b2 = x2 * x % P;
  const b4 = pow2(b2, 2n) * b2 % P;
  const b5 = pow2(b4, 1n) * x % P;
  const b10 = pow2(b5, 5n) * b5 % P;
  const b20 = pow2(b10, 10n) * b10 % P;
  const b40 = pow2(b20, 20n) * b20 % P;
  const b80 = pow2(b40, 40n) * b40 % P;
  const b160 = pow2(b80, 80n) * b80 % P;
  const b240 = pow2(b160, 80n) * b80 % P;
  const b250 = pow2(b240, 10n) * b10 % P;
  const pow_p_5_8 = pow2(b250, 2n) * x % P;
  return { pow_p_5_8, b2 };
}, "pow_2_252_3");
var RM1 = 0x2b8324804fc1df0b2b4d00993dfbd7a72f431806ad2fe478c4ee1b274a0ea0b0n;
var uvRatio = /* @__PURE__ */ __name((u, v) => {
  const v3 = M(v * v * v);
  const v7 = M(v3 * v3 * v);
  const pow = pow_2_252_3(u * v7).pow_p_5_8;
  let x = M(u * v3 * pow);
  const vx2 = M(v * x * x);
  const root1 = x;
  const root2 = M(x * RM1);
  const useRoot1 = vx2 === u;
  const useRoot2 = vx2 === M(-u);
  const noRoot = vx2 === M(-u * RM1);
  if (useRoot1)
    x = root1;
  if (useRoot2 || noRoot)
    x = root2;
  if ((M(x) & 1n) === 1n)
    x = M(-x);
  return { isValid: useRoot1 || useRoot2, value: x };
}, "uvRatio");
var modL_LE = /* @__PURE__ */ __name((hash) => modN(bytesToNumLE(hash)), "modL_LE");
var sha512a = /* @__PURE__ */ __name((...m) => etc.sha512Async(...m), "sha512a");
var hashFinishA = /* @__PURE__ */ __name((res) => sha512a(res.hashable).then(res.finish), "hashFinishA");
var veriOpts = { zip215: true };
var _verify = /* @__PURE__ */ __name((sig, msg, pub, opts = veriOpts) => {
  sig = toU8(sig, L2);
  msg = toU8(msg);
  pub = toU8(pub, L);
  const { zip215 } = opts;
  let A;
  let R;
  let s;
  let SB;
  let hashable = Uint8Array.of();
  try {
    A = Point.fromHex(pub, zip215);
    R = Point.fromHex(sig.slice(0, L), zip215);
    s = bytesToNumLE(sig.slice(L, L2));
    SB = G.multiply(s, false);
    hashable = concatBytes(R.toBytes(), A.toBytes(), msg);
  } catch (error) {
  }
  const finish = /* @__PURE__ */ __name((hashed) => {
    if (SB == null)
      return false;
    if (!zip215 && A.isSmallOrder())
      return false;
    const k = modL_LE(hashed);
    const RkA = R.add(A.multiply(k, false));
    return RkA.add(SB.negate()).clearCofactor().is0();
  }, "finish");
  return { hashable, finish };
}, "_verify");
var verifyAsync = /* @__PURE__ */ __name(async (s, m, p, opts = veriOpts) => hashFinishA(_verify(s, m, p, opts)), "verifyAsync");
var etc = {
  sha512Async: /* @__PURE__ */ __name(async (...messages) => {
    const s = subtle();
    const m = concatBytes(...messages);
    return u8n(await s.digest("SHA-512", m.buffer));
  }, "sha512Async"),
  sha512Sync: void 0,
  bytesToHex,
  hexToBytes,
  concatBytes,
  mod: M,
  invert,
  randomBytes
};
var W = 8;
var scalarBits = 256;
var pwindows = Math.ceil(scalarBits / W) + 1;
var pwindowSize = 2 ** (W - 1);
var precompute = /* @__PURE__ */ __name(() => {
  const points = [];
  let p = G;
  let b = p;
  for (let w = 0; w < pwindows; w++) {
    b = p;
    points.push(b);
    for (let i = 1; i < pwindowSize; i++) {
      b = b.add(p);
      points.push(b);
    }
    p = b.double();
  }
  return points;
}, "precompute");
var Gpows = void 0;
var ctneg = /* @__PURE__ */ __name((cnd, p) => {
  const n = p.negate();
  return cnd ? n : p;
}, "ctneg");
var wNAF = /* @__PURE__ */ __name((n) => {
  const comp = Gpows || (Gpows = precompute());
  let p = I;
  let f = G;
  const pow_2_w = 2 ** W;
  const maxNum = pow_2_w;
  const mask = big(pow_2_w - 1);
  const shiftBy = big(W);
  for (let w = 0; w < pwindows; w++) {
    let wbits = Number(n & mask);
    n >>= shiftBy;
    if (wbits > pwindowSize) {
      wbits -= maxNum;
      n += 1n;
    }
    const off = w * pwindowSize;
    const offF = off;
    const offP = off + Math.abs(wbits) - 1;
    const isEven = w % 2 !== 0;
    const isNeg = wbits < 0;
    if (wbits === 0) {
      f = f.add(ctneg(isEven, comp[offF]));
    } else {
      p = p.add(ctneg(isNeg, comp[offP]));
    }
  }
  return { p, f };
}, "wNAF");

// src/worker.js
var MAILBOX_MAX_ITEMS = 200;
var MAILBOX_MAX_BYTES = 2 * 1024 * 1024;
var MAILBOX_TTL_MS = 7 * 24 * 3600 * 1e3;
var SWEEP_EVERY_MS = 6 * 3600 * 1e3;
var worker_default = {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/healthz") return new Response("ok");
    if (url.pathname !== "/relay") return new Response("not found", { status: 404 });
    const mesh = url.searchParams.get("mesh");
    if (!mesh) return new Response("missing ?mesh", { status: 400 });
    const id = env.MESH.idFromName(mesh);
    return env.MESH.get(id).fetch(request);
  }
};
function b64(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
__name(b64, "b64");
function b64encode(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}
__name(b64encode, "b64encode");
function challengeContext(mesh, node, nonce) {
  const head = new TextEncoder().encode("aspen-relay-challenge-v1\0" + mesh + "\0" + node + "\0");
  const out = new Uint8Array(head.length + nonce.length);
  out.set(head, 0);
  out.set(nonce, head.length);
  return out;
}
__name(challengeContext, "challengeContext");
function certContext(mesh, node, edPub, xPub) {
  const head = new TextEncoder().encode("aspen-cert-v1\0" + mesh + "\0" + node + "\0");
  const out = new Uint8Array(head.length + edPub.length + xPub.length);
  out.set(head, 0);
  out.set(edPub, head.length);
  out.set(xPub, head.length + edPub.length);
  return out;
}
__name(certContext, "certContext");
async function verifyRegister(msg, mesh, rootPub, nonce) {
  if (!msg || typeof msg !== "object") return "bad register";
  if (msg.mesh !== mesh) return `wrong mesh: relay serves '${mesh}', node claims '${msg.mesh}'`;
  const cert = msg.cert;
  if (!cert || cert.node !== msg.node) return "cert node name does not match register";
  if (cert.mesh !== mesh) return "cert is for another mesh";
  let edPub, xPub, rootSig, chalSig;
  try {
    edPub = b64(cert.ed_public);
    xPub = b64(cert.x_public);
    rootSig = b64(cert.root_sig);
    chalSig = b64(msg.challenge_sig);
  } catch {
    return "malformed base64 in register";
  }
  const certOk = await verifyAsync(rootSig, certContext(mesh, cert.node, edPub, xPub), rootPub);
  if (!certOk) return "cert not valid for this mesh";
  const chalOk = await verifyAsync(chalSig, challengeContext(mesh, msg.node, nonce), edPub);
  if (!chalOk) return "challenge signature invalid";
  return true;
}
__name(verifyRegister, "verifyRegister");
var MeshRelay = class {
  static {
    __name(this, "MeshRelay");
  }
  constructor(state, env) {
    this.state = state;
    this.env = env;
  }
  rootFor(mesh) {
    const roots = JSON.parse(this.env.MESH_ROOTS || "{}");
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
    const mesh = url.searchParams.get("mesh");
    const rootPub = this.rootFor(mesh);
    if (!rootPub) return new Response("unknown mesh", { status: 403 });
    if (request.headers.get("Upgrade") !== "websocket") {
      return new Response("websocket only", { status: 426 });
    }
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    this.state.acceptWebSocket(server);
    const nonce = crypto.getRandomValues(new Uint8Array(32));
    server.serializeAttachment({ mesh, node: null, nonce: b64encode(nonce) });
    server.send(JSON.stringify({ nonce: b64encode(nonce) }));
    if (!await this.state.storage.getAlarm()) {
      await this.state.storage.setAlarm(Date.now() + SWEEP_EVERY_MS);
    }
    return new Response(null, { status: 101, webSocket: client });
  }
  async webSocketMessage(ws, data) {
    let msg;
    try {
      msg = JSON.parse(typeof data === "string" ? data : new TextDecoder().decode(data));
    } catch {
      return;
    }
    const a = ws.deserializeAttachment() || {};
    if (!a.node) {
      const rootPub = this.rootFor(a.mesh);
      let ok;
      try {
        ok = await verifyRegister(msg, a.mesh, rootPub, b64(a.nonce));
      } catch (e) {
        ok = "verify error: " + e;
      }
      if (ok !== true) {
        ws.send(JSON.stringify({ t: "rejected", reason: ok }));
        ws.close(1008, "rejected");
        return;
      }
      const stale = this.socketFor(msg.node);
      if (stale && stale !== ws) {
        try {
          stale.close(1e3, "replaced");
        } catch {
        }
      }
      const peers = this.presentNodes().filter((n) => n !== msg.node);
      ws.serializeAttachment({ mesh: a.mesh, node: msg.node, nonce: null });
      ws.send(JSON.stringify({ t: "welcome", peers }));
      this.broadcastPresence(msg.node, true);
      await this.drainMail(ws, msg.node);
      return;
    }
    if (msg.t === "route" && msg.to) {
      const dest = this.socketFor(msg.to);
      if (dest) {
        dest.send(JSON.stringify({ t: "route", from: a.node, data: msg.data }));
      } else {
        ws.send(JSON.stringify({ t: "undeliverable", to: msg.to }));
      }
      return;
    }
    if (msg.t === "store" && msg.to && msg.id != null) {
      const dest = this.socketFor(msg.to);
      if (dest) {
        dest.send(JSON.stringify({ t: "mail", from: a.node, id: msg.id, data: msg.data }));
        return;
      }
      const stored = await this.storeMail(msg.to, a.node, msg.id, msg.data);
      if (!stored) ws.send(JSON.stringify({ t: "mailbox_full", to: msg.to }));
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
    const msg = JSON.stringify({ t: "presence", node, online });
    for (const [ws, a] of this.sockets()) {
      if (a.node && a.node !== node) {
        try {
          ws.send(msg);
        } catch {
        }
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
      if (k === `${prefix}${from}:${id}`) continue;
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
          ws.send(JSON.stringify({ t: "mail", from: v.from, id: v.id, data: v.data }));
        } catch {
          break;
        }
      }
      await this.state.storage.delete(k);
    }
  }
  // Alarm: drop expired mail; re-arm while any remains.
  async alarm() {
    const all = await this.state.storage.list({ prefix: "mail:" });
    const cutoff = Date.now() - MAILBOX_TTL_MS;
    let remaining = 0;
    for (const [k, v] of all) {
      if (v.at < cutoff) await this.state.storage.delete(k);
      else remaining++;
    }
    if (remaining > 0) await this.state.storage.setAlarm(Date.now() + SWEEP_EVERY_MS);
  }
};

// node_modules/wrangler/templates/middleware/middleware-ensure-req-body-drained.ts
var drainBody = /* @__PURE__ */ __name(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } finally {
    try {
      if (request.body !== null && !request.bodyUsed) {
        const reader = request.body.getReader();
        while (!(await reader.read()).done) {
        }
      }
    } catch (e) {
      console.error("Failed to drain the unused request body.", e);
    }
  }
}, "drainBody");
var middleware_ensure_req_body_drained_default = drainBody;

// node_modules/wrangler/templates/middleware/middleware-miniflare3-json-error.ts
function reduceError(e) {
  return {
    name: e?.name,
    message: e?.message ?? String(e),
    stack: e?.stack,
    cause: e?.cause === void 0 ? void 0 : reduceError(e.cause)
  };
}
__name(reduceError, "reduceError");
var jsonError = /* @__PURE__ */ __name(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } catch (e) {
    const error = reduceError(e);
    const body = JSON.stringify(error);
    const headers = {
      "Content-Type": "application/json",
      "MF-Experimental-Error-Stack": "true"
    };
    const encoded = encodeURIComponent(body);
    if (encoded.length <= 8192) {
      headers["MF-Experimental-Error-Stack-Payload"] = encoded;
    }
    return new Response(body, { status: 500, headers });
  }
}, "jsonError");
var middleware_miniflare3_json_error_default = jsonError;

// .wrangler/tmp/bundle-Sp3Zhg/middleware-insertion-facade.js
var __INTERNAL_WRANGLER_MIDDLEWARE__ = [
  middleware_ensure_req_body_drained_default,
  middleware_miniflare3_json_error_default
];
var middleware_insertion_facade_default = worker_default;

// node_modules/wrangler/templates/middleware/common.ts
var __facade_middleware__ = [];
function __facade_register__(...args) {
  __facade_middleware__.push(...args.flat());
}
__name(__facade_register__, "__facade_register__");
function __facade_invokeChain__(request, env, ctx, dispatch, middlewareChain) {
  const [head, ...tail] = middlewareChain;
  const middlewareCtx = {
    dispatch,
    next(newRequest, newEnv) {
      return __facade_invokeChain__(newRequest, newEnv, ctx, dispatch, tail);
    }
  };
  return head(request, env, ctx, middlewareCtx);
}
__name(__facade_invokeChain__, "__facade_invokeChain__");
function __facade_invoke__(request, env, ctx, dispatch, finalMiddleware) {
  return __facade_invokeChain__(request, env, ctx, dispatch, [
    ...__facade_middleware__,
    finalMiddleware
  ]);
}
__name(__facade_invoke__, "__facade_invoke__");

// .wrangler/tmp/bundle-Sp3Zhg/middleware-loader.entry.ts
var __Facade_ScheduledController__ = class ___Facade_ScheduledController__ {
  constructor(scheduledTime, cron, noRetry) {
    this.scheduledTime = scheduledTime;
    this.cron = cron;
    this.#noRetry = noRetry;
  }
  scheduledTime;
  cron;
  static {
    __name(this, "__Facade_ScheduledController__");
  }
  #noRetry;
  noRetry() {
    if (!(this instanceof ___Facade_ScheduledController__)) {
      throw new TypeError("Illegal invocation");
    }
    this.#noRetry();
  }
};
function wrapExportedHandler(worker) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__ === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__.length === 0) {
    return worker;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__) {
    __facade_register__(middleware);
  }
  const fetchDispatcher = /* @__PURE__ */ __name(function(request, env, ctx) {
    if (worker.fetch === void 0) {
      throw new Error("Handler does not export a fetch() function.");
    }
    return worker.fetch(request, env, ctx);
  }, "fetchDispatcher");
  return {
    ...worker,
    fetch(request, env, ctx) {
      const dispatcher = /* @__PURE__ */ __name(function(type, init) {
        if (type === "scheduled" && worker.scheduled !== void 0) {
          const controller = new __Facade_ScheduledController__(
            Date.now(),
            init.cron ?? "",
            () => {
            }
          );
          return worker.scheduled(controller, env, ctx);
        }
      }, "dispatcher");
      return __facade_invoke__(request, env, ctx, dispatcher, fetchDispatcher);
    }
  };
}
__name(wrapExportedHandler, "wrapExportedHandler");
function wrapWorkerEntrypoint(klass) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__ === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__.length === 0) {
    return klass;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__) {
    __facade_register__(middleware);
  }
  return class extends klass {
    #fetchDispatcher = /* @__PURE__ */ __name((request, env, ctx) => {
      this.env = env;
      this.ctx = ctx;
      if (super.fetch === void 0) {
        throw new Error("Entrypoint class does not define a fetch() function.");
      }
      return super.fetch(request);
    }, "#fetchDispatcher");
    #dispatcher = /* @__PURE__ */ __name((type, init) => {
      if (type === "scheduled" && super.scheduled !== void 0) {
        const controller = new __Facade_ScheduledController__(
          Date.now(),
          init.cron ?? "",
          () => {
          }
        );
        return super.scheduled(controller);
      }
    }, "#dispatcher");
    fetch(request) {
      return __facade_invoke__(
        request,
        this.env,
        this.ctx,
        this.#dispatcher,
        this.#fetchDispatcher
      );
    }
  };
}
__name(wrapWorkerEntrypoint, "wrapWorkerEntrypoint");
var WRAPPED_ENTRY;
if (typeof middleware_insertion_facade_default === "object") {
  WRAPPED_ENTRY = wrapExportedHandler(middleware_insertion_facade_default);
} else if (typeof middleware_insertion_facade_default === "function") {
  WRAPPED_ENTRY = wrapWorkerEntrypoint(middleware_insertion_facade_default);
}
var middleware_loader_entry_default = WRAPPED_ENTRY;
export {
  MeshRelay,
  __INTERNAL_WRANGLER_MIDDLEWARE__,
  middleware_loader_entry_default as default
};
/*! Bundled license information:

@noble/ed25519/index.js:
  (*! noble-ed25519 - MIT License (c) 2019 Paul Miller (paulmillr.com) *)
*/
//# sourceMappingURL=worker.js.map
