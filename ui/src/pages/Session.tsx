import {
  useEffect,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { Link, useParams } from "react-router-dom";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, sessionEventsUrl } from "./../api";
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

// ---------------------------------------------------------------------------
// Item renderers

function Md({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}

function AssistantBubble({ item }: { item: AssistantBubbleItem }) {
  return (
    <div className="bubble bubble-assistant">
      {item.thinking && (
        <details className="thinking">
          <summary>thinking</summary>
          <div className="thinking-body">{item.thinking}</div>
        </details>
      )}
      {item.text ? (
        <Md text={item.text} />
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

function BusBubble({ item }: { item: BusBubbleItem }) {
  const nl = item.text.indexOf("\n");
  const header = nl >= 0 ? item.text.slice(0, nl) : item.text;
  const body = nl >= 0 ? item.text.slice(nl + 1) : "";
  return (
    <div className="bubble bubble-bus">
      <div className="bus-header mono">{header}</div>
      {body && <Md text={body} />}
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

function PermissionCard({
  item,
  onAnswer,
}: {
  item: PermissionCardItem;
  onAnswer: (allow: boolean, message?: string) => Promise<boolean>;
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
  const { agents } = useAppData();
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
          setLastTurn({
            subtype: typeof ev.subtype === "string" ? ev.subtype : "success",
            costUsd: typeof ev.total_cost_usd === "number" ? ev.total_cost_usd : null,
            durationMs: typeof ev.duration_ms === "number" ? ev.duration_ms : null,
          });
          break;
        case "exited":
          setExited({ code: ev.code ?? null });
          setBusy(false);
          setInterrupting(false);
          break;
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
    setActionError(null);
    try {
      const { uuid } = await api.sendMessage(name, text);
      dispatch({ type: "sent", localKey, uuid });
    } catch (e) {
      dispatch({ type: "send_failed", localKey });
      setBusy(false);
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

  async function reload() {
    if (reloading) return;
    setReloading(true);
    setReloadNote(null);
    setActionError(null);
    try {
      await api.reloadAgent(name);
      setReloadNote("skills reloaded");
      window.setTimeout(() => setReloadNote(null), 3000);
    } catch (e) {
      setActionError(`reload: ${errText(e)}`);
    } finally {
      setReloading(false);
    }
  }

  async function answerPermission(
    requestId: string,
    allow: boolean,
    message?: string,
  ): Promise<boolean> {
    setActionError(null);
    try {
      await api.answerPermission(
        name,
        requestId,
        allow ? { allow: true } : { allow: false, ...(message ? { message } : {}) },
      );
      // Optimistic settle; the WS permission_settled is idempotent on top.
      dispatch({
        type: "event",
        ev: { kind: "permission_settled", request_id: requestId, allow },
      });
      return true;
    } catch (e) {
      setActionError(`permission: ${errText(e)}`);
      return false;
    }
  }

  function renderItem(item: TranscriptItem) {
    switch (item.kind) {
      case "assistant":
        return <AssistantBubble key={item.id} item={item} />;
      case "user":
        return <UserBubble key={item.id} item={item} />;
      case "bus":
        return <BusBubble key={item.id} item={item} />;
      case "tool":
        return <ToolCard key={item.id} item={item} />;
      case "permission":
        return (
          <PermissionCard
            key={item.id}
            item={item}
            onAnswer={(allow, message) => answerPermission(item.requestId, allow, message)}
          />
        );
      case "turn_end":
        return <TurnEndMarker key={item.id} item={item} />;
    }
  }

  function composerKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }

  const composerDisabled = busy || exited !== null;

  return (
    <div className="session">
      <header className="session-head">
        <h1>
          <span className="mono">@{name}</span>
        </h1>
        {agent && (
          <span className="dim mono session-repo">
            {agent.repo ?? `node ${agent.node}`} · #{agent.channel}
          </span>
        )}
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
        <span className={`ws-state ws-${wsState}`}>
          <span
            className={wsState === "open" ? "dot dot-idle" : "dot dot-down"}
            aria-hidden="true"
          />
          {wsState === "open" ? "live" : wsState}
        </span>
      </header>

      {historyError && (
        <div className="error-inline">history: {historyError} (live stream only)</div>
      )}

      {exited && (
        <div className="exited-banner">
          session exited{exited.code !== null ? ` (code ${exited.code})` : ""} — start{" "}
          <span className="mono">@{name}</span> again from the <Link to="/">Mesh</Link> page
          {agent ? (
            <>
              {" "}
              with resume session id <span className="mono">{agent.session_id}</span>
            </>
          ) : (
            " with its resume session id"
          )}{" "}
          to continue.
        </div>
      )}

      <div
        className="transcript"
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
              <span className="working">working…</span>
              <button
                className="btn-interrupt"
                onClick={() => void interrupt()}
                disabled={interrupting}
              >
                {interrupting ? "interrupting…" : "interrupt"}
              </button>
            </>
          ) : (
            <span className="dim">{lastTurn ? `last turn: ${lastTurn.subtype}` : "idle"}</span>
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
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={composerKeyDown}
          placeholder={
            exited
              ? "session exited"
              : busy
                ? "working… unlocks at turn end"
                : `message @${name} — Enter sends, Shift+Enter for a newline`
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
