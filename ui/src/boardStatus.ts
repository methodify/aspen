/* A board's members and their state (BOARDS.md §10): who is on it — the
 * session panes of a laid-out board, or the fleet query of a dynamic one —
 * and, for the rail, the Boards page, the board's header and Now, how
 * many are busy, live and idle, down, waiting on the operator, or running
 * background work. */
import { useEffect, useState } from "react";
import { api, type Agent, type Board, type BoardNode } from "./api";
import type { Presence } from "./components";
import { presenceOf } from "./components";

export interface DynamicQuery {
  node?: string;
  channel?: string;
  state?: "busy" | "live" | "attention" | "activity" | "any";
  name?: string;
  style?: "grid" | "main";
}

export function matchQuery(a: Agent, q: DynamicQuery, attention: Set<string>): boolean {
  if (q.node && a.node !== q.node) return false;
  if (q.channel && a.channel !== q.channel) return false;
  if (q.name && !a.name.toLowerCase().includes(q.name.toLowerCase())) return false;
  switch (q.state ?? "live") {
    case "busy":
      return a.live && a.turn_state === "busy";
    case "live":
      return a.live;
    case "attention":
      return attention.has(a.name);
    case "activity":
      return (a.activities?.running ?? 0) > 0;
    default:
      return true;
  }
}

/** The agent names a laid-out board's session panes hold. */
export function paneAgents(n: BoardNode): string[] {
  if (n.kind === "split") return n.children.flatMap(paneAgents);
  return n.kind === "session" && n.agent ? [n.agent] : [];
}

/** Find a pane's agent in the fleet: by address, or — for a name a board
 *  carries fully qualified with this node (`bare@repo@node`) — without
 *  the node. */
function lookup(byName: Map<string, Agent>, name: string): Agent | undefined {
  const hit = byName.get(name);
  if (hit) return hit;
  const parts = name.split("@");
  if (parts.length === 3) {
    const local = byName.get(`${parts[0]}@${parts[1]}`);
    if (local && !local.remote) return local;
  }
  return undefined;
}

export interface BoardStatus {
  members: Agent[];
  /** Panes naming a session this console does not know (gone, or on an
   *  unreachable node). */
  unknown: number;
  busy: number;
  idle: number;
  off: number;
  waiting: number;
  activity: number;
  /** One reading for the whole board: busy if any member is, else idle if
   *  any is live, else off. */
  presence: Presence;
  /** Every member's presence, busy first — the constituent meter's bars. */
  bars: Presence[];
}

export function boardStatus(b: Board, agents: Agent[], waiting: Set<string>): BoardStatus {
  const byName = new Map(agents.map((a) => [a.name, a] as const));
  let members: Agent[];
  let unknown = 0;
  if (b.query) {
    members = agents.filter((a) => matchQuery(a, b.query as DynamicQuery, waiting));
  } else {
    const seen = new Set<string>();
    members = [];
    for (const n of paneAgents(b.layout)) {
      const a = lookup(byName, n);
      if (!a) {
        unknown++;
        continue;
      }
      if (seen.has(a.name)) continue;
      seen.add(a.name);
      members.push(a);
    }
  }
  const pres = members.map((a) => presenceOf(a.live, a.turn_state));
  const busy = pres.filter((p) => p === "busy").length;
  const idle = pres.filter((p) => p === "idle").length;
  const off = pres.length - busy - idle + unknown;
  const order: Record<Presence, number> = { busy: 0, idle: 1, off: 2 };
  const bars = [...pres, ...Array<Presence>(unknown).fill("off")].sort((x, y) => order[x] - order[y]);
  return {
    members,
    unknown,
    busy,
    idle,
    off,
    waiting: members.filter((a) => waiting.has(a.name)).length,
    activity: members.filter((a) => (a.activities?.running ?? 0) > 0).length,
    presence: busy ? "busy" : idle ? "idle" : "off",
    bars,
  };
}

/** "3 busy · 2 idle · 1 down · 1 waiting on you" */
export function describeStatus(s: BoardStatus): string {
  const parts: string[] = [];
  if (s.busy) parts.push(`${s.busy} busy`);
  if (s.idle) parts.push(`${s.idle} idle`);
  if (s.off) parts.push(`${s.off} down`);
  if (s.waiting) parts.push(`${s.waiting} waiting on you`);
  if (s.activity) parts.push(`${s.activity} with background work`);
  return parts.length ? parts.join(" · ") : "no sessions";
}

/** The boards, kept fresh: loaded once, again every 15 s, and whenever a
 *  board is saved here (`aspen:boards`). */
export function useBoards(): Board[] {
  const [boards, setBoards] = useState<Board[]>([]);
  useEffect(() => {
    let stop = false;
    const load = () => api.boards().then((b) => !stop && setBoards(b)).catch(() => {});
    void load();
    const t = window.setInterval(() => void load(), 15000);
    window.addEventListener("aspen:boards", load);
    return () => {
      stop = true;
      window.clearInterval(t);
      window.removeEventListener("aspen:boards", load);
    };
  }, []);
  return boards;
}
