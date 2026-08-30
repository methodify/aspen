// Pure transcript model + the three-layer streaming merge from
// docs/reference/CLAUDE_RUNTIME_REFERENCE.md §5.2 (snapshot-reconciling
// merge; tail-only merge keyed by upstream message id; chronology with tool
// cards) and §5.3–5.4 (turn end, interrupts).
//
// No React, no I/O — every function takes a TranscriptState and returns a
// new one, so this module is unit-testable in isolation.

import type { HistoryItem } from "./api";
import type {
  SessionEvent,
  AssistantMessageEvent,
  TextDeltaEvent,
  ToolUseEvent,
  ToolResultEvent,
  UserReplayEvent,
  PermissionAskedEvent,
  PermissionSettledEvent,
  TurnEndedEvent,
  RawEvent,
} from "./events";

export const BUS_HEADER = "[aspen bus]";
const INTERRUPT_MARKER = "[Request interrupted";

// ---------------------------------------------------------------------------
// Items

export interface AssistantBubbleItem {
  kind: "assistant";
  id: number;
  text: string;
  /** Collapsed/dim thinking text (from text_delta with thinking=true). */
  thinking: string;
  /** Upstream message id, once a snapshot has identified this bubble. */
  messageId: string | null;
  /** True while this is the streaming tail bubble. */
  open: boolean;
}

export interface UserBubbleItem {
  kind: "user";
  id: number;
  text: string;
  uuid: string | null;
  /** Client-side key for optimistic sends, resolved by the POST response. */
  localKey: string | null;
  /** Awaiting the replay ack. */
  pending: boolean;
  failed: boolean;
}

/** A bus injection observed on the wire (user-role frame with the envelope header). */
export interface BusBubbleItem {
  kind: "bus";
  id: number;
  /** First line is the `[aspen bus] from …` envelope header. */
  text: string;
}

export interface ToolCardItem {
  kind: "tool";
  id: number;
  toolUseId: string;
  name: string;
  input: unknown;
  result: string | null;
  isError: boolean;
  /** Result attached, or the turn ended (a card may not stay "running" after the turn). */
  done: boolean;
}

export interface PermissionCardItem {
  kind: "permission";
  id: number;
  requestId: string;
  toolName: string;
  input: unknown;
  suggestions: unknown;
  settled: boolean;
  /** "allowed" | "denied" | "settled" (source unknown). */
  outcome: string | null;
}

export interface TurnEndItem {
  kind: "turn_end";
  id: number;
  subtype: string;
  /** Session-cumulative cost as reported by the runtime. */
  costUsd: number | null;
  durationMs: number | null;
}

export type TranscriptItem =
  | AssistantBubbleItem
  | UserBubbleItem
  | BusBubbleItem
  | ToolCardItem
  | PermissionCardItem
  | TurnEndItem;

export interface TranscriptState {
  items: TranscriptItem[];
  nextId: number;
  /** Item id of the single open streaming bubble, or null. */
  openBubbleId: number | null;
}

export function emptyTranscript(): TranscriptState {
  return { items: [], nextId: 1, openBubbleId: null };
}

/**
 * Seed a transcript from GET /api/agents/{name}/transcript (rehydrated
 * history from disk). Everything is finalized: assistant bubbles are closed,
 * tool chips carry no input/result (the on-disk rehydration is name+id only),
 * bus-flagged user items render as bus bubbles. Live WS events append after.
 */
export function seedFromHistory(history: HistoryItem[]): TranscriptState {
  const items: TranscriptItem[] = [];
  let nextId = 1;
  for (const h of history) {
    const text = typeof h.text === "string" ? h.text : "";
    if (h.role === "user") {
      if (h.bus === true || text.startsWith(BUS_HEADER)) {
        items.push({ kind: "bus", id: nextId++, text });
      } else if (!text.startsWith(INTERRUPT_MARKER)) {
        items.push({
          kind: "user",
          id: nextId++,
          text,
          uuid: typeof h.uuid === "string" ? h.uuid : null,
          localKey: null,
          pending: false,
          failed: false,
        });
      }
    } else {
      if (text) {
        items.push({
          kind: "assistant",
          id: nextId++,
          text,
          thinking: "",
          messageId: null,
          open: false,
        });
      }
      for (const t of h.tools ?? []) {
        items.push({
          kind: "tool",
          id: nextId++,
          toolUseId: typeof t.id === "string" ? t.id : "",
          name: typeof t.name === "string" ? t.name : "tool",
          input: null,
          result: null,
          isError: false,
          done: true,
        });
      }
    }
  }
  return { items, nextId, openBubbleId: null };
}

// ---------------------------------------------------------------------------
// Small helpers

function asRecord(v: unknown): Record<string, unknown> | null {
  return v !== null && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
}

/**
 * §5.2 snapshot-reconciling rules:
 *  - incoming contains current → replace (fuller snapshot)
 *  - current ends with incoming → drop (duplicate re-emission)
 *  - else → append with a paragraph break
 */
export function reconcileSnapshot(current: string, incoming: string): string {
  if (!incoming) return current;
  if (!current) return incoming;
  if (incoming.includes(current)) return incoming;
  if (current.endsWith(incoming)) return current;
  return current + "\n\n" + incoming;
}

function indexOfItem(items: TranscriptItem[], id: number | null): number {
  if (id === null) return -1;
  return items.findIndex((it) => it.id === id);
}

/** Anything appended after the open bubble buries it — chronology is sacred. */
function isBuriedAt(items: TranscriptItem[], idx: number): boolean {
  return idx >= 0 && idx < items.length - 1;
}

function closeBubbleAt(items: TranscriptItem[], idx: number): TranscriptItem[] {
  if (idx < 0) return items;
  const it = items[idx];
  if (it.kind !== "assistant" || !it.open) return items;
  const next = items.slice();
  next[idx] = { ...it, open: false };
  return next;
}

// Text extraction from raw runtime envelopes -------------------------------

function contentOf(raw: unknown): unknown {
  const r = asRecord(raw);
  if (!r) return null;
  const msg = asRecord(r["message"]);
  if (msg && "content" in msg) return msg["content"];
  if ("content" in r) return r["content"];
  return null;
}

/** Join the text blocks of an envelope's content (skips thinking/tool blocks). */
function extractText(raw: unknown): string {
  const content = contentOf(raw);
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    const parts: string[] = [];
    for (const block of content) {
      const b = asRecord(block);
      if (b && b["type"] === "text" && typeof b["text"] === "string") {
        parts.push(b["text"]);
      }
    }
    return parts.join("\n\n");
  }
  return "";
}

function extractToolUse(ev: ToolUseEvent): { id: string; name: string; input: unknown } {
  const candidates: unknown[] = [];
  const content = contentOf(ev.raw);
  if (Array.isArray(content)) {
    for (const block of content) {
      const b = asRecord(block);
      if (b && b["type"] === "tool_use") candidates.push(b);
    }
  }
  candidates.push(ev.raw, ev);
  for (const c of candidates) {
    const r = asRecord(c);
    if (!r) continue;
    const name = r["name"] ?? r["tool_name"];
    if (typeof name !== "string" || !name) continue;
    const id = r["id"] ?? r["tool_use_id"];
    return {
      id: typeof id === "string" ? id : "",
      name,
      input: r["input"] ?? null,
    };
  }
  return { id: typeof ev.tool_use_id === "string" ? ev.tool_use_id : "", name: "tool", input: ev.input ?? null };
}

function renderResultContent(content: unknown): string {
  if (content === null || content === undefined) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((block) => {
        const b = asRecord(block);
        if (b && b["type"] === "text" && typeof b["text"] === "string") return b["text"];
        return JSON.stringify(block, null, 2);
      })
      .join("\n");
  }
  return JSON.stringify(content, null, 2);
}

function extractToolResult(ev: ToolResultEvent): {
  toolUseId: string;
  text: string;
  isError: boolean;
} {
  const candidates: unknown[] = [];
  const content = contentOf(ev.raw);
  if (Array.isArray(content)) {
    for (const block of content) {
      const b = asRecord(block);
      if (b && b["type"] === "tool_result") candidates.push(b);
    }
  }
  candidates.push(ev.raw, ev);
  for (const c of candidates) {
    const r = asRecord(c);
    if (!r) continue;
    const id = r["tool_use_id"];
    if (typeof id !== "string" || !id) continue;
    return {
      toolUseId: id,
      text: renderResultContent(r["content"]),
      isError: r["is_error"] === true,
    };
  }
  return {
    toolUseId: "",
    text: renderResultContent(ev.content ?? ev.raw),
    isError: ev.is_error === true,
  };
}

/** One-line summary of a tool call's most interesting input, for compact cards. */
export function toolSummary(input: unknown): string {
  const KEYS = [
    "file_path",
    "path",
    "command",
    "pattern",
    "query",
    "url",
    "host",
    "description",
    "prompt",
  ];
  const r = asRecord(input);
  if (r) {
    for (const k of KEYS) {
      const v = r[k];
      if (typeof v === "string" && v.trim()) {
        const line = v.trim().split("\n")[0]!;
        return line.length > 80 ? line.slice(0, 77) + "…" : line;
      }
    }
  }
  return "";
}

// ---------------------------------------------------------------------------
// Local (optimistic) operations

export function addLocalUserMessage(
  state: TranscriptState,
  text: string,
  localKey: string,
): TranscriptState {
  const item: UserBubbleItem = {
    kind: "user",
    id: state.nextId,
    text,
    uuid: null,
    localKey,
    pending: true,
    failed: false,
  };
  return { ...state, items: [...state.items, item], nextId: state.nextId + 1 };
}

export function markLocalUserMessage(
  state: TranscriptState,
  localKey: string,
  patch: { uuid?: string; failed?: boolean },
): TranscriptState {
  const idx = state.items.findIndex((it) => it.kind === "user" && it.localKey === localKey);
  if (idx < 0) return state;
  const items = state.items.slice();
  const bubble = items[idx] as UserBubbleItem;
  items[idx] = {
    ...bubble,
    uuid: patch.uuid ?? bubble.uuid,
    failed: patch.failed ?? bubble.failed,
    pending: patch.failed ? false : bubble.pending,
  };
  return { ...state, items };
}

// ---------------------------------------------------------------------------
// The reducer

export function applyEvent(state: TranscriptState, ev: SessionEvent): TranscriptState {
  switch (ev.kind) {
    case "text_delta":
      return applyDelta(state, ev);
    case "assistant_message":
      return applyAssistantMessage(state, ev);
    case "tool_use":
      return applyToolUse(state, ev);
    case "tool_result":
      return applyToolResult(state, ev);
    case "permission_asked":
      return applyPermissionAsked(state, ev);
    case "permission_settled":
      return applyPermissionSettled(state, ev);
    case "turn_ended":
      return applyTurnEnded(state, ev);
    case "user_replay":
      return applyUserReplay(state, ev);
    case "raw":
      return applyRaw(state, ev);
    default:
      // runtime_init / status / stderr / exited are handled at the page level.
      return state;
  }
}

/**
 * Ensure the open streaming bubble is at the tail; if it is buried (tool
 * cards or other items after it), finalize it and open a fresh bubble after
 * them. Returns the state plus the tail bubble's index.
 */
function withOpenTailBubble(state: TranscriptState): {
  state: TranscriptState;
  index: number;
} {
  const idx = indexOfItem(state.items, state.openBubbleId);
  if (idx >= 0 && !isBuriedAt(state.items, idx)) {
    return { state, index: idx };
  }
  let items = closeBubbleAt(state.items, idx);
  const bubble: AssistantBubbleItem = {
    kind: "assistant",
    id: state.nextId,
    text: "",
    thinking: "",
    messageId: null,
    open: true,
  };
  items = [...items, bubble];
  return {
    state: { items, nextId: state.nextId + 1, openBubbleId: bubble.id },
    index: items.length - 1,
  };
}

function applyDelta(state: TranscriptState, ev: TextDeltaEvent): TranscriptState {
  if (typeof ev.text !== "string" || ev.text === "") return state;
  const { state: s, index } = withOpenTailBubble(state);
  const items = s.items.slice();
  const bubble = items[index] as AssistantBubbleItem;
  items[index] = ev.thinking
    ? { ...bubble, thinking: bubble.thinking + ev.text }
    : { ...bubble, text: bubble.text + ev.text };
  return { ...s, items };
}

function applyAssistantMessage(
  state: TranscriptState,
  ev: AssistantMessageEvent,
): TranscriptState {
  const text = extractText(ev.raw);
  const mid = typeof ev.message_id === "string" ? ev.message_id : "";

  // 1. A bubble already keyed to this upstream message id (open or buried):
  //    this is a re-emission of a message we have — reconcile in place.
  if (mid) {
    const idIdx = state.items.findIndex(
      (it) => it.kind === "assistant" && it.messageId === mid,
    );
    if (idIdx >= 0) {
      const items = state.items.slice();
      const bubble = items[idIdx] as AssistantBubbleItem;
      items[idIdx] = { ...bubble, text: reconcileSnapshot(bubble.text, text) };
      return { ...state, items };
    }
  }

  // A snapshot with no text (e.g. a tools-only message) creates nothing.
  if (!text) return state;

  const openIdx = indexOfItem(state.items, state.openBubbleId);

  // 2. Open bubble at the tail: reconcile the snapshot into it and adopt the id.
  if (openIdx >= 0 && !isBuriedAt(state.items, openIdx)) {
    const items = state.items.slice();
    const bubble = items[openIdx] as AssistantBubbleItem;
    items[openIdx] = {
      ...bubble,
      text: reconcileSnapshot(bubble.text, text),
      messageId: mid || bubble.messageId,
    };
    return { ...state, items };
  }

  // 3. Buried open bubble + a NEW message id (or none at all): a new phase of
  //    the turn — finalize the buried bubble and open a new one after the tools.
  const items = closeBubbleAt(state.items, openIdx);
  const bubble: AssistantBubbleItem = {
    kind: "assistant",
    id: state.nextId,
    text,
    thinking: "",
    messageId: mid || null,
    open: true,
  };
  return {
    items: [...items, bubble],
    nextId: state.nextId + 1,
    openBubbleId: bubble.id,
  };
}

function applyToolUse(state: TranscriptState, ev: ToolUseEvent): TranscriptState {
  const t = extractToolUse(ev);
  const item: ToolCardItem = {
    kind: "tool",
    id: state.nextId,
    toolUseId: t.id,
    name: t.name,
    input: t.input,
    result: null,
    isError: false,
    done: false,
  };
  return { ...state, items: [...state.items, item], nextId: state.nextId + 1 };
}

function applyToolResult(state: TranscriptState, ev: ToolResultEvent): TranscriptState {
  const r = extractToolResult(ev);
  let idx = -1;
  for (let i = state.items.length - 1; i >= 0; i--) {
    const it = state.items[i]!;
    if (it.kind !== "tool") continue;
    if (r.toolUseId ? it.toolUseId === r.toolUseId : !it.done) {
      idx = i;
      break;
    }
  }
  if (idx < 0) return state;
  const items = state.items.slice();
  const card = items[idx] as ToolCardItem;
  items[idx] = { ...card, result: r.text, isError: r.isError, done: true };
  return { ...state, items };
}

function applyPermissionAsked(
  state: TranscriptState,
  ev: PermissionAskedEvent,
): TranscriptState {
  if (
    state.items.some((it) => it.kind === "permission" && it.requestId === ev.request_id)
  ) {
    return state;
  }
  const item: PermissionCardItem = {
    kind: "permission",
    id: state.nextId,
    requestId: ev.request_id,
    toolName: ev.tool_name,
    input: ev.input ?? null,
    suggestions: ev.suggestions ?? null,
    settled: false,
    outcome: null,
  };
  return { ...state, items: [...state.items, item], nextId: state.nextId + 1 };
}

function applyPermissionSettled(
  state: TranscriptState,
  ev: PermissionSettledEvent,
): TranscriptState {
  const idx = state.items.findIndex(
    (it) => it.kind === "permission" && it.requestId === ev.request_id,
  );
  if (idx < 0) return state;
  const card = state.items[idx] as PermissionCardItem;
  if (card.settled) return state;
  let outcome = "settled";
  if (ev.allow === true || ev.behavior === "allow") outcome = "allowed";
  else if (ev.allow === false || ev.behavior === "deny") outcome = "denied";
  const items = state.items.slice();
  items[idx] = { ...card, settled: true, outcome };
  return { ...state, items };
}

function applyTurnEnded(state: TranscriptState, ev: TurnEndedEvent): TranscriptState {
  // §5.3: finalize the open bubble and settle any tool card still "running".
  let items: TranscriptItem[] = state.items.map((it) => {
    if (it.kind === "assistant" && it.open) return { ...it, open: false };
    if (it.kind === "tool" && !it.done) return { ...it, done: true };
    return it;
  });
  const marker: TurnEndItem = {
    kind: "turn_end",
    id: state.nextId,
    subtype: typeof ev.subtype === "string" ? ev.subtype : "success",
    costUsd: typeof ev.total_cost_usd === "number" ? ev.total_cost_usd : null,
    durationMs: typeof ev.duration_ms === "number" ? ev.duration_ms : null,
  };
  items = [...items, marker];
  return { items, nextId: state.nextId + 1, openBubbleId: null };
}

function pushBusBubble(state: TranscriptState, text: string): TranscriptState {
  // Dedupe: the same injection may surface via both `raw` and `user_replay`.
  if (state.items.some((it) => it.kind === "bus" && it.text === text)) return state;
  const item: BusBubbleItem = { kind: "bus", id: state.nextId, text };
  return { ...state, items: [...state.items, item], nextId: state.nextId + 1 };
}

function applyUserReplay(state: TranscriptState, ev: UserReplayEvent): TranscriptState {
  const uuid =
    typeof ev.uuid === "string" && ev.uuid
      ? ev.uuid
      : (() => {
          const r = asRecord(ev.raw);
          const u = r?.["uuid"];
          return typeof u === "string" ? u : "";
        })();
  const text = typeof ev.text === "string" && ev.text ? ev.text : extractText(ev.raw);

  // 1. Replay ack for one of our own sends, matched by uuid.
  if (uuid) {
    const idx = state.items.findIndex(
      (it) => it.kind === "user" && it.uuid === uuid && it.pending,
    );
    if (idx >= 0) {
      const items = state.items.slice();
      const bubble = items[idx] as UserBubbleItem;
      items[idx] = { ...bubble, pending: false };
      return { ...state, items };
    }
  }

  // 2. A bus injection whose body rode along on the replay.
  if (text.startsWith(BUS_HEADER)) return pushBusBubble(state, text);

  // 3. The synthetic interrupt message — never render as something typed. §5.4
  if (text.startsWith(INTERRUPT_MARKER)) return state;

  // 4. Ack racing the POST response: match a pending bubble by text.
  if (text) {
    const idx = state.items.findIndex(
      (it) => it.kind === "user" && it.pending && it.text === text,
    );
    if (idx >= 0) {
      const items = state.items.slice();
      const bubble = items[idx] as UserBubbleItem;
      items[idx] = { ...bubble, pending: false, uuid: uuid || bubble.uuid };
      return { ...state, items };
    }
  }

  // Otherwise it is an ack for traffic we did not originate — record nothing.
  return state;
}

function applyRaw(state: TranscriptState, ev: RawEvent): TranscriptState {
  const r = asRecord(ev.raw);
  if (!r || r["type"] !== "user") return state;
  const text = extractText(ev.raw);
  if (text.startsWith(BUS_HEADER)) return pushBusBubble(state, text);
  return state;
}
