// Typed client for the aspen node REST API (docs/API.md). All paths are
// relative, so the same bundle works via the vite dev proxy and when served
// by the node itself.

export interface NodeInfo {
  node: string;
  version: string;
}

export type TurnState = "idle" | "busy";

export interface Agent {
  name: string;
  repo: string;
  channel: string;
  session_id: string;
  charter: string | null;
  live: boolean;
  turn_state: TurnState | null;
  pending: number;
}

export type Urgency = "gating" | "normal" | "notice";

export type DeliveredVia = "wake" | "boundary" | "interrupt" | "inbox" | "rode-along";

export interface BusMessage {
  id: number;
  sender: string;
  to_display: string;
  recipient: string;
  urgency: Urgency;
  body: string;
  thread: string | null;
  record: string | null;
  created_at: number;
  delivered_at: number | null;
  delivered_via: DeliveredVia | null;
  ingested_at: number | null;
}

/** A tool call chip on a rehydrated assistant item. */
export interface HistoryToolChip {
  id: string;
  name: string;
}

/** REST `TranscriptItem`: rehydrated history from the runtime's on-disk transcript. */
export interface HistoryItem {
  role: "user" | "assistant";
  text: string;
  /** user items only: an [aspen bus] injection */
  bus?: boolean;
  /** assistant items only */
  tools?: HistoryToolChip[];
  uuid?: string;
  timestamp?: string;
}

export interface SessionInfo {
  session_id: string;
  title: string | null;
  entrypoint: string | null;
  modified: number;
  user_messages: number;
}

export interface PlantAgentRequest {
  name: string;
  repo: string;
  charter?: string;
  model?: string;
  allow_all?: boolean;
  resume?: string;
}

export interface BusSendRequest {
  to: string;
  body: string;
  urgency?: Urgency;
  thread?: string;
  record?: string;
}

export interface PermissionAnswer {
  allow: boolean;
  /** Shown to the model on deny. */
  message?: string;
  /** Optional replacement tool input on allow. */
  updated_input?: unknown;
}

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, init);
  } catch (e) {
    throw new ApiError(0, e instanceof Error ? e.message : "network error");
  }
  const text = await res.text();
  if (!res.ok) {
    let detail = text.trim();
    // If the body is JSON with an error/message field, prefer that.
    try {
      const parsed: unknown = JSON.parse(text);
      if (parsed && typeof parsed === "object") {
        const obj = parsed as Record<string, unknown>;
        const msg = obj["error"] ?? obj["message"];
        if (typeof msg === "string" && msg) detail = msg;
      }
    } catch {
      // plain-text body; keep as-is
    }
    throw new ApiError(res.status, detail || `${res.status} ${res.statusText}`);
  }
  if (!text) return {} as T;
  return JSON.parse(text) as T;
}

function post<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

const enc = encodeURIComponent;

export const api = {
  node: () => request<NodeInfo>("/api/node"),

  agents: () => request<Agent[]>("/api/agents"),
  plantAgent: (req: PlantAgentRequest) => post<Agent>("/api/agents", req),
  deleteAgent: (name: string) =>
    request<Record<string, never>>(`/api/agents/${enc(name)}`, { method: "DELETE" }),

  sendMessage: (name: string, text: string) =>
    post<{ uuid: string }>(`/api/agents/${enc(name)}/message`, { text }),
  interrupt: (name: string) =>
    post<Record<string, never>>(`/api/agents/${enc(name)}/interrupt`),
  answerPermission: (name: string, requestId: string, answer: PermissionAnswer) =>
    post<Record<string, never>>(
      `/api/agents/${enc(name)}/permission/${enc(requestId)}`,
      answer,
    ),
  transcript: (name: string) =>
    request<HistoryItem[]>(`/api/agents/${enc(name)}/transcript`),
  sessions: (repo: string) => request<SessionInfo[]>(`/api/sessions?repo=${enc(repo)}`),

  busLog: (n = 200) => request<BusMessage[]>(`/api/bus/log?n=${n}`),
  busSend: (req: BusSendRequest) => post<{ notes: string[] }>("/api/bus/send", req),

  inbox: () => request<BusMessage[]>("/api/operator/inbox"),
  markInboxRead: () => post<Record<string, never>>("/api/operator/inbox/read"),
};

/** WebSocket URL for a session's event stream, honoring the page origin. */
export function sessionEventsUrl(name: string): string {
  const proto = window.location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${window.location.host}/api/agents/${enc(name)}/events`;
}
