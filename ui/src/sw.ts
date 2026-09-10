/// <reference lib="webworker" />
// The console's service worker (PROPOSALS-2026-09-D.md §2, §4). Precaches
// the shell and nothing under /api; takes a new build only when the page
// asks (the "new console — reload" prompt); shows Web Push notices as OS
// notifications, keeps the dock badge, and opens the session on click.

import { cleanupOutdatedCaches, createHandlerBoundToURL, precacheAndRoute } from "workbox-precaching";
import { NavigationRoute, registerRoute } from "workbox-routing";

declare let self: ServiceWorkerGlobalScope;

// Injected at build time by vite-plugin-pwa.
precacheAndRoute(self.__WB_MANIFEST);
cleanupOutdatedCaches();

// Navigations get the shell — never an API path.
const base = new URL(self.registration.scope).pathname;
registerRoute(new NavigationRoute(createHandlerBoundToURL(`${base}index.html`), { denylist: [/\/api\//] }));

self.addEventListener("message", (event) => {
  if (event.data && event.data.type === "SKIP_WAITING") void self.skipWaiting();
});

interface PushNotice {
  title?: string;
  body?: string | null;
  kind?: string;
  agent?: string | null;
  node?: string | null;
  link?: string | null;
  ts?: number;
}

/** Where a notice's link opens: a hash route under the hosted base, a
 *  path under a node's own origin. The scope tells which. */
function targetUrl(link: string | null | undefined): string {
  const scope = self.registration.scope;
  const hosted = scope.endsWith("/aspen/");
  const path = link && link.startsWith("/") ? link : "/";
  return hosted ? `${scope}#${path}` : new URL(path, scope).toString();
}

self.addEventListener("push", (event) => {
  let n: PushNotice = {};
  try {
    n = (event.data?.json() ?? {}) as PushNotice;
  } catch {
    n = { title: event.data?.text() ?? "Aspen" };
  }
  const title = n.title ?? "Aspen";
  const body = [n.body ?? "", n.node ? `on ${n.node}` : ""].filter(Boolean).join(" · ");
  const show = self.registration.showNotification(title, {
    body,
    tag: `${n.kind ?? "notice"}:${n.agent ?? ""}:${n.ts ?? ""}`,
    icon: `${base}icons/aspen-192.png`,
    badge: `${base}icons/aspen-192.png`,
    data: { url: targetUrl(n.link) },
  });
  const badge = (self.navigator as Navigator & { setAppBadge?: (n?: number) => Promise<void> }).setAppBadge?.();
  event.waitUntil(Promise.all([show, badge ?? Promise.resolve()]));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = (event.notification.data as { url?: string } | undefined)?.url ?? self.registration.scope;
  event.waitUntil(
    (async () => {
      const all = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      for (const c of all) {
        if (c.url.startsWith(self.registration.scope) && "focus" in c) {
          await c.focus();
          if ("navigate" in c) {
            try {
              await c.navigate(url);
            } catch {
              /* cross-origin or refused; the focus is what matters */
            }
          }
          return;
        }
      }
      await self.clients.openWindow(url);
    })(),
  );
});
