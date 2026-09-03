// Now — the operator's home. Two bands, one question each:
//
//   NEEDS YOU   everything waiting on a decision, answerable in place:
//               permission gates, questions, operator mail, agents that
//               just finished an ask (your cue to give the next one),
//               agents blocked on a peer, and exits.
//   THE FLEET   every live agent as a WORK card — what it was asked, what
//               it's doing right now and for how long, what it has changed,
//               how far through its context, what it cost. Presence is a
//               byte; this is the story. Long-idle and dead agents fold
//               into a strip at the bottom.
//
// This replaces Command, Sessions, and the rail's fleet list.

import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  api,
  type Activity,
  type Agent,
  type BusMessage,
  type Needs,
  type Urgency,
} from "../api";
import { usePoll } from "../hooks";
import { useAppData } from "../App";
import { useHotkeys } from "../hotkeys";
import { useTrustedStart } from "../trust";
import { NewSessionPanel } from "../sessionStart";
import { NodeChip, PermCard, QuestionCard } from "../needs";
import {
  ClassBadge,
  ClassSelect,
  Empty,
  ErrorBar,
  MessageRow,
  Meter,
  presenceOf,
  relTime,
} from "../components";
import "./command.css";
import "./now.css";

const FINISHED_WINDOW_S = 30 * 60;
const LONG_IDLE_S = 2 * 60 * 60;

function fmtDur(s: number): string {
  if (s < 60) return `${Math.max(0, Math.round(s))}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d`;
}

function ctxPct(a: Agent): number | null {
  const s = a.summary;
  if (!s?.context_tokens) return null;
  const win = s.context_window ?? 200000;
  return Math.min(100, Math.round((s.context_tokens / win) * 100));
}

/** Derived cue: finished the last ask and went idle recently, with no
 *  newer ask since — the operator's moment to direct. */
function finishedRecently(a: Agent, now: number): boolean {
  const s = a.summary;
  if (!a.live || a.turn_state === "busy" || !s?.idle_since || !s.last_ask_at) return false;
  if (s.idle_since < s.last_ask_at) return false;
  return now - s.idle_since < FINISHED_WINDOW_S;
}

function GitChip({ a }: { a: Agent }) {
  const g = a.git;
  if (!g) return null;
  return (
    <span
      className="mono-meta"
      title={`git: ${g.branch ?? "detached"} · ${g.dirty} changed · +${g.ahead}/−${g.behind}`}
    >
      ⎇ {g.branch ?? "detached"}
      {g.dirty > 0 && <span style={{ color: "var(--sig-normal)" }}> · {g.dirty} changed</span>}
      {(g.ahead > 0 || g.behind > 0) && ` · +${g.ahead}/−${g.behind}`}
    </span>
  );
}

function WorkCard({
  a,
  now,
  onOpen,
  onInterrupt,
  onStop,
}: {
  a: Agent;
  now: number;
  onOpen: () => void;
  onInterrupt: () => void;
  onStop: () => void;
}) {
  const p = presenceOf(a.live, a.turn_state);
  const s = a.summary;
  const pct = ctxPct(a);
  const busyFor = a.busy_since != null ? now - a.busy_since : null;
  const idleFor = s?.idle_since != null ? now - s.idle_since : null;
  const state =
    a.turn_state === "busy"
      ? `${a.last_tool ?? "thinking"}${busyFor != null ? ` · ${fmtDur(busyFor)}` : ""}`
      : idleFor != null
        ? `idle ${fmtDur(idleFor)}`
        : "idle";
  return (
    <div
      className={`work-card ${p}`}
      role="link"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter") onOpen();
      }}
    >
      <div className="wc-head">
        <Meter presence={p} />
        <span className="wc-name mono">@{a.bare ?? a.name}</span>
        {a.title && <span className="wc-title">{a.title}</span>}
        <span className="mono-meta">#{a.channel}</span>
        {a.node && a.remote && <NodeChip node={a.node} />}
        <span style={{ flex: 1 }} />
        <span className={`wc-state ${p}`}>{state}</span>
        {a.pending > 0 && (
          <span className="chip mono" style={{ color: "var(--sig-gate)" }}>
            {a.pending} pending
          </span>
        )}
      </div>
      <GitChip a={a} />
      <div className="wc-story">
        {s?.last_ask ? (
          <div className="wc-ask">
            <span className="wc-k">asked</span> {s.last_ask}
          </div>
        ) : (
          <div className="wc-ask dim">no ask this process yet</div>
        )}
        {s?.last_reply && a.turn_state !== "busy" && (
          <div className="wc-reply">
            <span className="wc-k">said</span> {s.last_reply}
          </div>
        )}
      </div>
      <div className="wc-stats">
        <span title="turns this process">{s?.turns ?? 0} turns</span>
        {pct !== null && (
          <span title={`~${s?.context_tokens?.toLocaleString()} tokens in last request`}>
            <span className="ctx-bar" aria-hidden>
              <span style={{ width: `${pct}%` }} />
            </span>
            ctx {pct}%
          </span>
        )}
        {s?.cost_usd != null && <span>${s.cost_usd.toFixed(2)}</span>}
        {s && s.files_touched > 0 && (
          <span title={s.files.join("\n")} style={{ color: "var(--sig-normal)" }}>
            {s.files_touched} file{s.files_touched === 1 ? "" : "s"} touched
          </span>
        )}
        <span style={{ flex: 1 }} />
        {a.turn_state === "busy" && (
          <button
            className="btn ghost sm"
            onClick={(e) => {
              e.stopPropagation();
              onInterrupt();
            }}
          >
            interrupt
          </button>
        )}
        <button
          className="btn ghost sm"
          onClick={(e) => {
            e.stopPropagation();
            onStop();
          }}
        >
          stop
        </button>
      </div>
    </div>
  );
}

export default function Now() {
  const nav = useNavigate();
  const { agents, refreshAgents, refreshInbox } = useAppData();
  const activityPoll = usePoll<Activity>(api.activity, 3000);
  const needsPoll = usePoll<Needs>(api.needs, 2000);
  const trust = useTrustedStart();
  const [panelOpen, setPanelOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [replyText, setReplyText] = useState("");
  const [replyClass, setReplyClass] = useState<Urgency>("normal");
  const [showFolded, setShowFolded] = useState(false);
  const now = Date.now() / 1000;

  useHotkeys("now", [
    { key: "n", description: "new session", handler: () => setPanelOpen(true) },
    { key: "/", description: "filter the fleet", handler: () => document.getElementById("now-filter")?.focus() },
  ]);

  const prompts = needsPoll.data?.prompts ?? [];
  const inbox = needsPoll.data?.inbox ?? [];
  const waiting = activityPoll.data?.waiting ?? [];

  const q = query.trim().toLowerCase();
  const match = (a: Agent) =>
    q === "" ||
    a.name.toLowerCase().includes(q) ||
    a.channel.toLowerCase().includes(q) ||
    (a.title ?? "").toLowerCase().includes(q);

  const { active, folded } = useMemo(() => {
    const active: Agent[] = [];
    const folded: Agent[] = [];
    for (const a of agents.filter(match)) {
      const idleFor = a.summary?.idle_since != null ? now - a.summary.idle_since : null;
      if (!a.live || (idleFor != null && idleFor > LONG_IDLE_S)) folded.push(a);
      else active.push(a);
    }
    const rank = (a: Agent) => (a.turn_state === "busy" ? 0 : a.pending > 0 ? 1 : 2);
    active.sort((x, y) => rank(x) - rank(y) || x.name.localeCompare(y.name));
    return { active, folded };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents, q]);

  const finished = active.filter((a) => finishedRecently(a, now));
  const exited = agents.filter(
    (a) => !a.live && a.last_exit_at != null && now - a.last_exit_at < FINISHED_WINDOW_S,
  );
  const needsCount = prompts.length + inbox.length + finished.length + waiting.length + exited.length;

  async function sendReply(to: string) {
    if (!replyText.trim()) return;
    setErr(null);
    try {
      await api.busSend({ to: `@${to}`, body: replyText.trim(), urgency: replyClass });
      setReplyTo(null);
      setReplyText("");
      await refreshInbox();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "send failed");
    }
  }

  async function act(label: string, f: () => Promise<unknown>) {
    setErr(null);
    try {
      await f();
      await refreshAgents();
    } catch (e) {
      setErr(`${label}: ${e instanceof Error ? e.message : "failed"}`);
    }
  }

  const busy = agents.filter((a) => a.live && a.turn_state === "busy").length;
  const idle = agents.filter((a) => a.live && a.turn_state !== "busy").length;

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Now</span>
        <span className="mono-meta">
          {busy} busy · {idle} idle · {agents.length - busy - idle} down
          {needsCount > 0 && (
            <span style={{ color: "var(--sig-gate)" }}> · {needsCount} need you</span>
          )}
        </span>
        <span style={{ flex: 1 }} />
        <input
          id="now-filter"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="filter @name, #repo, title"
          className="mono"
          style={{ width: 240 }}
          aria-label="filter fleet"
        />
        <button className="btn primary sm" onClick={() => setPanelOpen(true)}>
          + new session
        </button>
      </div>

      <div className="stage-body">
        {trust.dialog}
        <ErrorBar error={err} />
        {panelOpen && (
          <NewSessionPanel
            startFn={trust.start}
            repoPaths={[]}
            existing={agents.map((a) => a.name)}
            onClose={() => setPanelOpen(false)}
            onStarted={async (name) => {
              setPanelOpen(false);
              await refreshAgents();
              nav(`/session/${encodeURIComponent(name)}`);
            }}
          />
        )}

        {/* ─────────────────────────── NEEDS YOU ─────────────────────────── */}
        <section className="now-band">
          <div className="now-band-head">
            <span className="label">Needs you</span>
            <span className="mono-meta">{needsCount === 0 ? "nothing — the fleet is running itself" : `${needsCount}`}</span>
          </div>
          {prompts.map((p) =>
            p.is_question ? (
              <QuestionCard key={`${p.agent}:${p.request_id}`} prompt={p} onAnswered={() => void needsPoll.refresh()} />
            ) : (
              <PermCard key={`${p.agent}:${p.request_id}`} prompt={p} onAnswered={() => void needsPoll.refresh()} />
            ),
          )}
          {inbox.map((m: BusMessage) => (
            <div key={m.id} className="need-card">
              <MessageRow
                urgency={m.urgency}
                sender={m.sender}
                meta={<span className="mono-meta">{relTime(m.created_at)} ago</span>}
              >
                {m.body}
              </MessageRow>
              {replyTo === m.sender ? (
                <div className="need-reply">
                  <ClassSelect value={replyClass} onChange={setReplyClass} />
                  <input
                    value={replyText}
                    onChange={(e) => setReplyText(e.target.value)}
                    placeholder={`reply to @${m.sender}`}
                    autoFocus
                    onKeyDown={(e) => e.key === "Enter" && void sendReply(m.sender)}
                    style={{ flex: 1 }}
                  />
                  <button className="btn sm" onClick={() => void sendReply(m.sender)}>send</button>
                  <button className="btn ghost sm" onClick={() => setReplyTo(null)}>cancel</button>
                </div>
              ) : (
                <div className="need-actions">
                  <ClassBadge urgency={m.urgency} />
                  <span style={{ flex: 1 }} />
                  <button className="btn sm" onClick={() => setReplyTo(m.sender)}>reply</button>
                  <button className="btn ghost sm" onClick={() => nav(`/session/${encodeURIComponent(m.sender)}`)}>open</button>
                </div>
              )}
            </div>
          ))}
          {waiting.map((w) => (
            <div key={`${w.agent}:${w.on}`} className="need-card need-cue">
              <span className="chip mono" style={{ color: "var(--sig-normal)" }}>blocked</span>
              <span className="mono">@{w.agent}</span>
              <span className="dim">is likely waiting on</span>
              <span className="mono">@{w.on}</span>
              <span className="mono-meta">{relTime(w.since)} ago · “{w.snippet}”</span>
              <span style={{ flex: 1 }} />
              <button className="btn ghost sm" onClick={() => nav(`/session/${encodeURIComponent(w.on)}`)}>open @{w.on.split("@")[0]}</button>
            </div>
          ))}
          {finished.map((a) => (
            <div key={`fin:${a.name}`} className="need-card need-cue">
              <span className="chip mono" style={{ color: "var(--live)" }}>finished</span>
              <span className="mono">@{a.bare ?? a.name}</span>
              <span className="dim">done with</span>
              <span className="wc-ask" style={{ flex: 1, minWidth: 0 }}>{a.summary?.last_ask}</span>
              <span className="mono-meta">{relTime(a.summary!.idle_since!)} ago</span>
              <button className="btn sm" onClick={() => nav(`/session/${encodeURIComponent(a.name)}`)}>direct</button>
            </div>
          ))}
          {exited.map((a) => (
            <div key={`exit:${a.name}`} className="need-card need-cue">
              <span className="chip mono" style={{ color: "var(--sig-gate)" }}>exited</span>
              <span className="mono">@{a.bare ?? a.name}</span>
              <span className="dim">
                {a.last_exit_code === 0 || a.last_exit_code == null ? "ended" : `code ${a.last_exit_code}`} ·{" "}
                {relTime(a.last_exit_at!)} ago
              </span>
              <span style={{ flex: 1 }} />
              <button className="btn sm" onClick={() => void act("revive", () => api.revive(a.name))}>revive</button>
            </div>
          ))}
        </section>

        {/* ─────────────────────────── THE FLEET ─────────────────────────── */}
        <section className="now-band">
          <div className="now-band-head">
            <span className="label">The fleet</span>
            <span className="mono-meta">{active.length} active</span>
          </div>
          {agents.length === 0 ? (
            <Empty mark="◦">
              No sessions in the mesh yet.{" "}
              <button className="btn ghost sm" onClick={() => setPanelOpen(true)}>+ new session</button>
            </Empty>
          ) : active.length === 0 ? (
            <Empty mark="—">{q ? `Nothing active matches “${query}”.` : "Nothing active right now."}</Empty>
          ) : (
            <div className="work-grid">
              {active.map((a) => (
                <WorkCard
                  key={a.name}
                  a={a}
                  now={now}
                  onOpen={() => nav(`/session/${encodeURIComponent(a.name)}`)}
                  onInterrupt={() => void act("interrupt", () => api.interrupt(a.name))}
                  onStop={() => void act("stop", () => api.deleteAgent(a.name))}
                />
              ))}
            </div>
          )}
          {folded.length > 0 && (
            <div className="now-folded">
              <button className="btn ghost sm" onClick={() => setShowFolded((v) => !v)}>
                {showFolded ? "▾" : "▸"} {folded.length} quiet or down
              </button>
              {showFolded &&
                folded.map((a) => {
                  const p = presenceOf(a.live, a.turn_state);
                  return (
                    <div key={a.name} className="folded-row">
                      <Meter presence={p} />
                      <span className="mono">@{a.bare ?? a.name}</span>
                      <span className="mono-meta">#{a.channel}{a.node && a.remote ? ` · ${a.node}` : ""}</span>
                      <span className="mono-meta">
                        {a.live
                          ? `idle ${fmtDur(now - (a.summary?.idle_since ?? now))}`
                          : a.last_exit_at
                            ? `down ${relTime(a.last_exit_at)}`
                            : "down"}
                      </span>
                      <span style={{ flex: 1 }} />
                      <button className="btn ghost sm" onClick={() => nav(`/session/${encodeURIComponent(a.name)}`)}>open</button>
                      {!a.live && !a.remote && (
                        <button className="btn sm" onClick={() => void act("revive", () => api.revive(a.name))}>revive</button>
                      )}
                    </div>
                  );
                })}
            </div>
          )}
        </section>
      </div>
    </>
  );
}
