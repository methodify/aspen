// Command — the mesh-wide triage cockpit. NEEDS YOU (every open permission
// gate, structured question, and piece of operator mail across the mesh,
// actionable in place, oldest first) over WAITING (likely-blocked agents)
// over IN FLIGHT (live presence + the recent bus traffic, filterable).

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import {
  api,
  type Activity,
  type BusMessage,
  type Needs,
  type OpenPrompt,
  type Urgency,
} from "../api";
import { usePoll } from "../hooks";
import { useAppData } from "../App";
import {
  Capsule,
  ClassBadge,
  ClassSelect,
  Empty,
  ErrorBar,
  Meter,
  MessageRow,
  presenceOf,
  relTime,
} from "../components";
import "./command.css";

/* ── AskUserQuestion input shape (§7.6) ─────────────────────────────── */

interface QOption {
  label: string;
  description?: string;
}
interface Question {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: QOption[];
}

function questionsOf(input: unknown): Question[] {
  if (!input || typeof input !== "object") return [];
  const raw = (input as { questions?: unknown }).questions;
  if (!Array.isArray(raw)) return [];
  const out: Question[] = [];
  for (const q of raw) {
    if (!q || typeof q !== "object") continue;
    const o = q as Record<string, unknown>;
    if (typeof o.question !== "string") continue;
    const options: QOption[] = [];
    if (Array.isArray(o.options)) {
      for (const op of o.options) {
        if (!op || typeof op !== "object") continue;
        const oo = op as Record<string, unknown>;
        if (typeof oo.label !== "string") continue;
        options.push({
          label: oo.label,
          ...(typeof oo.description === "string" ? { description: oo.description } : {}),
        });
      }
    }
    out.push({
      question: o.question,
      ...(typeof o.header === "string" ? { header: o.header } : {}),
      ...(typeof o.multiSelect === "boolean" ? { multiSelect: o.multiSelect } : {}),
      options,
    });
  }
  return out;
}

/* ── Collapsed key-field summary of a permission's tool input ───────── */

const KEY_FIELDS = ["file_path", "command", "path", "url", "pattern", "prompt", "description"];

function summarizeInput(input: unknown): { k: string; v: string }[] {
  if (!input || typeof input !== "object") return [];
  const obj = input as Record<string, unknown>;
  const out: { k: string; v: string }[] = [];
  for (const k of KEY_FIELDS) {
    const v = obj[k];
    if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
      out.push({ k, v: String(v) });
    }
  }
  if (out.length === 0) {
    for (const [k, v] of Object.entries(obj).slice(0, 3)) {
      if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
        out.push({ k, v: String(v) });
      }
    }
  }
  return out;
}

function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
}

function NodeChip({ node }: { node: string | null }) {
  if (!node) return null;
  return <span className="node-chip">{node}</span>;
}

/* ── Permission gate card ───────────────────────────────────────────── */

function PermCard({ prompt, onAnswered }: { prompt: OpenPrompt; onAnswered: () => void }) {
  const [expanded, setExpanded] = useState(false);
  const [denying, setDenying] = useState(false);
  const [denyMsg, setDenyMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const canAlways = Array.isArray(prompt.suggestions) && prompt.suggestions.length > 0;
  const summary = summarizeInput(prompt.input);

  async function answer(a: { allow: boolean; message?: string; updated_permissions?: unknown }) {
    setBusy(true);
    setErr(null);
    try {
      await api.answerPermission(prompt.agent, prompt.request_id, a);
      onAnswered();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "answer failed");
      setBusy(false);
    }
  }

  return (
    <div className="needs-card perm">
      <div className="needs-head">
        <span className="needs-kind">gate</span>
        <span className="mono" style={{ fontWeight: 600 }}>@{prompt.agent}</span>
        <NodeChip node={prompt.node} />
        <span className="mono" style={{ fontSize: 12 }}>{prompt.tool_name}</span>
        <span style={{ flex: 1 }} />
        <span className="mono-meta">asked {relTime(prompt.asked_at)} ago</span>
      </div>
      <div className="needs-body">
        {summary.length === 0 && !expanded ? (
          <div className="mono-meta">no summarizable input</div>
        ) : (
          !expanded &&
          summary.map(({ k, v }) => (
            <div key={k} className="perm-kv">
              <span className="k">{k}</span>
              <span className="v" title={v}>{v}</span>
            </div>
          ))
        )}
        {expanded && <pre className="needs-input-full">{prettyJson(prompt.input)}</pre>}
        <button className="btn ghost sm" style={{ marginTop: 6 }} onClick={() => setExpanded((x) => !x)}>
          {expanded ? "collapse" : "expand input"}
        </button>
        {err && <div className="error-bar" style={{ marginTop: 8 }}>{err}</div>}
        <div className="needs-actions">
          {denying ? (
            <>
              <input
                autoFocus
                className="deny-input"
                placeholder="why not — shown to the model"
                value={denyMsg}
                onChange={(e) => setDenyMsg(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && void answer({ allow: false, message: denyMsg.trim() || undefined })}
              />
              <button className="btn ghost sm" disabled={busy} onClick={() => setDenying(false)}>cancel</button>
              <button
                className="btn danger sm"
                disabled={busy}
                onClick={() => answer({ allow: false, message: denyMsg.trim() || undefined })}
              >
                deny
              </button>
            </>
          ) : (
            <>
              <button className="btn primary sm" disabled={busy} onClick={() => answer({ allow: true })}>
                allow
              </button>
              {canAlways && (
                <button
                  className="btn sm"
                  disabled={busy}
                  onClick={() => answer({ allow: true, updated_permissions: prompt.suggestions })}
                >
                  always allow
                </button>
              )}
              <button className="btn ghost sm" disabled={busy} onClick={() => setDenying(true)}>
                deny…
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/* ── Question card (AskUserQuestion) ────────────────────────────────── */

function QuestionCard({ prompt, onAnswered }: { prompt: OpenPrompt; onAnswered: () => void }) {
  const questions = useMemo(() => questionsOf(prompt.input), [prompt.input]);
  const [picks, setPicks] = useState<Record<string, string[]>>({});
  const [response, setResponse] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  function toggle(q: Question, label: string) {
    setPicks((p) => {
      const cur = p[q.question] ?? [];
      if (q.multiSelect) {
        return {
          ...p,
          [q.question]: cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label],
        };
      }
      return { ...p, [q.question]: cur.includes(label) ? [] : [label] };
    });
  }

  async function submit(skip: boolean) {
    setBusy(true);
    setErr(null);
    const answers: Record<string, string | string[]> = {};
    if (!skip) {
      for (const q of questions) {
        const sel = picks[q.question] ?? [];
        if (sel.length === 0) continue;
        answers[q.question] = q.multiSelect ? sel : (sel[0] as string);
      }
    }
    const echo =
      prompt.input && typeof prompt.input === "object"
        ? (prompt.input as { questions?: unknown }).questions
        : undefined;
    const updated_input: Record<string, unknown> = { questions: echo, answers };
    const free = response.trim();
    if (!skip && free) updated_input.response = free;
    try {
      await api.answerPermission(prompt.agent, prompt.request_id, { allow: true, updated_input });
      onAnswered();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "answer failed");
      setBusy(false);
    }
  }

  const anyPick = Object.values(picks).some((v) => v.length > 0) || response.trim().length > 0;

  return (
    <div className="needs-card question">
      <div className="needs-head">
        <span className="needs-kind">question</span>
        <span className="mono" style={{ fontWeight: 600 }}>@{prompt.agent}</span>
        <NodeChip node={prompt.node} />
        <span style={{ flex: 1 }} />
        <span className="mono-meta">asked {relTime(prompt.asked_at)} ago</span>
      </div>
      <div className="needs-body">
        {questions.length === 0 && <div className="mono-meta">malformed question payload</div>}
        {questions.map((q) => {
          const sel = picks[q.question] ?? [];
          return (
            <div key={q.question} className="q-block">
              {q.header && <div className="label" style={{ marginBottom: 4 }}>{q.header}</div>}
              <div className="q-text">{q.question}</div>
              <div className="q-options">
                {q.options.map((o) => (
                  <button
                    key={o.label}
                    type="button"
                    className="q-option"
                    aria-pressed={sel.includes(o.label)}
                    onClick={() => toggle(q, o.label)}
                  >
                    {o.label}
                    {o.description && <span className="q-desc">{o.description}</span>}
                  </button>
                ))}
              </div>
              {q.multiSelect && <div className="mono-meta" style={{ marginTop: 4 }}>multi-select</div>}
            </div>
          );
        })}
        <textarea
          className="q-response"
          rows={2}
          placeholder="optional free-text response"
          value={response}
          onChange={(e) => setResponse(e.target.value)}
        />
        {err && <div className="error-bar" style={{ marginTop: 8 }}>{err}</div>}
        <div className="needs-actions">
          <button className="btn primary sm" disabled={busy || !anyPick} onClick={() => submit(false)}>
            submit answers
          </button>
          <button className="btn ghost sm" disabled={busy} onClick={() => submit(true)}>
            skip
          </button>
        </div>
      </div>
    </div>
  );
}

/* ── The page ───────────────────────────────────────────────────────── */

export default function Command() {
  const nav = useNavigate();
  const { refreshInbox } = useAppData();
  const activityPoll = usePoll<Activity>(api.activity, 2000);
  const activity = activityPoll.data;
  const needsPoll = usePoll<Needs>(api.needs, 2000);
  const needs = needsPoll.data;

  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [replyText, setReplyText] = useState("");
  const [replyClass, setReplyClass] = useState<Urgency>("normal");
  const [err, setErr] = useState<string | null>(null);

  async function sendReply(to: string) {
    if (!replyText.trim()) return;
    setErr(null);
    try {
      await api.busSend({ to: `@${to}`, body: replyText.trim(), urgency: replyClass });
      setReplyTo(null);
      setReplyText("");
      await Promise.all([needsPoll.refresh(), refreshInbox()]);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "send failed");
    }
  }

  async function clearMail() {
    await api.markNeedsRead();
    await Promise.all([needsPoll.refresh(), refreshInbox()]);
  }

  const onAnswered = () => {
    void needsPoll.refresh();
  };

  const prompts = needs?.prompts ?? [];
  const mail = needs?.inbox ?? [];

  // Merge the three kinds oldest-first, so the longest-blocked item leads.
  const items: { at: number; key: string; el: ReactNode }[] = prompts.map((p) => ({
    at: p.asked_at,
    key: `p:${p.agent}:${p.request_id}`,
    el: p.is_question ? (
      <QuestionCard key={`p:${p.agent}:${p.request_id}`} prompt={p} onAnswered={onAnswered} />
    ) : (
      <PermCard key={`p:${p.agent}:${p.request_id}`} prompt={p} onAnswered={onAnswered} />
    ),
  }));

  const sessions = activity?.sessions ?? [];
  const busy = sessions.filter((s) => s.live && s.turn_state === "busy");
  const idle = sessions.filter((s) => s.live && s.turn_state !== "busy");
  const down = sessions.filter((s) => !s.live);
  const waiting = activity?.waiting ?? [];

  // Recent-traffic filters: any active filter switches the feed to a deeper
  // server-side bus/log query; otherwise the activity trail streams as-is.
  const [fSender, setFSender] = useState("");
  const [fUrg, setFUrg] = useState("");
  const [fQ, setFQ] = useState("");
  const hasFilter = fSender.trim() !== "" || fUrg !== "" || fQ.trim() !== "";
  const trafficFetcher = useMemo(() => {
    if (!hasFilter) return async () => null;
    const filters = {
      ...(fSender.trim() ? { sender: fSender.trim() } : {}),
      ...(fUrg ? { urgency: fUrg } : {}),
      ...(fQ.trim() ? { q: fQ.trim() } : {}),
    };
    return () => api.busLog(60, filters);
  }, [hasFilter, fSender, fUrg, fQ]);
  const trafficPoll = usePoll<BusMessage[] | null>(trafficFetcher, 3000);
  const trafficRefresh = trafficPoll.refresh;
  useEffect(() => {
    void trafficRefresh();
  }, [trafficFetcher, trafficRefresh]);

  const traffic = hasFilter
    ? [...(trafficPoll.data ?? [])].reverse()
    : [...(activity?.trail ?? [])].reverse();

  const needsCount = items.length + mail.length;

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Command</span>
        <span className="mono-meta">
          {busy.length} busy · {idle.length} idle · {down.length} down
        </span>
      </div>
      <div className="stage-body" style={{ display: "grid", gap: 24, gridTemplateColumns: "minmax(320px, 1fr) minmax(340px, 1.3fr)", alignItems: "start" }}>
        {/* NEEDS YOU + WAITING */}
        <div>
          <section>
            <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
              <span className="label">Needs you</span>
              <span style={{ flex: 1 }} />
              {needsPoll.error && <span className="mono-meta" style={{ color: "var(--sig-gate)", marginRight: 8 }}>offline</span>}
              {mail.length > 0 && (
                <button className="btn ghost sm" onClick={clearMail}>clear all</button>
              )}
            </div>
            <ErrorBar error={err} />
            {needsCount === 0 ? (
              <Empty mark="—">Nothing needs you. The mesh is quiet.</Empty>
            ) : (
              [
                ...items,
                ...mail.map((m) => {
                  const mailKey = `m:${m.node ?? "local"}:${m.id}`;
                  return {
                    at: m.created_at,
                    key: mailKey,
                    el: (
                      <MessageRow
                        key={mailKey}
                        urgency={m.urgency}
                        sender={m.sender}
                        meta={
                          <>
                            <NodeChip node={m.node} />
                            <span className="mono-meta">{relTime(m.created_at)} ago</span>
                          </>
                        }
                        actions={
                          replyTo === mailKey ? (
                            <div style={{ width: "100%", display: "flex", flexDirection: "column", gap: 8 }}>
                              <textarea
                                autoFocus
                                rows={2}
                                placeholder={`reply to @${m.sender}`}
                                value={replyText}
                                onChange={(e) => setReplyText(e.target.value)}
                              />
                              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                                <ClassSelect value={replyClass} onChange={setReplyClass} />
                                <span style={{ flex: 1 }} />
                                <button className="btn ghost sm" onClick={() => setReplyTo(null)}>cancel</button>
                                <button className="btn primary sm" onClick={() => sendReply(m.sender)}>send</button>
                              </div>
                            </div>
                          ) : (
                            <>
                              <button className="btn sm" onClick={() => { setReplyTo(mailKey); setReplyText(""); }}>reply</button>
                              {!m.node && !m.sender.includes("@") && (
                                <button className="btn sm" onClick={() => nav(`/session/${encodeURIComponent(m.sender)}`)}>
                                  open @{m.sender}
                                </button>
                              )}
                            </>
                          )
                        }
                      >
                        {m.body}
                      </MessageRow>
                    ),
                  };
                }),
              ]
                .sort((a, b) => a.at - b.at)
                .map((it) => it.el)
            )}
          </section>

          {/* WAITING */}
          {waiting.length > 0 && (
            <section style={{ marginTop: 24 }}>
              <div className="label" style={{ marginBottom: 12 }}>Waiting</div>
              {waiting.map((w) => (
                <div key={`${w.agent}:${w.on}`} className="waiting-row">
                  <span className="mono" style={{ fontWeight: 500 }}>@{w.agent}</span>
                  <span className="mono-meta">likely waiting on</span>
                  <span className="mono">@{w.on}</span>
                  <span className="mono-meta">· for {relTime(w.since)}</span>
                  <span style={{ flex: 1 }} />
                  {!w.agent.includes("@") && (
                    <button className="btn ghost sm" onClick={() => nav(`/session/${encodeURIComponent(w.agent)}`)}>
                      open @{w.agent}
                    </button>
                  )}
                  {w.snippet && <span className="waiting-snippet" title={w.snippet}>{w.snippet}</span>}
                </div>
              ))}
            </section>
          )}
        </div>

        {/* IN FLIGHT */}
        <section>
          <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
            <span className="label">In flight</span>
            <span style={{ flex: 1 }} />
            {activityPoll.error && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>offline</span>}
          </div>
          {sessions.length === 0 ? (
            <Empty mark="◦">No sessions running. Start one from Sessions or Library.</Empty>
          ) : (
            <div className="grid" style={{ marginBottom: 20 }}>
              {sessions.map((s) => {
                const p = presenceOf(s.live, s.turn_state);
                return (
                  <div
                    key={`${s.name}:${s.node}`}
                    className="strip"
                    style={{ display: "flex", alignItems: "center", gap: 12, cursor: "pointer" }}
                    onClick={() => nav(`/session/${encodeURIComponent(s.name)}`)}
                    role="button"
                    tabIndex={0}
                    onKeyDown={(e) => e.key === "Enter" && nav(`/session/${encodeURIComponent(s.name)}`)}
                  >
                    <Meter presence={p} />
                    <span className="mono" style={{ fontWeight: 500 }}>@{s.name}</span>
                    {s.title && <span className="flight-title" title={s.title}>{s.title}</span>}
                    <span className="mono-meta">#{s.channel}</span>
                    <span style={{ flex: 1 }} />
                    {p === "busy" ? (
                      <span className="micro flight-streaming">
                        streaming{s.busy_since ? ` ${relTime(s.busy_since)}` : ""}
                        {s.last_tool ? ` · ${s.last_tool}` : ""}
                      </span>
                    ) : (
                      <span className="micro" style={{ color: p === "off" ? "var(--offline)" : "var(--idle)" }}>
                        {p === "off" ? "offline" : "idle"}
                      </span>
                    )}
                    {s.pending > 0 && <span className="badge-count">{s.pending}</span>}
                  </div>
                );
              })}
            </div>
          )}

          <div className="label" style={{ marginBottom: 8 }}>Recent traffic</div>
          <div className="traffic-filters">
            <input placeholder="sender" value={fSender} onChange={(e) => setFSender(e.target.value)} />
            <select value={fUrg} onChange={(e) => setFUrg(e.target.value)} aria-label="urgency filter">
              <option value="">any class</option>
              <option value="gating">gating</option>
              <option value="normal">normal</option>
              <option value="notice">notice</option>
            </select>
            <input placeholder="body contains…" value={fQ} onChange={(e) => setFQ(e.target.value)} />
            {hasFilter && (
              <button className="btn ghost sm" onClick={() => { setFSender(""); setFUrg(""); setFQ(""); }}>
                clear
              </button>
            )}
          </div>
          <div className="strip flat" style={{ padding: "8px 12px", maxHeight: 320, overflow: "auto" }}>
            {traffic.length === 0 ? (
              <div className="mono-meta">{hasFilter ? "no matches" : "no traffic yet"}</div>
            ) : (
              traffic.map((m) => (
                <div key={m.id} style={{ display: "flex", alignItems: "center", gap: 8, padding: "3px 0" }}>
                  <Capsule urgency={m.urgency} />
                  <ClassBadge urgency={m.urgency} />
                  <span className="mono" style={{ fontSize: 12 }}>@{m.sender}</span>
                  <span className="mono-meta">→ {m.to_display}</span>
                  <span className="mono-meta" style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {m.body}
                  </span>
                  <span className="mono-meta">{relTime(m.created_at)}</span>
                </div>
              ))
            )}
          </div>
        </section>
      </div>
    </>
  );
}
