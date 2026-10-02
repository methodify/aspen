import { describe, expect, it } from "vitest";
import { boardStatus, describeStatus } from "./boardStatus";
import type { Agent, Board } from "./api";

const agent = (name: string, live: boolean, turn: "busy" | "idle" | null, extra: Partial<Agent> = {}): Agent =>
  ({ name, live, turn_state: turn, node: "n", channel: "c", ...extra }) as unknown as Agent;

const board = (agents: string[]): Board => ({
  id: "b",
  name: "b",
  updated_at: 0,
  layout: {
    kind: "split",
    id: "s",
    dir: "row",
    sizes: agents.map(() => 1),
    children: agents.map((a, i) => ({ kind: "session", id: `p${i}`, agent: a })),
  } as unknown as Board["layout"],
});

describe("boardStatus", () => {
  it("reads busy if any member is busy, bars busy first", () => {
    const fleet = [agent("a@r", true, "idle"), agent("b@r", true, "busy"), agent("c@r", false, null)];
    const st = boardStatus(board(["a@r", "b@r", "c@r"]), fleet, new Set(["a@r"]));
    expect(st.presence).toBe("busy");
    expect(st.bars).toEqual(["busy", "idle", "off"]);
    expect([st.busy, st.idle, st.off, st.waiting]).toEqual([1, 1, 1, 1]);
    expect(describeStatus(st)).toBe("1 busy · 1 idle · 1 down · 1 waiting on you");
  });
  it("reads idle when live but quiet, off when nothing runs; unknown panes count as down", () => {
    const fleet = [agent("a@r", true, "idle"), agent("c@r", false, null)];
    expect(boardStatus(board(["a@r", "c@r"]), fleet, new Set()).presence).toBe("idle");
    const st = boardStatus(board(["c@r", "gone@r"]), fleet, new Set());
    expect(st.presence).toBe("off");
    expect(st.unknown).toBe(1);
    expect(st.off).toBe(2);
  });
  it("finds a self-qualified pane name and counts a member once", () => {
    const fleet = [agent("a@r", true, "busy")];
    const st = boardStatus(board(["a@r@thisnode", "a@r"]), fleet, new Set());
    expect(st.members.length).toBe(1);
    expect(st.busy).toBe(1);
  });
});

import { normalizeModels, runtimeModels } from "./pages/sessionExtras";

describe("runtimeModels", () => {
  it("takes Claude's list from the handshake and Codex's from the runtime info", () => {
    const claude = { handshake: { models: [{ value: "default" }, { value: "opus" }] }, runtime: { models: [] } };
    const codex = { handshake: { thread: {} } as { models?: unknown[] }, runtime: { models: [{ value: "gpt-6-astra", isDefault: true }, { value: "gpt-5.6-sol" }] } };
    expect(runtimeModels(claude).length).toBe(2);
    expect(normalizeModels(runtimeModels(codex)).map((m) => m.id)).toEqual(["gpt-6-astra", "gpt-5.6-sol"]);
    expect(runtimeModels(null)).toEqual([]);
  });
});
