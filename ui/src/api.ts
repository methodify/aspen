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
  service_state?: "ready" | "draining" | "updating" | "evacuating";
  service_detail?: string | null;
  started_at?: number;
  /** The harnesses this node can run sessions on (HARNESSES.md). */
  harnesses?: HarnessInfo[];
}

export type Harness = "claude" | "codex";

/** What a harness can do; every flag gates a control (HARNESSES.md §1). */
export interface HarnessCapabilities {
  streaming: boolean;
  interrupt: boolean;
  mid_turn_inject: boolean;
  permission_callback: boolean;
  in_process_mcp: boolean;
  resume: boolean;
  fork?: boolean;
  set_model?: boolean;
  set_mode?: boolean;
  context_usage?: boolean;
  reload?: boolean;
  slash_commands?: boolean;
  skill_mentions?: boolean;
  subagents?: boolean;
  plugin_dirs?: boolean;
  replay_ack?: boolean;
  question_prompts?: boolean;
  always_allow?: boolean;
  transcript_on_disk?: boolean;
  cost_from_harness?: boolean;
}

export interface HarnessInfo {
  name: Harness;
  binary: string | null;
  version: string | null;
  capabilities: HarnessCapabilities;
  modes: { id: string; label: string; hint?: string; posture?: Posture | null }[];
}

/** Aspen's operator-facing posture, one vocabulary across harnesses. */
export type Posture = "ask" | "edits" | "plan" | "auto" | "guarded";
export type ToolKind = "shell" | "file_write" | "file_edit" | "file_read" | "search" | "web" | "mcp" | "agent" | "question" | "other";
export type PromptKind = "permission" | "question" | "elicitation";

/** One answer a prompt accepts, as the harness offers it; `id` goes back verbatim. */
export interface DecisionOption {
  id: string;
  label: string;
  allow: boolean;
  scope?: "once" | "session" | "always";
  payload?: unknown;
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
  /** Running-activity counts (live sessions only). */
  activities?: ActivityCounts | null;
  /** Plugins the running process started with, and newer cached versions (PROPOSALS §7). */
  plugins?: ActivePlugin[];
  plugin_updates?: PluginUpdate[];
  /** Set when the session was moved elsewhere: its new full address. */
  moved_to?: string | null;
  /** Which runtime the session runs on (HARNESSES.md). */
  harness?: Harness;
  /** What that runtime can do — present while live. */
  capabilities?: HarnessCapabilities | null;
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
  /** How this process started, when the operator should know (e.g. it
   *  was forked because the session was live elsewhere). */
  spawn_note?: string | null;
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
  /** For a remote agent: the mesh its node was reached through (MESHES.md). */
  mesh?: string | null;
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
/** Activity (PROPOSALS §8): background work beside the main turn. */
export interface Activity {
  id: string;
  kind: "task" | "agent" | "workflow" | "monitor" | string;
  label: string;
  tool_use_id: string;
  started_at: string | null;
  ended_at: string | null;
  status: "running" | "completed" | "stopped" | "failed" | "done" | "unknown" | string;
  detail: Record<string, unknown>;
  has_transcript?: boolean;
}
/** One running item from `GET /api/activities` (PROPOSALS-B §2): a
 *  ledger entry tagged with its agent's full address and node. */
export interface FleetActivityItem {
  id: string;
  kind: string;
  label: string;
  tool_use_id: string;
  started_at: string | null;
  ended_at: string | null;
  status: string;
  detail: Record<string, unknown>;
  has_transcript?: boolean;
  agent: string;
  node: string | null;
}
export interface ActivityCounts {
  running: number;
  agents: number;
  tasks: number;
  workflows: number;
  monitors: number;
}

/** Plugins (PROPOSALS §7). */
export type MarketSource = { source: "github"; repo: string } | { source: "git"; url: string } | { source: "directory"; path: string };
export interface Marketplace {
  name: string;
  source: MarketSource;
  added_at: number;
  updated_at: number;
}
export interface PluginRule {
  id: string;
  marketplace: string;
  plugin: string;
  scope_kind: "mesh" | "node" | "repo" | "session" | string;
  scope: string;
  enabled: boolean;
  pin?: string | null;
  updated_at: number;
  deleted?: boolean;
}
export interface CatalogPlugin {
  marketplace: string;
  name: string;
  description?: string | null;
  category?: string | null;
  version?: string | null;
  current: string;
  source: unknown;
  cached: string[];
}
export interface PluginCatalog {
  synced_at: Record<string, number>;
  errors: Record<string, string>;
  plugins: CatalogPlugin[];
}
export interface PluginRegistry {
  marketplaces: Marketplace[];
  rules: PluginRule[];
  catalog: PluginCatalog;
  node: string | null;
}
export interface ActivePlugin {
  marketplace: string;
  plugin: string;
  version: string;
  path: string;
  via: string;
}
export interface PluginUpdate {
  plugin: string;
  marketplace: string;
  running: string;
  available: string;
}

/** Boards (PROPOSALS §6): a split tree of panes. */
export interface BoardPane {
  kind: "session" | "view" | "empty";
  id: string;
  agent?: string;
  path?: string;
}
export interface BoardSplit {
  kind: "split";
  id: string;
  dir: "row" | "col";
  sizes: number[];
  children: BoardNode[];
}
export type BoardNode = BoardSplit | BoardPane;
export interface Board {
  id: string;
  name: string;
  layout: BoardNode;
  /** Pair mode (BOARDS.md §pairs): pane-id pairs joined on a bus thread. */
  pairs?: [string, string][] | null;
  /** A dynamic board: panes are whatever matches. */
  query?: { node?: string; channel?: string; state?: "busy" | "live" | "attention" | "activity" | "any"; name?: string; style?: "grid" | "main" };
  updated_at: number;
  deleted?: boolean;
}

/** The target node's report after a move/copy (PROPOSALS §5). */
export interface MoveReport {
  name: string;
  node: string;
  repo: string;
  session_id: string;
  mode: string;
  files: number;
  bytes: number;
  memory_conflicts: string[];
  notes: string[];
  residue: number;
  revived: boolean;
  revive_note: string | null;
  rehomed: number;
  harness_version: string | null;
}

/** A pasted attachment on an outgoing message; `n` matches its marker. */
export interface OutgoingAttachment {
  n: number;
  name: string;
  media_type: string;
  /** base64 */
  data: string;
}

/** A file the session named in a tool call (artifacts.rs). */
export interface Artifact {
  path: string;
  kind: "wrote" | "edited" | "read" | string;
  at: string | null;
  tool: string;
}

/** Stat of a served file (GET …/file?stat=1). */
export interface FileStat {
  exists: boolean;
  path: string;
  is_dir?: boolean;
  size?: number;
  mtime?: number | null;
  media_type?: string;
  name?: string;
}

export interface HistoryToolChip {
  id: string;
  name: string;
  /** Capped by the node (REHYDRATE_TOOL_CAP); long strings end in a "truncated" note. */
  input?: unknown;
  result?: string | null;
  is_error?: boolean;
}

/** A pasted image in a user message, as stored in the transcript. */
export interface HistoryImage {
  media_type: string;
  data?: string;
  /** Present instead of `data` when the node left the image out for size. */
  omitted_bytes?: number;
}

/** REST `TranscriptItem`: rehydrated history from the runtime's on-disk transcript. */
export interface HistoryItem {
  role: "user" | "assistant";
  text: string;
  /** user items only: an [aspen bus] injection */
  bus?: boolean;
  /** assistant items only */
  tools?: HistoryToolChip[];
  /** user items only: pasted images */
  images?: HistoryImage[];
  uuid?: string;
  timestamp?: string;
}

export interface SessionInfo {
  session_id: string;
  title: string | null;
  /** Which runtime wrote it (HARNESSES.md). */
  harness?: Harness;
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
  /** The harness new sessions here run on unless told otherwise (null = claude). */
  default_harness?: Harness | null;
  /** While this node is in more than one mesh: which meshes see this repo. */
  meshes?: string[] | null;
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
  /** Start from this template (id or name); the other fields override it. */
  template?: string;
  charter?: string;
  model?: string;
  allow_all?: boolean;
  resume?: string;
  /** Run in bypassPermissions; omit to use the repo's stored default. */
  skip_permissions?: boolean;
  /** The operator reviewed the repo's autorun surface (trust gate). */
  acknowledge_trust?: boolean;
  /** When `resume` names a session being written elsewhere: the operator's
   *  answer. Absent → the node refuses (409) and the console asks. */
  resume_choice?: "fork" | "in_place";
  /** Display title for the new agent (e.g. an mcc session name). */
  title?: string;
  /** Per-session harness CLI args, appended after the harness defaults. */
  extra_args?: string;
  /** Which runtime; omit for the repo's default (then the node's). */
  harness?: Harness;
  /** Aspen's posture; the adapter maps it to its own mode. */
  posture?: Posture;
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
export interface NotifySettings {
  webhook?: string;
  command?: string;
  kinds?: string[];
}
export interface ReplicationSettings {
  to?: string | null;
  repos?: string[];
  accept?: boolean;
  keep_days?: number;
}
export interface Settings {
  harness?: Record<string, { args: string }>;
  update?: UpdatePolicy;
  notify?: NotifySettings;
  replication?: ReplicationSettings;
  memory?: MemorySettings;
}

/** A session template (PLUGINS.md §templates): a named recipe, mesh-wide. */
export interface TemplateSpec {
  name?: string;
  repo?: string;
  /** Which runtime; omit for the repo's default. */
  harness?: Harness;
  model?: string;
  permission?: "ask" | "skip";
  charter?: string;
  extra_args?: string;
  plugins?: { marketplace: string; plugin: string; pin?: string | null }[];
  board?: { id: string; mode: "fill" | "right" | "down" } | null;
  title?: string;
}
export interface Template {
  id: string;
  name: string;
  spec: TemplateSpec;
  updated_at: number;
  deleted?: boolean;
}
export interface TemplateSpawnResult {
  name: string;
  bare: string;
  node: string;
  template: string;
  board: { id: string; mode: "fill" | "right" | "down" } | null;
}

/** What a move would involve (MIGRATION.md preflight). */
export interface MovePreflight {
  source_up: boolean;
  replica?: ReplicaInfo | null;
  source?: {
    tiers: { A: number; B: number; C: number };
    files: number;
    harness: string | null;
    dirty: number;
    branch: string | null;
    busy: boolean;
    live: boolean;
  };
  target?: { counterpart: string | null; harness: string | null; state: string; accepting: boolean; error?: string };
  blockers: string[];
  warnings?: string[];
}

/** A replica of a peer's session held on this node (REPLICATION.md). */
export interface ReplicaInfo {
  node: string;
  agent: string;
  session_id: string;
  repo: string;
  title: string | null;
  bytes: number;
  files: number;
  as_of: number;
  held_on: string;
  has_transcript: boolean;
}

/** Notices (NOTIFICATIONS.md). */
export type NoticeKind = "turn_ended" | "question" | "permission" | "activity_settled" | "exited" | "inbox" | string;
export interface Notice {
  id: number;
  ts: number;
  node: string | null;
  agent: string;
  kind: NoticeKind;
  title: string;
  body: string | null;
  link: string | null;
}
/** Usage (USAGE.md). */
export interface ModelUsage {
  input: number;
  output: number;
  cache_read: number;
  cache_create: number;
  calls: number;
  cost_usd: number | null;
}
export interface SessionUsage {
  session_id: string;
  models: Record<string, ModelUsage>;
  total: ModelUsage;
  turns: number;
  cost_usd: number | null;
  subagents: number;
  subagent_tokens: number;
  first_ts: string | null;
  last_ts: string | null;
  lines_added: number | null;
  lines_removed: number | null;
}
export interface UsageRow {
  agent: string;
  node: string | null;
  repo: string;
  channel: string;
  title: string | null;
  session_id: string;
  live: boolean;
  usage: SessionUsage;
  window: { cost_usd: number; turns: number };
}
export interface NoticesPage {
  notices: Notice[];
  cursors: Record<string, number>;
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
  /** Which of the prompt's `decisions` this is. */
  decision_id?: string;
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
  prompt_kind?: PromptKind;
  tool_kind?: ToolKind;
  /** The bounded answers this prompt takes, in order; empty = allow/deny only. */
  decisions?: DecisionOption[];
  /** For question prompts: the harness's question payload. */
  questions?: unknown;
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

/** A memory conflict (MEMORY.md): both nodes edited a memory file. */
export interface MemoryConflict {
  node: string | null;
  repo: string;
  handle: string;
  rel: string;
  from: string;
  copy: string;
  at: number;
}
export interface Needs {
  prompts: OpenPrompt[];
  inbox: (BusMessage & { node: string | null })[];
  adoptions?: Adoption[];
  memory?: MemoryConflict[];
}
export interface MemorySettings {
  sync?: boolean;
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
  service_state?: "ready" | "draining" | "updating" | "evacuating" | null;
  service_detail?: string | null;
  policy?: string | null;
  inventory?: Inventory | null;
  last_outcome?: Partial<UpdateOutcome> | null;
}

export interface MeshPeer {
  node: string;
  /** Which mesh the peer is in (MESHES.md). */
  mesh?: string | null;
  url: string | null;
  /** That node's console, derived from its dial URL (a guess when headless). */
  console_url?: string | null;
  link_up: boolean;
  agents: number;
  fingerprint?: string;
  /** The peer holds the mesh's root key — certify happens there. */
  has_root?: boolean | null;
  /** "direct" or "relay:<url>" while linked. */
  link_kind?: string | null;
  /** Where the peer says it can be reached (empty = a spoke by choice). */
  advertised?: { dial_urls: string[]; relay_urls: string[] } | null;
  /** Every URL this node may dial for the peer, with reach memory:
   *  consecutive failures, seconds of backoff left, last success. */
  candidates?: DialCandidate[] | null;
  health?: PeerHealth;
}

export interface DialCandidate {
  url: string;
  fails: number;
  retry_in_secs: number;
  last_ok: number | null;
  last_error: string | null;
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

export interface MeshMembership {
  mesh: string;
  primary: boolean;
  policy: "full" | "observe" | string;
  peers: string[];
  relays: string[];
  root_here: boolean;
}
export interface MeshInfo {
  in_mesh: boolean;
  mesh?: string;
  /** Every mesh this node is in (MESHES.md), the primary first. */
  meshes?: MeshMembership[];
  multi_mesh?: boolean;
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
    /** What this node tells peers about reaching it; empty = spoke. */
    advertised?: { dial_urls: string[]; relay_urls: string[] };
  } | null;
  root_public?: string;
  peers?: MeshPeer[];
  relay?: {
    url: string | null;
    connected_at: number | null;
    /** Every configured relay with its client state. */
    relays?: {
      url: string;
      connected_at: number | null;
      last_error: string | null;
      last_error_at: number | null;
      /** Who else the relay says is present (the first thing to check when two nodes on one relay don't see each other). */
      present?: string[];
      /** The node hosting this relay, when it is a node. */
      host?: string | null;
    }[];
    /** Relays peers host, learned from their rosters (not configured). */
    discovered?: { url: string; from: string; connected_at: number | null }[];
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
  /** Which runtime, and what it can do (HARNESSES.md). */
  harness?: Harness;
  capabilities?: HarnessCapabilities;
  /** The harness's own modes, for the mode select. */
  modes?: { id: string; label: string; hint?: string; posture?: Posture | null }[];
  /** The neutral runtime info: model, mode, models, commands, skills. */
  runtime?: {
    model?: string | null;
    mode?: string | null;
    models?: unknown[];
    commands?: unknown[];
    skills?: { name?: string; description?: string | null; path?: string; scope?: string }[];
    context_window?: number | null;
  } | null;
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
  /** The parsed JSON body, when the server sent one (structured refusals
   *  like the trust gate and live-elsewhere carry their details here). */
  readonly body: Record<string, unknown> | null;

  constructor(status: number, message: string, body: Record<string, unknown> | null = null) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
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

/** Server clock minus browser clock, in seconds, from the `Date` header
 *  of every response. Elapsed-time displays use `serverNow()` so a browser
 *  whose clock drifts from the node (WSL vs Windows, a laptop vs a server)
 *  still shows "running 42s" rather than "0s". */
let clockSkew = 0;
export function serverNow(): number {
  return Date.now() / 1000 + clockSkew;
}
function noteServerDate(res: Response): void {
  const d = res.headers.get("date");
  if (!d) return;
  const t = Date.parse(d);
  if (Number.isFinite(t)) clockSkew = (t - Date.now()) / 1000;
}

/** The console-as-peer tunnel (tunnel.ts): when on, every request goes
 *  through the relay to the attached node instead of this origin. */
import { tunnel } from "./tunnel";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  if (tunnel.enabled) {
    let r: { status: number; body?: string; body_b64?: string };
    try {
      const h: Record<string, string> = {};
      new Headers(init?.headers ?? {}).forEach((v, k) => {
        h[k] = v;
      });
      r = await tunnel.http(init?.method ?? "GET", path, typeof init?.body === "string" ? init.body : undefined, h);
    } catch (e) {
      throw new ApiError(0, e instanceof Error ? e.message : "tunnel error");
    }
    const text = r.body ?? (r.body_b64 ? atob(r.body_b64) : "");
    if (r.status < 200 || r.status >= 300) {
      let detail = text.trim();
      let parsedBody: Record<string, unknown> | null = null;
      try {
        const parsed: unknown = JSON.parse(text);
        if (parsed && typeof parsed === "object") {
          parsedBody = parsed as Record<string, unknown>;
          const msg = parsedBody["error"] ?? parsedBody["message"];
          if (typeof msg === "string" && msg) detail = msg;
        }
      } catch {
        // plain text
      }
      throw new ApiError(r.status, detail || `${r.status}`, parsedBody);
    }
    if (!text) return {} as T;
    return JSON.parse(text) as T;
  }
  let res: Response;
  try {
    const token = nodeToken();
    const headers = new Headers(init?.headers ?? {});
    if (token) headers.set("X-Aspen-Token", token);
    res = await fetch(path, { ...init, headers });
    noteServerDate(res);
  } catch (e) {
    throw new ApiError(0, e instanceof Error ? e.message : "network error");
  }
  const text = await res.text();
  if (!res.ok) {
    let detail = text.trim();
    let parsedBody: Record<string, unknown> | null = null;
    // If the body is JSON with an error/message field, prefer that.
    try {
      const parsed: unknown = JSON.parse(text);
      if (parsed && typeof parsed === "object") {
        const obj = parsed as Record<string, unknown>;
        parsedBody = obj;
        const msg = obj["error"] ?? obj["message"];
        if (typeof msg === "string" && msg) detail = msg;
      }
    } catch {
      // plain-text body; keep as-is
    }
    throw new ApiError(res.status, detail || `${res.status} ${res.statusText}`, parsedBody);
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
  /** `choice` answers the live-elsewhere gate (409) the same way a start does. */
  revive: (name: string, choice?: "fork" | "in_place") =>
    post<Agent>(`/api/agents/${enc(name)}/revive`, choice ? { resume_choice: choice } : {}),
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

  /** `queued` (HTTP 202) when the agent's node is unreachable right now:
   *  the text went on the bus and delivers when the link returns. */
  sendMessage: (name: string, text: string, attachments?: OutgoingAttachment[]) =>
    post<{ uuid?: string; queued?: boolean; note?: string }>(`/api/agents/${enc(name)}/message`, {
      text,
      attachments: attachments && attachments.length ? attachments : undefined,
    }),
  interrupt: (name: string) =>
    post<Record<string, never>>(`/api/agents/${enc(name)}/interrupt`),
  answerPermission: (name: string, requestId: string, answer: PermissionAnswer) =>
    post<Record<string, never>>(
      `/api/agents/${enc(name)}/permission/${enc(requestId)}`,
      answer,
    ),
  transcript: (name: string) =>
    request<HistoryItem[]>(`/api/agents/${enc(name)}/transcript`),
  /** Items after the user line `after` (a delta), or everything when that
   *  line is no longer there (`after_found: false`). */
  transcriptAfter: (name: string, after: string) =>
    request<{ items: HistoryItem[]; after_found: boolean }>(`/api/agents/${enc(name)}/transcript?after=${enc(after)}`),
  artifacts: (name: string) => request<Artifact[]>(`/api/agents/${enc(name)}/artifacts`),
  activities: (name: string) => request<Activity[]>(`/api/agents/${enc(name)}/activities`),
  fleetActivities: () => request<FleetActivityItem[]>("/api/activities"),
  usage: (from: number) => request<UsageRow[]>(`/api/usage?from=${from}`),
  agentUsage: (name: string) => request<UsageRow[]>(`/api/agents/${enc(name)}/usage`),
  notices: (since: string) => request<NoticesPage>(`/api/notices?since=${encodeURIComponent(since)}`),
  subagent: (name: string, id: string) => request<HistoryItem[]>(`/api/agents/${enc(name)}/subagent/${enc(id)}`),
  plugins: () => request<PluginRegistry>("/api/plugins"),
  pluginsSync: (marketplace?: string) =>
    request<PluginCatalog>(`/api/plugins/sync${marketplace ? `?marketplace=${enc(marketplace)}` : ""}`, { method: "POST" }),
  putMarketplace: (name: string, source: MarketSource) =>
    request<{ ok: boolean }>(`/api/plugins/marketplaces/${enc(name)}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ source }) }),
  deleteMarketplace: (name: string) => request<{ ok: boolean }>(`/api/plugins/marketplaces/${enc(name)}`, { method: "DELETE" }),
  putPluginRule: (r: PluginRule) =>
    request<{ ok: boolean }>(`/api/plugins/rules/${enc(r.id)}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(r) }),
  deletePluginRule: (id: string) => request<{ ok: boolean }>(`/api/plugins/rules/${enc(id)}`, { method: "DELETE" }),
  pluginsEffective: (agent: string) =>
    request<{ would_start_with: ActivePlugin[]; missing: string[]; running: ActivePlugin[]; updates: PluginUpdate[] }>(`/api/plugins/effective?agent=${enc(agent)}`),
  boards: () => request<Board[]>("/api/boards"),
  putBoard: async (b: Board) => {
    const r = await request<{ ok: boolean }>(`/api/boards/${enc(b.id)}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
    window.dispatchEvent(new Event("aspen:boards"));
    return r;
  },
  deleteBoard: async (id: string) => {
    const r = await request<{ ok: boolean }>(`/api/boards/${enc(id)}`, { method: "DELETE" });
    window.dispatchEvent(new Event("aspen:boards"));
    return r;
  },
  /** Move (or copy) a session to another node; resolves the target's report. */
  replicas: () => request<ReplicaInfo[]>("/api/replicas"),
  exposeRepo: (path: string, meshes: string[]) => post<{ ok: boolean }>("/api/repos/expose", { path, meshes }),
  templates: () => request<Template[]>("/api/templates"),
  putTemplate: (id: string, name: string, spec: TemplateSpec) =>
    request<{ ok: boolean; template: Template }>(`/api/templates/${enc(id)}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ name, spec }) }),
  deleteTemplate: (id: string) => request<{ ok: boolean }>(`/api/templates/${enc(id)}`, { method: "DELETE" }),
  templateSpawn: (id: string, body: { node?: string; name?: string; repo?: string; charter?: string; model?: string; extra_args?: string; skip_permissions?: boolean; title?: string; acknowledge_trust?: boolean }) =>
    post<TemplateSpawnResult>(`/api/templates/${enc(id)}/spawn`, body),
  evacuate: (node: string, to: string) => post<unknown>(`/api/mesh/${enc(node)}/evacuate`, { to }),
  movePreflight: (name: string, to: string) => request<MovePreflight>(`/api/agents/${enc(name)}/move/preflight?to=${enc(to)}`),
  resolveMemory: (c: MemoryConflict, choice: "mine" | "theirs") =>
    post<{ ok: boolean }>("/api/memory/resolve", { node: c.node, repo: c.repo, rel: c.rel, copy: c.copy, choice }),
  agentReplica: (name: string) => request<{ replica: ReplicaInfo | null; home_up: boolean }>(`/api/agents/${enc(name)}/replica`),
  moveAgent: (name: string, body: { to: string; mode: "move" | "copy"; repo?: string; name?: string; apply_patch?: boolean; from_replica?: boolean }) =>
    post<MoveReport>(`/api/agents/${enc(name)}/move`, body),
  fileStat: (name: string, path: string) =>
    request<FileStat>(`/api/agents/${enc(name)}/file?path=${enc(path)}&stat=1`),
  /** The file's text (viewer); the request carries the token header. */
  fileText: async (name: string, path: string): Promise<string> => {
    const token = nodeToken();
    const headers = new Headers();
    if (token) headers.set("X-Aspen-Token", token);
    const res = await fetch(`/api/agents/${enc(name)}/file?path=${enc(path)}`, { headers });
    if (!res.ok) throw new ApiError(res.status, (await res.text()) || res.statusText);
    return res.text();
  },
  /** A URL for <img>/<iframe>/<a>: carries the token as a query param. */
  fileUrl: (name: string, path: string, opts?: { download?: boolean }): string => {
    const token = nodeToken();
    return `/api/agents/${enc(name)}/file?path=${enc(path)}${opts?.download ? "&download=1" : ""}${token ? `&token=${enc(token)}` : ""}`;
  },
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
  setRepoHarness: (path: string, harness: Harness | null, node?: string) =>
    post<{ ok: boolean }>("/api/repos/harness", { path, harness, ...(node ? { node } : {}) }),
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
/** A session's live event stream: a WebSocket to this origin, or a `sub`
 *  over the tunnel. Returns a closer. */
export function openSessionEvents(
  name: string,
  handlers: { onOpen: () => void; onMessage: (data: string) => void; onClose: () => void },
): () => void {
  if (tunnel.enabled) {
    let opened = false;
    const stop = tunnel.subscribe(
      name,
      (ev) => {
        if (!opened) {
          opened = true;
          handlers.onOpen();
        }
        handlers.onMessage(typeof ev === "string" ? ev : JSON.stringify(ev));
      },
      () => handlers.onClose(),
    );
    // The node answers the first event only when something happens;
    // report open once the tunnel itself is up.
    if (tunnel.state === "up") {
      opened = true;
      handlers.onOpen();
    }
    return stop;
  }
  const ws = new WebSocket(sessionEventsUrl(name));
  ws.onopen = () => handlers.onOpen();
  ws.onmessage = (e: MessageEvent) => handlers.onMessage(String(e.data));
  ws.onclose = () => handlers.onClose();
  ws.onerror = () => ws.close();
  return () => {
    ws.onclose = null;
    ws.close();
  };
}

export function sessionEventsUrl(name: string): string {
  const proto = window.location.protocol === "https:" ? "wss" : "ws";
  const token = nodeToken();
  return `${proto}://${window.location.host}/api/agents/${enc(name)}/events${token ? `?token=${enc(token)}` : ""}`;
}
