import { describe, expect, it } from "vitest";
import { fragmentFrame } from "./tunnel";

describe("fragmentFrame", () => {
  it("sends a small frame whole", () => {
    expect(fragmentFrame("{\"v\":1}", 16)).toEqual(["{\"v\":1}"]);
  });
  it("splits a big frame into pieces the node reassembles", () => {
    const frame = JSON.stringify({ v: 1, ciphertext: "A".repeat(1000) });
    const pieces = fragmentFrame(frame, 100);
    expect(pieces.length).toBe(Math.ceil(frame.length / 100));
    // The node's Reassembler keys on this prefix (federation.rs).
    for (const p of pieces) expect(p.startsWith("{\"frag\":")).toBe(true);
    const parts = pieces.map((p) => (JSON.parse(p) as { frag: { id: string; i: number; n: number; d: string } }).frag);
    expect(new Set(parts.map((f) => f.id)).size).toBe(1);
    expect(parts.every((f, i) => f.i === i && f.n === pieces.length)).toBe(true);
    expect(parts.map((f) => f.d).join("")).toBe(frame);
  });
});
