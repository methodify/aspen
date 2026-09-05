// Typed client for the aspen node REST API (docs/API.md). All paths are
// relative, so the same bundle works via the vite dev proxy and when served
// by the node itself.

export interface NodeInfo {
  node: string;
  version: string;
  /** Build stamp of the daemon serving the API. */
  sha?: string;
  built?: string;
  /** Where the daemon listens, and whether that is loopback only (then no
   *  other machine can dial this node). `hostname` is the OS hostname —
   *  the first guess for a dial URL other machines can use. */
  listen?: string;
  loopback_only?: boolean;
  hostname?: string;
  /** Servicing summary (GET /api/update has the rest). */
  update_available?: string | null;
  update_skipped?: boolean;
  withdrawn?: boolean;
  service_state?: "ready" | "draining" | "updating";
  service_detail?: string | null;
  started_at?: number;
}

/** The self-update policy (settings.update; docs/SERVICING.md §2). */
export interface UpdatePolicy {
  mode?: "notify" | "auto" | null;
  window?: string | null;
  soak?: string | null;
  skip?: string | null;
  check?: boolean | null;
}

export interface ReleaseInfo {
  version: string;
  tag: string;
  published_at: number | null;
  notes: string | null;
  assets: string[];
}

export type ServiceState =
  | { state: "ready" }
  | {
      state: "draining";
      since: number;
      by: string;
      when: "quiet" | "now";
      waiting_on: string[];
      overdue: boolean;
      target: string;
    }
  | { state: "updating"; since: number; by: string; target: string };

export interface UpdateOutcome {
  from: string;
  to: string;
  ok: boolean;
  rolled_back: boolean;
  error: string | null;
  trigger: string;
  started_at: number;
  finished_at: number;
}

export interface Inventory {
  os: string;
  arch: string;
  claude_version: string | null;
  started_at: number;
  pid: number;
}

export interface Rollout {
  target: string;
  when: string;
  order: string[];
  done: string[];
  current: string | null;
  failed: [string, string] | null;
  stopped: boolean;
  finished: boolean;
  started_at: number;
  finished_at: number | null;
}

/** GET /api/update — one node's full servicing status. */
export interface UpdateStatus {
  current: string;
  sha?: string | null;
  available: ReleaseInfo | null;
  latest: string | null;
  behind: boolean;
  withdrawn: boolean;
  skipped: boolean;
  soaked: boolean | null;
  last_check: { at: number; ok: boolean; error: string | null } | null;
  state: ServiceState;
  policy: UpdatePolicy;
  policy_effective: { auto: boolean; in_window: boolean; quiet_secs: number };
  waiting_on: string[];
  inventory: Inventory;
  last_outcome: UpdateOutcome | null;
  rollout: Rollout | null;
}

export type TurnState = "idle" | "busy";

export interface Agent {
  /** The address: `bare@repo` locally, `bare@repo@node` for a remote
   *  agent. Route key and bus address alike. */
  name: string;
  /** The short name (`arch`); names are per repo. */
  bare?: string;
  /** Work summary while live (null when down or unreachable). */
  summary?: WorkSummary | null;
  /** The repo's git state (local agents). */
  git?: GitState | null;
  last_exit_code?: number | null;
  last_exit_at?: number | null;
  /** null for remote agents (their repo lives on another node). */
  repo: string | null;
  channel: string;
  session_id: string;
  charter: string | null;
  live: boolean;
  turn_state: TurnState | null;
  pending: number;
  /** Which mesh node hosts this agent. */
  node: string;
  /** true for agents hosted on another node (names like `west@beta`). */
  remote?: boolean;
  /** Operator-set display title (the @name stays the bus identity). */
  title?: string | null;
  /** Epoch seconds the current turn started, when busy. */
  busy_since?: number | null;
  /** The most recent tool this turn. */
  last_tool?: string | null;
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
  /** epoch seconds */
  modified: number;
  user_messages: number;
  /** Name from the repo's .mcc/sessions register, when present. */
  mcc_name: string | null;
  /** Args configured in mcc (permission flags already filtered out). */
  mcc_args: string | null;
  /** mcc had --dangerously-skip-permissions configured. */
  mcc_skip: boolean | null;
}

/** A tip left behind by branch/swap, or a manual bookmark. */
export interface Bookmark {
  id: number;
  session_id: string;
  message_uuid: string | null;
  label: string | null;
  reason: "branch" | "swap" | "manual" | string;
  created_at: number;
}

/** GET /api/agents/{name}/bookmarks */
export interface BookmarksInfo {
  head: string | null;
  /** Parent chain of the head, nearest first. */
  lineage: { session_id: string; fork_message: string | null }[];
  bookmarks: Bookmark[];
}

/** What an agent is doing — accumulated by the node since this process started. */
export interface WorkSummary {
  last_ask: string | null;
  last_ask_at: number | null;
  last_reply: string | null;
  turns: number;
  idle_since: number | null;
  busy_since: number | null;
  last_tool: string | null;
  cost_usd: number | null;
  context_tokens: number | null;
  context_window: number | null;
  files_touched: number;
  files: string[];
  tool_calls: number;
}

/** Repo git state (branch, dirty count, ahead/behind), refreshed by the node. */
export interface GitState {
  branch: string | null;
  dirty: number;
  ahead: number;
  behind: number;
  checked_at: number;
}

/** One entry in the fleet event log (GET /api/history). */
export interface FleetEvent {
  id: number;
  ts: number;
  agent: string;
  kind: "ask" | "turn" | "tool" | "prompt" | "exit" | "spawn" | "revive" | "branch" | string;
  detail: Record<string, unknown> | null;
  node: string;
}

export interface History {
  from: number;
  to: number;
  /** This node's name — `x@y@self` in peer-reported data is our local `x@y`. */
  self: string;
  events: FleetEvent[];
  messages: (BusMessage & { node?: string })[];
}

/** A declared pathway between two endpoints (GET /api/links). Endpoints:
 *  `agent:name@repo[@node]`, `repo:handle[@node]`, `node:name`, `operator`. */
export interface Link {
  id: number;
  src: string;
  dst: string;
  two_way: boolean;
  purpose: string | null;
  urgency: string | null;
  created_at: number;
}

/** A remembered repository (GET /api/repos). */
export interface Repo {
  path: string;
  /** Address segment + channel name; defaults to the basename, renamable. */
  handle?: string;
  git?: GitState | null;
  skip_permissions: boolean;
  /** Present on remote (node_repos) rows; local rows use live_agents. */
  live?: number;
  /** epoch seconds (local rows only) */
  last_used_at?: number;
  sessions: number;
  /** local rows only; remote rows carry `live` instead */
  live_agents?: number;
  trusted?: boolean;
  has_autorun?: boolean;
}

export interface StartAgentRequest {
  name: string;
  repo: string;
  charter?: string;
  model?: string;
  allow_all?: boolean;
  resume?: string;
  /** Run in bypassPermissions; omit to use the repo's stored default. */
  skip_permissions?: boolean;
  /** The operator reviewed the repo's autorun surface (trust gate). */
  acknowledge_trust?: boolean;
  /** Display title for the new agent (e.g. an mcc session name). */
  title?: string;
  /** Per-session harness CLI args, appended after the harness defaults. */
  extra_args?: string;
  /** Target node; omit or self name = local, a peer name spawns remotely. */
  node?: string;
}

/** A node's repos in the mesh-wide Library view (GET /api/mesh/repos). */
export interface MeshRepoNode {
  node: string;
  self: boolean;
  reachable: boolean;
  repos: Repo[];
}

/** One repo found by POST /api/repos/discover. */
export interface DiscoveredRepo {
  path: string;
  sessions: number;
  added: boolean;
}

/** Node settings (GET/PUT /api/settings). PUT merges top-level keys. */
export interface Settings {
  harness?: Record<string, { args: string }>;
  update?: UpdatePolicy;
}

export interface BusSendRequest {
  to: string;
  body: string;
  urgency?: Urgency;
  thread?: string;
  record?: string;
}

/** A skill or slash-command file found under a repo's `.claude/` tree. */
export interface SkillEntry {
  name: string;
  rel: string;
  kind: "skill" | "command";
  description: string | null;
}

export interface SkillSaveResult {
  ok: boolean;
  /** Live sessions in the repo that were reloaded (when `reload` was set). */
  reloaded_sessions: number;
}

export interface PermissionAnswer {
  allow: boolean;
  /** Shown to the model on deny. */
  message?: string;
  /** Replacement tool input on allow — for questions, {questions, answers, response?} per §7.6. */
  updated_input?: unknown;
  /** "Always allow": echo the prompt's `suggestions` verbatim. */
  updated_permissions?: unknown;
}

/** A channel the operator can address (GET /api/channels). */
export interface Channel {
  name: string;
  kind: "repo" | "custom";
  topic: string | null;
  /** member addresses (bare names, `name@node`, or `operator`) */
  members: string[];
  member_count: number | null;
}

/** One logical channel post — a fan-out collapsed to a single entry. */
export interface ChannelPost {
  post: string;
  sender: string;
  urgency: Urgency;
  body: string;
  thread: string | null;
  record: string | null;
  created_at: number;
  recipients: number;
  delivered: number;
  ingested: number;
}

/** A session in the mesh-wide activity snapshot. */
export interface ActivitySession {
  name: string;
  node: string;
  channel: string;
  repo: string | null;
  title?: string | null;
  live: boolean;
  turn_state: TurnState | null;
  busy_since?: number | null;
  last_tool?: string | null;
  pending: number;
  remote: boolean;
}

export interface Activity {
  sessions: ActivitySession[];
  trail: BusMessage[];
  inbox: number;
  /** Likely-waiting-on inference from the trail (honest heuristic). */
  waiting: WaitingEdge[];
}

/** One open permission prompt / question, mesh-wide (GET /api/needs). */
export interface OpenPrompt {
  /** `name` locally, `name@node` for remote agents — answer via that name. */
  agent: string;
  /** null = this node. */
  node: string | null;
  request_id: string;
  tool_name: string;
  input: unknown;
  /** PermissionUpdate[] from the CLI — echo verbatim as updated_permissions for "always allow". */
  suggestions: unknown;
  asked_at: number;
  is_question: boolean;
}

/** A session that happened to an agent outside Aspen (adoption.rs): a
 *  fork of its session, or its session driven from a terminal. */
export interface Adoption {
  id: number;
  repo: string;
  session_id: string;
  kind: "fork" | "resumed";
  /** The agent it relates to (`name@repo`, `@node` appended for peers). */
  of_agent: string | null;
  parent_session: string | null;
  fork_message: string | null;
  title: string | null;
  entrypoint: string | null;
  first_seen: number;
  resolved: "carry" | "split" | "ignore" | "revive" | null;
  resolved_at: number | null;
  resolved_as: string | null;
  node: string | null;
}

export interface Needs {
  prompts: OpenPrompt[];
  inbox: (BusMessage & { node: string | null })[];
  adoptions?: Adoption[];
}

export interface DmPair {
  a: string;
  b: string;
  last_at: number;
  messages: number;
}


export interface PeerHealth {
  last_error: string | null;
  last_error_at: number | null;
  last_up: number | null;
  last_down: number | null;
  last_roster: number | null;
  version: string | null;
  sha: string | null;
  fingerprint: string | null;
  /** Servicing, from the peer's roster. */
  update_available?: string | null;
  service_state?: "ready" | "draining" | "updating" | null;
  service_detail?: string | null;
  policy?: string | null;
  inventory?: Inventory | null;
  last_outcome?: Partial<UpdateOutcome> | null;
}

export interface MeshPeer {
  node: string;
  url: string | null;
  /** That node's console, derived from its dial URL (a guess when headless). */
  console_url?: string | null;
  link_up: boolean;
  agents: number;
  fingerprint?: string;
  /** The peer holds the mesh's root key — certify happens there. */
  has_root?: boolean | null;
  health?: PeerHealth;
}

/** A queued mesh change (the console authors; `aspen mesh apply` executes). */
export interface MeshProposal {
  id: string;
  kind: "enroll" | "certify" | "join" | "peers_add" | "relay" | "init" | "peers_remove" | "leave" | string;
  args: Record<string, unknown>;
  created_at: number;
  source: string;
}
export interface MeshOutcome {
  id: string;
  kind: string;
  ok: boolean;
  message: string;
  artifact?: string | null;
  applied_at: number;
}
export interface MeshPending {
  proposals: MeshProposal[];
  outcomes: MeshOutcome[];
}

export interface MeshInfo {
  in_mesh: boolean;
  mesh?: string;
  node: string;
  identity?: {
    node: string;
    fingerprint: string;
    certified: boolean;
    cert_blob?: string | null;
    enroll_blob?: string | null;
    version?: string;
    sha?: string;
    has_root?: boolean;
    root_key_path?: string | null;
  } | null;
  root_public?: string;
  peers?: MeshPeer[];
  relay?: {
    url: string | null;
    connected_at: number | null;
    /** Every configured relay with its client state. */
    relays?: { url: string; connected_at: number | null; last_error: string | null; last_error_at: number | null }[];
    /** Bus rows handed to a relay mailbox, awaiting the peer's ack. */
    mailed?: number;
    hosted_path?: string;
    hosted_present?: string[];
    /** Mail waiting in this node's own relay: recipient → items. */
    hosted_waiting?: Record<string, number>;
  };
  pending?: MeshPending;
}

/** POST /api/mesh/inspect — what a pasted blob is and would do here. */
export interface BlobInfo {
  kind: "enroll" | "bundle" | "cert";
  node: string;
  mesh?: string;
  fingerprint: string;
  certifier?: string;
  certifier_fingerprint?: string;
  certifier_url?: string | null;
  relay?: string | null;
  warnings: string[];
  next: string;
}

/** { handshake, inventory } — the runtime's own view of a session. */
export interface RuntimeInfo {
  handshake: {
    commands?: { name: string; description?: string; argumentHint?: string }[];
    models?: unknown[];
    output_style?: string;
    [k: string]: unknown;
  } | null;
  inventory: Record<string, unknown> | null;
}

export interface WaitingEdge {
  agent: string;
  on: string;
  since: number;
  snippet: string;
}

/** The repo's autorun surface, shown by the trust gate before first spawn. */
export interface RepoAutorun {
  hooks: string[];
  mcp_servers: string[];
  skills: string[];
  plugins: string[];
  has_autorun: boolean;
}

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/** The node token. A daemon listening beyond loopback requires it on every
 *  API call; the console URL carries it once (`?token=…`), we keep it for
 *  the session (per origin) and send it as a header from then on. Loopback
 *  daemons need none, and a stale token is simply ignored by them. */
const TOKEN_KEY = "aspen.token";
export function nodeToken(): string | null {
  try {
    const fromUrl = new URLSearchParams(window.location.search).get("token");
    if (fromUrl) {
      sessionStorage.setItem(TOKEN_KEY, fromUrl);
      localStorage.setItem(TOKEN_KEY, fromUrl);
      return fromUrl;
    }
    return sessionStorage.getItem(TOKEN_KEY) ?? localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    const token = nodeToken();
    const headers = new Headers(init?.headers ?? {});
    if (token) headers.set("X-Aspen-Token", token);
    res = await fetch(path, { ...init, headers });
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
  startAgent: (req: StartAgentRequest) => post<Agent>("/api/agents", req),
  deleteAgent: (name: string) =>
    request<Record<string, never>>(`/api/agents/${enc(name)}`, { method: "DELETE" }),
  revive: (name: string) => post<Agent>(`/api/agents/${enc(name)}/revive`),
  /** Branch here: bookmark the current tip, fork, move the head (carry) —
   *  or, with `as`, start the fork as a NEW agent and keep this one (split).
   *  Returns the agent that continues on the fork. */
  branch: (name: string, label?: string, at?: string, as?: string) =>
    post<Agent>(`/api/agents/${enc(name)}/branch`, {
      ...(label ? { label } : {}),
      ...(at ? { at } : {}),
      ...(as ? { as } : {}),
    }),
  bookmarks: (name: string) => request<BookmarksInfo>(`/api/agents/${enc(name)}/bookmarks`),
  resumeBookmark: (name: string, id: number, as?: string) =>
    post<Agent>(`/api/agents/${enc(name)}/bookmarks/${id}/resume`, as ? { as } : {}),
  adoptions: () => request<Adoption[]>("/api/adoptions"),
  resolveAdoption: (id: number, action: Adoption["resolved"] & string, name?: string, node?: string | null) =>
    post<{ ok: boolean; agent?: string }>(`/api/adoptions/${id}`, {
      action,
      ...(name ? { name } : {}),
      ...(node ? { node } : {}),
    }),
  deleteBookmark: (name: string, id: number) =>
    request<{ ok: boolean }>(`/api/agents/${enc(name)}/bookmarks/${id}`, { method: "DELETE" }),

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
  reloadAgent: (name: string) =>
    post<Record<string, unknown>>(`/api/agents/${enc(name)}/reload`),
  sessions: (repo: string, node?: string) =>
    request<SessionInfo[]>(
      `/api/sessions?repo=${enc(repo)}${node ? `&node=${enc(node)}` : ""}`,
    ),
  discoverRepos: (node?: string) =>
    post<{ found: DiscoveredRepo[] }>("/api/repos/discover", node ? { node } : {}),
  meshRepos: () => request<{ nodes: MeshRepoNode[] }>("/api/mesh/repos"),
  history: (from: number, to: number, agent?: string) =>
    request<History>(
      `/api/history?from=${from}&to=${to}${agent ? `&agent=${enc(agent)}` : ""}`,
    ),
  links: () => request<Link[]>("/api/links"),
  addLink: (l: { from: string; to: string; two_way?: boolean; purpose?: string; urgency?: string }) =>
    post<{ ok: boolean; id: number }>("/api/links", l),
  deleteLink: (id: number) => request<{ ok: boolean }>(`/api/links/${id}`, { method: "DELETE" }),
  settings: () => request<Settings>("/api/settings"),
  saveSettings: (s: Settings) =>
    request<{ ok: boolean }>("/api/settings", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(s),
    }),

  repos: () => request<Repo[]>("/api/repos"),
  addRepo: (path: string, skipPermissions?: boolean) =>
    post<Repo>("/api/repos", {
      path,
      ...(skipPermissions !== undefined ? { skip_permissions: skipPermissions } : {}),
    }),
  setRepoSkip: (path: string, skipPermissions: boolean, node?: string) =>
    post<{ ok: boolean }>("/api/repos/skip", {
      path,
      skip_permissions: skipPermissions,
      ...(node ? { node } : {}),
    }),
  renameRepo: (path: string, handle: string, node?: string) =>
    post<{ ok: boolean }>("/api/repos/rename", { path, handle, ...(node ? { node } : {}) }),
  forgetRepo: (path: string, node?: string) =>
    post<{ ok: boolean }>("/api/repos/forget", { path, ...(node ? { node } : {}) }),

  busLog: (
    n = 200,
    filters?: {
      sender?: string;
      recipient?: string;
      thread?: string;
      record?: string;
      urgency?: string;
      q?: string;
      /** only rows not yet delivered */
      pending?: boolean;
    },
  ) => {
    const qs = new URLSearchParams({ n: String(n) });
    for (const [k, v] of Object.entries(filters ?? {})) {
      if (v === true) qs.set(k, "true");
      else if (typeof v === "string" && v) qs.set(k, v);
    }
    return request<BusMessage[]>(`/api/bus/log?${qs.toString()}`);
  },
  postReceipts: (post: string) => request<BusMessage[]>(`/api/bus/post/${enc(post)}`),
  busSend: (req: BusSendRequest) => post<{ notes: string[] }>("/api/bus/send", req),

  repoSkills: (repo: string) =>
    request<SkillEntry[]>(`/api/repo/skills?repo=${enc(repo)}`),
  readSkill: (repo: string, rel: string) =>
    request<{ content: string }>(`/api/repo/skill?repo=${enc(repo)}&rel=${enc(rel)}`),
  writeSkill: (repo: string, rel: string, content: string, reload = true) =>
    request<SkillSaveResult>("/api/repo/skill", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ repo, rel, content, reload }),
    }),
  deleteSkill: (repo: string, rel: string) =>
    request<{ ok: boolean }>(`/api/repo/skill?repo=${enc(repo)}&rel=${enc(rel)}`, {
      method: "DELETE",
    }),

  inbox: () => request<BusMessage[]>("/api/operator/inbox"),
  markInboxRead: () => post<Record<string, never>>("/api/operator/inbox/read"),

  activity: () => request<Activity>("/api/activity"),

  channels: () => request<Channel[]>("/api/channels"),
  createChannel: (name: string, topic?: string, members?: string[]) =>
    post<{ ok: boolean; name: string }>("/api/channels", { name, topic, members }),
  deleteChannel: (name: string) =>
    request<{ ok: boolean }>(`/api/channels/${enc(name)}`, { method: "DELETE" }),
  channelLog: (name: string, n = 100) =>
    request<ChannelPost[]>(`/api/channels/${enc(name)}/log?n=${n}`),
  runtime: (name: string) => request<RuntimeInfo>(`/api/agents/${enc(name)}/runtime`),
  contextUsage: (name: string) =>
    request<Record<string, unknown>>(`/api/agents/${enc(name)}/context`),
  setModel: (name: string, model: string | null) =>
    post<Record<string, never>>(`/api/agents/${enc(name)}/model`, { model }),
  setMode: (name: string, mode: string) =>
    post<Record<string, never>>(`/api/agents/${enc(name)}/mode`, { mode }),
  setTitle: (name: string, title: string | null) =>
    post<Record<string, never>>(`/api/agents/${enc(name)}/title`, { title }),
  setCharter: (name: string, charter: string | null) =>
    post<Record<string, never>>(`/api/agents/${enc(name)}/charter`, { charter }),

  needs: () => request<Needs>("/api/needs"),
  markNeedsRead: () => post<Record<string, never>>("/api/needs/read"),

  dms: () => request<DmPair[]>("/api/dms"),
  dmLog: (a: string, b: string, n = 200) =>
    request<BusMessage[]>(`/api/dm?a=${enc(a)}&b=${enc(b)}&n=${n}`),

  repoAutorun: (repo: string) =>
    request<RepoAutorun>(`/api/repo/autorun?repo=${enc(repo)}`),

  mesh: () => request<MeshInfo>("/api/mesh"),

  // servicing (docs/SERVICING.md)
  update: (node?: string) => request<UpdateStatus>(`/api/update${node ? `?node=${enc(node)}` : ""}`),
  requestUpdate: (when: "quiet" | "now", node?: string) =>
    post<ServiceState>("/api/update", { when, ...(node ? { node } : {}) }),
  cancelUpdate: (node?: string) =>
    request<{ ok: boolean; cancelled: boolean }>("/api/update", {
      method: "DELETE",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(node ? { node } : {}),
    }),
  checkUpdate: (node?: string) =>
    post<{ ok: boolean; latest: string; behind: boolean }>("/api/update/check", node ? { node } : {}),
  /** Check the release channel on this node and every linked peer. */
  checkUpdatesAll: () =>
    post<{ ok: boolean; behind: number; results: Record<string, { ok: boolean; latest?: string; behind?: boolean; error?: string }> }>(
      "/api/update/check",
      { node: "*" },
    ),
  /** `node: "*"` sets the policy on this node and every linked peer. */
  setUpdatePolicy: (policy: UpdatePolicy, node?: string) =>
    request<{ ok: boolean; results?: Record<string, { ok: boolean; error?: string }> }>("/api/update/policy", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ policy, ...(node ? { node } : {}) }),
    }),
  updateFleet: (when: "quiet" | "now") => post<Rollout>("/api/update/fleet", { when }),
  stopFleet: () => request<{ ok: boolean; stopping: boolean }>("/api/update/fleet", { method: "DELETE" }),
  logs: (node?: string, lines = 200) =>
    request<{ lines: string[] }>(`/api/logs?lines=${lines}${node ? `&node=${enc(node)}` : ""}`),
  meshInspect: (blob: string) => post<BlobInfo>("/api/mesh/inspect", { blob }),
  meshPending: () => request<MeshPending>("/api/mesh/pending"),
  meshPropose: (kind: string, args: Record<string, unknown>) =>
    post<{ ok: boolean; proposal: MeshProposal; apply: string }>("/api/mesh/pending", { kind, args }),
  meshWithdraw: (id: string) => request<{ ok: boolean }>(`/api/mesh/pending/${enc(id)}`, { method: "DELETE" }),
  meshClearOutcomes: () => request<{ ok: boolean }>("/api/mesh/pending/outcomes", { method: "DELETE" }),
  trustRepo: (path: string) => post<{ ok: boolean }>("/api/repos/trust", { path }),
  untrustRepo: (path: string) => post<{ ok: boolean }>("/api/repos/untrust", { path }),

  addChannelMember: (name: string, member: string) =>
    post<{ ok: boolean }>(`/api/channels/${enc(name)}/members`, { member }),
  removeChannelMember: (name: string, member: string) =>
    post<{ ok: boolean }>(`/api/channels/${enc(name)}/members/remove`, { member }),
};

/** WebSocket URL for a session's event stream, honoring the page origin. */
export function sessionEventsUrl(name: string): string {
  const proto = window.location.protocol === "https:" ? "wss" : "ws";
  const token = nodeToken();
  return `${proto}://${window.location.host}/api/agents/${enc(name)}/events${token ? `?token=${enc(token)}` : ""}`;
}
