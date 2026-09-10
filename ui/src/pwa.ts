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
