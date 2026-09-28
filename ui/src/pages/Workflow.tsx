/* One workflow run of a session (PROPOSALS-2026-09-O §2.3): its phases,
 * the agents of the selected phase with model, state, time, tokens and
 * last tool, the logs, each agent's report, and the script. Live while it
 * runs (the node keeps the harness's streamed progress); from the
 * harness's files once it ended. An agent opens in the subagent view. */
import { useEffect, useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, type WorkflowAgent, type WorkflowRun } from "../api";
import { ErrorBar } from "../components";
import { fmtTokens } from "./sessionExtras";
import "./workflow.css";

function ms(n: number | null | undefined): string {
  if (!n || n < 0) return "—";
  const s = Math.floor(n / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function epochMs(v: number | string | null | undefined): number | null {
  if (v == null) return null;
  if (typeof v === "number") return v;
  const t = Date.parse(v);
  return Number.isNaN(t) ? null : t;
}

function shortModel(m: string | null | undefined): string {
  if (!m) return "";
  return m.replace(/^claude-/, "").replace(/\[1m\]$/, " 1M");
}

/** An agent's elapsed time: its own duration when the harness gave one,
 *  else since it started (running), else from start to its last progress. */
function agentElapsed(a: WorkflowAgent, now: number): string {
  if (a.duration_ms && a.state !== "running") return ms(a.duration_ms);
  if (a.started_at) {
    const end = a.state === "running" ? now : (a.last_progress_at ?? now);
    return ms(end - a.started_at);
  }
  return ms(a.duration_ms);
}

function stateClass(s: string | null): string {
  switch (s) {
    case "running":
      return "wf-running";
    case "done":
    case "completed":
      return "wf-done";
    case "failed":
    case "error":
    case "killed":
      return "wf-failed";
    case "queued":
      return "wf-queued";
    default:
      return "";
  }
}

export default function Workflow() {
  const { name, run } = useParams<{ name: string; run: string }>();
  const [wf, setWf] = useState<WorkflowRun | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [phase, setPhase] = useState<number | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const running = wf?.status === "running";
  useEffect(() => {
    if (!name || !run) return;
    let dead = false;
    const load = () =>
      api
        .workflow(name, run)
        .then((v) => {
          if (dead) return;
          setWf(v);
          setErr(null);
        })
        .catch((e) => !dead && setErr(e instanceof Error ? e.message : String(e)));
    void load();
    const t = window.setInterval(() => {
      setNow(Date.now());
      void load();
    }, 3000);
    return () => {
      dead = true;
      window.clearInterval(t);
    };
  }, [name, run]);
  // Follow the phase in progress until the operator picks one.
  const current = useMemo(() => {
    if (!wf) return null;
    const live = wf.phases.find((p) => p.agents.some((a) => a.state === "running"));
    return (live ?? wf.phases[wf.phases.length - 1])?.index ?? null;
  }, [wf]);
  const shown = wf?.phases.find((p) => p.index === (phase ?? current)) ?? wf?.phases[0] ?? null;
  const all = wf?.phases.flatMap((p) => p.agents) ?? [];
  const done = all.filter((a) => a.state === "done" || a.state === "completed").length;
  const failed = all.filter((a) => a.state === "failed" || a.state === "error").length;
  const tokens = wf?.total_tokens ?? wf?.live?.tokens ?? (all.some((a) => a.tokens) ? all.reduce((n, a) => n + (a.tokens ?? 0), 0) : null);
  const tools = wf?.total_tool_calls ?? wf?.live?.tool_uses ?? (all.some((a) => a.tool_calls) ? all.reduce((n, a) => n + (a.tool_calls ?? 0), 0) : null);
  const started = epochMs(wf?.started_at ?? null);
  const elapsed = wf?.duration_ms ?? (started ? (running ? now : epochMs(wf?.ended_at ?? null) ?? now) - started : null);
  const agentHref = (id: string) => `/session/${encodeURIComponent(name!)}/agent/${encodeURIComponent(id)}`;
  const reportOf = (a: WorkflowAgent) => {
    const key = (a.label ?? "").replace(/^[^:]*:/, "").replace(/\s*\(retry \d+\)$/, "");
    return wf?.results.find((r) => r.key === key || r.key === a.agent_id)?.report ?? a.result_preview ?? null;
  };
  return (
    <div className="page wf-page">
      <div className="wf-head">
        <Link to={`/session/${encodeURIComponent(name ?? "")}`} className="btn ghost sm">← @{(name ?? "").split("@")[0]}</Link>
        <span className="wf-name mono">{wf?.name ?? run}</span>
        {wf?.status && <span className={`chip mono ${stateClass(wf.status)}`}>{wf.status}</span>}
        {wf && wf.attempts && wf.attempts > 1 ? <span className="chip mono" title="this run was resumed; attempts share one run id">{wf.attempts} attempts</span> : null}
        <span style={{ flex: 1 }} />
        <span className="mono-meta" title="elapsed">{ms(elapsed)}</span>
        <span className="mono-meta">{all.length || wf?.agent_count || 0} agents</span>
        {tokens != null && <span className="mono-meta">{fmtTokens(tokens)} tok</span>}
        {tools != null && <span className="mono-meta">{tools.toLocaleString()} tools</span>}
        {wf && <span className="mono-meta" title={wf.source === "live" ? "live: the harness's streamed progress" : wf.source === "state" ? "the run's state file (written when it ended)" : "the run's journal and agent files"}>· {wf.source}</span>}
      </div>
      {wf?.description && <p className="wf-desc">{wf.description}</p>}
      <ErrorBar error={err} />
      {!wf && !err && <div className="dim">loading…</div>}
      {wf && (
        <>
          <div className="wf-phases" role="tablist">
            {wf.phases.map((p) => {
              const pd = p.agents.filter((a) => a.state === "done" || a.state === "completed").length;
              const pr = p.agents.filter((a) => a.state === "running").length;
              const active = p.index === shown?.index;
              return (
                <button key={p.index} role="tab" aria-selected={active} className={`wf-phase${active ? " active" : ""}${pr ? " live" : ""}`} onClick={() => setPhase(p.index)} title={p.detail ?? undefined}>
                  <span className="wf-phase-title">{p.title ?? `phase ${p.index}`}</span>
                  <span className="mono-meta">{p.agents.length ? `${pd}/${p.agents.length}` : "—"}{pr ? ` · ${pr} running` : ""}</span>
                </button>
              );
            })}
            <span style={{ flex: 1 }} />
            <span className="mono-meta">{done}/{all.length} done{failed ? ` · ${failed} failed` : ""}</span>
          </div>
          {shown?.detail && <div className="mono-meta wf-phase-detail">{shown.detail}</div>}
          <div className="wf-agents">
            {(shown?.agents ?? []).length === 0 && <div className="empty">no agents in this phase yet</div>}
            {(shown?.agents ?? []).map((a) => {
              const isOpen = open === a.agent_id;
              const report = reportOf(a);
              return (
                <div key={a.agent_id || a.label || Math.random()} className={`wf-agent ${stateClass(a.state)}${isOpen ? " open" : ""}`}>
                  <div className="wf-agent-row" onClick={() => setOpen(isOpen ? null : a.agent_id)}>
                    <span className={`wf-dot ${stateClass(a.state)}`} aria-hidden />
                    <span className="wf-agent-state mono-meta">{a.state ?? "?"}</span>
                    <span className="wf-agent-label">
                      {a.has_transcript ? (
                        <Link to={agentHref(a.agent_id)} onClick={(e) => e.stopPropagation()} title="open this agent's transcript">{a.label ?? a.agent_id}</Link>
                      ) : (
                        a.label ?? a.agent_id
                      )}
                      {a.attempt && a.attempt > 1 && !/\(retry \d+\)/.test(a.label ?? "") ? <span className="chip mono wf-retry" title={a.retry_reason ?? "retried"}>retry {a.attempt - 1}</span> : null}
                    </span>
                    <span className="mono-meta wf-col wf-model">{shortModel(a.model)}</span>
                    <span className="mono-meta wf-col wf-el">{agentElapsed(a, now)}</span>
                    <span className="mono-meta wf-col wf-tok">{a.tokens != null ? `${fmtTokens(a.tokens)} tok` : ""}</span>
                    <span className="mono-meta wf-col wf-tools">{a.tool_calls != null ? `${a.tool_calls} tools` : ""}</span>
                    <span className="mono-meta wf-last" title={a.last_tool_summary ?? undefined}>{a.last_tool ? `${a.last_tool}${a.last_tool_summary ? ` · ${a.last_tool_summary}` : ""}` : ""}</span>
                  </div>
                  {isOpen && (
                    <div className="wf-agent-detail">
                      {a.retry_reason && <div className="mono-meta">retried: {a.retry_reason}</div>}
                      {a.prompt_preview && (
                        <div className="wf-kv"><span className="k">prompt</span><span className="v wf-pre">{a.prompt_preview}</span></div>
                      )}
                      <div className="wf-kv">
                        <span className="k">report</span>
                        {report ? (
                          <div className="v md"><ReactMarkdown remarkPlugins={[remarkGfm]}>{report}</ReactMarkdown></div>
                        ) : (
                          <span className="v dim">{a.state === "running" ? "still working" : "no report"}</span>
                        )}
                      </div>
                      {a.has_transcript && <Link className="btn sm" to={agentHref(a.agent_id)}>open transcript</Link>}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
          {wf.logs.length > 0 && (
            <div className="wf-section">
              <span className="label">logs</span>
              {wf.logs.map((l, i) => (
                <div key={i} className="mono-meta wf-log">{l}</div>
              ))}
            </div>
          )}
          {wf.script_path && (
            <div className="wf-section">
              <span className="label">script</span>
              <Link className="mono-meta" to={`/view/${encodeURIComponent(name ?? "")}?path=${encodeURIComponent(wf.script_path)}`}>{wf.script_path.split(/[\\/]/).pop()}</Link>
            </div>
          )}
        </>
      )}
    </div>
  );
}
