import {
  memo,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  useCallback,
  type KeyboardEvent,
} from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { createPortal } from "react-dom";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  api,
  openSessionEvents,
  type BookmarksInfo,
  type PermissionAnswer,
  type RuntimeInfo,
  type OpenPrompt,
  type Artifact,
  type HistoryImage,
  type OutgoingAttachment,
  type Board,
  type BoardNode,
  type ActivePlugin,
  type PluginUpdate,
  type Activity,
  type UsageRow,
  type ReplicaInfo,
  type MovePreflight,
  serverNow,
  type ActivityCounts,
  type McpList,
  type ProcessInfo,
} from "./../api";
import { parseSessionEvent, type SessionEvent } from "./../events";
import {
  addLocalUserMessage,
  applyEvent,
  emptyTranscript,
  markLocalUserMessage,
  seedFromHistory,
  toolSummary,
  type AssistantBubbleItem,
  type BusBubbleItem,
  type PermissionCardItem,
  type ToolCardItem,
  type TranscriptItem,
  type TranscriptState,
  type TurnEndItem,
  type UserBubbleItem,
  type TaskNoticeItem,
  settleTools,
  addOpenPrompts,
  cachedTranscript,
  rememberTranscript,
  loadPersistedTranscript,
  persistTranscript,
  transcriptHead,
  mergeAfter,
} from "./../transcript";
import { useAppData } from "./../App";
import { Meter, presenceOf, relTime } from "./../components";
import { useHotkeys } from "./../hotkeys";
import { useLiveGate } from "./../trust";
import { ToolBody, resultHint } from "./../toolViews";
import { linkifyPaths } from "./../pathLinks";
import "./view.css";
import {
  buildQuestionUpdatedInput,
  filterSlashCommands,
  fmtTokens,
  hasSuggestions,
  loadRenderMode,
  normalizeModels,
  parseQuestions,
  slashCommandsOf,
  slashPartialOf,
  statusModeOf,
  statusNoteOf,
  storeRenderMode,
  summarizeContext,
  summarizeSuggestions,
  toolUseNameOf,
  type ContextSummary,
  type RenderMode,
  type SlashCommand,
} from "./sessionExtras";
import "./session.css";

// ---------------------------------------------------------------------------
// Transcript reducer (thin shell over the pure module)

/** Items rendered on open; the rest are one click away. */
const RECENT_WINDOW = 150;
const EARLIER_STEP = 300;

interface PendingAttachment {
  n: number;
  name: string;
  media_type: string;
  data: string;
  size: number;
  preview: string | null;
}

type Action =
  | { type: "seed"; state: TranscriptState }
  | { type: "event"; ev: SessionEvent }
  | { type: "local_send"; text: string; localKey: string; images?: HistoryImage[] }
  | { type: "sent"; localKey: string; uuid: string }
  | { type: "send_failed"; localKey: string }
  | { type: "settle_tools" }
  | { type: "open_prompts"; prompts: OpenPrompt[] };

function reducer(state: TranscriptState, action: Action): TranscriptState {
  switch (action.type) {
    case "seed":
      return action.state;
    case "event":
      return applyEvent(state, action.ev);
    case "local_send":
      return addLocalUserMessage(state, action.text, action.localKey, action.images);
    case "sent":
      return markLocalUserMessage(state, action.localKey, { uuid: action.uuid });
    case "send_failed":
      return markLocalUserMessage(state, action.localKey, { failed: true });
    case "settle_tools":
      return settleTools(state);
    case "open_prompts":
      return addOpenPrompts(state, action.prompts);
  }
}

type WsState = "connecting" | "open" | "reconnecting";

interface TurnInfo {
  subtype: string;
  costUsd: number | null;
  durationMs: number | null;
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const PERMISSION_MODES = [
  "default",
  "acceptEdits",
  "plan",
  "bypassPermissions",
  "dontAsk",
] as const;

// ---------------------------------------------------------------------------
// Item renderers

/** Internal links (the viewer) route in-app; everything else opens a tab. */
function MdLink({ href, children }: { href?: string; children?: React.ReactNode }) {
  if (href && href.startsWith("/view/")) {
    return (
      <Link to={href} className="path-link" title="open in the viewer">
        {children}
      </Link>
    );
  }
  return (
    <a href={href} target="_blank" rel="noreferrer">
      {children}
    </a>
  );
}

const Md = memo(function Md({ text, agent }: { text: string; agent?: string }) {
  const src = agent ? linkifyPaths(text, agent) : text;
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MdLink }}>
        {src}
      </ReactMarkdown>
    </div>
  );
});

/**
 * Markdown rendered the way a terminal renders it — the claude TUI look.
 * One monospace size throughout; structure carried by weight, color, and
 * character prefixes (• bullets, │ quotes, ─ rules), never by font size.
 */
const TuiMd = memo(function TuiMd({ text, agent }: { text: string; agent?: string }) {
  const src = agent ? linkifyPaths(text, agent) : text;
  return (
    <div className="tui-md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: MdLink,
          h1: ({ children }) => <div className="tui-h tui-h1">{children}</div>,
          h2: ({ children }) => <div className="tui-h tui-h2">{children}</div>,
          h3: ({ children }) => <div className="tui-h">{children}</div>,
          h4: ({ children }) => <div className="tui-h">{children}</div>,
          h5: ({ children }) => <div className="tui-h">{children}</div>,
          h6: ({ children }) => <div className="tui-h">{children}</div>,
          hr: () => <div className="tui-hr" aria-hidden />,
        }}
      >
        {src}
      </ReactMarkdown>
    </div>
  );
});

const AssistantBubble = memo(function AssistantBubble({ item, source, agent }: { item: AssistantBubbleItem; source?: boolean; agent?: string }) {
  return (
    <div className="bubble bubble-assistant">
      {item.thinking && (
        <details className="thinking">
          <summary>thinking</summary>
          <div className="thinking-body">{item.thinking}</div>
        </details>
      )}
      {item.text ? (
        source ? (
          <pre className="src-body">{item.text}</pre>
        ) : (
          <Md text={item.text} agent={agent} />
        )
      ) : (
        item.open && <span className="dim">…</span>
      )}
      {item.open && item.text && <span className="caret" aria-hidden="true" />}
    </div>
  );
});

const UserBubble = memo(function UserBubble({ item }: { item: UserBubbleItem }) {
  return (
    <div className="bubble bubble-user">
      <div className="bubble-tag mono">@operator</div>
      <div className="user-text">{item.text}</div>
      {item.images && item.images.length > 0 && (
        <div className="user-images">
          {item.images.map((im, i) =>
            im.data ? (
              <img key={i} src={`data:${im.media_type};base64,${im.data}`} alt={`attachment ${i + 1}`} />
            ) : (
              <span key={i} className="mono-meta">{`[image ${i + 1}: ${Math.round((im.omitted_bytes ?? 0) / 1024)} KB, not loaded]`}</span>
            ),
          )}
        </div>
      )}
      {item.pending && !item.failed && <div className="bubble-note dim">sending…</div>}
      {item.failed && <div className="bubble-note error-text">send failed — not delivered</div>}
    </div>
  );
});

/** A task notification, collapsed to one line (PROPOSALS-B §12): the
 *  harness reporting a background task, subagent or workflow back to the
 *  model. Click to read the full text. */
const NoticeCard = memo(function NoticeCard({ item, tui }: { item: TaskNoticeItem; tui?: boolean }) {
  const status = item.status ?? "done";
  const bad = status === "failed" || status === "error" || status === "killed";
  return (
    <details className={`tool-card notice-card${tui ? " tool-card-tui" : ""}`}>
      <summary>
        <span className={bad ? "dot dot-error" : "dot dot-idle"} />
        <span className="tool-name mono">task notification</span>
        {item.summary && <span className="tool-summary mono">{item.summary}</span>}
        <span className={`tool-hint mono-meta${bad ? " error-text" : ""}`}>
          {status}
          {item.taskId ? ` · ${item.taskId.slice(0, 12)}` : ""}
        </span>
      </summary>
      <div className="tool-detail">
        <pre className="src-body notice-body">{item.text}</pre>
      </div>
    </details>
  );
});

/** The status bar's cost figure, opening into the session's usage
 *  (USAGE.md): tokens by model, subagents folded in, the harness's own
 *  session total. */
function UsagePopover({ agent, liveCost }: { agent: string; liveCost: number | null }) {
  const [open, setOpen] = useState(false);
  const [row, setRow] = useState<UsageRow | null>(null);
  const [at, setAt] = useState({ top: 0, left: 0 });
  useEffect(() => {
    if (!open) return;
    api
      .agentUsage(agent)
      .then((rows) => setRow(rows[0] ?? null))
      .catch(() => setRow(null));
  }, [open, agent]);
  const cost = liveCost ?? row?.usage.cost_usd ?? (row && row.window.cost_usd > 0 ? row.window.cost_usd : null);
  const observedOnly = liveCost === null && row?.usage.cost_usd === null;
  const fmtT = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(0)}k` : String(n));
  return (
    <span className="artifacts-wrap">
      <button
        className="status-right mono usage-btn"
        title="session cost (the harness's figure) — click for tokens by model"
        onClick={(e) => {
          const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
          setAt({ top: r.top - 8, left: Math.max(8, r.right - 420) });
          setOpen((o) => !o);
        }}
      >
        {cost !== null ? `session $${cost.toFixed(2)}` : "session $—"}
      </button>
      {open &&
        createPortal(
          <div className="artifacts-menu usage-pop" style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width: 420, transform: "translateY(-100%)" }} role="menu" onMouseLeave={() => setOpen(false)}>
            {row === null ? (
              <div className="row dim">loading…</div>
            ) : (
              <>
                <div className="row">
                  <span className="label">Usage</span>
                  <span className="spacer" />
                  <span className="mono-meta">
                    {row.usage.turns} turns · {row.usage.total.calls} calls
                    {row.usage.subagents > 0 ? ` · ${row.usage.subagents} subagents (${fmtT(row.usage.subagent_tokens)} tokens)` : ""}
                  </span>
                </div>
                <table className="usage-mini">
                  <thead>
                    <tr>
                      <th>model</th>
                      <th>in</th>
                      <th>cache rd</th>
                      <th>cache wr</th>
                      <th>out</th>
                      <th>cost</th>
                    </tr>
                  </thead>
                  <tbody>
                    {Object.entries(row.usage.models).filter(([m, u]) => !m.startsWith("<") || u.output > 0).map(([m, u]) => (
                      <tr key={m}>
                        <td className="mono">{m.replace(/^claude-/, "")}</td>
                        <td className="mono num">{fmtT(u.input)}</td>
                        <td className="mono num">{fmtT(u.cache_read)}</td>
                        <td className="mono num">{fmtT(u.cache_create)}</td>
                        <td className="mono num">{fmtT(u.output)}</td>
                        <td className="mono num">{u.cost_usd !== null ? `$${u.cost_usd.toFixed(2)}` : "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <div className="row mono-meta">
                  {observedOnly ? `observed $${row.window.cost_usd.toFixed(2)} over ${row.window.turns} turns (the harness has not priced this session)` : `session total $${(row.usage.cost_usd ?? 0).toFixed(2)}`}
                  {row.usage.lines_added !== null ? ` · +${row.usage.lines_added}/−${row.usage.lines_removed ?? 0} lines` : ""}
                  {" · "}
                  <Link to="/usage" onClick={() => setOpen(false)}>
                    all sessions
                  </Link>
                </div>
              </>
            )}
          </div>,
          document.body,
        )}
    </span>
  );
}

const BusBubble = memo(function BusBubble({ item, source, agent }: { item: BusBubbleItem; source?: boolean; agent?: string }) {
  const nl = item.text.indexOf("\n");
  const header = nl >= 0 ? item.text.slice(0, nl) : item.text;
  const body = nl >= 0 ? item.text.slice(nl + 1) : "";
  return (
    <div className="bubble bubble-bus">
      <div className="bus-header mono">{header}</div>
      {body && (source ? <pre className="src-body">{body}</pre> : <Md text={body} agent={agent} />)}
    </div>
  );
});

/** A tool call. Open while it is the active call (the newest item of a
 *  busy turn) or when the operator pinned it open; a click toggles and
 *  pins. `now` ticks while the turn is busy so the running timer moves. */
const ToolCard = memo(function ToolCard({
  item,
  open,
  onToggle,
  now,
  tui,
  agent,
}: {
  item: ToolCardItem;
  open: boolean;
  onToggle: (open: boolean) => void;
  now: number;
  tui?: boolean;
  agent?: string;
}) {
  const expandable = item.input !== null || item.result !== null;
  const summary = toolSummary(item.input);
  if (!expandable) {
    // History without inputs (older nodes): a plain chip.
    return (
      <div className={tui ? "cline cline-tool" : "tool-chip"}>
        <span className="mono">{tui ? `[tool] ${item.name}` : item.name}</span>
      </div>
    );
  }
  const hint = resultHint(item);
  const elapsed =
    !item.done && item.startedAt !== null ? Math.max(0, Math.floor((now - item.startedAt) / 1000)) : null;
  return (
    <details
      className={tui ? "tool-card tool-card-tui" : "tool-card"}
      open={open}
      onToggle={(e) => {
        const next = (e.currentTarget as HTMLDetailsElement).open;
        if (next !== open) onToggle(next);
      }}
    >
      <summary>
        <span
          className={
            !item.done ? "dot dot-busy" : item.isError ? "dot dot-error" : "dot dot-idle"
          }
        />
        <span className="tool-name mono">{item.name}</span>
        {summary && <span className="tool-summary mono">{summary}</span>}
        {hint && <span className={`tool-hint mono-meta${item.isError ? " error-text" : ""}`}>{hint}</span>}
        {!item.done && (
          <span className="chip chip-busy">{elapsed === null ? "running" : `running ${elapsed}s`}</span>
        )}
      </summary>
      <div className="tool-detail">
        <ToolBody item={item} agent={agent} />
      </div>
    </details>
  );
});

/**
 * AskUserQuestion rendered as a question card (§7.6): options as buttons,
 * multiSelect toggles, an optional free-text note, and an explicit skip.
 * A question is a conversation, not an alarm — amber, not the gate red.
 */
function QuestionCard({
  item,
  onAnswer,
}: {
  item: PermissionCardItem;
  onAnswer: (answer: PermissionAnswer) => Promise<boolean>;
}) {
  const questions = useMemo(() => parseQuestions(item.input) ?? [], [item.input]);
  const [picks, setPicks] = useState<string[][]>(() => questions.map(() => []));
  const [note, setNote] = useState("");
  const [submitting, setSubmitting] = useState(false);

  if (item.settled) {
    return (
      <div className="perm-settled mono">
        <span className={item.outcome === "denied" ? "perm-denied" : "perm-allowed"}>
          {item.outcome === "allowed" ? "answered" : (item.outcome ?? "settled")}
        </span>{" "}
        question
      </div>
    );
  }

  function toggle(qi: number, label: string, multi: boolean) {
    setPicks((prev) => {
      const next = prev.slice();
      const cur = next[qi] ?? [];
      next[qi] = multi
        ? cur.includes(label)
          ? cur.filter((l) => l !== label)
          : [...cur, label]
        : cur.length === 1 && cur[0] === label
          ? []
          : [label];
      return next;
    });
  }

  const hasAny = picks.some((p) => p.length > 0) || note.trim() !== "";

  async function submit() {
    setSubmitting(true);
    const updated_input = buildQuestionUpdatedInput(item.input, questions, picks, note);
    const ok = await onAnswer({ allow: true, updated_input });
    if (!ok) setSubmitting(false);
  }

  async function skip() {
    // Explicit skip: a bare allow sends no answers (§7.6).
    setSubmitting(true);
    const ok = await onAnswer({ allow: true });
    if (!ok) setSubmitting(false);
  }

  return (
    <div className="q-card">
      <div className="q-head">
        <span className="chip chip-question">question</span>
        <span className="mono dim">@agent asks</span>
      </div>
      {questions.map((q, qi) => (
        <div className="q-block" key={qi}>
          {q.header && <div className="q-header-label">{q.header}</div>}
          <div className="q-question">{q.question}</div>
          <div className="q-opts" role="group" aria-label={q.question}>
            {q.options.map((o) => (
              <button
                key={o.label}
                type="button"
                className="q-opt"
                aria-pressed={(picks[qi] ?? []).includes(o.label)}
                title={o.description ?? undefined}
                disabled={submitting}
                onClick={() => toggle(qi, o.label, q.multiSelect)}
              >
                {o.label}
              </button>
            ))}
          </div>
          {q.multiSelect && <div className="q-multi-hint">select all that apply</div>}
        </div>
      ))}
      <div className="q-free">
        <input
          value={note}
          placeholder="optional note to the agent"
          disabled={submitting}
          onChange={(e) => setNote(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && hasAny) void submit();
          }}
        />
      </div>
      <div className="q-actions">
        <button className="btn-answer" disabled={submitting || !hasAny} onClick={() => void submit()}>
          answer
        </button>
        <button className="btn-skip" disabled={submitting} onClick={() => void skip()}>
          skip (no answer)
        </button>
      </div>
    </div>
  );
}

function PermissionCard({
  item,
  onAnswer,
  onAlways,
}: {
  item: PermissionCardItem;
  onAnswer: (allow: boolean, message?: string) => Promise<boolean>;
  onAlways: (() => Promise<boolean>) | null;
}) {
  const [denyOpen, setDenyOpen] = useState(false);
  const [denyMsg, setDenyMsg] = useState("");
  const [submitting, setSubmitting] = useState(false);

  if (item.settled) {
    return (
      <div className="perm-settled mono">
        <span className={item.outcome === "denied" ? "perm-denied" : "perm-allowed"}>
          {item.outcome ?? "settled"}
        </span>{" "}
        {item.toolName}
      </div>
    );
  }

  async function answer(allow: boolean) {
    setSubmitting(true);
    const ok = await onAnswer(allow, allow ? undefined : denyMsg.trim() || undefined);
    if (!ok) setSubmitting(false);
  }

  async function always() {
    if (!onAlways) return;
    setSubmitting(true);
    const ok = await onAlways();
    if (!ok) setSubmitting(false);
  }

  const grantScope = onAlways
    ? (summarizeSuggestions(item.suggestions) ?? "applies the CLI's suggested rule")
    : null;

  return (
    <div className="perm-card">
      <div className="perm-head">
        <span className="chip chip-perm">permission</span>
        <span className="mono perm-tool">{item.toolName}</span>
      </div>
      <pre className="perm-input">{JSON.stringify(item.input, null, 2)}</pre>
      <div className="perm-actions">
        <button className="btn-allow" disabled={submitting} onClick={() => void answer(true)}>
          Allow
        </button>
        {onAlways && (
          <button
            className="btn-always"
            disabled={submitting}
            title="allow now and apply the CLI's suggested rule for next time"
            onClick={() => void always()}
          >
            Always allow
          </button>
        )}
        {!denyOpen ? (
          <button className="btn-deny" disabled={submitting} onClick={() => setDenyOpen(true)}>
            Deny…
          </button>
        ) : (
          <>
            <input
              autoFocus
              className="deny-msg"
              placeholder="shown to the model"
              value={denyMsg}
              onChange={(e) => setDenyMsg(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void answer(false);
                if (e.key === "Escape") setDenyOpen(false);
              }}
            />
            <button className="btn-deny" disabled={submitting} onClick={() => void answer(false)}>
              Deny
            </button>
            <button className="btn-quiet" onClick={() => setDenyOpen(false)}>
              cancel
            </button>
          </>
        )}
        {grantScope && <div className="perm-grant-scope">always allow: {grantScope}</div>}
      </div>
    </div>
  );
}

const TurnEndMarker = memo(function TurnEndMarker({ item }: { item: TurnEndItem }) {
  return (
    <div className="turn-end mono">
      turn ended · {item.subtype}
      {item.durationMs !== null && <> · {(item.durationMs / 1000).toFixed(1)}s</>}
      {item.costUsd !== null && <> · session ${item.costUsd.toFixed(2)}</>}
    </div>
  );
});

// ---------------------------------------------------------------------------
// The page

function fmtElapsed(startIso: string | null, endIso: string | null): string {
  if (!startIso) return "";
  const a = Date.parse(startIso);
  const b = endIso ? Date.parse(endIso) : serverNow() * 1000;
  if (!Number.isFinite(a) || !Number.isFinite(b)) return "";
  const s = Math.max(0, Math.round((b - a) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** "activity ▾": the session's ledger (PROPOSALS §8) — background tasks,
 *  subagents, workflows, monitors — running first; agents open their own
 *  transcript. */
function ActivityMenu({ agent, counts, openSignal }: { agent: string; counts: ActivityCounts | null; openSignal?: number }) {
  const [open, setOpen] = useState(false);
  const [acts, setActs] = useState<Activity[] | null>(null);
  const [procs, setProcs] = useState<ProcessInfo[]>([]);
  const [detail, setDetail] = useState<string | null>(null);
  const [stopping, setStopping] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [at, setAt] = useState({ top: 0, left: 0 });
  const [, setTick] = useState(0);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  // The status line's count opens this menu (PROPOSALS-MCP.md §6.4).
  useEffect(() => {
    if (!openSignal) return;
    const r = btnRef.current?.getBoundingClientRect();
    if (r) setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 580)) });
    setOpen(true);
  }, [openSignal]);
  useEffect(() => {
    if (!open) return;
    const load = () => {
      api.activities(agent).then(setActs).catch(() => setActs([]));
      api.processes(agent).then((p) => setProcs(p.processes)).catch(() => setProcs([]));
    };
    void load();
    const t = window.setInterval(() => {
      void load();
      setTick((n) => n + 1);
    }, 3000);
    return () => window.clearInterval(t);
  }, [open, agent]);
  const running = counts?.running ?? 0;
  const label = counts
    ? [counts.agents ? `${counts.agents} agent${counts.agents === 1 ? "" : "s"}` : "", counts.tasks ? `${counts.tasks} task${counts.tasks === 1 ? "" : "s"}` : "", counts.workflows ? `${counts.workflows} wf` : "", counts.monitors ? `${counts.monitors} mon` : ""].filter(Boolean).join(" · ")
    : "";
  const sorted = (acts ?? []).slice().sort((x, y) => Number(y.status === "running") - Number(x.status === "running"));
  // Child processes no ledger row explains: what hooks and plugins started.
  const orphans = procs.filter((p) => !p.activity);
  async function stop(what: { activity?: string; pid?: number }, key: string) {
    setStopping(key);
    setNote(null);
    try {
      const r = await api.processStop(agent, what);
      setNote(`stopped pid ${r.pid}`);
      api.activities(agent).then(setActs).catch(() => undefined);
      api.processes(agent).then((p) => setProcs(p.processes)).catch(() => undefined);
    } catch (e) {
      setNote(e instanceof Error ? e.message : "stop failed");
    } finally {
      setStopping(null);
    }
  }
  return (
    <span className="artifacts-wrap">
      <button
        ref={btnRef}
        className={`charter-toggle${running ? " activity-live" : ""}`}
        onClick={(e) => {
          const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
          setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 580)) });
          setOpen((o) => !o);
        }}
        title="background tasks, subagents, workflows and monitors of this session"
      >
        activity{running ? ` ${label}` : ""} ▾
      </button>
      {open &&
        createPortal(
          <div className="artifacts-menu activity-menu" style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width: 560 }} role="menu" onMouseLeave={() => setOpen(false)}>
            {acts === null ? (
              <div className="row dim">loading…</div>
            ) : sorted.length === 0 ? (
              <div className="row dim">nothing beside the main turn — no background tasks, subagents, workflows or monitors yet</div>
            ) : (
              sorted.map((a) => {
                const key = `${a.kind}:${a.id}:${a.tool_use_id}`;
                const script = typeof a.detail["command"] === "string" ? String(a.detail["command"]) : null;
                const output = typeof a.detail["output"] === "string" ? String(a.detail["output"]) : typeof a.detail["summary"] === "string" ? String(a.detail["summary"]) : null;
                const isOpen = detail === key;
                return (
                  <div className={`act-block${isOpen ? " open" : ""}`} key={key}>
                    <div className={`row act-row act-${a.status}`} onClick={() => setDetail(isOpen ? null : key)} style={{ cursor: "pointer" }} title="click for details">
                      <span className={`chip mono act-kind act-${a.kind}`}>{a.kind}</span>
                      <span className="act-body">
                        <span className="act-label">
                          {a.kind === "agent" && a.has_transcript ? (
                            <Link to={`/session/${encodeURIComponent(agent)}/agent/${encodeURIComponent(a.id)}`} onClick={() => setOpen(false)} title="open this agent's transcript">
                              {a.label}
                            </Link>
                          ) : (
                            a.label
                          )}
                        </span>
                        <span className="mono-meta act-detail">
                          {(a.kind === "task" || a.kind === "monitor") && script ? `$ ${script.slice(0, 120)}` : ""}
                          {a.kind === "agent" && typeof a.detail["agent_type"] === "string" ? `${a.detail["agent_type"]} · ` : ""}
                          {a.kind === "agent" && typeof a.detail["prompt"] === "string" ? String(a.detail["prompt"]).slice(0, 120) : ""}
                          {a.kind === "monitor" && !script && typeof a.detail["delay_seconds"] === "number" ? `in ${a.detail["delay_seconds"]}s` : ""}
                          {typeof a.detail["summary"] === "string" ? ` — ${a.detail["summary"]}` : ""}
                        </span>
                      </span>
                      <span className={`mono-meta act-status ${a.status === "running" ? "live" : ""}`}>
                        {a.status === "running" ? `running ${fmtElapsed(a.started_at, null)}` : `${a.status}${a.ended_at ? ` · ${fmtElapsed(a.started_at, a.ended_at)}` : ""}`}
                      </span>
                    </div>
                    {isOpen && (
                      <div className="act-details">
                        <div className="act-kv"><span className="k">status</span><span className={`v mono ${a.status === "running" ? "live" : ""}`}>{a.status}{a.detail["stopped_by"] === "operator" ? " (by you)" : ""}</span></div>
                        <div className="act-kv"><span className="k">runtime</span><span className="v mono">{a.status === "running" ? fmtElapsed(a.started_at, null) : fmtElapsed(a.started_at, a.ended_at)}</span></div>
                        {script && <div className="act-kv"><span className="k">script</span><pre className="v mono act-script">{script}</pre></div>}
                        {typeof a.detail["description"] === "string" && <div className="act-kv"><span className="k">purpose</span><span className="v">{String(a.detail["description"])}</span></div>}
                        <div className="act-kv"><span className="k">output</span>{output ? <pre className="v mono act-output">{output}</pre> : <span className="v dim">no output yet</span>}</div>
                        {a.status === "running" && script && (
                          <div className="act-actions">
                            <button className="btn danger sm" disabled={stopping === key} onClick={(e) => { e.stopPropagation(); void stop({ activity: a.id }, key); }} title="terminate the process running this script">
                              {stopping === key ? "stopping…" : "stop"}
                            </button>
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                );
              })
            )}
            {orphans.length > 0 && (
              <>
                <div className="row"><span className="label">processes under this session</span><span className="mono-meta">not from a tool call — a hook or a plugin started them</span></div>
                {orphans.map((p) => (
                  <div className="row act-row" key={p.pid}>
                    <span className="chip mono act-kind">pid {p.pid}</span>
                    <span className="act-body"><span className="mono-meta act-detail" title={p.cmdline}>{p.cmdline.slice(0, 140)}</span></span>
                    <span className="mono-meta">{typeof p.age_secs === "number" ? `${Math.floor(p.age_secs / 60)}m` : ""}</span>
                    <button className="btn ghost sm" disabled={stopping === `pid:${p.pid}`} onClick={() => void stop({ pid: p.pid }, `pid:${p.pid}`)} title="terminate this process">stop</button>
                  </div>
                ))}
              </>
            )}
            {note && <div className="row mono-meta">{note}</div>}
          </div>,
          document.body,
        )}
    </span>
  );
}

/** "mcp ▾" (PROPOSALS-MCP.md §3.3): the session's MCP servers as the
 *  harness sees them — status, tools, error, and the controls the harness
 *  offers. Refresh asks the harness now, so what the operator sees is the
 *  current picture, not the last turn boundary's. */
function McpMenu({ agent, summary, openSignal }: { agent: string; summary: { total: number; failed: number; needs_auth: number } | null | undefined; openSignal?: number }) {
  const [open, setOpen] = useState(false);
  const [list, setList] = useState<McpList | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [rowErr, setRowErr] = useState<Record<string, string>>({});
  const [adding, setAdding] = useState(false);
  const [addName, setAddName] = useState("");
  const [addKind, setAddKind] = useState<"stdio" | "http">("stdio");
  const [addTarget, setAddTarget] = useState("");
  const [at, setAt] = useState({ top: 0, left: 0 });
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const load = useCallback((refresh: boolean) => {
    setBusy(refresh ? "refresh" : null);
    api.mcp(agent, refresh)
      .then((l) => { setList(l); setErr(null); })
      .catch((e) => setErr(e instanceof Error ? e.message : "could not read MCP status"))
      .finally(() => setBusy(null));
  }, [agent]);
  useEffect(() => {
    if (!openSignal) return;
    const r = btnRef.current?.getBoundingClientRect();
    if (r) setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 580)) });
    setOpen(true);
    load(false);
  }, [openSignal, load]);
  const down = (summary?.failed ?? 0) + (summary?.needs_auth ?? 0);
  async function act(server: string, what: "reconnect" | "enable" | "disable" | "auth") {
    setBusy(`${what}:${server}`);
    setRowErr((m) => ({ ...m, [server]: "" }));
    try {
      if (what === "reconnect") {
        const r = await api.mcpReconnect(agent, server);
        if (!r.ok) setRowErr((m) => ({ ...m, [server]: r.error ?? "reconnect failed" }));
        setList((l) => (l ? { ...l, servers: r.servers, refreshed_at: Date.now() / 1000 } : l));
      } else if (what === "auth") {
        const r = await api.mcpAuth(agent, server);
        if (r.url) window.open(r.url, "_blank", "noopener");
        else setRowErr((m) => ({ ...m, [server]: "no authentication URL was offered" }));
      } else {
        const r = await api.mcpToggle(agent, server, what === "enable");
        setList((l) => (l ? { ...l, servers: r.servers, refreshed_at: Date.now() / 1000 } : l));
      }
    } catch (e) {
      setRowErr((m) => ({ ...m, [server]: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(null);
    }
  }
  async function add() {
    if (!addName.trim() || !addTarget.trim()) return;
    setBusy("add");
    setErr(null);
    try {
      const parts = addTarget.trim().split(/\s+/);
      const config = addKind === "stdio" ? { type: "stdio", command: parts[0], args: parts.slice(1) } : { type: "http", url: addTarget.trim() };
      const r = await api.mcpAdd(agent, addName.trim(), config);
      setList((l) => (l ? { ...l, servers: r.servers, refreshed_at: Date.now() / 1000 } : l));
      setAdding(false);
      setAddName("");
      setAddTarget("");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "add failed");
    } finally {
      setBusy(null);
    }
  }
  const statusColor = (st: string) => (st === "connected" ? "var(--sig-normal)" : st === "failed" ? "var(--sig-gate)" : st === "needs_auth" ? "var(--sig-busy, #c80)" : "var(--text-dim)");
  return (
    <span className="artifacts-wrap">
      <button
        ref={btnRef}
        className="charter-toggle"
        style={down ? { color: "var(--sig-gate)" } : undefined}
        onClick={(e) => {
          const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
          setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 580)) });
          setOpen((o) => !o);
          if (!open) load(false);
        }}
        title="MCP servers of this session: status, tools, reconnect, enable/disable, authenticate"
      >
        mcp{summary?.total ? ` ${summary.total}` : ""}{down ? ` · ${down} down` : ""} ▾
      </button>
      {open &&
        createPortal(
          <div className="artifacts-menu mcp-menu" style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width: 560 }} role="menu" onMouseLeave={() => setOpen(false)}>
            <div className="row">
              <span className="label">MCP servers</span>
              <span style={{ flex: 1 }} />
              {list && list.refreshed_at > 0 && <span className="mono-meta" title="when the harness was last asked">as of {relTime(list.refreshed_at)} ago</span>}
              <button className="btn ghost sm" disabled={busy === "refresh"} onClick={() => load(true)} title="ask the harness for the current state now">
                {busy === "refresh" ? "asking…" : "refresh"}
              </button>
            </div>
            {err && <div className="row error-text mono-meta">{err}</div>}
            {list === null && !err && <div className="row dim">loading…</div>}
            {list && list.servers.length === 0 && <div className="row dim">no MCP servers in this session</div>}
            {list?.servers.map((m) => (
              <div className="act-block" key={m.name}>
                <div className="row act-row">
                  <span className="chip mono" style={{ color: statusColor(m.status), borderColor: statusColor(m.status) }}>{m.status.replace("_", " ")}</span>
                  <span className="act-body">
                    <span className="act-label mono">
                      {m.name}
                      {m.plugin && <span className="mono-meta"> · plugin {m.plugin}</span>}
                      {m.scope && !m.plugin && <span className="mono-meta"> · {m.scope}</span>}
                    </span>
                    <span className="mono-meta act-detail" title={m.tools.join(", ")}>
                      {m.tools.length ? `${m.tools.length} tool${m.tools.length === 1 ? "" : "s"}` : "no tools"}
                      {m.server ? ` · ${m.server}` : ""}
                      {m.command ? ` · ${m.transport ?? ""} ${m.command}` : ""}
                    </span>
                    {(m.error || rowErr[m.name]) && <span className="error-text mono-meta">{rowErr[m.name] || m.error}</span>}
                  </span>
                  <span className="act-actions">
                    {list.can.reconnect && m.status !== "disabled" && (
                      <button className="btn ghost sm" disabled={busy === `reconnect:${m.name}`} onClick={() => void act(m.name, "reconnect")} title={list.can.toggle ? "reconnect this server" : "reload every server (this harness has no per-server reconnect)"}>
                        {busy === `reconnect:${m.name}` ? "…" : "reconnect"}
                      </button>
                    )}
                    {list.can.toggle &&
                      (m.status === "disabled" ? (
                        <button className="btn ghost sm" disabled={busy === `enable:${m.name}`} onClick={() => void act(m.name, "enable")}>enable</button>
                      ) : (
                        <button className="btn ghost sm" disabled={busy === `disable:${m.name}`} onClick={() => void act(m.name, "disable")}>disable</button>
                      ))}
                    {list.can.auth && m.status === "needs_auth" && (
                      <button className="btn sm" disabled={busy === `auth:${m.name}`} onClick={() => void act(m.name, "auth")} title="open the authentication flow in a new tab">authenticate</button>
                    )}
                  </span>
                </div>
              </div>
            ))}
            {list?.can.add && (
              <div className="row" style={{ flexWrap: "wrap", gap: 6 }}>
                {!adding ? (
                  <button className="btn ghost sm" onClick={() => setAdding(true)} title="add a server to this running session (no restart); not written to .mcp.json">add server…</button>
                ) : (
                  <>
                    <input className="mono" placeholder="name" value={addName} onChange={(e) => setAddName(e.target.value)} style={{ width: 120 }} />
                    <select value={addKind} onChange={(e) => setAddKind(e.target.value as "stdio" | "http")}>
                      <option value="stdio">stdio</option>
                      <option value="http">http</option>
                    </select>
                    <input className="mono" placeholder={addKind === "stdio" ? "command and args" : "https://…"} value={addTarget} onChange={(e) => setAddTarget(e.target.value)} style={{ flex: 1, minWidth: 200 }} />
                    <button className="btn primary sm" disabled={busy === "add" || !addName.trim() || !addTarget.trim()} onClick={() => void add()}>{busy === "add" ? "adding…" : "add"}</button>
                    <button className="btn ghost sm" onClick={() => setAdding(false)}>cancel</button>
                  </>
                )}
              </div>
            )}
          </div>,
          document.body,
        )}
    </span>
  );
}

/** "plugins ▾": what this session runs with (PROPOSALS §7), what it
 *  would start with now, and a link to the matrix. */
function PluginsMenu({ agent, running, updates }: { agent: string; running: ActivePlugin[]; updates: PluginUpdate[] }) {
  const [open, setOpen] = useState(false);
  const [eff, setEff] = useState<{ would_start_with: ActivePlugin[]; missing: string[] } | null>(null);
  const [at, setAt] = useState({ top: 0, left: 0 });
  return (
    <span className="artifacts-wrap">
      <button
        className="charter-toggle"
        onClick={(e) => {
          const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
          setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 460)) });
          setOpen((o) => !o);
          if (!open) api.pluginsEffective(agent).then(setEff).catch(() => setEff({ would_start_with: [], missing: [] }));
        }}
        title="plugins this session runs with, by rule"
      >
        plugins{running.length ? ` ${running.length}` : ""}{updates.length ? " ↑" : ""} ▾
      </button>
      {open &&
        createPortal(
          <div className="artifacts-menu plugins-menu" style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width: 440 }} role="menu" onMouseLeave={() => setOpen(false)}>
            <div className="row"><span className="label">running with</span></div>
            {running.length === 0 && <div className="row dim">no plugins (by rule, or not running)</div>}
            {running.map((p) => {
              const u = updates.find((x) => x.plugin === p.plugin && x.marketplace === p.marketplace);
              return (
                <div className="row" key={`${p.plugin}@${p.marketplace}`}>
                  <span className="mono">{p.plugin}</span>
                  <span className="mono-meta">@{p.marketplace} · {p.version} · via {p.via}</span>
                  {u && <span className="chip chip-busy" title="a newer version is cached; restart to pick it up">{u.available} cached</span>}
                </div>
              );
            })}
            {eff && (eff.would_start_with.some((w) => !running.some((r) => r.plugin === w.plugin && r.marketplace === w.marketplace)) || running.some((r) => !eff.would_start_with.some((w) => w.plugin === r.plugin && w.marketplace === r.marketplace))) && (
              <>
                <div className="row"><span className="label">would start with now</span></div>
                {eff.would_start_with.map((p) => (
                  <div className="row" key={`w-${p.plugin}@${p.marketplace}`}>
                    <span className="mono">{p.plugin}</span>
                    <span className="mono-meta">@{p.marketplace} · {p.version} · via {p.via}</span>
                  </div>
                ))}
                {eff.would_start_with.length === 0 && <div className="row dim">nothing</div>}
              </>
            )}
            {eff && eff.missing.length > 0 && <div className="row error-text mono-meta">not cached yet: {eff.missing.join(", ")}</div>}
            <div className="row">
              <Link to={`/plugins`} className="mono-meta" onClick={() => setOpen(false)}>manage plugins…</Link>
            </div>
          </div>,
          document.body,
        )}
    </span>
  );
}

/** "add to board": put this session in a board's first empty pane, or
 *  split its last pane. */
function AddToBoard({ agent }: { agent: string }) {
  const [open, setOpen] = useState(false);
  const [boards, setBoards] = useState<Board[] | null>(null);
  const [at, setAt] = useState({ top: 0, left: 0 });
  const nav = useNavigate();
  async function add(b: Board, how: "fill" | "right" | "down") {
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
    let layout: BoardNode = b.layout;
    let placed = false;
    if (how === "fill") [layout, placed] = fill(layout);
    if (!placed) {
      const dir = how === "down" ? "col" : "row";
      const pane: BoardNode = { kind: "session", id: `${Date.now().toString(36)}`, agent };
      layout = { kind: "split", id: `${Date.now().toString(36)}s`, dir, sizes: [50, 50], children: [layout, pane] };
    }
    await api.putBoard({ ...b, layout, updated_at: Date.now() / 1000 }).catch(() => {});
    setOpen(false);
    nav(`/board/${b.id}`);
  }
  return (
    <span className="artifacts-wrap">
      <button
        className="charter-toggle"
        onClick={(e) => {
          const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
          setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 400)) });
          setOpen((o) => !o);
          if (!open) api.boards().then(setBoards).catch(() => setBoards([]));
        }}
        title="put this session in a board"
      >
        board ▾
      </button>
      {open &&
        createPortal(
          <div className="artifacts-menu" style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width: 380 }} role="menu" onMouseLeave={() => setOpen(false)}>
            {boards === null ? (
              <div className="row dim">loading…</div>
            ) : (
              <>
                {boards.filter((b) => !b.query).map((b) => (
                  <div className="row" key={b.id}>
                    <span className="mono" style={{ flex: 1 }}>{b.name}</span>
                    <button className="btn sm" onClick={() => void add(b, "fill")} title="into the first empty pane, else split right">add</button>
                    <button className="btn sm" onClick={() => void add(b, "right")} title="split the board right">⫿</button>
                    <button className="btn sm" onClick={() => void add(b, "down")} title="split the board down">⫽</button>
                  </div>
                ))}
                <div className="row">
                  <Link to="/boards" className="mono-meta" onClick={() => setOpen(false)}>new board…</Link>
                </div>
              </>
            )}
          </div>,
          document.body,
        )}
    </span>
  );
}

/** How a session view behaves inside a board pane (PROPOSALS §6):
 *  pane-scoped draft, focus-driven autofocus, compact chrome, and a
 *  hook after each send for broadcast. */
export interface PaneMode {
  id: string;
  focused: boolean;
  compact?: boolean;
  onFocus?: () => void;
  onSent?: (text: string) => void;
}

export function SessionView({ name, pane, subagent }: { name: string; pane?: PaneMode; subagent?: string }) {
  const liveGate = useLiveGate();
  const nav = useNavigate();
  const { agents, agentsLoaded, refreshAgents } = useAppData();
  const agent = agents.find((a) => a.name === name);

  const [transcript, dispatch] = useReducer(reducer, undefined, emptyTranscript);
  const subPollRef = useRef<number | null>(null);
  const transcriptRef = useRef(transcript);
  transcriptRef.current = transcript;
  useEffect(() => {
    // A page unload skips React's cleanup; save what we have first.
    const onHide = () => {
      if (transcriptRef.current.items.length) persistTranscript(name, transcriptRef.current, true);
    };
    window.addEventListener("pagehide", onHide);
    return () => {
      window.removeEventListener("pagehide", onHide);
      if (transcriptRef.current.items.length) {
        rememberTranscript(name, transcriptRef.current);
        persistTranscript(name, transcriptRef.current);
      }
    };
  }, [name]);
  const [wsState, setWsState] = useState<WsState>("connecting");
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [interrupting, setInterrupting] = useState(false);
  const [lastTurn, setLastTurn] = useState<TurnInfo | null>(null);
  const [exited, setExited] = useState<{ code: number | null } | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // The composer draft survives leaving the view (PROPOSALS §1): per
  // agent, per browser, cleared on send.
  const draftKey = pane ? `aspen.draft.${name}.${pane.id}` : `aspen.draft.${name}`;
  const [draft, setDraft] = useState(() => {
    try {
      return localStorage.getItem(draftKey) ?? "";
    } catch {
      return "";
    }
  });
  useEffect(() => {
    const t = window.setTimeout(() => {
      try {
        if (draft) localStorage.setItem(draftKey, draft);
        else localStorage.removeItem(draftKey);
      } catch {
        // storage unavailable: the draft just doesn't persist
      }
    }, 300);
    return () => window.clearTimeout(t);
  }, [draft, draftKey]);
  useEffect(() => {
    if (pane?.focused) composerRef.current?.focus({ preventScroll: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane?.focused]);
  // Tool cards the operator opened or closed by hand; auto open/close
  // (the active call) applies only to untouched cards.
  const [pinnedTools, setPinnedTools] = useState<Map<number, boolean>>(() => new Map());
  // Pasted/dropped attachments (PROPOSALS §4): each gets a marker in the
  // text at the caret; the node places the content where the marker is.
  const [attachments, setAttachments] = useState<PendingAttachment[]>([]);
  const attachSeq = useRef(0);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const ATTACH_MAX = 8 * 1024 * 1024;
  const ATTACH_TOTAL = 24 * 1024 * 1024;
  const attachTotal = attachments.reduce((n, a) => n + a.size, 0);
  const attachOver = attachments.some((a) => a.size > ATTACH_MAX) || attachTotal > ATTACH_TOTAL;
  async function addFiles(files: File[]) {
    if (files.length === 0) return;
    const added: PendingAttachment[] = [];
    for (const f of files) {
      const n = ++attachSeq.current;
      const buf = new Uint8Array(await f.arrayBuffer());
      let bin = "";
      for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
      const name = f.name || (f.type.startsWith("image/") ? `pasted-${n}.${f.type.split("/")[1] ?? "png"}` : `pasted-${n}`);
      added.push({
        n,
        name,
        media_type: f.type || "application/octet-stream",
        data: btoa(bin),
        size: f.size,
        preview: f.type.startsWith("image/") ? URL.createObjectURL(f) : null,
      });
    }
    setAttachments((cur) => [...cur, ...added]);
    // Markers at the caret, in order.
    const el = composerRef.current;
    const markers = added.map((a) => `[attachment ${a.n}: ${a.name}]`).join(" ");
    const start = el?.selectionStart ?? draft.length;
    const end = el?.selectionEnd ?? draft.length;
    const before = draft.slice(0, start);
    const after = draft.slice(end);
    const sep1 = before && !/\s$/.test(before) ? " " : "";
    const sep2 = after && !/^\s/.test(after) ? " " : "";
    const next = `${before}${sep1}${markers}${sep2}${after}`;
    setDraft(next);
    requestAnimationFrame(() => {
      if (el) {
        const pos = (before + sep1 + markers).length;
        el.focus();
        el.setSelectionRange(pos, pos);
      }
    });
  }
  function removeAttachment(n: number) {
    setAttachments((cur) => cur.filter((a) => a.n !== n));
    setDraft((d) => d.replace(new RegExp(`\\s?\\[attachment ${n}: [^\\]]*\\]`), ""));
  }
  function onComposerPaste(e: React.ClipboardEvent<HTMLTextAreaElement>) {
    const files: File[] = [];
    for (const it of Array.from(e.clipboardData.items)) {
      if (it.kind === "file") {
        const f = it.getAsFile();
        if (f) files.push(f);
      }
    }
    if (files.length) {
      e.preventDefault();
      void addFiles(files);
    }
  }
  function onComposerDrop(e: React.DragEvent<HTMLElement>) {
    const files = Array.from(e.dataTransfer.files);
    if (files.length) {
      e.preventDefault();
      void addFiles(files);
    }
  }
  const [reloading, setReloading] = useState(false);
  const [reloadNote, setReloadNote] = useState<string | null>(null);
  const [confirmStop, setConfirmStop] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [reviving, setReviving] = useState(false);

  // Liveness truth comes from the polled roster, not just the WS `exited`
  // event — a page that loads (or reconnects) after the process died would
  // otherwise never learn it and strand the operator with dead controls.
  // The roster also clears the banner when the session is revived from
  // another surface. Remote agents count too: a remote row is present only
  // while its node's link is up (rosters vanish with the link), so
  // `live=false` on one means "not running there", and revive proxies to
  // that node. Before this, a down remote agent showed "reconnecting"
  // forever with no way to start it (seen live from the mac).
  useEffect(() => {
    if (!agentsLoaded || !agent) return;
    if (!agent.live) {
      // While a revive is in flight the roster lags a beat; don't flash
      // the banner back over it.
      if (reviving) return;
      setExited((x) => x ?? { code: null });
      setBusy(false);
      setInterrupting(false);
    } else {
      setExited(null);
    }
  }, [agentsLoaded, agent, reviving]);

  // --- interactive extras state ---
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [ctx, setCtx] = useState<ContextSummary | null>(null);
  const [ctxOpen, setCtxOpen] = useState(false);
  const [renderMode, setRenderMode] = useState<RenderMode>(loadRenderMode);
  const [modelValue, setModelValue] = useState("default");
  const [modeValue, setModeValue] = useState("default");
  // Signals that open the header menus from elsewhere (the status line,
  // `/mcp` in the composer); each increment opens once.
  const [mcpSignal, setMcpSignal] = useState(0);
  const [activitySignal, setActivitySignal] = useState(0);
  const [ctlNote, setCtlNote] = useState<string | null>(null);
  const [ctlError, setCtlError] = useState<string | null>(null);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [localLastTool, setLocalLastTool] = useState<string | null>(null);
  const [nowTick, setNowTick] = useState(() => Date.now());
  const [titleEditing, setTitleEditing] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [titleOverride, setTitleOverride] = useState<string | null | undefined>(undefined);
  const [charterOpen, setCharterOpen] = useState(false);
  // Branching: the agent name points at a head; branch-here leaves a
  // bookmark and forks; the history drawer lists lineage + bookmarks.
  const [historyOpen, setHistoryOpen] = useState(false);
  // Artifacts: files this session wrote/edited/read (PROPOSALS §3).
  const [artifactsOpen, setArtifactsOpen] = useState(false);
  // Move / copy to another node (PROPOSALS §5).
  const [moveOpen, setMoveOpen] = useState(false);
  const [moveMode, setMoveMode] = useState<"move" | "copy">("move");
  const [moveTo, setMoveTo] = useState("");
  const [moveRepo, setMoveRepo] = useState("");
  const [moveBusy, setMoveBusy] = useState(false);
  const [moveErr, setMoveErr] = useState<string | null>(null);
  const [nodes, setNodes] = useState<{ node: string; up: boolean; me: boolean }[]>([]);
  async function loadNodes(preferMe = false) {
    try {
      const m = await api.mesh();
      const mine = m.node ?? "";
      const list = [{ node: mine, up: true, me: true }, ...(m.peers ?? []).map((p) => ({ node: p.node, up: !!p.link_up, me: false }))];
      setNodes(list);
      const home = agent?.node ?? name.split("@")[2] ?? mine;
      if (preferMe && home !== mine) {
        setMoveTo(mine);
        return;
      }
      const first = list.find((n) => n.node !== home && n.up);
      if (first) setMoveTo(first.node);
    } catch {
      setNodes([]);
    }
  }
  // Preflight (MIGRATION.md): what the move would carry and whether the
  // target can take it, fetched whenever the dialog's target changes.
  const [preflight, setPreflight] = useState<MovePreflight | null>(null);
  useEffect(() => {
    if (!moveOpen || !moveTo) {
      setPreflight(null);
      return;
    }
    let live = true;
    setPreflight(null);
    api
      .movePreflight(name, moveTo)
      .then((p) => live && setPreflight(p))
      .catch((e) => live && setPreflight({ source_up: true, blockers: [e instanceof Error ? e.message : "preflight failed"] }));
    return () => {
      live = false;
    };
  }, [moveOpen, moveTo, name]);
  // "bring here" from Now (or a link) opens the dialog aimed at this node.
  const [searchParams, setSearchParams] = useSearchParams();
  useEffect(() => {
    if (searchParams.get("bring") === null || subagent) return;
    setMoveMode("move");
    setMoveErr(null);
    setMoveOpen(true);
    void loadNodes(true);
    setSearchParams({}, { replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams, subagent]);
  // A replica held here of a remote session whose home is down
  // (REPLICATION.md): the transcript endpoint serves it; this tells the
  // page to say so and to offer starting from it.
  const [replica, setReplica] = useState<{ replica: ReplicaInfo | null; home_up: boolean } | null>(null);
  // A remote address has a node segment (`bare@repo@node`); the agent row
  // itself vanishes from the fleet list while its node is down, so the
  // check keys off the name.
  const remoteName = name.split("@").length >= 3;
  useEffect(() => {
    if (!remoteName || subagent) {
      setReplica(null);
      return;
    }
    let live = true;
    const load = () => api.agentReplica(name).then((r) => live && setReplica(r)).catch(() => {});
    void load();
    const t = window.setInterval(load, 10000);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, [name, remoteName, subagent]);
  const replicaShown = !!(replica && !replica.home_up && replica.replica);

  async function doMove() {
    if (!moveTo) return;
    setMoveBusy(true);
    setMoveErr(null);
    try {
      const r = await api.moveAgent(name, { to: moveTo, mode: moveMode, repo: moveRepo.trim() || undefined, from_replica: replicaShown || undefined });
      setMoveOpen(false);
      const target = r.node === (nodes.find((n) => n.me)?.node ?? "") ? r.name : `${r.name}@${r.node}`;
      setCtlNote(`${moveMode === "copy" ? "copied" : "moved"} to ${r.node} · ${r.files} files · ${Math.round(r.bytes / 1024)} KB${r.notes.length ? ` · ${r.notes.length} note(s)` : ""}`);
      void refreshAgents();
      nav(`/session/${encodeURIComponent(target)}`);
    } catch (e) {
      setMoveErr(errText(e));
    } finally {
      setMoveBusy(false);
    }
  }
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const [artifactsAt, setArtifactsAt] = useState<{ top: number; left: number }>({ top: 0, left: 0 });
  useEffect(() => {
    if (!artifactsOpen) return;
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && t.closest(".artifacts-menu, .artifacts-wrap")) return;
      setArtifactsOpen(false);
    };
    const esc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") setArtifactsOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [artifactsOpen]);
  async function loadArtifacts() {
    try {
      setArtifacts(await api.artifacts(name));
    } catch {
      setArtifacts([]);
    }
  }
  const [history, setHistory] = useState<BookmarksInfo | null>(null);
  const [branching, setBranching] = useState(false);
  const [branchLabel, setBranchLabel] = useState<string | null>(null);
  const [branchAs, setBranchAs] = useState("");
  const [resumeAs, setResumeAs] = useState<{ id: number; name: string } | null>(null);
  const [charterDraft, setCharterDraft] = useState<string | null>(null);
  const [charterSaving, setCharterSaving] = useState(false);
  const [charterOverride, setCharterOverride] = useState<string | null | undefined>(undefined);
  const [acDismissed, setAcDismissed] = useState(false);
  const [acIdx, setAcIdx] = useState(0);
  const busyLocalStartRef = useRef<number | null>(null);
  const ctlTimerRef = useRef<number | undefined>(undefined);

  function flashCtlNote(msg: string) {
    setCtlError(null);
    setCtlNote(msg);
    if (ctlTimerRef.current !== undefined) window.clearTimeout(ctlTimerRef.current);
    ctlTimerRef.current = window.setTimeout(() => setCtlNote(null), 4000);
  }

  async function fetchContext() {
    // Only called at turn end or when idle — never mid-turn (docs §9).
    try {
      const payload = await api.contextUsage(name);
      setCtx(summarizeContext(payload));
    } catch {
      setCtx(null); // hide gracefully
    }
  }

  async function loadRuntime() {
    try {
      const rt = await api.runtime(name);
      setRuntime(rt);
      const inv = rt.inventory;
      const m = inv?.["model"];
      if (typeof m === "string" && m) setModelValue(m);
      const pm = inv?.["permissionMode"] ?? inv?.["permission_mode"] ?? rt.runtime?.mode;
      if (typeof pm === "string" && pm) setModeValue(pm);
    } catch {
      setRuntime(null); // autocomplete/model list degrade gracefully
    }
  }

  useEffect(() => {
    void loadRuntime();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name]);

  // Context on mount, but only once the agent is known idle (never mid-turn).
  // Opened mid-turn, the page starts busy: the composer locks and the
  // active tool card opens, exactly as if the turn had begun on screen;
  // `turn_ended` on the socket unlocks it as usual.
  const ctxSeededRef = useRef(false);
  const agentRef = useRef(agent);
  agentRef.current = agent;
  useEffect(() => {
    if (ctxSeededRef.current || !agent) return;
    ctxSeededRef.current = true;
    if (agent.turn_state === "busy" && agent.live) {
      setBusy(true);
      if (busyLocalStartRef.current === null) busyLocalStartRef.current = Date.now();
      setNowTick(Date.now());
    } else {
      dispatch({ type: "settle_tools" });
    }
    if (agent.turn_state !== "busy") void fetchContext();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agent]);

  // Event handling (transcript + session meta) behind a ref so the WS
  // callbacks always see the latest closure.
  const handleEventRef = useRef<(ev: SessionEvent) => void>(() => {});
  useEffect(() => {
    handleEventRef.current = (ev: SessionEvent) => {
      dispatch({ type: "event", ev });
      switch (ev.kind) {
        case "turn_ended":
          // §5.3: `turn_ended` is the single unlock signal for the composer.
          setBusy(false);
          // The state after this event is the one worth keeping across
          // page loads (the reducer has applied it by the next tick).
          window.setTimeout(() => persistTranscript(name, transcriptRef.current), 0);
          setInterrupting(false);
          setStatusNote(null);
          busyLocalStartRef.current = null;
          setLastTurn({
            subtype: typeof ev.subtype === "string" ? ev.subtype : "success",
            costUsd: typeof ev.total_cost_usd === "number" ? ev.total_cost_usd : null,
            durationMs: typeof ev.duration_ms === "number" ? ev.duration_ms : null,
          });
          void fetchContext();
          break;
        case "exited":
          setExited({ code: ev.code ?? null });
          setBusy(false);
          setInterrupting(false);
          setStatusNote(null);
          break;
        case "tool_use": {
          const tn = toolUseNameOf(ev);
          if (tn) setLocalLastTool(tn);
          setStatusNote(null);
          break;
        }
        case "text_delta":
        case "assistant_message":
          setStatusNote(null);
          break;
        case "status": {
          const note = statusNoteOf(ev.raw);
          if (note !== null) setStatusNote(note);
          const mode = statusModeOf(ev.raw);
          if (mode) setModeValue(mode);
          break;
        }
        default:
          break;
      }
    };
  });

  // History first, then live WS with reconnect + backoff.
  useEffect(() => {
    let disposed = false;
    let closeEvents: (() => void) | null = null;
    let timer: number | undefined;
    let attempt = 0;

    function connect() {
      if (disposed) return;
      closeEvents = openSessionEvents(name, {
        onOpen: () => {
          if (disposed) return;
          attempt = 0;
          setWsState("open");
        },
        onMessage: (data) => {
          if (disposed) return;
          const ev = parseSessionEvent(data);
          if (ev) handleEventRef.current(ev);
        },
        onClose: () => {
          if (disposed) return;
          setWsState("reconnecting");
          attempt += 1;
          const delay = Math.min(15000, 1000 * 2 ** Math.min(attempt - 1, 4));
          timer = window.setTimeout(connect, delay);
        },
      });
    }

    async function start() {
      if (subagent) {
        // Read-only view of a subagent's transcript: no socket, no cache;
        // re-read every 3s while the page is open (the file grows while
        // the agent runs). connect() is never reached.
        const poll = async () => {
          try {
            const items = await api.subagent(name, subagent);
            if (disposed) return;
            dispatch({ type: "seed", state: settleTools(seedFromHistory(items)) });
          } catch (e) {
            if (!disposed) setActionError(`subagent: ${errText(e)}`);
          }
        };
        await poll();
        if (!disposed) subPollRef.current = window.setInterval(() => void poll(), 3000);
        return;
      }
      try {
        // A cached state shows at once; only the tail after its last user
        // line is fetched (the whole history if that line is gone).
        const cached = cachedTranscript(name) ?? (await loadPersistedTranscript(name));
        if (disposed) return;
        const head = cached ? transcriptHead(cached) : null;
        if (cached) dispatch({ type: "seed", state: cached });
        // Whatever was fetched is persisted at once — the moment we have
        // it, not at teardown, which a hard reload may skip.
        if (cached && head) {
          const delta = await api.transcriptAfter(name, head);
          if (disposed) return;
          const merged = delta.after_found ? mergeAfter(cached, head, delta.items) : seedFromHistory(delta.items);
          dispatch({ type: "seed", state: merged });
          if (delta.items.length || !delta.after_found) persistTranscript(name, merged);
        } else {
          const history = await api.transcript(name);
          if (disposed) return;
          const seeded = seedFromHistory(history);
          dispatch({ type: "seed", state: seeded });
          persistTranscript(name, seeded);
        }
        // History that ends mid-call shows the call running only while
        // the agent really is busy.
        const a = agentRef.current;
        if (a && !(a.live && a.turn_state === "busy")) dispatch({ type: "settle_tools" });
        // Prompts raised before this page connected are still waiting on
        // the node; the socket only carries new ones.
        try {
          const needs = await api.needs();
          const mine = needs.prompts.filter((p) => p.agent === name);
          if (mine.length) dispatch({ type: "open_prompts", prompts: mine });
        } catch {
          // needs unavailable: prompts appear when the next one is raised
        }
        setHistoryError(null);
      } catch (e) {
        if (disposed) return;
        setHistoryError(errText(e));
      }
      connect();
    }

    void start();
    return () => {
      disposed = true;
      if (subPollRef.current) window.clearInterval(subPollRef.current);
      if (timer !== undefined) window.clearTimeout(timer);
      closeEvents?.();
    };
  }, [name]);

  // Working-seconds tick while busy.
  useEffect(() => {
    if (!busy) return;
    const t = window.setInterval(() => setNowTick(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [busy]);

  // Auto-scroll: stick to the bottom unless the operator scrolled away.
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickRef = useRef(true);
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [transcript.items]);

  async function send() {
    const text = draft.trim();
    if (!text || exited) return;
    // TUI-local commands the console has a surface for open it instead
    // of going to a harness that cannot run them (PROPOSALS-MCP.md §3.3).
    if (/^\/mcp(\s|$)/.test(text)) {
      setDraft("");
      setMcpSignal((n) => n + 1);
      return;
    }
    if (/^\/(tasks|bashes|monitors)(\s|$)/.test(text)) {
      setDraft("");
      setActivitySignal((n) => n + 1);
      return;
    }
    // Aspen-level command: /branch [label] — handled here, never sent.
    if (/^\/branch(\s|$)/.test(text)) {
      setDraft("");
      const rest = text.replace(/^\/branch\s*/, "");
      const m = /^(.*?)(?:\s+as\s+@?([A-Za-z0-9_-]+))?\s*$/.exec(rest);
      await branchHere(m?.[1] ?? rest, m?.[2]);
      return;
    }
    if (attachOver) {
      setActionError("an attachment is over the size cap (8 MB each, 24 MB per message)");
      return;
    }
    const localKey = crypto.randomUUID();
    const outgoing: OutgoingAttachment[] = attachments.map(({ n, name: an, media_type, data }) => ({ n, name: an, media_type, data }));
    const images: HistoryImage[] = attachments
      .filter((a) => a.media_type.startsWith("image/"))
      .map((a) => ({ media_type: a.media_type, data: a.data }));
    dispatch({ type: "local_send", text, localKey, images });
    setDraft("");
    setAttachments([]);
    setBusy(true);
    busyLocalStartRef.current = Date.now();
    setNowTick(Date.now());
    setActionError(null);
    try {
      const res = await api.sendMessage(name, text, outgoing);
      if (res.queued) {
        // Not lost: it rides the bus until the node's link returns.
        dispatch({ type: "send_failed", localKey });
        setBusy(false);
        busyLocalStartRef.current = null;
        setCtlNote(res.note ?? "queued — delivers when the node's link returns");
        return;
      }
      dispatch({ type: "sent", localKey, uuid: res.uuid ?? localKey });
      pane?.onSent?.(text);
    } catch (e) {
      dispatch({ type: "send_failed", localKey });
      setBusy(false);
      busyLocalStartRef.current = null;
      setActionError(`send: ${errText(e)}`);
    }
  }

  async function loadHistory() {
    try {
      setHistory(await api.bookmarks(name));
    } catch (e) {
      setActionError(`history: ${errText(e)}`);
    }
  }

  /// Branch here. Carry (no `as`): the tip becomes a bookmark (labeled),
  /// the session forks, and this name continues on the fork — the head
  /// moves at the first turn on the branch. Split (`as` given): the fork
  /// starts as a NEW agent and this one keeps its session; we open it.
  /// Also: `/branch [label] [as <name>]`.
  async function branchHere(label: string, as?: string) {
    if (branching) return;
    setBranching(true);
    setBranchLabel(null);
    setBranchAs("");
    setActionError(null);
    try {
      const asName = as?.trim() || undefined;
      const res = await api.branch(name, label.trim() || undefined, undefined, asName);
      if (asName) {
        nav(`/session/${encodeURIComponent(res.name)}`);
        return;
      }
      setCtlNote(`branched — the previous tip is bookmarked${label.trim() ? ` as “${label.trim()}”` : ""}`);
      await loadHistory();
      setHistoryOpen(true);
    } catch (e) {
      setActionError(`branch: ${errText(e)}`);
    } finally {
      setBranching(false);
    }
  }

  async function resumeBookmark(id: number, as?: string) {
    setActionError(null);
    try {
      const res = await api.resumeBookmark(name, id, as?.trim() || undefined);
      if (as?.trim()) {
        nav(`/session/${encodeURIComponent(res.name)}`);
        return;
      }
      setCtlNote("resumed the bookmark — the line you were on is bookmarked too");
      await loadHistory();
    } catch (e) {
      setActionError(`resume: ${errText(e)}`);
    }
  }

  async function interrupt() {
    setInterrupting(true);
    setActionError(null);
    try {
      await api.interrupt(name);
      // Stay "busy" until the error-flavored turn_ended lands. §5.4
    } catch (e) {
      setInterrupting(false);
      setActionError(`interrupt: ${errText(e)}`);
    }
  }

  useHotkeys("session", [
    {
      key: "i",
      description: "interrupt the running turn",
      when: () => busy && !interrupting,
      handler: () => void interrupt(),
    },
    {
      key: "x",
      description: "stop this session (asks to confirm)",
      when: () => exited === null,
      handler: () => setConfirmStop(true),
    },
    { key: "Enter", description: "send (Shift+Enter for a newline)" },
    { key: "/", description: "slash-command autocomplete in the composer" },
  ]);

  async function stopSession() {
    setStopping(true);
    setActionError(null);
    try {
      await api.deleteAgent(name);
      // The exited banner arrives via the WS `exited` event; the process
      // takes the clean shutdown ladder.
    } catch (e) {
      setActionError(`stop: ${errText(e)}`);
    } finally {
      setStopping(false);
      setConfirmStop(false);
    }
  }

  const [restarting, setRestarting] = useState(false);
  /** Stop and revive in place: same session id, new process, new plugin set. */
  async function restartForPlugins() {
    setRestarting(true);
    setActionError(null);
    try {
      await api.deleteAgent(name);
      await new Promise((r) => window.setTimeout(r, 800));
      await api.revive(name, "in_place");
      void refreshAgents();
    } catch (e) {
      setActionError(`restart: ${errText(e)}`);
    } finally {
      setRestarting(false);
    }
  }

  async function reviveSession() {
    setReviving(true);
    setActionError(null);
    try {
      // The live-elsewhere gate (409) puts fork / resume-anyway to the
      // operator here, exactly as a start would; cancel leaves it down.
      if ((await liveGate.guard((c) => api.revive(name, c))) === null) return;
      // Same session id resumes; the WS reconnect loop picks the live
      // session back up and history is already on screen.
      setExited(null);
      setBusy(false);
      void refreshAgents();
    } catch (e) {
      setActionError(`resume: ${errText(e)}`);
    } finally {
      setReviving(false);
    }
  }

  async function reload() {
    if (reloading) return;
    setReloading(true);
    setReloadNote(null);
    setActionError(null);
    try {
      await api.reloadAgent(name);
      setReloadNote("skills reloaded");
      window.setTimeout(() => setReloadNote(null), 3000);
      void loadRuntime(); // commands/inventory may have changed
    } catch (e) {
      setActionError(`reload: ${errText(e)}`);
    } finally {
      setReloading(false);
    }
  }

  async function answerPermissionWith(
    requestId: string,
    answer: PermissionAnswer,
  ): Promise<boolean> {
    setActionError(null);
    try {
      await api.answerPermission(name, requestId, answer);
      // Optimistic settle; the WS permission_settled is idempotent on top.
      dispatch({
        type: "event",
        ev: { kind: "permission_settled", request_id: requestId, allow: answer.allow },
      });
      return true;
    } catch (e) {
      setActionError(`permission: ${errText(e)}`);
      return false;
    }
  }

  function answerPermission(
    requestId: string,
    allow: boolean,
    message?: string,
  ): Promise<boolean> {
    return answerPermissionWith(
      requestId,
      allow ? { allow: true } : { allow: false, ...(message ? { message } : {}) },
    );
  }

  // --- header control actions ---

  async function changeModel(v: string) {
    setModelValue(v);
    try {
      await api.setModel(name, v === "default" ? null : v);
      flashCtlNote(`model → ${v} — takes effect next turn`);
    } catch (e) {
      setCtlError(`model: ${errText(e)}`);
    }
  }

  async function changeMode(v: string) {
    setModeValue(v);
    try {
      await api.setMode(name, v);
      flashCtlNote(`mode → ${v}`);
    } catch (e) {
      setCtlError(`mode: ${errText(e)}`);
    }
  }

  const title = titleOverride !== undefined ? titleOverride : (agent?.title ?? null);

  async function saveTitle() {
    const v = titleDraft.trim();
    setTitleEditing(false);
    try {
      await api.setTitle(name, v || null);
      setTitleOverride(v || null);
      flashCtlNote(v ? "title saved" : "title cleared");
    } catch (e) {
      setCtlError(`title: ${errText(e)}`);
    }
  }

  const charter = charterOverride !== undefined ? charterOverride : (agent?.charter ?? null);

  async function saveCharter() {
    if (charterDraft === null) return;
    const v = charterDraft.trim();
    setCharterSaving(true);
    try {
      await api.setCharter(name, v || null);
      setCharterOverride(v || null);
      setCharterDraft(null);
      flashCtlNote(v ? "charter saved" : "charter cleared");
    } catch (e) {
      setCtlError(`charter: ${errText(e)}`);
    } finally {
      setCharterSaving(false);
    }
  }

  function changeRenderMode(m: RenderMode) {
    setRenderMode(m);
    storeRenderMode(m);
  }

  // --- slash-command autocomplete ---

  const commands = useMemo(() => slashCommandsOf(runtime), [runtime]);
  // Skills as `$name` mentions (Codex's form; HARNESSES.md §6): the same
  // menu, matched on a trailing `$partial`.
  const skills = useMemo<SlashCommand[]>(
    () =>
      (runtime?.runtime?.skills ?? [])
        .filter((sk) => typeof sk.name === "string" && sk.name)
        .map((sk) => ({ name: `$${sk.name}`, description: sk.description ?? null, argumentHint: null })),
    [runtime],
  );
  const slashPartial = slashPartialOf(draft);
  const skillPartial = useMemo(() => {
    if (skills.length === 0) return null;
    const m = /(?:^|\s)\$([A-Za-z0-9_-]*)$/.exec(draft);
    return m ? m[1]! : null;
  }, [draft, skills.length]);
  const acMatches = useMemo(() => {
    if (acDismissed) return [];
    if (slashPartial !== null) return filterSlashCommands(commands, slashPartial);
    if (skillPartial !== null) {
      const q = skillPartial.toLowerCase();
      return skills.filter((sk) => sk.name.slice(1).toLowerCase().startsWith(q));
    }
    return [];
  }, [commands, skills, slashPartial, skillPartial, acDismissed]);
  const acOpen = acMatches.length > 0;

  useEffect(() => {
    setAcIdx(0);
  }, [slashPartial, skillPartial]);

  function pickCommand(cmd: SlashCommand | undefined) {
    if (!cmd) return;
    if (cmd.name.startsWith("$")) {
      setDraft((d) => `${d.replace(/\$[A-Za-z0-9_-]*$/, "")}${cmd.name} `);
    } else {
      setDraft(`/${cmd.name} `);
    }
    setAcDismissed(false);
  }

  function onDraftChange(v: string) {
    setDraft(v);
    if (acDismissed && slashPartialOf(v) === null && !/(?:^|\s)\$[A-Za-z0-9_-]*$/.test(v)) setAcDismissed(false);
  }

  // --- model options ---

  const modelOptions = useMemo(() => {
    const opts = normalizeModels(runtime?.handshake?.models).filter(
      (o) => o.id !== "default",
    );
    if (modelValue !== "default" && !opts.some((o) => o.id === modelValue)) {
      opts.push({ id: modelValue, label: modelValue });
    }
    return opts;
  }, [runtime, modelValue]);

  // --- rendering ---

  function renderConsoleItem(item: TranscriptItem) {
    switch (item.kind) {
      case "assistant": {
        if (!item.text && !item.open) return null;
        return (
          <div key={item.id} className="cline cline-assistant">
            {item.text ? <TuiMd text={item.text} agent={name} /> : "…"}
            {item.open && item.text && <span className="caret" aria-hidden="true" />}
          </div>
        );
      }
      case "user":
        return (
          <div key={item.id} className="cline cline-user">
            {"> " + item.text}
          </div>
        );
      case "notice":
        return <NoticeCard key={item.id} item={item} tui />;
      case "bus": {
        // Keep the [aspen bus] header line as raw terminal text; the body
        // is agent prose and renders as TUI markdown like everything else.
        const nl = item.text.indexOf("\n");
        const header = nl >= 0 ? item.text.slice(0, nl) : item.text;
        const body = nl >= 0 ? item.text.slice(nl + 1) : "";
        return (
          <div key={item.id} className="cline cline-bus">
            <div>{header}</div>
            {body && <TuiMd text={body} agent={name} />}
          </div>
        );
      }
      case "tool":
        return toolCard(item, true);
      case "permission":
        if (item.settled) {
          return (
            <div key={item.id} className="cline cline-tool">
              [permission] {item.outcome ?? "settled"} {item.toolName}
            </div>
          );
        }
        // Unsettled prompts stay interactive in every render mode.
        return renderPermissionItem(item);
      case "turn_end":
        return (
          <div key={item.id} className="cline cline-turn">
            ── turn ended · {item.subtype}
          </div>
        );
    }
  }

  function renderPermissionItem(item: PermissionCardItem) {
    const isQuestion =
      item.toolName === "AskUserQuestion" || parseQuestions(item.input) !== null;
    if (isQuestion) {
      return (
        <QuestionCard
          key={item.id}
          item={item}
          onAnswer={(answer) => answerPermissionWith(item.requestId, answer)}
        />
      );
    }
    return (
      <PermissionCard
        key={item.id}
        item={item}
        onAnswer={(allow, message) => answerPermission(item.requestId, allow, message)}
        onAlways={
          hasSuggestions(item.suggestions)
            ? () =>
                answerPermissionWith(item.requestId, {
                  allow: true,
                  updated_permissions: item.suggestions,
                })
            : null
        }
      />
    );
  }

  // A long transcript renders its tail first (markdown for thousands of
  // items takes seconds); earlier items come in on request. The window
  // is counted from the end so new items never push it.
  const [showEarlier, setShowEarlier] = useState(0);
  const windowSize = RECENT_WINDOW + showEarlier;
  const hiddenEarlier = Math.max(0, transcript.items.length - windowSize);
  const visibleItems = hiddenEarlier > 0 ? transcript.items.slice(hiddenEarlier) : transcript.items;

  // The active call: the newest unfinished tool while the turn is busy
  // (a permission prompt or status line may sit after it; the harness
  // reports the result only when the call ends).
  let activeToolId: number | null = null;
  if (busy) {
    for (let i = transcript.items.length - 1; i >= 0; i--) {
      const it = transcript.items[i]!;
      if (it.kind === "tool") {
        if (!it.done) activeToolId = it.id;
        break;
      }
    }
  }
  function toolCard(item: ToolCardItem, tui: boolean) {
    const pinned = pinnedTools.get(item.id);
    const open = pinned !== undefined ? pinned : item.id === activeToolId;
    return (
      <ToolCard
        key={item.id}
        item={item}
        open={open}
        now={nowTick}
        tui={tui}
        agent={name}
        onToggle={(o) => setPinnedTools((m) => new Map(m).set(item.id, o))}
      />
    );
  }

  function renderItem(item: TranscriptItem) {
    if (renderMode === "console") return renderConsoleItem(item);
    const source = renderMode === "source";
    switch (item.kind) {
      case "assistant":
        return <AssistantBubble key={item.id} item={item} source={source} agent={name} />;
      case "user":
        return <UserBubble key={item.id} item={item} />;
      case "notice":
        return <NoticeCard key={item.id} item={item} />;
      case "bus":
        return <BusBubble key={item.id} item={item} source={source} agent={name} />;
      case "tool":
        return toolCard(item, false);
      case "permission":
        return renderPermissionItem(item);
      case "turn_end":
        return <TurnEndMarker key={item.id} item={item} />;
    }
  }

  function composerKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (acOpen) {
      const n = acMatches.length;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setAcIdx((i) => (i + 1) % n);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setAcIdx((i) => (i - 1 + n) % n);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        pickCommand(acMatches[Math.min(acIdx, n - 1)]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setAcDismissed(true);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
      return;
    }
    // Escape hands the keyboard back to the page (nav keys, ?, i, x…).
    if (e.key === "Escape") {
      e.currentTarget.blur();
    }
  }

  // The composer stays open while the agent works: Claude queues a
  // mid-turn message for the next turn (and coalesces several), Codex
  // steers the running turn — the TUI's behavior, and the reference's
  // rule ("send immediately, always").
  const composerDisabled = exited !== null;

  const pendingPerms = transcript.items.filter(
    (it): it is PermissionCardItem => it.kind === "permission" && !it.settled,
  );

  // Working seconds: prefer the server's busy_since, fall back to local start.
  let workSecs: number | null = null;
  if (busy) {
    const since = agent?.busy_since;
    if (typeof since === "number" && since > 0) {
      workSecs = Math.max(0, Math.floor(nowTick / 1000 - since));
    } else if (busyLocalStartRef.current !== null) {
      workSecs = Math.max(0, Math.floor((nowTick - busyLocalStartRef.current) / 1000));
    }
  }
  const lastTool = localLastTool ?? agent?.last_tool ?? null;

  const ctxPct = ctx?.percent !== null && ctx?.percent !== undefined ? Math.round(ctx.percent) : null;

  return (
    <div className={pane ? `session in-pane${pane.compact ? " compact" : ""}` : "session"} onMouseDownCapture={pane?.onFocus}>
      <header className="session-head">
        {agent && <Meter presence={presenceOf(agent.live, agent.turn_state)} />}
        <h1>
          <span className="mono">@{name}</span>
        </h1>
        {agent?.harness && (
          <span className="chip mono harness-chip" title={`this session runs on ${agent.harness}`}>
            {agent.harness}
          </span>
        )}
        {titleEditing ? (
          <input
            autoFocus
            className="session-title-input"
            value={titleDraft}
            placeholder="title (empty clears)"
            onChange={(e) => setTitleDraft(e.target.value)}
            onBlur={() => void saveTitle()}
            onKeyDown={(e) => {
              if (e.key === "Enter") void saveTitle();
              if (e.key === "Escape") setTitleEditing(false);
            }}
          />
        ) : (
          <span
            className={title ? "session-title" : "session-title unset"}
            title="click to edit title"
            onClick={() => {
              setTitleDraft(title ?? "");
              setTitleEditing(true);
            }}
          >
            {title ?? "add title"}
          </span>
        )}
        {agent && (
          <span className="dim mono session-repo">
            {agent.repo ?? `node ${agent.node}`} · #{agent.channel}
          </span>
        )}
        <span className="spacer" />
        {reloadNote ? (
          <span className="ok-inline mono reload-note">{reloadNote}</span>
        ) : (
          <button
            className="btn-reload"
            onClick={() => void reload()}
            disabled={reloading || exited !== null}
            title="reload this session's plugins/skills/commands"
          >
            {reloading ? "reloading…" : "reload"}
          </button>
        )}
        {exited === null &&
          (branchLabel !== null ? (
            <span className="stop-confirm">
              <input
                className="mono"
                value={branchLabel}
                onChange={(e) => setBranchLabel(e.target.value)}
                placeholder="bookmark label (optional)"
                autoFocus
                style={{ width: 180 }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void branchHere(branchLabel, branchAs);
                  if (e.key === "Escape") setBranchLabel(null);
                }}
                aria-label="bookmark label"
              />
              <input
                className="mono"
                value={branchAs}
                onChange={(e) => setBranchAs(e.target.value)}
                placeholder="continue as @… (optional)"
                style={{ width: 170 }}
                title="empty: @name follows the branch and the tip is bookmarked. A name: the branch becomes a new agent and this one keeps its session."
                onKeyDown={(e) => {
                  if (e.key === "Enter") void branchHere(branchLabel, branchAs);
                  if (e.key === "Escape") setBranchLabel(null);
                }}
                aria-label="continue the branch as a new agent"
              />
              <button className="btn sm" disabled={branching} onClick={() => void branchHere(branchLabel, branchAs)}>
                {branching ? "branching…" : branchAs.trim() ? `split → @${branchAs.trim()}` : "branch here"}
              </button>
              <button className="btn ghost sm" onClick={() => setBranchLabel(null)}>
                cancel
              </button>
            </span>
          ) : (
            <button
              className="btn-reload"
              onClick={() => setBranchLabel("")}
              disabled={branching}
              title="branch here: bookmark this point and continue on a fork — come back to the bookmark any time (also: type /branch [label])"
            >
              branch
            </button>
          ))}
        {exited === null &&
          (confirmStop ? (
            <span className="stop-confirm">
              <button
                className="btn danger sm"
                disabled={stopping}
                onClick={() => void stopSession()}
              >
                {stopping ? "stopping…" : "confirm stop"}
              </button>
              <button className="btn ghost sm" onClick={() => setConfirmStop(false)}>
                cancel
              </button>
            </span>
          ) : (
            <button
              className="btn-reload btn-stop"
              onClick={() => setConfirmStop(true)}
              title="stop this session — the claude process exits cleanly; the conversation stays on disk and can be resumed"
            >
              stop
            </button>
          ))}
        <span className={`ws-state ws-${wsState}`}>
          <span
            className={wsState === "open" ? "dot dot-idle" : "dot dot-down"}
            aria-hidden="true"
          />
          {subagent ? "read-only" : exited ? "not running" : wsState === "open" ? "live" : wsState}
        </span>
      </header>

      <div className="session-controls">
        <span className="ctl-group">
          <span className="ctl-label">model</span>
          <select
            className="ctl-select"
            value={modelValue}
            disabled={exited !== null}
            onChange={(e) => void changeModel(e.target.value)}
            title="switch model — takes effect next turn"
          >
            <option value="default">default</option>
            {modelOptions.map((o) => (
              <option key={o.id} value={o.id} title={o.description}>
                {o.label}
              </option>
            ))}
          </select>
        </span>
        <span className="ctl-group">
          <span className="ctl-label">mode</span>
          <select
            className="ctl-select"
            value={modeValue}
            disabled={exited !== null}
            onChange={(e) => void changeMode(e.target.value)}
            title="permission mode"
          >
            {(runtime?.modes?.length
              ? runtime.modes.map((m) => ({ id: m.id, label: m.label, hint: m.hint }))
              : PERMISSION_MODES.map((m) => ({ id: m, label: m, hint: undefined }))
            ).map((m) => (
              <option key={m.id} value={m.id} title={m.hint}>
                {m.label}
              </option>
            ))}
          </select>
        </span>
        <div className="seg" role="group" aria-label="render mode">
          {(["chat", "console", "source"] as const).map((m) => (
            <button
              key={m}
              type="button"
              aria-pressed={renderMode === m}
              onClick={() => changeRenderMode(m)}
            >
              {m}
            </button>
          ))}
        </div>
        <button
          className="charter-toggle"
          aria-expanded={charterOpen}
          onClick={() => setCharterOpen((o) => !o)}
        >
          charter {charterOpen ? "▴" : "▾"}
        </button>
        <button
          className="charter-toggle"
          aria-expanded={historyOpen}
          onClick={() => {
            const next = !historyOpen;
            setHistoryOpen(next);
            if (next) void loadHistory();
          }}
          title="this name's lineage and bookmarks"
        >
          history {historyOpen ? "▴" : "▾"}
        </button>
        <span className="artifacts-wrap">
          <button
            className="charter-toggle"
            aria-expanded={artifactsOpen}
            onClick={(e) => {
              const next = !artifactsOpen;
              const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
              // Keep the menu on screen: it is up to 640px wide.
              setArtifactsAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - 656)) });
              setArtifactsOpen(next);
              if (next) void loadArtifacts();
            }}
            title="files this session wrote, edited, or read — open any in the viewer"
          >
            artifacts {artifactsOpen ? "▴" : "▾"}
          </button>
          {artifactsOpen && createPortal(
            <div className="artifacts-menu" role="menu" style={{ position: "fixed", top: artifactsAt.top, left: artifactsAt.left, right: "auto" }}>
              {artifacts === null ? (
                <div className="row dim">loading…</div>
              ) : artifacts.length === 0 ? (
                <div className="row dim">nothing named by a tool call yet</div>
              ) : (
                artifacts.map((a) => (
                  <div className="row" key={a.path}>
                    <span className={`chip mono art-${a.kind}`}>{a.kind}</span>
                    <Link className="mono" to={`/view/${encodeURIComponent(name)}?path=${encodeURIComponent(a.path)}`} onClick={() => setArtifactsOpen(false)} title={a.path}>
                      {a.path}
                    </Link>
                    {a.at && <span className="mono-meta">{relTime(Date.parse(a.at) / 1000)} ago</span>}
                  </div>
                ))
              )}
            </div>,
            document.body,
          )}
        </span>
        {!agent?.moved_to && (
          <button
            className="charter-toggle"
            onClick={() => {
              setMoveOpen(true);
              setMoveErr(null);
              void loadNodes();
            }}
            title="move this session to another node (it resumes there with its context), or copy it there as a fork"
          >
            move…
          </button>
        )}
        {!agent?.moved_to && name.split("@").length >= 3 && (
          <button
            className="charter-toggle"
            onClick={() => {
              setMoveMode("move");
              setMoveErr(null);
              setMoveOpen(true);
              void loadNodes(true);
            }}
            title="move this session to this console's node, with the counterpart repo"
          >
            bring here
          </button>
        )}
        <AddToBoard agent={name} />
        <PluginsMenu agent={name} running={agent?.plugins ?? []} updates={agent?.plugin_updates ?? []} />
        <McpMenu agent={name} summary={agent?.mcp ?? null} openSignal={mcpSignal} />
        <ActivityMenu agent={name} counts={agent?.activities ?? null} openSignal={activitySignal} />
        {moveOpen &&
          createPortal(
            <div className="trust-backdrop" onClick={() => !moveBusy && setMoveOpen(false)} role="presentation">
              <div className="trust-panel" role="dialog" aria-label="move session" onClick={(e) => e.stopPropagation()}>
                <div className="trust-head">
                  <span className="label">{moveMode === "copy" ? "Copy" : "Move"} @{name} to another node</span>
                  <span style={{ flex: 1 }} />
                  <button className="btn ghost sm" onClick={() => setMoveOpen(false)} disabled={moveBusy}>esc</button>
                </div>
                <p className="trust-lede">
                  <b>Move</b>: the session stops here and resumes there with its whole context; this address becomes a
                  pointer to the new one. <b>Copy</b>: a fork starts there (new session id, history kept, lineage
                  recorded) and this one keeps running.
                </p>
                <div className="move-form">
                  <label>
                    <span className="mono-meta">to node</span>
                    <select value={moveTo} onChange={(e) => setMoveTo(e.target.value)} disabled={moveBusy}>
                      {nodes.filter((n) => n.node !== (agent?.node ?? name.split("@")[2] ?? nodes.find((x) => x.me)?.node)).map((n) => (
                        <option key={n.node} value={n.node} disabled={!n.up}>
                          {n.node}{n.me ? " (this console)" : ""}{n.up ? "" : " — link down"}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    <span className="mono-meta">mode</span>
                    <span className="seg">
                      <button className={moveMode === "move" ? "on" : ""} onClick={() => setMoveMode("move")} disabled={moveBusy}>move</button>
                      <button className={moveMode === "copy" ? "on" : ""} onClick={() => setMoveMode("copy")} disabled={moveBusy}>copy</button>
                    </span>
                  </label>
                  <label>
                    <span className="mono-meta">repo path there (blank = the counterpart by git origin, else by name)</span>
                    <input className="mono" value={moveRepo} onChange={(e) => setMoveRepo(e.target.value)} placeholder="/path/on/that/node" disabled={moveBusy} />
                  </label>
                </div>
                {moveTo && (
                  <div className="preflight">
                    {preflight === null ? (
                      <span className="mono-meta">checking…</span>
                    ) : (
                      <>
                        {preflight.source && (
                          <span className="mono-meta">
                            carries {Math.round((preflight.source.tiers.A + preflight.source.tiers.B) / 1024)} KB of transcript
                            {preflight.source.tiers.C > 0 ? ` · ${Math.round(preflight.source.tiers.C / 1024)} KB memory` : ""} · {preflight.source.files} files
                            {preflight.source.branch ? ` · ${preflight.source.branch}` : ""}
                            {preflight.source.harness ? ` · ${preflight.source.harness_name ?? "claude"} ${preflight.source.harness}` : ""}
                            {preflight.target?.harness ? ` → ${preflight.target.harness}` : ""}
                            {preflight.target?.counterpart ? ` · repo there: ${preflight.target.counterpart}` : ""}
                          </span>
                        )}
                        {(preflight.warnings ?? []).map((w) => (
                          <span key={w} className="mono-meta" style={{ color: "var(--sig-normal)" }}>{w}</span>
                        ))}
                        {preflight.blockers.map((b) => (
                          <span key={b} className="mono-meta error-text">{b}</span>
                        ))}
                      </>
                    )}
                  </div>
                )}
                {replicaShown && (
                  <p className="trust-lede">
                    The home node is down: the copy will start from the replica held on {replica?.replica?.held_on} (as of {relTime(replica?.replica?.as_of ?? 0)} ago). The original may still be running there; when it returns, keep one.
                  </p>
                )}
                {moveErr && <div className="error-bar">{moveErr}</div>}
                <div className="trust-actions">
                  <button className="btn ghost" onClick={() => setMoveOpen(false)} disabled={moveBusy}>cancel</button>
                  <button className="btn primary" onClick={() => void doMove()} disabled={moveBusy || !moveTo || (preflight !== null && preflight.blockers.length > 0 && !moveRepo.trim())}>
                    {moveBusy ? (moveMode === "copy" ? "copying…" : "moving…") : moveMode === "copy" ? "copy there" : "move there"}
                  </button>
                </div>
              </div>
            </div>,
            document.body,
          )}
        {ctlNote && <span className="ctl-note">{ctlNote}</span>}
        {ctlError && <span className="ctl-error">{ctlError}</span>}
        <span className="spacer" />
        {ctx && (ctxPct !== null || ctx.categories.length > 0) && (
          <div
            className="ctx-meter"
            onClick={() => setCtxOpen((o) => !o)}
            title="context usage — click for breakdown"
          >
            <span className="ctl-label">ctx</span>
            {ctxPct !== null && (
              <>
                <span className="ctx-bar">
                  <span
                    className={
                      ctxPct >= 90 ? "ctx-fill hot" : ctxPct >= 70 ? "ctx-fill warm" : "ctx-fill"
                    }
                    style={{ width: `${ctxPct}%` }}
                  />
                </span>
                <span className="ctx-pct">{ctxPct}%</span>
              </>
            )}
            {ctxOpen && (
              <div className="ctx-pop" onClick={(e) => e.stopPropagation()}>
                {ctx.categories.slice(0, 8).map((c) => (
                  <div className="ctx-row" key={c.name}>
                    <span>{c.name}</span>
                    <b>{fmtTokens(c.tokens)}</b>
                  </div>
                ))}
                {(ctx.usedTokens !== null || ctx.maxTokens !== null) && (
                  <div className="ctx-row ctx-row-total">
                    <span>used / max</span>
                    <b>
                      {ctx.usedTokens !== null ? fmtTokens(ctx.usedTokens) : "—"} /{" "}
                      {ctx.maxTokens !== null ? fmtTokens(ctx.maxTokens) : "—"}
                    </b>
                  </div>
                )}
                {ctx.autoCompactThreshold !== null && (
                  <div className="ctx-row">
                    <span>auto-compact at</span>
                    <b>{fmtTokens(ctx.autoCompactThreshold)}</b>
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {historyOpen && (
        <div className="charter-drawer">
          {history === null ? (
            <div className="dim">loading…</div>
          ) : (
            <>
              <div className="charter-caption" style={{ marginBottom: 6 }}>
                head {history.head ? history.head.slice(0, 8) : "—"}
                {history.lineage.length > 0 &&
                  ` ← ${history.lineage.map((l) => l.session_id.slice(0, 8)).join(" ← ")}`}
                {" · "}
                the name follows the head; branching leaves the tip here as a bookmark. “as new agent” starts a sibling instead.
              </div>
              {history.bookmarks.length === 0 ? (
                <div className="dim">no bookmarks yet — branch to leave one.</div>
              ) : (
                history.bookmarks.map((b) => (
                  <div
                    key={b.id}
                    style={{ display: "flex", alignItems: "center", gap: 10, padding: "4px 0" }}
                  >
                    <span className="chip mono" title={b.reason}>
                      {b.reason === "branch" ? "left by branch" : b.reason === "swap" ? "left by resume" : b.reason}
                    </span>
                    <span className="mono" style={{ color: "var(--text-hi)" }}>
                      {b.label || "(no label)"}
                    </span>
                    <span className="mono-meta">{b.session_id.slice(0, 8)}</span>
                    <span className="mono-meta">{relTime(b.created_at)} ago</span>
                    <span style={{ flex: 1 }} />
                    {resumeAs?.id === b.id ? (
                      <>
                        <input
                          className="mono"
                          value={resumeAs.name}
                          onChange={(e) => setResumeAs({ id: b.id, name: e.target.value })}
                          autoFocus
                          placeholder="new agent name"
                          style={{ width: 140 }}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") void resumeBookmark(b.id, resumeAs.name);
                            if (e.key === "Escape") setResumeAs(null);
                          }}
                          aria-label="resume the bookmark as a new agent"
                        />
                        <button className="btn primary sm" onClick={() => void resumeBookmark(b.id, resumeAs.name)}>
                          start @{resumeAs.name.trim() || "…"}
                        </button>
                        <button className="btn ghost sm" onClick={() => setResumeAs(null)}>cancel</button>
                      </>
                    ) : (
                      <>
                        <button className="btn sm" onClick={() => void resumeBookmark(b.id)} title="this name moves to a fork of the bookmark; the line you're on is bookmarked">
                          resume here
                        </button>
                        <button className="btn ghost sm" onClick={() => setResumeAs({ id: b.id, name: "" })} title="a new agent starts from this bookmark; this one stays where it is">
                          as new agent…
                        </button>
                      </>
                    )}
                    <button
                      className="btn ghost sm"
                      onClick={() =>
                        void api.deleteBookmark(name, b.id).then(loadHistory).catch((e) => setActionError(errText(e)))
                      }
                    >
                      forget
                    </button>
                  </div>
                ))
              )}
            </>
          )}
        </div>
      )}

      {charterOpen && (
        <div className="charter-drawer">
          {charterDraft === null ? (
            <>
              {charter ? (
                <pre className="charter-body">{charter}</pre>
              ) : (
                <div className="dim" style={{ marginBottom: 8 }}>
                  no charter set.
                </div>
              )}
              <div className="charter-actions">
                <button className="btn sm" onClick={() => setCharterDraft(charter ?? "")}>
                  edit
                </button>
                <span className="charter-caption">
                  applies at next revive — a charter rides the system prompt
                </span>
              </div>
            </>
          ) : (
            <>
              <textarea
                className="charter-edit"
                value={charterDraft}
                placeholder="charter (empty clears)"
                onChange={(e) => setCharterDraft(e.target.value)}
              />
              <div className="charter-actions">
                <button
                  className="btn sm primary"
                  disabled={charterSaving}
                  onClick={() => void saveCharter()}
                >
                  {charterSaving ? "saving…" : "save"}
                </button>
                <button className="btn sm ghost" onClick={() => setCharterDraft(null)}>
                  cancel
                </button>
                <span className="charter-caption">
                  applies at next revive — a charter rides the system prompt
                </span>
              </div>
            </>
          )}
        </div>
      )}

      {historyError && (
        <div className="error-inline">history: {historyError} (live stream only)</div>
      )}

      {pendingPerms.length > 0 && (
        <div className="perm-dock">
          <span className="chip chip-perm">permission</span>
          <span>
            {pendingPerms.length === 1
              ? `@${name} needs approval for ${pendingPerms[0].toolName}`
              : `@${name} needs ${pendingPerms.length} approvals`}
          </span>
          <span style={{ flex: 1 }} />
          <button
            className="btn sm"
            onClick={() => {
              const el = scrollRef.current?.querySelector(".perm-card, .q-card");
              el?.scrollIntoView({ behavior: "smooth", block: "center" });
            }}
          >
            jump to it
          </button>
        </div>
      )}

      {liveGate.dialog}
      {subagent && (
        <div className="exited-banner">
          <span>
            subagent <span className="mono">{subagent}</span> of <Link to={`/session/${encodeURIComponent(name)}`}>@{name}</Link> — read-only, re-read every 3s while it runs
          </span>
        </div>
      )}
      {(agent?.plugin_updates?.length ?? 0) > 0 && !exited && !subagent && (
        <div className="plugin-nag">
          <span>
            newer plugin version{agent!.plugin_updates!.length === 1 ? "" : "s"} cached:{" "}
            {agent!.plugin_updates!.map((u) => `${u.plugin} ${u.running} → ${u.available}`).join(", ")} — this session keeps what it started with until it restarts.
          </span>
          <button className="btn sm primary" disabled={busy || restarting} onClick={() => void restartForPlugins()}>
            {restarting ? "restarting…" : "restart to pick them up"}
          </button>
        </div>
      )}
      {agent?.moved_to && (
        <div className="exited-banner">
          <span>
            moved — this session now lives at <Link to={`/session/${encodeURIComponent(agent.moved_to)}`}>@{agent.moved_to}</Link>; messages to this address are refused with that pointer.
          </span>
        </div>
      )}
      {replicaShown && replica?.replica && (
        <div className="exited-banner replica-banner">
          <span>
            {replica.replica.node} is down — showing the replica held on {replica.replica.held_on} as of {relTime(replica.replica.as_of)} ago
            ({Math.round(replica.replica.bytes / 1024)} KB, {replica.replica.files} files). Messages cannot be delivered until the node returns.
          </span>
          <button
            className="btn primary sm"
            onClick={() => {
              setMoveMode("copy");
              setMoveTo(nodes.find((n) => n.me)?.node ?? "");
              setMoveOpen(true);
            }}
            title="start a session here from this replica (a fork; the original may still run when its node returns)"
          >
            start from the replica here
          </button>
        </div>
      )}
      {exited && !agent?.moved_to && !replicaShown && (
        <div className="exited-banner">
          <span>
            {agent?.remote ? `not running on ${agent.node}` : "session exited"}
            {exited.code !== null ? ` (code ${exited.code})` : ""} — the conversation is on disk and can continue.
          </span>
          <button
            className="btn primary sm"
            disabled={reviving}
            onClick={() => void reviveSession()}
          >
            {reviving ? "resuming…" : `resume @${name}`}
          </button>
          <span className="mono-meta">
            or from <Link to="/">Now</Link> / <Link to="/mesh?view=list">Mesh</Link>
          </span>
        </div>
      )}

      <div
        className={renderMode === "console" ? "transcript console" : "transcript"}
        ref={scrollRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
        }}
      >
        {transcript.items.length === 0 && (
          <div className="empty">no transcript yet — say something below.</div>
        )}
        {hiddenEarlier > 0 && (
          <div className="earlier-bar">
            <button className="btn sm" onClick={() => setShowEarlier((n) => n + EARLIER_STEP)}>
              show {Math.min(EARLIER_STEP, hiddenEarlier)} earlier
            </button>
            <button className="btn ghost sm" onClick={() => setShowEarlier(Infinity)}>
              show all {hiddenEarlier} earlier
            </button>
          </div>
        )}
        {visibleItems.map(renderItem)}
      </div>

      <div className="status-line">
        <span className="status-left mono">
          {(agent?.activities?.running ?? 0) > 0 && (
            <button
              className="status-activity"
              onClick={() => setActivitySignal((n) => n + 1)}
              title="background work of this session — click to manage"
            >
              {[agent?.activities?.monitors ? `${agent.activities.monitors} monitor${agent.activities.monitors === 1 ? "" : "s"}` : "", agent?.activities?.tasks ? `${agent.activities.tasks} task${agent.activities.tasks === 1 ? "" : "s"}` : "", agent?.activities?.agents ? `${agent.activities.agents} agent${agent.activities.agents === 1 ? "" : "s"}` : "", agent?.activities?.workflows ? `${agent.activities.workflows} wf` : ""].filter(Boolean).join(" · ")}
              {" · "}
            </button>
          )}
          {busy ? (
            <>
              <span className="working">
                {workSecs !== null ? `working ${workSecs}s` : "working…"}
              </span>
              {lastTool && <span className="status-tool">· last tool {lastTool}</span>}
              {statusNote && <span className="status-note">{statusNote}</span>}
              <button
                className="btn-interrupt"
                onClick={() => void interrupt()}
                disabled={interrupting}
              >
                {interrupting ? "interrupting…" : "interrupt"}
              </button>
            </>
          ) : (
            <>
              <span className="dim">{lastTurn ? `last turn: ${lastTurn.subtype}` : "idle"}</span>
              {statusNote && <span className="status-note">{statusNote}</span>}
              {agent?.spawn_note && <span className="status-note" title={agent.spawn_note}>forked — {agent.spawn_note.split(" — ")[0]}</span>}
            </>
          )}
        </span>
        {actionError && <span className="error-text">{actionError}</span>}
        <UsagePopover agent={name} liveCost={lastTurn?.costUsd ?? null} />
      </div>

      {!subagent && <div className="composer">
        {acOpen && (
          <div className="ac-pop" role="listbox" aria-label="slash commands">
            <div className="ac-list">
            {acMatches.map((c, i) => (
              <div
                key={c.name}
                role="option"
                aria-selected={i === acIdx}
                className={i === acIdx ? "ac-item sel" : "ac-item"}
                ref={(el) => {
                  // Arrow keys move the selection; keep it visible in the
                  // scrolling list.
                  if (el && i === acIdx) el.scrollIntoView({ block: "nearest" });
                }}
                onMouseDown={(e) => {
                  e.preventDefault();
                  pickCommand(c);
                }}
                onMouseEnter={() => setAcIdx(i)}
              >
                <span className="mono ac-name">{c.name.startsWith("$") ? c.name : `/${c.name}`}</span>
                {c.argumentHint && <span className="mono ac-args">{c.argumentHint}</span>}
                {c.description && <span className="ac-desc">{c.description}</span>}
              </div>
            ))}
            </div>
            <div className="ac-foot mono-meta">
              {acMatches.length} command{acMatches.length === 1 ? "" : "s"} · ↑↓ move · Tab/Enter pick · Esc close
            </div>
          </div>
        )}
        {attachments.length > 0 && (
          <div className="attach-row">
            {attachments.map((a) => (
              <span key={a.n} className={`attach-chip${a.size > ATTACH_MAX ? " over" : ""}`} title={`${a.name} · ${Math.round(a.size / 1024)} KB · marker [attachment ${a.n}: ${a.name}]`}>
                {a.preview ? <img src={a.preview} alt="" /> : <span className="attach-icon">📄</span>}
                <span className="mono attach-name">{a.n}: {a.name}</span>
                <span className="mono-meta">{Math.round(a.size / 1024)} KB</span>
                <button className="attach-x" onClick={() => removeAttachment(a.n)} title="remove">×</button>
              </span>
            ))}
            {attachOver && <span className="error-text mono-meta">over the cap (8 MB each, 24 MB total)</span>}
          </div>
        )}
        <textarea
          ref={composerRef}
          value={draft}
          onChange={(e) => onDraftChange(e.target.value)}
          onKeyDown={composerKeyDown}
          onPaste={onComposerPaste}
          onDrop={onComposerDrop}
          onDragOver={(e) => e.preventDefault()}
          placeholder={
            exited
              ? "session exited"
              : busy
                ? agent?.harness === "codex"
                  ? "working… Enter steers this turn, Shift+Enter for a newline"
                  : "working… Enter queues for the next turn, Shift+Enter for a newline"
                : `message @${name} — Enter sends, Shift+Enter for a newline, / for commands`
          }
          disabled={composerDisabled}
          rows={2}
          autoFocus={!pane || pane.focused}
        />
        <button
          onClick={() => void send()}
          disabled={composerDisabled || !draft.trim() || attachOver}
          title="Enter · paste or drop files to attach"
        >
          send
        </button>
      </div>}
    </div>
  );
}

export default function Session() {
  const { name, agentId } = useParams<{ name: string; agentId?: string }>();
  if (!name) return <div className="page">no session name.</div>;
  // Keyed so switching agents fully resets transcript + socket state.
  return <SessionView key={`${name}:${agentId ?? ""}`} name={name} subagent={agentId} />;
}
