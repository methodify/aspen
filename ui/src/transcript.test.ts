import { describe, expect, it } from "vitest";
import type { SessionEvent } from "./events";
import {
  addLocalUserMessage,
  applyEvent,
  emptyTranscript,
  markLocalUserMessage,
  reconcileSnapshot,
  seedFromHistory,
  type AssistantBubbleItem,
  type BusBubbleItem,
  type PermissionCardItem,
  type ToolCardItem,
  type TranscriptState,
  type UserBubbleItem,
} from "./transcript";

function run(events: SessionEvent[], initial?: TranscriptState): TranscriptState {
  return events.reduce(applyEvent, initial ?? emptyTranscript());
}

function delta(text: string, thinking = false): SessionEvent {
  return { kind: "text_delta", text, thinking };
}

function snapshot(messageId: string, text: string): SessionEvent {
  return {
    kind: "assistant_message",
    message_id: messageId,
    raw: { type: "assistant", message: { id: messageId, content: [{ type: "text", text }] } },
  };
}

function toolUse(id: string, name: string, input: unknown): SessionEvent {
  return { kind: "tool_use", raw: { id, name, input } };
}

function toolResult(toolUseId: string, content: unknown, isError = false): SessionEvent {
  return { kind: "tool_result", raw: { tool_use_id: toolUseId, content, is_error: isError } };
}

function bubbles(s: TranscriptState): AssistantBubbleItem[] {
  return s.items.filter((i): i is AssistantBubbleItem => i.kind === "assistant");
}

describe("reconcileSnapshot", () => {
  it("replaces when incoming contains current", () => {
    expect(reconcileSnapshot("Hel", "Hello there")).toBe("Hello there");
  });
  it("drops when current ends with incoming (duplicate re-emission)", () => {
    expect(reconcileSnapshot("Intro.\n\nThe lighthouse.", "The lighthouse.")).toBe(
      "Intro.\n\nThe lighthouse.",
    );
  });
  it("appends with a paragraph break otherwise", () => {
    expect(reconcileSnapshot("Part one.", "Part two.")).toBe("Part one.\n\nPart two.");
  });
  it("ignores empty incoming", () => {
    expect(reconcileSnapshot("kept", "")).toBe("kept");
  });
});

describe("text_delta", () => {
  it("appends deltas into a single open tail bubble", () => {
    const s = run([delta("Hello, "), delta("world"), delta("!")]);
    const b = bubbles(s);
    expect(b).toHaveLength(1);
    expect(b[0]!.text).toBe("Hello, world!");
    expect(b[0]!.open).toBe(true);
    expect(s.openBubbleId).toBe(b[0]!.id);
  });

  it("routes thinking deltas into the collapsed thinking channel, not the text", () => {
    const s = run([delta("pondering…", true), delta("Answer.")]);
    const b = bubbles(s);
    expect(b).toHaveLength(1);
    expect(b[0]!.thinking).toBe("pondering…");
    expect(b[0]!.text).toBe("Answer.");
  });
});

describe("assistant_message snapshots", () => {
  it("replaces the open bubble when the snapshot contains the streamed text", () => {
    const s = run([delta("The lightho"), snapshot("m1", "The lighthouse stands.")]);
    const b = bubbles(s);
    expect(b).toHaveLength(1);
    expect(b[0]!.text).toBe("The lighthouse stands.");
    expect(b[0]!.messageId).toBe("m1");
  });

  it("drops a duplicate re-emission instead of doubling ('lighthouselighthouse')", () => {
    const s = run([
      delta("The lighthouse stands."),
      snapshot("m1", "The lighthouse stands."),
      snapshot("m1", "The lighthouse stands."),
    ]);
    const b = bubbles(s);
    expect(b).toHaveLength(1);
    expect(b[0]!.text).toBe("The lighthouse stands.");
  });

  it("appends a genuinely new block to the open bubble with a paragraph break", () => {
    const s = run([snapshot("m1", "First block."), snapshot("m1", "Second block.")]);
    const b = bubbles(s);
    expect(b).toHaveLength(1);
    expect(b[0]!.text).toBe("First block.\n\nSecond block.");
  });

  it("buried bubble + NEW message_id: finalizes it and opens a new bubble after the tools", () => {
    const s = run([
      delta("Let me check."),
      snapshot("m1", "Let me check."),
      toolUse("t1", "Read", { file_path: "/src/main.rs" }),
      toolResult("t1", "fn main() {}"),
      delta("Found "),
      snapshot("m2", "Found it."),
    ]);
    expect(s.items.map((i) => i.kind)).toEqual(["assistant", "tool", "assistant"]);
    const [first, , second] = s.items as [AssistantBubbleItem, ToolCardItem, AssistantBubbleItem];
    expect(first.text).toBe("Let me check.");
    expect(first.messageId).toBe("m1");
    expect(first.open).toBe(false); // finalized when buried
    expect(second.text).toBe("Found it.");
    expect(second.messageId).toBe("m2");
    expect(second.open).toBe(true);
    expect(s.openBubbleId).toBe(second.id);
  });

  it("merges into a buried bubble ONLY on matching message_id (final full re-emission)", () => {
    const s = run([
      snapshot("m1", "Let me check."),
      toolUse("t1", "Read", { file_path: "/x" }),
      snapshot("m1", "Let me check."), // final re-emission of the buried message
    ]);
    expect(s.items.map((i) => i.kind)).toEqual(["assistant", "tool"]);
    const b = bubbles(s);
    expect(b).toHaveLength(1);
    expect(b[0]!.text).toBe("Let me check.");
  });

  it("a tools-only (empty text) snapshot creates no bubble", () => {
    const s = run([{ kind: "assistant_message", message_id: "m9", raw: { message: { id: "m9", content: [] } } }]);
    expect(s.items).toHaveLength(0);
  });
});

describe("tool cards", () => {
  it("attaches tool_result to its tool_use by tool_use_id", () => {
    const s = run([
      toolUse("t1", "Bash", { command: "ls" }),
      toolUse("t2", "Read", { file_path: "/y" }),
      toolResult("t2", "contents"),
      toolResult("t1", "a b c", true),
    ]);
    const cards = s.items.filter((i): i is ToolCardItem => i.kind === "tool");
    expect(cards).toHaveLength(2);
    expect(cards[0]!.result).toBe("a b c");
    expect(cards[0]!.isError).toBe(true);
    expect(cards[1]!.result).toBe("contents");
    expect(cards[1]!.done).toBe(true);
  });
});

describe("turn_ended", () => {
  it("closes the open bubble, settles running tool cards, and records the marker", () => {
    const s = run([
      delta("Working."),
      toolUse("t1", "Bash", { command: "sleep 99" }),
      {
        kind: "turn_ended",
        subtype: "success",
        total_cost_usd: 0.42,
        duration_ms: 1234,
        result_text: "Working.",
      },
    ]);
    expect(s.openBubbleId).toBeNull();
    expect(bubbles(s)[0]!.open).toBe(false);
    const card = s.items.find((i): i is ToolCardItem => i.kind === "tool")!;
    expect(card.done).toBe(true);
    const marker = s.items[s.items.length - 1]!;
    expect(marker.kind).toBe("turn_end");
    if (marker.kind === "turn_end") {
      expect(marker.costUsd).toBe(0.42);
      expect(marker.subtype).toBe("success");
    }
  });
});

describe("permissions", () => {
  it("renders a card and collapses it on permission_settled with the same request_id", () => {
    const s = run([
      { kind: "permission_asked", request_id: "r1", tool_name: "Edit", input: { file_path: "/z" } },
      { kind: "permission_settled", request_id: "r1", allow: true },
    ]);
    const card = s.items.find((i): i is PermissionCardItem => i.kind === "permission")!;
    expect(card.settled).toBe(true);
    expect(card.outcome).toBe("allowed");
  });
});

describe("seedFromHistory", () => {
  it("renders rehydrated history as finalized bubbles, tool chips, and bus bubbles", () => {
    const s = seedFromHistory([
      { role: "user", text: "hello", uuid: "u-1" },
      { role: "assistant", text: "Checking.", tools: [{ id: "t1", name: "Read" }] },
      { role: "user", text: "[aspen bus] from @arch · #proj\nping", bus: true },
      { role: "assistant", text: "Done." },
    ]);
    expect(s.items.map((i) => i.kind)).toEqual(["user", "assistant", "tool", "bus", "assistant"]);
    expect(s.openBubbleId).toBeNull();
    const user = s.items[0] as UserBubbleItem;
    expect(user.pending).toBe(false);
    expect(user.uuid).toBe("u-1");
    const chip = s.items[2] as ToolCardItem;
    expect(chip.name).toBe("Read");
    expect(chip.done).toBe(true);
    for (const b of bubbles(s)) expect(b.open).toBe(false);
  });

  it("live events append cleanly after seeded history", () => {
    let s = seedFromHistory([{ role: "assistant", text: "Earlier reply." }]);
    s = run([delta("New "), delta("turn.")], s);
    const b = bubbles(s);
    expect(b).toHaveLength(2);
    expect(b[0]!.text).toBe("Earlier reply.");
    expect(b[1]!.text).toBe("New turn.");
    expect(b[1]!.open).toBe(true);
  });
});

describe("operator sends and replay acks", () => {
  it("marks the optimistic bubble delivered on a uuid-matched user_replay", () => {
    let s = addLocalUserMessage(emptyTranscript(), "hello agent", "lk1");
    s = markLocalUserMessage(s, "lk1", { uuid: "u-1" });
    s = applyEvent(s, { kind: "user_replay", uuid: "u-1" });
    const b = s.items.find((i): i is UserBubbleItem => i.kind === "user")!;
    expect(b.pending).toBe(false);
  });

  it("falls back to text matching when the ack races the POST response", () => {
    let s = addLocalUserMessage(emptyTranscript(), "hello agent", "lk1");
    s = applyEvent(s, {
      kind: "user_replay",
      uuid: "u-1",
      raw: { type: "user", message: { role: "user", content: "hello agent" } },
    });
    const b = s.items.find((i): i is UserBubbleItem => i.kind === "user")!;
    expect(b.pending).toBe(false);
    expect(b.uuid).toBe("u-1");
  });
});

describe("bus traffic", () => {
  const busText =
    "[aspen bus] from @arch (contextua @ gpu-box) · #contextua · thread t-7\nplease review the diff";

  it("renders user-role raw frames with the bus header as bus bubbles", () => {
    const s = run([
      { kind: "raw", raw: { type: "user", message: { role: "user", content: busText } } },
    ]);
    const b = s.items.find((i): i is BusBubbleItem => i.kind === "bus")!;
    expect(b.text).toBe(busText);
  });

  it("dedupes the same injection arriving via raw and user_replay", () => {
    const s = run([
      { kind: "raw", raw: { type: "user", message: { role: "user", content: busText } } },
      { kind: "user_replay", uuid: "u-9", raw: { type: "user", message: { content: busText } } },
    ]);
    expect(s.items.filter((i) => i.kind === "bus")).toHaveLength(1);
  });

  it("suppresses the synthetic interrupt user message", () => {
    const s = run([
      {
        kind: "user_replay",
        raw: { type: "user", message: { content: "[Request interrupted by user]" } },
      },
    ]);
    expect(s.items).toHaveLength(0);
  });
});
