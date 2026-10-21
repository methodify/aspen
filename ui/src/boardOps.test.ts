import { describe, expect, it } from "vitest";
import type { Board, BoardNode } from "./api";
import { benchPane, benchSwap, panesOf, stageBench, unbench } from "./boardOps";

const s = (id: string, agent: string): BoardNode => ({ kind: "session", id, agent });
const board = (layout: BoardNode, bench: Board["bench"] = []): Board => ({ id: "b", name: "t", layout, bench, updated_at: 0 });
const agentsOf = (b: Board) => panesOf(b.layout).map((p) => p.agent ?? p.kind);

describe("the bench", () => {
  const three: BoardNode = { kind: "split", id: "root", dir: "row", sizes: [50, 25, 25], children: [s("p1", "a"), s("p2", "b"), s("p3", "c")] };

  it("benches a pane and brings it back to its slot", () => {
    const benched = benchPane({ ...board(three), pairs: [["p2", "p3"]] }, "p2");
    expect(agentsOf(benched)).toEqual(["a", "c"]);
    expect(benched.bench!.map((e) => e.agent)).toEqual(["b"]);
    expect(benched.bench![0]!.slot).toMatchObject({ parent: "root", index: 1, size: 25 });
    expect(benched.pairs).toEqual([]);
    const back = unbench(benched, "b");
    expect(agentsOf(back)).toEqual(["a", "b", "c"]);
    expect(back.bench).toEqual([]);
    const root = back.layout as Extract<BoardNode, { kind: "split" }>;
    expect(Math.round(root.sizes.reduce((x, y) => x + y, 0))).toBe(100);
  });

  it("the last pane benched leaves an empty board, and comes back into it", () => {
    const one = benchPane(board(s("p1", "a")), "p1");
    expect(one.layout.kind).toBe("empty");
    expect(agentsOf(unbench(one, "a"))).toEqual(["a"]);
  });

  it("with its slot gone, a member splits the pane with the most room", () => {
    const b = board({ kind: "split", id: "r2", dir: "row", sizes: [70, 30], children: [s("p1", "a"), s("p2", "b")] }, [{ agent: "z", at: 0, slot: { parent: "gone", index: 0, size: 50 } }]);
    const back = unbench(b, "z");
    expect(agentsOf(back)).toEqual(["a", "z", "b"]);
  });

  it("a swap puts the benched member in the pane and benches whoever was there", () => {
    const b = board(three, [{ agent: "z", at: 0 }]);
    const swapped = benchSwap(b, "p1", "z");
    expect(agentsOf(swapped)).toEqual(["z", "b", "c"]);
    expect(swapped.bench!.map((e) => e.agent)).toEqual(["a"]);
  });

  it("shows no one on the bench who is also on the stage", () => {
    expect(stageBench(board(three, [{ agent: "a", at: 0 }, { agent: "z", at: 0 }, { agent: "z", at: 1 }])).map((e) => e.agent)).toEqual(["z"]);
  });
});
