// Needs-you cards: permission prompts and questions, answerable in place.
// Shared by Now (the operator's inbox of consequence) and anywhere else a
// prompt needs answering.

import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, type Adoption, type OpenPrompt } from "./api";
import { relTime } from "./components";
import "./pages/command.css";

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

export function questionsOf(input: unknown): Question[] {
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

export function summarizeInput(input: unknown): { k: string; v: string }[] {
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

export function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
}

export function NodeChip({ node }: { node: string | null }) {
  if (!node) return null;
  return <span className="node-chip">{node}</span>;
}

/* ── Permission gate card ───────────────────────────────────────────── */

export function PermCard({ prompt, onAnswered }: { prompt: OpenPrompt; onAnswered: () => void }) {
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

export function QuestionCard({ prompt, onAnswered }: { prompt: OpenPrompt; onAnswered: () => void }) {
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


/* ── adoption: a session happened to an agent outside Aspen ─────────── */

/** Which identity follows a session that was forked or driven from outside
 *  Aspen. Nothing moves until a human answers; ignore is the default. */
export function AdoptionCard({ a, onDone }: { a: Adoption; onDone: () => void }) {
  const nav = useNavigate();
  const [splitName, setSplitName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const agent = a.of_agent ?? "?";
  const bare = agent.split("@")[0];
  const who = a.entrypoint === "sdk-cli" || a.entrypoint === "cli" ? "a terminal" : a.entrypoint ? `“${a.entrypoint}”` : "elsewhere";
  async function act(action: "carry" | "split" | "ignore" | "revive", name?: string) {
    setBusy(true);
    setErr(null);
    try {
      const r = await api.resolveAdoption(a.id, action, name, a.node);
      onDone();
      if ((action === "carry" || action === "split") && r.agent) {
        nav(`/session/${encodeURIComponent(a.node ? `${r.agent}@${a.node}` : r.agent)}`);
      }
    } catch (e) {
      setErr(e instanceof Error ? e.message : "failed");
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="need-card need-cue" style={{ flexWrap: "wrap" }}>
      <span className="chip mono" style={{ color: "var(--sig-normal)" }}>{a.kind === "fork" ? "forked" : "driven elsewhere"}</span>
      {a.kind === "fork" ? (
        <span>
          a branch of <span className="mono">@{bare}</span>’s session appeared from {who}
          {a.title ? <span className="dim"> — “{a.title}”</span> : null}
        </span>
      ) : (
        <span>
          <span className="mono">@{bare}</span>’s session is being driven from {who} while the agent is down
          {a.title ? <span className="dim"> — “{a.title}”</span> : null}
        </span>
      )}
      <span className="mono-meta">{a.session_id.slice(0, 8)} · {relTime(a.first_seen)} ago</span>
      {a.node && <NodeChip node={a.node} />}
      <span style={{ flex: 1 }} />
      {a.kind === "fork" && splitName === null && (
        <>
          <button className="btn sm" disabled={busy} onClick={() => void act("carry")} title={`@${bare} moves to the branch; its current tip is bookmarked`}>
            carry @{bare} here
          </button>
          <button className="btn sm" disabled={busy} onClick={() => setSplitName(`${bare}-2`)} title="the branch becomes a new agent; the original keeps its session">
            new agent…
          </button>
        </>
      )}
      {a.kind === "fork" && splitName !== null && (
        <>
          <input
            className="mono"
            value={splitName}
            onChange={(e) => setSplitName(e.target.value)}
            autoFocus
            style={{ width: 140 }}
            aria-label="new agent name"
            onKeyDown={(e) => {
              if (e.key === "Enter") void act("split", splitName);
              if (e.key === "Escape") setSplitName(null);
            }}
          />
          <button className="btn primary sm" disabled={busy || !splitName.trim()} onClick={() => void act("split", splitName)}>
            start @{splitName.trim() || "…"}
          </button>
          <button className="btn ghost sm" onClick={() => setSplitName(null)}>cancel</button>
        </>
      )}
      {a.kind === "resumed" && (
        <button className="btn sm" disabled={busy} onClick={() => void act("revive")} title="bring the agent back on this session (once the other side is done with it)">
          revive @{bare}
        </button>
      )}
      <button className="btn ghost sm" disabled={busy} onClick={() => void act("ignore")}>ignore</button>
      {err && <span className="mono-meta" style={{ color: "var(--sig-gate)", flexBasis: "100%" }}>{err}</span>}
    </div>
  );
}
