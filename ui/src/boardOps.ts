// Board placement shared by the session header's "board ▾" and template
// spawns (PLUGINS.md §templates): put an agent on a board — into the
// first empty pane, or by splitting the board right or down.

import { api, type BenchEntry, type Board, type BoardNode, type BoardPane } from "./api";

export type PlaceMode = "fill" | "right" | "down";

export function placeInLayout(layout: BoardNode, agent: string, how: PlaceMode): BoardNode {
  const fill = (n: BoardNode): [BoardNode, boolean] => {
    if (n.kind === "split") {
      let done = false;
      const children = n.children.map((c) => {
        if (done) return c;
        const [r, d] = fill(c);
        done = d;
        return r;
      });
      return [{ ...n, children }, done];
    }
    if (n.kind === "empty") return [{ kind: "session", id: n.id, agent }, true];
    return [n, false];
  };
  let out = layout;
  let placed = false;
  if (how === "fill") [out, placed] = fill(layout);
  if (!placed) {
    const dir = how === "down" ? "col" : "row";
    const pane: BoardNode = { kind: "session", id: `${Date.now().toString(36)}`, agent };
    out = { kind: "split", id: `${Date.now().toString(36)}s`, dir, sizes: [50, 50], children: [layout, pane] };
  }
  return out;
}

/** Put `agent` on board `b` and save it. A benched member comes back
 *  where it sat instead (PROPOSALS-2026-10-T §3). */
export async function placeOnBoard(b: Board, agent: string, how: PlaceMode): Promise<void> {
  if (b.query) return; // dynamic boards compute their own panes
  const next = (b.bench ?? []).some((e) => e.agent === agent) ? unbench(b, agent) : { ...b, layout: placeInLayout(b.layout, agent, how) };
  await api.putBoard({ ...next, updated_at: Date.now() / 1000 });
}

/** Put `agent` on the board with this id, if it exists; returns whether it did. */
export async function placeOnBoardId(boardId: string, agent: string, how: PlaceMode): Promise<boolean> {
  const boards = await api.boards();
  const b = boards.find((x) => x.id === boardId);
  if (!b) return false;
  await placeOnBoard(b, agent, how);
  return true;
}

// ------------------------------------------------------------ the tree

let seq = 0;
export function newPaneId(): string {
  seq += 1;
  return `${Date.now().toString(36)}-${seq}`;
}

export function panesOf(n: BoardNode): BoardPane[] {
  return n.kind === "split" ? n.children.flatMap(panesOf) : [n];
}

export function mapTree(n: BoardNode, f: (p: BoardPane) => BoardNode): BoardNode {
  return n.kind === "split" ? { ...n, children: n.children.map((c) => mapTree(c, f)) } : f(n);
}

/** Remove a pane; a split left with one child collapses to it. */
export function removePane(root: BoardNode, id: string): BoardNode | null {
  if (root.kind !== "split") return root.id === id ? null : root;
  const kept: BoardNode[] = [];
  const sizes: number[] = [];
  root.children.forEach((c, i) => {
    const r = removePane(c, id);
    if (r) {
      kept.push(r);
      sizes.push(root.sizes[i] ?? 50);
    }
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0]!;
  const total = sizes.reduce((a, b) => a + b, 0) || 1;
  return { ...root, children: kept, sizes: sizes.map((s) => (s / total) * 100) };
}

// ------------------------------------------------------------ the bench
// (PROPOSALS-2026-10-T §3): members not shown right now, still the team.

/** Where a pane sits: its split, its index there, its share. */
function slotOf(root: BoardNode, paneId: string): BenchEntry["slot"] {
  if (root.kind !== "split") return null;
  const i = root.children.findIndex((c) => c.kind !== "split" && c.id === paneId);
  if (i >= 0) return { parent: root.id, index: i, size: root.sizes[i] ?? 100 / root.children.length };
  for (const c of root.children) {
    const r = slotOf(c, paneId);
    if (r) return r;
  }
  return null;
}

/** The bench as shown: entries whose member is not on the stage (a merge
 *  or a second console may leave one on both). */
export function stageBench(b: Board): BenchEntry[] {
  const onStage = new Set(panesOf(b.layout).flatMap((p) => (p.kind === "session" && p.agent ? [p.agent] : [])));
  const seen = new Set<string>();
  return (b.bench ?? []).filter((e) => !onStage.has(e.agent) && !seen.has(e.agent) && seen.add(e.agent));
}

/** Bench a session pane: it leaves the layout (its slot remembered) and
 *  joins the bench; a pair it was in ends on the board (the link stays). */
export function benchPane(b: Board, paneId: string): Board {
  const pane = panesOf(b.layout).find((p) => p.id === paneId);
  if (!pane || pane.kind !== "session" || !pane.agent) return b;
  const slot = slotOf(b.layout, paneId);
  return {
    ...b,
    layout: removePane(b.layout, paneId) ?? { kind: "empty", id: newPaneId() },
    bench: [...(b.bench ?? []).filter((e) => e.agent !== pane.agent), { agent: pane.agent, at: Date.now() / 1000, slot }],
    pairs: (b.pairs ?? []).filter(([x, y]) => x !== paneId && y !== paneId),
  };
}

function findSplit(n: BoardNode, id: string): boolean {
  return n.kind === "split" && (n.id === id || n.children.some((c) => findSplit(c, id)));
}

function insertAt(n: BoardNode, parent: string, index: number, size: number, pane: BoardPane): BoardNode {
  if (n.kind !== "split") return n;
  if (n.id !== parent) return { ...n, children: n.children.map((c) => insertAt(c, parent, index, size, pane)) };
  const s = Math.min(90, Math.max(10, size));
  const children = [...n.children];
  const at = Math.min(index, children.length);
  children.splice(at, 0, pane);
  const sizes = n.sizes.map((x) => (x * (100 - s)) / 100);
  sizes.splice(at, 0, s);
  return { ...n, children, sizes };
}

/** The pane with the most room (its share of the board). */
function largestPane(n: BoardNode, share = 1): { id: string; share: number } | null {
  if (n.kind !== "split") return { id: n.id, share };
  let best: { id: string; share: number } | null = null;
  n.children.forEach((c, i) => {
    const r = largestPane(c, (share * (n.sizes[i] ?? 100 / n.children.length)) / 100);
    if (r && (!best || r.share > best.share)) best = r;
  });
  return best;
}

/** Bring a member back: to its slot if that split still exists, else
 *  into the first empty pane, else beside the pane with the most room. */
export function unbench(b: Board, agent: string): Board {
  const entry = (b.bench ?? []).find((e) => e.agent === agent);
  const bench = (b.bench ?? []).filter((e) => e.agent !== agent);
  const pane: BoardPane = { kind: "session", id: newPaneId(), agent };
  if (panesOf(b.layout).some((p) => p.kind === "session" && p.agent === agent)) return { ...b, bench };
  if (b.layout.kind === "empty") return { ...b, bench, layout: { ...pane, id: b.layout.id } };
  if (entry?.slot && findSplit(b.layout, entry.slot.parent)) {
    return { ...b, bench, layout: insertAt(b.layout, entry.slot.parent, entry.slot.index, entry.slot.size, pane) };
  }
  if (panesOf(b.layout).some((p) => p.kind === "empty")) return { ...b, bench, layout: placeInLayout(b.layout, agent, "fill") };
  const big = largestPane(b.layout);
  if (!big) return { ...b, bench, layout: pane };
  return {
    ...b,
    bench,
    layout: mapTree(b.layout, (p) => (p.id === big.id ? { kind: "split", id: newPaneId(), dir: "row", sizes: [50, 50], children: [p, pane] } : p)),
  };
}

/** Put a benched member in a pane; whoever was there takes its place on
 *  the bench (remembering this slot). An empty pane just fills. */
export function benchSwap(b: Board, paneId: string, agent: string): Board {
  const pane = panesOf(b.layout).find((p) => p.id === paneId);
  if (!pane) return b;
  let bench = (b.bench ?? []).filter((e) => e.agent !== agent);
  if (pane.kind === "session" && pane.agent && pane.agent !== agent) {
    bench = [...bench.filter((e) => e.agent !== pane.agent), { agent: pane.agent, at: Date.now() / 1000, slot: slotOf(b.layout, paneId) }];
  }
  return {
    ...b,
    bench,
    layout: mapTree(b.layout, (p) => (p.id === paneId ? { kind: "session", id: paneId, agent } : p)),
    pairs: (b.pairs ?? []).filter(([x, y]) => x !== paneId && y !== paneId),
  };
}
