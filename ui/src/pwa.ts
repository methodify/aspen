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

// ── Web Push (PROPOSALS-2026-09-D.md §4) ───────────────────────────────

import { api } from "./api";

function b64urlToBytes(s: string): Uint8Array {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const bin = atob((s + pad).replace(/-/g, "+").replace(/_/g, "/"));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
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

/** Ask permission, subscribe with the node's VAPID key, register with the
 *  node. `console` names this browser to the node. */
export async function pushSubscribe(consoleName: string, kinds: string[]): Promise<PushSubscription> {
  if (!pushSupported()) throw new Error("this browser has no Web Push");
  const perm = await Notification.requestPermission();
  if (perm !== "granted") throw new Error(perm === "denied" ? "notifications are blocked for this site" : "permission was not granted");
  const { public_key } = await api.pushVapid();
  const reg = await navigator.serviceWorker.ready;
  let sub = await reg.pushManager.getSubscription();
  if (!sub) {
    sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64urlToBytes(public_key) as BufferSource });
  }
  await api.pushSubscribe(sub.toJSON(), consoleName, kinds);
  return sub;
}

export async function pushUnsubscribe(): Promise<void> {
  const sub = await pushCurrent();
  if (!sub) return;
  try {
    await api.pushUnsubscribe(sub.endpoint);
  } finally {
    await sub.unsubscribe();
  }
}
