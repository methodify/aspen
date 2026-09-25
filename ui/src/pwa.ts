// The service worker's two signals (PROPOSALS-2026-09-D.md §2): a new
// build is ready (reload to take it), and the app is installable. No
// runtime caching of /api lives here — the worker precaches the shell only.

import { registerSW } from "virtual:pwa-register";

type Listener = () => void;
const listeners = new Set<Listener>();
let needRefresh = false;
let installPrompt: (Event & { prompt: () => Promise<void> }) | null = null;
let update: ((reload?: boolean) => Promise<void>) | null = null;

function emit() {
  for (const l of listeners) l();
}

export const pwa = {
  get needRefresh() {
    return needRefresh;
  },
  get canInstall() {
    return installPrompt !== null;
  },
  /** Take the new build now. */
  reload() {
    if (update) void update(true);
    else window.location.reload();
  },
  /** Ask the worker for a newer build, then reload either way (the More
   *  sheet's version row): a stale installed app gets unstuck by hand. */
  checkAndReload() {
    const done = () => window.location.reload();
    if (!("serviceWorker" in navigator)) return done();
    navigator.serviceWorker
      .getRegistration()
      .then((reg) => (reg ? reg.update() : undefined))
      .catch(() => {})
      .finally(() => window.setTimeout(done, 400));
  },
  async install() {
    const p = installPrompt;
    if (!p) return;
    installPrompt = null;
    emit();
    await p.prompt();
  },
  onChange(l: Listener) {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
  start() {
    if (!("serviceWorker" in navigator)) return;
    update = registerSW({
      onNeedRefresh() {
        needRefresh = true;
        emit();
      },
      // An installed app (a phone's home screen) rarely navigates, so the
      // browser rarely re-checks the worker on its own: ask on every
      // return to the foreground, and hourly while open.
      onRegisteredSW(_url, reg) {
        if (!reg) return;
        const check = () => void reg.update().catch(() => {});
        document.addEventListener("visibilitychange", () => {
          if (document.visibilityState === "visible") check();
        });
        window.addEventListener("focus", check);
        window.addEventListener("pageshow", check);
        window.setInterval(check, 60 * 60_000);
      },
    });
    window.addEventListener("beforeinstallprompt", (e) => {
      e.preventDefault();
      installPrompt = e as Event & { prompt: () => Promise<void> };
      emit();
    });
    window.addEventListener("appinstalled", () => {
      installPrompt = null;
      emit();
    });
  },
};

/** The dock badge (§1): the needs-you count, cleared at zero. */
export function setBadge(n: number) {
  const nav = navigator as Navigator & { setAppBadge?: (n?: number) => Promise<void>; clearAppBadge?: () => Promise<void> };
  try {
    if (n > 0) void nav.setAppBadge?.(n);
    else void nav.clearAppBadge?.();
  } catch {
    /* unsupported */
  }
}

// ── Web Push (PROPOSALS-2026-09-D.md §4; several meshes: PROPOSALS-2026-09-F.md §2.5) ──
//
// A browser holds one push subscription per app, and that subscription
// is bound to one sender (VAPID) key. A console that holds several
// meshes registers the same subscription in each, so the key must be the
// same everywhere: it is the console's own, made here, kept browser-wide,
// and handed to every node the console subscribes on. A node without one
// (older consoles) uses its own key. The key authorizes nothing but
// pushes to this browser, which the node could already do.

import { api } from "./api";
import { activeProfile, listProfiles, readFrom, scoped } from "./profiles";

export interface SenderKey {
  /** Raw 32-byte P-256 private scalar, base64url without padding. */
  private_key: string;
  /** Uncompressed 65-byte P-256 public point, base64url without padding. */
  public_key: string;
}

const SENDER_KEY = "aspen.push.sender";
/** Per mesh: whether this device subscribed on a node there. */
const PUSH_ON_KEY = "aspen.push.on";

function b64urlToBytes(s: string): Uint8Array {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const bin = atob((s + pad).replace(/-/g, "+").replace(/_/g, "/"));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
function bytesToB64url(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The console's sender key, made on first use (WebCrypto P-256; the
 *  private scalar is exported once, to hand to nodes). */
export async function senderKey(): Promise<SenderKey> {
  try {
    const raw = localStorage.getItem(SENDER_KEY);
    if (raw) {
      const k = JSON.parse(raw) as SenderKey;
      if (k.private_key && k.public_key) return k;
    }
  } catch {
    /* fall through */
  }
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const jwk = await crypto.subtle.exportKey("jwk", pair.privateKey);
  if (!jwk.d || !jwk.x || !jwk.y) throw new Error("could not export the push key");
  const pub = new Uint8Array(65);
  pub[0] = 4;
  pub.set(b64urlToBytes(jwk.x), 1);
  pub.set(b64urlToBytes(jwk.y), 33);
  const k: SenderKey = { private_key: jwk.d, public_key: bytesToB64url(pub) };
  try {
    localStorage.setItem(SENDER_KEY, JSON.stringify(k));
  } catch {
    /* storage unavailable: a new key next time, and a re-subscribe */
  }
  return k;
}

export function pushSupported(): boolean {
  return "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
}

/** This browser's subscription with the push service, if any. */
export async function pushCurrent(): Promise<PushSubscription | null> {
  if (!pushSupported()) return null;
  const reg = await navigator.serviceWorker.ready;
  return reg.pushManager.getSubscription();
}

/** Whether this device is subscribed in the active mesh. */
export function pushOnHere(): boolean {
  try {
    return localStorage.getItem(scoped(PUSH_ON_KEY)) === "1";
  } catch {
    return false;
  }
}
function setPushOnHere(on: boolean) {
  try {
    if (on) localStorage.setItem(scoped(PUSH_ON_KEY), "1");
    else localStorage.removeItem(scoped(PUSH_ON_KEY));
  } catch {
    /* storage unavailable */
  }
}
/** Meshes (other than the active one) where this device is subscribed. */
export function pushOnElsewhere(): string[] {
  const me = activeProfile()?.id;
  return listProfiles()
    .filter((p) => p.id !== me && readFrom(p.id, PUSH_ON_KEY) === "1")
    .map((p) => p.label ?? p.mesh ?? p.node ?? "another mesh");
}

function sameKey(sub: PushSubscription, key: SenderKey): boolean {
  const k = sub.options.applicationServerKey;
  if (!k) return false;
  return bytesToB64url(new Uint8Array(k)) === key.public_key;
}

/** Ask permission, subscribe with the console's sender key, register on
 *  the node this console talks to. A subscription made with another key
 *  (a node's own, before v0.29) is replaced. `console` names this
 *  browser to the node. */
export async function pushSubscribe(consoleName: string, kinds: string[]): Promise<PushSubscription> {
  if (!pushSupported()) throw new Error("this browser has no Web Push");
  const perm = await Notification.requestPermission();
  if (perm !== "granted") throw new Error(perm === "denied" ? "notifications are blocked for this site" : "permission was not granted");
  const key = await senderKey();
  const reg = await navigator.serviceWorker.ready;
  let sub = await reg.pushManager.getSubscription();
  if (sub && !sameKey(sub, key)) {
    // Made with a node's key: tell that node, drop it, make ours.
    try {
      await api.pushUnsubscribe(sub.endpoint);
    } catch {
      /* the node forgets it on the first gone response */
    }
    await sub.unsubscribe();
    sub = null;
  }
  if (!sub) {
    sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64urlToBytes(key.public_key) as BufferSource });
  }
  await api.pushSubscribe(sub.toJSON(), consoleName, kinds, key);
  setPushOnHere(true);
  return sub;
}

/** Unregister on this mesh's node; drop the browser subscription only
 *  when no other mesh still uses it. */
export async function pushUnsubscribe(): Promise<void> {
  const sub = await pushCurrent();
  setPushOnHere(false);
  if (!sub) return;
  try {
    await api.pushUnsubscribe(sub.endpoint);
  } finally {
    if (pushOnElsewhere().length === 0) await sub.unsubscribe();
  }
}
