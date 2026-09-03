// History — what happened across the fleet. A timeline: one lane per agent,
// turns as spans (an ask starts one, a turn-end closes it), tool calls as
// ticks, prompts as amber marks, bus messages as dots on the sender's lane.
// Brush the overview to zoom; the log below stays in step with the window
// and with whatever you hover. This is the fifth surface — the answer to
// "what happened today?" — and nothing else tries to be it.

import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, type BusMessage, type FleetEvent, type History as HistoryData } from "../api";
import { useHotkeys } from "../hotkeys";
import { Empty, ErrorBar, relTime } from "../components";
import "./history.css";

type Span = { agent: string; start: number; end: number | null; ask: string; reply?: string; cost?: number | null; node: string };
type Tick = { agent: string; ts: number; kind: "tool" | "prompt" | "exit" | "spawn" | "revive" | "branch"; label: string; node: string };
type Dot = { agent: string; ts: number; label: string; urgency: string; node: string };

const H_LANE = 26;
const W_LABEL = 190;
const GUTTER = 12;

function fmtClock(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

/** Peers report our agents as `x@y@<us>`; fold those onto our local key. */
function localize(addr: string, self: string): string {
  const parts = addr.split("@");
  return parts.length === 3 && parts[2] === self ? `${parts[0]}@${parts[1]}` : addr;
}
function fmtDur(s: number): string {
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}
function dayStart(d: Date): number {
  const x = new Date(d);
  x.setHours(0, 0, 0, 0);
  return x.getTime() / 1000;
}

/** Fold raw events into spans and ticks per agent. */
function fold(events: FleetEvent[]): { spans: Span[]; ticks: Tick[] } {
  const open = new Map<string, Span>();
  const spans: Span[] = [];
  const ticks: Tick[] = [];
  for (const e of events) {
    const d = (e.detail ?? {}) as Record<string, unknown>;
    switch (e.kind) {
      case "ask": {
        // A new ask while one is open: close the old one at this moment.
        const prev = open.get(e.agent);
        if (prev) {
          prev.end = e.ts;
          spans.push(prev);
        }
        const from = d["from"] === "bus" ? `bus: ${((d["senders"] as string[]) ?? []).join(", ")}` : String(d["text"] ?? "");
        open.set(e.agent, { agent: e.agent, start: e.ts, end: null, ask: from, node: e.node });
        break;
      }
      case "turn": {
        const sp = open.get(e.agent);
        if (sp) {
          sp.end = e.ts;
          sp.reply = typeof d["reply"] === "string" ? (d["reply"] as string) : undefined;
          sp.cost = typeof d["cost_delta"] === "number" ? (d["cost_delta"] as number) : null;
          spans.push(sp);
          open.delete(e.agent);
        } else {
          // A turn we didn't see start (e.g. began before the window).
          const dur = typeof d["duration_ms"] === "number" ? (d["duration_ms"] as number) / 1000 : 0;
          spans.push({ agent: e.agent, start: e.ts - dur, end: e.ts, ask: "(turn)", reply: d["reply"] as string | undefined, cost: d["cost_delta"] as number | null, node: e.node });
        }
        break;
      }
      case "tool":
        ticks.push({ agent: e.agent, ts: e.ts, kind: "tool", label: `${d["name"]}${d["path"] ? ` ${String(d["path"]).split("/").pop()}` : d["command"] ? ` ${d["command"]}` : ""}`, node: e.node });
        break;
      case "prompt":
        ticks.push({ agent: e.agent, ts: e.ts, kind: "prompt", label: `permission: ${d["tool"]}`, node: e.node });
        break;
      case "exit":
        ticks.push({ agent: e.agent, ts: e.ts, kind: "exit", label: `exited${d["code"] != null ? ` (${d["code"]})` : ""}${d["daemon_shutdown"] ? " — daemon stop" : ""}`, node: e.node });
        break;
      case "spawn":
      case "revive":
      case "branch":
        ticks.push({ agent: e.agent, ts: e.ts, kind: e.kind, label: e.kind, node: e.node });
        break;
      default:
        break;
    }
  }
  for (const sp of open.values()) spans.push(sp); // still running
  return { spans, ticks };
}

export default function History() {
  const nav = useNavigate();
  const [day, setDay] = useState<number>(() => dayStart(new Date()));
  const [data, setData] = useState<HistoryData | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [agentFilter, setAgentFilter] = useState("");
  // Zoom window within the day (epoch seconds).
  const [win, setWin] = useState<[number, number] | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);

  const dayEnd = day + 86400;
  const now = Date.now() / 1000;
  const to = Math.min(dayEnd, now + 60);

  useHotkeys("history", [
    { key: "[", description: "previous day", handler: () => setDay((d) => d - 86400) },
    { key: "]", description: "next day", handler: () => setDay((d) => Math.min(dayStart(new Date()), d + 86400)) },
    { key: "0", description: "reset zoom", handler: () => setWin(null) },
  ]);

  useEffect(() => {
    let live = true;
    const load = () =>
      api
        .history(day, to, agentFilter.trim() || undefined)
        .then((d) => live && (setData(d), setErr(null)))
        .catch((e) => live && setErr(e instanceof Error ? e.message : "failed"));
    void load();
    const t = window.setInterval(load, 15000);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, [day, to, agentFilter]);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const { spans, ticks, dots, agents } = useMemo(() => {
    const self = data?.self ?? "";
    const events = (data?.events ?? []).map((e) => ({ ...e, agent: localize(e.agent, self) }));
    const { spans, ticks } = fold(events);
    const dots: Dot[] = (data?.messages ?? []).map((m: BusMessage & { node?: string }) => ({
      agent: localize(m.sender, self),
      ts: m.created_at,
      label: `→ ${m.to_display}: ${m.body.slice(0, 80)}`,
      urgency: m.urgency,
      node: m.node ?? "",
    }));
    const agents = [...new Set([...spans.map((s) => s.agent), ...ticks.map((t) => t.agent), ...dots.map((d) => d.agent)])]
      .filter((a) => a !== "operator")
      .sort();
    return { spans, ticks, dots, agents };
  }, [data]);

  const [w0, w1] = win ?? [day, to];
  const plotW = Math.max(200, width - W_LABEL - GUTTER * 2);
  const x = (ts: number) => W_LABEL + GUTTER + ((ts - w0) / (w1 - w0)) * plotW;
  const laneY = (i: number) => 28 + i * H_LANE;
  const svgH = 28 + agents.length * H_LANE + 12;

  // Overview brush
  const [drag, setDrag] = useState<[number, number] | null>(null);
  const ovRef = useRef<SVGSVGElement>(null);
  const ovX = (ts: number) => ((ts - day) / (to - day)) * plotW;
  const ovTs = (px: number) => day + (px / plotW) * (to - day);
  function ovPos(e: React.MouseEvent): number {
    const r = ovRef.current?.getBoundingClientRect();
    return r ? Math.max(0, Math.min(plotW, e.clientX - r.left)) : 0;
  }

  // Summary for the window
  const inWin = (t: number) => t >= w0 && t <= w1;
  const turns = spans.filter((s) => inWin(s.start)).length;
  const cost = spans.filter((s) => inWin(s.start)).reduce((a, s) => a + (s.cost ?? 0), 0);
  const msgs = dots.filter((d) => inWin(d.ts)).length;
  const prompts = ticks.filter((t) => t.kind === "prompt" && inWin(t.ts)).length;

  // The log: everything in the window, chronological
  const log = useMemo(() => {
    const items: { ts: number; agent: string; text: string; kind: string; key: string }[] = [];
    for (const s of spans) if (inWin(s.start)) items.push({ ts: s.start, agent: s.agent, text: `asked: ${s.ask}${s.end ? ` — ${fmtDur(s.end - s.start)}${s.reply ? ` → ${s.reply}` : ""}` : " (running)"}`, kind: "turn", key: `s:${s.agent}:${s.start}` });
    for (const t of ticks) if (inWin(t.ts) && t.kind !== "tool") items.push({ ts: t.ts, agent: t.agent, text: t.label, kind: t.kind, key: `t:${t.agent}:${t.ts}` });
    for (const d of dots) if (inWin(d.ts)) items.push({ ts: d.ts, agent: d.agent, text: d.label, kind: `bus-${d.urgency}`, key: `d:${d.agent}:${d.ts}` });
    return items.sort((a, b) => a.ts - b.ts);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spans, ticks, dots, w0, w1]);

  const dayLabel = new Date(day * 1000).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
  const isToday = day === dayStart(new Date());

  return (
    <>
      <div className="stage-head">
        <span className="t-display">History</span>
        <span className="mono-meta">
          {turns} turns · ${cost.toFixed(2)} · {msgs} messages · {prompts} prompts
          {win ? ` · ${fmtClock(w0)}–${fmtClock(w1)}` : ""}
        </span>
        <span style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={() => setDay((d) => d - 86400)} title="previous day ([)">◂</button>
        <span className="mono" style={{ minWidth: 120, textAlign: "center" }}>{isToday ? "today" : dayLabel}</span>
        <button className="btn ghost sm" disabled={isToday} onClick={() => setDay((d) => d + 86400)} title="next day (])">▸</button>
        <span className="class-select" role="group" aria-label="zoom">
          {([["1h", 3600], ["3h", 10800], ["day", 0]] as const).map(([label, secs]) => (
            <button
              key={label}
              type="button"
              onClick={() => setWin(secs === 0 ? null : [Math.max(day, to - secs), to])}
              style={{
                background: (secs === 0 ? !win : win && Math.round(w1 - w0) === secs) ? "var(--bg-strip-2)" : "var(--bg-well)",
                color: "var(--text-mid)",
              }}
            >
              {label}
            </button>
          ))}
        </span>
        <input
          value={agentFilter}
          onChange={(e) => setAgentFilter(e.target.value)}
          placeholder="agent (name@repo)"
          className="mono"
          style={{ width: 180 }}
          aria-label="filter by agent"
        />
      </div>
      <div className="stage-body" ref={wrapRef}>
        <ErrorBar error={err} />
        {data && agents.length === 0 ? (
          <Empty mark="—">Nothing happened {isToday ? "today" : `on ${dayLabel}`} yet.</Empty>
        ) : (
          <>
            {/* overview brush: the whole day */}
            <svg ref={ovRef} className="hist-overview" width={plotW} height={22} style={{ marginLeft: W_LABEL + GUTTER }}
              onMouseDown={(e) => setDrag([ovPos(e), ovPos(e)])}
              onMouseMove={(e) => drag && setDrag([drag[0], ovPos(e)])}
              onMouseUp={(e) => {
                if (drag) {
                  const end = ovPos(e);
                  const [a, b] = [Math.min(drag[0], end), Math.max(drag[0], end)];
                  if (b - a > 4) setWin([ovTs(a), ovTs(b)]);
                  else {
                    // A click: one hour around that moment.
                    const c = ovTs(end);
                    setWin([Math.max(day, c - 1800), Math.min(to, c + 1800)]);
                  }
                }
                setDrag(null);
              }}
              onMouseLeave={() => setDrag(null)}
            >
              {spans.map((s, i) => (
                <rect key={i} x={ovX(s.start)} y={6} width={Math.max(1, ovX(s.end ?? now) - ovX(s.start))} height={10} fill="var(--live)" opacity={0.5} />
              ))}
              {dots.map((d, i) => (
                <rect key={`d${i}`} x={ovX(d.ts)} y={3} width={1} height={16} fill="var(--sig-normal)" opacity={0.7} />
              ))}
              {(drag || win) && (() => {
                const [a, b] = drag ? [Math.min(...drag), Math.max(...drag)] : [ovX(w0), ovX(w1)];
                return <rect x={a} y={0} width={b - a} height={22} fill="var(--sig-notice)" opacity={0.18} stroke="var(--sig-notice)" />;
              })()}
            </svg>

            {/* lanes */}
            <svg className="hist-lanes" width={width} height={svgH}>
              {/* time gridlines */}
              {Array.from({ length: 7 }, (_, i) => w0 + ((w1 - w0) * i) / 6).map((t, i) => (
                <g key={i}>
                  <line x1={x(t)} x2={x(t)} y1={18} y2={svgH - 8} stroke="var(--line)" strokeWidth={1} />
                  <text x={x(t)} y={12} textAnchor="middle" fill="var(--text-dim)" style={{ font: "500 10px/1 var(--font-mono)" }}>{fmtClock(t)}</text>
                </g>
              ))}
              {isToday && now >= w0 && now <= w1 && <line x1={x(now)} x2={x(now)} y1={18} y2={svgH - 8} stroke="var(--sig-gate)" strokeWidth={1} strokeDasharray="3 3" />}
              {agents.map((a, i) => (
                <g key={a} transform={`translate(0 ${laneY(i)})`}>
                  <text x={W_LABEL} y={H_LANE / 2 + 4} textAnchor="end" fill="var(--text-hi)" style={{ font: "500 11px/1 var(--font-mono)", cursor: "pointer" }} onClick={() => nav(`/session/${encodeURIComponent(a)}`)}>
                    @{a}
                  </text>
                  <line x1={W_LABEL + GUTTER} x2={W_LABEL + GUTTER + plotW} y1={H_LANE / 2} y2={H_LANE / 2} stroke="var(--line)" strokeWidth={1} />
                  {spans.filter((s) => s.agent === a && (s.end ?? now) >= w0 && s.start <= w1).map((s, j) => {
                    const key = `s:${s.agent}:${s.start}`;
                    return (
                      <rect key={j} x={Math.max(x(s.start), W_LABEL + GUTTER)} y={6} width={Math.max(2, Math.min(x(s.end ?? now), W_LABEL + GUTTER + plotW) - Math.max(x(s.start), W_LABEL + GUTTER))} height={H_LANE - 12}
                        fill={s.end ? "var(--live)" : "var(--sig-notice)"} opacity={hover === key ? 1 : 0.55}
                        onMouseEnter={() => setHover(key)} onMouseLeave={() => setHover(null)}>
                        <title>{`${s.ask}\n${s.end ? fmtDur(s.end - s.start) : "running"}${s.reply ? `\n→ ${s.reply}` : ""}${s.cost != null ? `\n$${s.cost.toFixed(3)}` : ""}`}</title>
                      </rect>
                    );
                  })}
                  {ticks.filter((t) => t.agent === a && inWin(t.ts)).map((t, j) => (
                    <g key={`t${j}`}>
                      <title>{`${fmtClock(t.ts)} ${t.label}`}</title>
                      {t.kind === "tool" ? (
                        <line x1={x(t.ts)} x2={x(t.ts)} y1={H_LANE / 2 - 4} y2={H_LANE / 2 + 4} stroke="var(--text-hi)" strokeWidth={1} opacity={0.6} />
                      ) : t.kind === "prompt" ? (
                        <rect x={x(t.ts) - 3} y={3} width={6} height={6} fill="var(--sig-normal)" transform={`rotate(45 ${x(t.ts)} 6)`} />
                      ) : t.kind === "exit" ? (
                        <rect x={x(t.ts) - 2} y={2} width={4} height={H_LANE - 4} fill="var(--sig-gate)" />
                      ) : (
                        <rect x={x(t.ts) - 2} y={2} width={4} height={H_LANE - 4} fill="var(--sig-notice)" />
                      )}
                    </g>
                  ))}
                  {dots.filter((d) => d.agent === a && inWin(d.ts)).map((d, j) => (
                    <circle key={`d${j}`} cx={x(d.ts)} cy={H_LANE - 5} r={3} fill={d.urgency === "gating" ? "var(--sig-gate)" : d.urgency === "notice" ? "var(--sig-notice)" : "var(--sig-normal)"}
                      onMouseEnter={() => setHover(`d:${d.agent}:${d.ts}`)} onMouseLeave={() => setHover(null)}>
                      <title>{`${fmtClock(d.ts)} ${d.label}`}</title>
                    </circle>
                  ))}
                </g>
              ))}
            </svg>

            <div className="hist-legend micro">
              <span><span className="sw" style={{ background: "var(--live)" }} /> turn</span>
              <span><span className="sw" style={{ background: "var(--sig-notice)" }} /> running / spawn</span>
              <span><span className="sw tick" /> tool call</span>
              <span><span className="sw diamond" /> permission prompt</span>
              <span><span className="sw dot" /> bus message</span>
              <span><span className="sw" style={{ background: "var(--sig-gate)" }} /> exit</span>
              <span className="dim">drag the strip above to zoom · [ ] days · 0 reset</span>
            </div>

            {/* the log */}
            <div className="hist-log">
              {log.length === 0 ? (
                <Empty mark="—">Nothing in this window.</Empty>
              ) : (
                log.map((it) => (
                  <div key={it.key} className={`hist-row${hover === it.key ? " hot" : ""}`} onMouseEnter={() => setHover(it.key)} onMouseLeave={() => setHover(null)}>
                    <span className="mono-meta" style={{ width: 44, flex: "none" }}>{fmtClock(it.ts)}</span>
                    <span className={`chip mono kind-${it.kind}`}>{it.kind.replace("bus-", "")}</span>
                    <span className="mono" style={{ color: "var(--text-hi)", cursor: "pointer" }} onClick={() => nav(`/session/${encodeURIComponent(it.agent)}`)}>@{it.agent}</span>
                    <span className="hist-text">{it.text}</span>
                    <span className="mono-meta">{relTime(it.ts)} ago</span>
                  </div>
                ))
              )}
            </div>
          </>
        )}
      </div>
    </>
  );
}
