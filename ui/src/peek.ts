// Peek (docs/PROPOSALS-2026-09-F.md §4, F-6): the needs-you count of every
// mesh this console is *not* looking at, for the switcher. Each inactive
// profile gets its own reader — a fetch to its direct node, or a second
// tunnel into its relay with that profile's identity — polled every
// minute. One socket per mesh while the console is open; push covers the
// same question while it is closed.

import { Tunnel, type ConsoleIdentity, type TunnelConfig } from "./tunnel";
import { activeProfile, hosted, listProfiles, readFrom } from "./profiles";

export interface PeekState {
  /** Messages waiting on the operator there, or null while unknown. */
  needs: number | null;
  /** "up" when the last read succeeded; "down" with a reason otherwise. */
  state: "idle" | "up" | "down";
  error?: string | null;
  at?: number;
}

interface Reader {
  profileId: string;
  tunnel: Tunnel | null;
  timer: number;
  stop: () => void;
}

const INTERVAL_MS = 60_000;
const states = new Map<string, PeekState>();
const readers = new Map<string, Reader>();
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

type DirectConn = { kind: "direct"; url: string; token: string | null };
type RelayConn = { kind: "relay"; relay: string; node: string };

/** How another profile gets into its mesh, from its own stored slice. */
function connectionOf(pid: string): DirectConn | RelayConn | null {
  try {
    const t = readFrom(pid, "aspen.console.tunnel");
    const tun = t ? (JSON.parse(t) as TunnelConfig) : null;
    if (tun?.enabled && tun.relay && tun.node) return { kind: "relay", relay: tun.relay, node: tun.node };
    const raw = readFrom(pid, "aspen.connections");
    const list = raw ? (JSON.parse(raw) as (DirectConn | RelayConn)[]) : [];
    const activeId = readFrom(pid, "aspen.connections.active");
    const c = list.find((x) => (x as { id?: string }).id === activeId) ?? list[0];
    return c ?? null;
  } catch {
    return null;
  }
}

function identityOf(pid: string): ConsoleIdentity | null {
  try {
    const raw = readFrom(pid, "aspen.console.identity");
    return raw ? (JSON.parse(raw) as ConsoleIdentity) : null;
  } catch {
    return null;
  }
}

function set(pid: string, s: PeekState) {
  states.set(pid, s);
  emit();
}

async function readDirect(pid: string, c: DirectConn) {
  const headers: Record<string, string> = {};
  if (c.token) headers["X-Aspen-Token"] = c.token;
  const r = await fetch(`${c.url.replace(/\/+$/, "")}/api/operator/inbox`, { headers });
  if (!r.ok) throw new Error(`node answered ${r.status}`);
  const inbox = (await r.json()) as unknown[];
  set(pid, { needs: inbox.length, state: "up", at: Date.now() });
}

async function readRelay(pid: string, t: Tunnel) {
  const r = await t.http("GET", "/api/operator/inbox");
  if (r.status < 200 || r.status >= 300) throw new Error(`node answered ${r.status}`);
  const inbox = JSON.parse(r.body ?? (r.body_b64 ? atob(r.body_b64) : "[]")) as unknown[];
  set(pid, { needs: inbox.length, state: "up", at: Date.now() });
}

function startReader(pid: string) {
  const c = connectionOf(pid);
  if (!c) {
    set(pid, { needs: null, state: "down", error: "not connected" });
    return;
  }
  let tunnel: Tunnel | null = null;
  let stopped = false;
  const tick = async () => {
    if (stopped) return;
    try {
      if (c.kind === "direct") await readDirect(pid, c);
      else {
        if (!tunnel) {
          const id = identityOf(pid);
          if (!id?.cert) throw new Error("no cert in that mesh yet");
          tunnel = new Tunnel();
          // Never write this profile's identity into the active one's slot.
          tunnel.persist = false;
          await tunnel.start({ enabled: true, relay: c.relay, node: c.node }, id);
        }
        await readRelay(pid, tunnel);
      }
    } catch (e) {
      if (!stopped) set(pid, { needs: states.get(pid)?.needs ?? null, state: "down", error: e instanceof Error ? e.message : String(e), at: Date.now() });
    }
  };
  set(pid, { needs: null, state: "idle" });
  void tick();
  const timer = window.setInterval(() => void tick(), INTERVAL_MS);
  readers.set(pid, {
    profileId: pid,
    tunnel,
    timer,
    stop: () => {
      stopped = true;
      window.clearInterval(timer);
      tunnel?.stop();
    },
  });
}

export const peek = {
  /** Readers for every profile but the active one. Idempotent. */
  start() {
    if (!hosted) return;
    const me = activeProfile()?.id;
    const want = new Set(listProfiles().filter((p) => p.id !== me).map((p) => p.id));
    for (const [pid, r] of readers) {
      if (!want.has(pid)) {
        r.stop();
        readers.delete(pid);
        states.delete(pid);
      }
    }
    for (const pid of want) if (!readers.has(pid)) startReader(pid);
    emit();
  },
  /** After a switch: the old active mesh gains a reader, the new one loses its. */
  restart() {
    peek.start();
  },
  stop() {
    for (const [, r] of readers) r.stop();
    readers.clear();
    states.clear();
    emit();
  },
  get(pid: string): PeekState | undefined {
    return states.get(pid);
  },
  onChange(l: () => void): () => void {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
};
