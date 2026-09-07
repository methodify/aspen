// Board placement shared by the session header's "board ▾" and template
// spawns (PLUGINS.md §templates): put an agent on a board — into the
// first empty pane, or by splitting the board right or down.

import { api, type Board, type BoardNode } from "./api";

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

/** Put `agent` on board `b` and save it. */
export async function placeOnBoard(b: Board, agent: string, how: PlaceMode): Promise<void> {
  if (b.query) return; // dynamic boards compute their own panes
  const layout = placeInLayout(b.layout, agent, how);
  await api.putBoard({ ...b, layout, updated_at: Date.now() / 1000 });
}

/** Put `agent` on the board with this id, if it exists; returns whether it did. */
export async function placeOnBoardId(boardId: string, agent: string, how: PlaceMode): Promise<boolean> {
  const boards = await api.boards();
  const b = boards.find((x) => x.id === boardId);
  if (!b) return false;
  await placeOnBoard(b, agent, how);
  return true;
}
