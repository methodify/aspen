import {
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { Link, useParams } from "react-router-dom";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, sessionEventsUrl, type PermissionAnswer, type RuntimeInfo } from "./../api";
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
} from "./../transcript";
import { useAppData } from "./../App";
import { Meter, presenceOf } from "./../components";
import { useHotkeys } from "./../hotkeys";
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

type Action =
  | { type: "seed"; state: TranscriptState }
  | { type: "event"; ev: SessionEvent }
  | { type: "local_send"; text: string; localKey: string }
  | { type: "sent"; localKey: string; uuid: string }
  | { type: "send_failed"; localKey: string };

function reducer(state: TranscriptState, action: Action): TranscriptState {
  switch (action.type) {
    case "seed":
      return action.state;
    case "event":
      return applyEvent(state, action.ev);
    case "local_send":
      return addLocalUserMessage(state, action.text, action.localKey);
    case "sent":
      return markLocalUserMessage(state, action.localKey, { uuid: action.uuid });
    case "send_failed":
      return markLocalUserMessage(state, action.localKey, { failed: true });
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

function Md({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}

/**
 * Markdown rendered the way a terminal renders it — the claude TUI look.
 * One monospace size throughout; structure carried by weight, color, and
 * character prefixes (• bullets, │ quotes, ─ rules), never by font size.
 */
function TuiMd({ text }: { text: string }) {
  return (
    <div className="tui-md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          h1: ({ children }) => <div className="tui-h tui-h1">{children}</div>,
          h2: ({ children }) => <div className="tui-h tui-h2">{children}</div>,
          h3: ({ children }) => <div className="tui-h">{children}</div>,
          h4: ({ children }) => <div className="tui-h">{children}</div>,
          h5: ({ children }) => <div className="tui-h">{children}</div>,
          h6: ({ children }) => <div className="tui-h">{children}</div>,
          hr: () => <div className="tui-hr" aria-hidden />,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}

function AssistantBubble({ item, source }: { item: AssistantBubbleItem; source?: boolean }) {
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
          <Md text={item.text} />
        )
      ) : (
        item.open && <span className="dim">…</span>
      )}
      {item.open && item.text && <span className="caret" aria-hidden="true" />}
    </div>
  );
}

function UserBubble({ item }: { item: UserBubbleItem }) {
  return (
    <div className="bubble bubble-user">
      <div className="bubble-tag mono">@operator</div>
      <div className="user-text">{item.text}</div>
      {item.pending && !item.failed && <div className="bubble-note dim">sending…</div>}
      {item.failed && <div className="bubble-note error-text">send failed — not delivered</div>}
    </div>
  );
}

function BusBubble({ item, source }: { item: BusBubbleItem; source?: boolean }) {
  const nl = item.text.indexOf("\n");
  const header = nl >= 0 ? item.text.slice(0, nl) : item.text;
  const body = nl >= 0 ? item.text.slice(nl + 1) : "";
  return (
    <div className="bubble bubble-bus">
      <div className="bus-header mono">{header}</div>
      {body && (source ? <pre className="src-body">{body}</pre> : <Md text={body} />)}
    </div>
  );
}

function ToolCard({ item }: { item: ToolCardItem }) {
  const expandable = item.input !== null || item.result !== null;
  const summary = toolSummary(item.input);
  if (!expandable) {
    // Rehydrated history carries only { id, name }: render a plain chip.
    return (
      <div className="tool-chip">
        <span className="mono">{item.name}</span>
      </div>
    );
  }
  return (
    <details className="tool-card">
      <summary>
        <span
          className={
            !item.done ? "dot dot-busy" : item.isError ? "dot dot-error" : "dot dot-idle"
          }
        />
        <span className="tool-name mono">{item.name}</span>
        {summary && <span className="tool-summary mono">{summary}</span>}
        {!item.done && <span className="chip chip-busy">running</span>}
        {item.isError && <span className="chip chip-error">error</span>}
      </summary>
      <div className="tool-detail">
        <div className="tool-section-label">input</div>
        <pre>{JSON.stringify(item.input, null, 2)}</pre>
        {item.result !== null && (
          <>
            <div className="tool-section-label">result</div>
            <pre>{item.result}</pre>
          </>
        )}
        {item.result === null && item.done && (
          <div className="dim tool-section-label">no result recorded (turn ended)</div>
        )}
      </div>
    </details>
  );
}

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

function TurnEndMarker({ item }: { item: TurnEndItem }) {
  return (
    <div className="turn-end mono">
      turn ended · {item.subtype}
      {item.durationMs !== null && <> · {(item.durationMs / 1000).toFixed(1)}s</>}
      {item.costUsd !== null && <> · session ${item.costUsd.toFixed(2)}</>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The page

function SessionView({ name }: { name: string }) {
  const { agents, agentsLoaded, refreshAgents } = useAppData();
  const agent = agents.find((a) => a.name === name);

  const [transcript, dispatch] = useReducer(reducer, undefined, emptyTranscript);
  const [wsState, setWsState] = useState<WsState>("connecting");
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [interrupting, setInterrupting] = useState(false);
  const [lastTurn, setLastTurn] = useState<TurnInfo | null>(null);
  const [exited, setExited] = useState<{ code: number | null } | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [reloading, setReloading] = useState(false);
  const [reloadNote, setReloadNote] = useState<string | null>(null);
  const [confirmStop, setConfirmStop] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [reviving, setReviving] = useState(false);

  // Liveness truth comes from the polled roster, not just the WS `exited`
  // event — a page that loads (or reconnects) after the process died would
  // otherwise never learn it and strand the operator with dead controls.
  // The roster also clears the banner when the session is revived from
  // another surface. (Remote agents on a down node are excluded: their
  // `live=false` means "unreachable", not "exited".)
  useEffect(() => {
    if (!agentsLoaded || !agent || agent.remote) return;
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
  const [ctlNote, setCtlNote] = useState<string | null>(null);
  const [ctlError, setCtlError] = useState<string | null>(null);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [localLastTool, setLocalLastTool] = useState<string | null>(null);
  const [nowTick, setNowTick] = useState(() => Date.now());
  const [titleEditing, setTitleEditing] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [titleOverride, setTitleOverride] = useState<string | null | undefined>(undefined);
  const [charterOpen, setCharterOpen] = useState(false);
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
      const pm = inv?.["permissionMode"] ?? inv?.["permission_mode"];
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
  const ctxSeededRef = useRef(false);
  useEffect(() => {
    if (ctxSeededRef.current || !agent) return;
    ctxSeededRef.current = true;
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
    let ws: WebSocket | null = null;
    let timer: number | undefined;
    let attempt = 0;

    function connect() {
      if (disposed) return;
      ws = new WebSocket(sessionEventsUrl(name));
      ws.onopen = () => {
        if (disposed) return;
        attempt = 0;
        setWsState("open");
      };
      ws.onmessage = (e: MessageEvent) => {
        if (disposed) return;
        const ev = parseSessionEvent(e.data);
        if (ev) handleEventRef.current(ev);
      };
      ws.onclose = () => {
        if (disposed) return;
        setWsState("reconnecting");
        attempt += 1;
        const delay = Math.min(15000, 1000 * 2 ** Math.min(attempt - 1, 4));
        timer = window.setTimeout(connect, delay);
      };
      ws.onerror = () => {
        ws?.close();
      };
    }

    async function start() {
      try {
        const history = await api.transcript(name);
        if (disposed) return;
        dispatch({ type: "seed", state: seedFromHistory(history) });
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
      if (timer !== undefined) window.clearTimeout(timer);
      ws?.close();
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
    if (!text || busy || exited) return;
    const localKey = crypto.randomUUID();
    dispatch({ type: "local_send", text, localKey });
    setDraft("");
    setBusy(true);
    busyLocalStartRef.current = Date.now();
    setNowTick(Date.now());
    setActionError(null);
    try {
      const { uuid } = await api.sendMessage(name, text);
      dispatch({ type: "sent", localKey, uuid });
    } catch (e) {
      dispatch({ type: "send_failed", localKey });
      setBusy(false);
      busyLocalStartRef.current = null;
      setActionError(`send: ${errText(e)}`);
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

  async function reviveSession() {
    setReviving(true);
    setActionError(null);
    try {
      await api.revive(name);
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
  const slashPartial = slashPartialOf(draft);
  const acMatches = useMemo(
    () => (slashPartial !== null && !acDismissed ? filterSlashCommands(commands, slashPartial) : []),
    [commands, slashPartial, acDismissed],
  );
  const acOpen = acMatches.length > 0;

  useEffect(() => {
    setAcIdx(0);
  }, [slashPartial]);

  function pickCommand(cmd: SlashCommand | undefined) {
    if (!cmd) return;
    setDraft(`/${cmd.name} `);
    setAcDismissed(false);
  }

  function onDraftChange(v: string) {
    setDraft(v);
    if (acDismissed && slashPartialOf(v) === null) setAcDismissed(false);
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
            {item.text ? <TuiMd text={item.text} /> : "…"}
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
      case "bus": {
        // Keep the [aspen bus] header line as raw terminal text; the body
        // is agent prose and renders as TUI markdown like everything else.
        const nl = item.text.indexOf("\n");
        const header = nl >= 0 ? item.text.slice(0, nl) : item.text;
        const body = nl >= 0 ? item.text.slice(nl + 1) : "";
        return (
          <div key={item.id} className="cline cline-bus">
            <div>{header}</div>
            {body && <TuiMd text={body} />}
          </div>
        );
      }
      case "tool":
        return (
          <div key={item.id} className="cline cline-tool">
            [tool] {item.name}
          </div>
        );
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

  function renderItem(item: TranscriptItem) {
    if (renderMode === "console") return renderConsoleItem(item);
    const source = renderMode === "source";
    switch (item.kind) {
      case "assistant":
        return <AssistantBubble key={item.id} item={item} source={source} />;
      case "user":
        return <UserBubble key={item.id} item={item} />;
      case "bus":
        return <BusBubble key={item.id} item={item} source={source} />;
      case "tool":
        return <ToolCard key={item.id} item={item} />;
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

  const composerDisabled = busy || exited !== null;

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
    <div className="session">
      <header className="session-head">
        {agent && <Meter presence={presenceOf(agent.live, agent.turn_state)} />}
        <h1>
          <span className="mono">@{name}</span>
        </h1>
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
          {wsState === "open" ? "live" : wsState}
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
            {PERMISSION_MODES.map((m) => (
              <option key={m} value={m}>
                {m}
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

      {exited && (
        <div className="exited-banner">
          <span>
            session exited{exited.code !== null ? ` (code ${exited.code})` : ""} — the
            conversation is on disk and can continue.
          </span>
          <button
            className="btn primary sm"
            disabled={reviving}
            onClick={() => void reviveSession()}
          >
            {reviving ? "resuming…" : `resume @${name}`}
          </button>
          <span className="mono-meta">
            or from <Link to="/sessions">Sessions</Link> / <Link to="/library">Library</Link>
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
        {transcript.items.map(renderItem)}
      </div>

      <div className="status-line">
        <span className="status-left mono">
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
            </>
          )}
        </span>
        {actionError && <span className="error-text">{actionError}</span>}
        <span className="status-right mono" title="session-cumulative cost, not per-turn">
          {lastTurn?.costUsd !== null && lastTurn?.costUsd !== undefined
            ? `session $${lastTurn.costUsd.toFixed(2)}`
            : "session $—"}
        </span>
      </div>

      <div className="composer">
        {acOpen && (
          <div className="ac-pop" role="listbox" aria-label="slash commands">
            {acMatches.map((c, i) => (
              <div
                key={c.name}
                role="option"
                aria-selected={i === acIdx}
                className={i === acIdx ? "ac-item sel" : "ac-item"}
                onMouseDown={(e) => {
                  e.preventDefault();
                  pickCommand(c);
                }}
                onMouseEnter={() => setAcIdx(i)}
              >
                <span className="mono ac-name">/{c.name}</span>
                {c.argumentHint && <span className="mono ac-args">{c.argumentHint}</span>}
                {c.description && <span className="ac-desc">{c.description}</span>}
              </div>
            ))}
          </div>
        )}
        <textarea
          value={draft}
          onChange={(e) => onDraftChange(e.target.value)}
          onKeyDown={composerKeyDown}
          placeholder={
            exited
              ? "session exited"
              : busy
                ? "working… unlocks at turn end"
                : `message @${name} — Enter sends, Shift+Enter for a newline, / for commands`
          }
          disabled={composerDisabled}
          rows={2}
          autoFocus
        />
        <button
          onClick={() => void send()}
          disabled={composerDisabled || !draft.trim()}
          title="Enter"
        >
          send
        </button>
      </div>
    </div>
  );
}

export default function Session() {
  const { name } = useParams<{ name: string }>();
  if (!name) return <div className="page">no session name.</div>;
  // Keyed so switching agents fully resets transcript + socket state.
  return <SessionView key={name} name={name} />;
}
