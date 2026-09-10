// Connections (PROPOSALS-2026-09-D.md §3): where this console talks to.
// Served by a node, the implicit connection is that node — same origin,
// nothing to configure. Hosted (methodify.github.io/aspen), the console
// keeps a named list: a node on this machine reached directly
// (`http://127.0.0.1:7420` — the one http origin an https page may
// fetch), or a node reached through a relay as a mesh peer (tunnel.ts).
// One is active; api.ts prefixes every request with it.

import { loadConfig, saveConfig, tunnel } from "./tunnel";

export type Connection =
  | { id: string; name: string; kind: "direct"; url: string; token: string | null }
  | { id: string; name: string; kind: "relay"; relay: string; node: string };

const LIST_KEY = "aspen.connections";
const ACTIVE_KEY = "aspen.connections.active";

export const hosted: boolean = typeof __ASPEN_HOSTED__ !== "undefined" && __ASPEN_HOSTED__;

export function listConnections(): Connection[] {
  try {
    const raw = localStorage.getItem(LIST_KEY);
    return raw ? (JSON.parse(raw) as Connection[]) : [];
  } catch {
    return [];
  }
}
function saveList(list: Connection[]) {
  localStorage.setItem(LIST_KEY, JSON.stringify(list));
}

export function activeConnection(): Connection | null {
  try {
    const id = localStorage.getItem(ACTIVE_KEY);
    return listConnections().find((c) => c.id === id) ?? null;
  } catch {
    return null;
  }
}

function newId(): string {
  return Math.random().toString(36).slice(2, 10);
}

type NewConnection =
  | { id?: string; name: string; kind: "direct"; url: string; token: string | null }
  | { id?: string; name: string; kind: "relay"; relay: string; node: string };

export function addConnection(c: NewConnection): Connection {
  const conn = { ...c, id: c.id ?? newId() } as Connection;
  const list = listConnections().filter((x) => x.id !== conn.id);
  list.push(conn);
  saveList(list);
  return conn;
}

export function removeConnection(id: string) {
  saveList(listConnections().filter((c) => c.id !== id));
  if (localStorage.getItem(ACTIVE_KEY) === id) {
    localStorage.removeItem(ACTIVE_KEY);
    tunnel.stop();
    saveConfig({ ...loadConfig(), enabled: false });
  }
}

/** Record which connection is active without touching the tunnel or
 *  reloading — for the attach page, which is starting the tunnel itself. */
export function markActive(id: string) {
  localStorage.setItem(ACTIVE_KEY, id);
}

/** Make a connection the active one: a relay connection turns the
 *  tunnel on for its relay and node; a direct one turns it off. The
 *  page reloads so every poll starts over against the new target. */
export function activate(id: string | null) {
  if (id === null) {
    localStorage.removeItem(ACTIVE_KEY);
    tunnel.stop();
    saveConfig({ ...loadConfig(), enabled: false });
  } else {
    const c = listConnections().find((x) => x.id === id);
    if (!c) return;
    localStorage.setItem(ACTIVE_KEY, id);
    if (c.kind === "relay") {
      saveConfig({ enabled: true, relay: c.relay, node: c.node });
    } else {
      tunnel.stop();
      saveConfig({ ...loadConfig(), enabled: false });
    }
  }
  window.location.reload();
}

/** The URL prefix for API requests: a direct connection's node, else
 *  this origin (served by a node, or a relay connection — the tunnel
 *  takes those requests before they reach fetch). */
export function apiBase(): string {
  const c = activeConnection();
  if (c?.kind === "direct") return c.url.replace(/\/+$/, "");
  return "";
}

/** The token a direct connection carries, if any. */
export function connectionToken(): string | null {
  const c = activeConnection();
  return c?.kind === "direct" ? c.token : null;
}

/** A direct URL an https page may reach: loopback only. Anything else
 *  is mixed content and the browser blocks it; say so before trying. */
export function directUrlProblem(url: string): string | null {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return "not a URL — try http://127.0.0.1:7420";
  }
  if (u.protocol !== "http:" && u.protocol !== "https:") return "http:// or https:// only";
  if (window.location.protocol === "https:" && u.protocol === "http:") {
    const h = u.hostname;
    if (h !== "127.0.0.1" && h !== "localhost" && h !== "[::1]") {
      return "this page is https; a plain-http node is reachable only on this machine (127.0.0.1). For a node elsewhere, connect through a relay.";
    }
  }
  return null;
}
