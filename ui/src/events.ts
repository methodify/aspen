// Typed SessionEvent union matching the aspen node WS contract (docs/API.md).
// Frames are JSON text, tagged by snake_case `kind`.

export interface RuntimeInitEvent {
  kind: "runtime_init";
  raw?: unknown;
}

export interface TextDeltaEvent {
  kind: "text_delta";
  text: string;
  thinking?: boolean;
}

export interface AssistantMessageEvent {
  kind: "assistant_message";
  message_id: string;
  raw?: unknown;
}

export interface ToolUseEvent {
  kind: "tool_use";
  tool_use_id?: string;
  tool_name?: string;
  name?: string;
  input?: unknown;
  raw?: unknown;
}

export interface ToolResultEvent {
  kind: "tool_result";
  tool_use_id?: string;
  content?: unknown;
  is_error?: boolean;
  raw?: unknown;
}

export interface UserReplayEvent {
  kind: "user_replay";
  uuid?: string;
  text?: string;
  raw?: unknown;
}

export interface PermissionAskedEvent {
  kind: "permission_asked";
  request_id: string;
  tool_name: string;
  input?: unknown;
  suggestions?: unknown;
}

export interface PermissionSettledEvent {
  kind: "permission_settled";
  request_id: string;
  /** Not guaranteed by the contract; used when present (and by local optimistic settles). */
  allow?: boolean;
  behavior?: string;
  raw?: unknown;
}

export interface TurnEndedEvent {
  kind: "turn_ended";
  subtype?: string;
  /** SESSION-CUMULATIVE, not per-turn. Always label it "session $X". */
  total_cost_usd?: number;
  duration_ms?: number;
  result_text?: string | null;
}

export interface StatusEvent {
  kind: "status";
  raw?: unknown;
}

export interface StderrEvent {
  kind: "stderr";
  text?: string;
  line?: string;
  raw?: unknown;
}

export interface RawEvent {
  kind: "raw";
  raw?: unknown;
}

export interface ExitedEvent {
  kind: "exited";
  code: number | null;
}

export type SessionEvent =
  | RuntimeInitEvent
  | TextDeltaEvent
  | AssistantMessageEvent
  | ToolUseEvent
  | ToolResultEvent
  | UserReplayEvent
  | PermissionAskedEvent
  | PermissionSettledEvent
  | TurnEndedEvent
  | StatusEvent
  | StderrEvent
  | RawEvent
  | ExitedEvent;

const KNOWN_KINDS = new Set<string>([
  "runtime_init",
  "text_delta",
  "assistant_message",
  "tool_use",
  "tool_result",
  "user_replay",
  "permission_asked",
  "permission_settled",
  "turn_ended",
  "status",
  "stderr",
  "raw",
  "exited",
]);

/**
 * Parse one WS text frame into a SessionEvent. Unknown kinds are folded into
 * a `raw` event (forward compatibility); unparseable frames yield null.
 */
export function parseSessionEvent(data: unknown): SessionEvent | null {
  if (typeof data !== "string") return null;
  let value: unknown;
  try {
    value = JSON.parse(data);
  } catch {
    return null;
  }
  if (value === null || typeof value !== "object") return null;
  const kind = (value as { kind?: unknown }).kind;
  if (typeof kind !== "string") return null;
  if (!KNOWN_KINDS.has(kind)) return { kind: "raw", raw: value };
  return value as SessionEvent;
}
