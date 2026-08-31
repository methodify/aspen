// MeshMap — the signature spatial view: "the mesh as a place." A live
// topological map rendered as inline SVG. Sessions are tracked contacts,
// grouped into node regions and, within a node, by #channel (repo). Custom
// channels are drawn as edges arcing between their member contacts — the
// "conversations across repos/nodes" made visible. Everything is polled every
// 2s; color comes only from the Switchboard tokens.

import { useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import type { Activity, ActivitySession, Channel, MeshInfo, WaitingEdge } from "../api";
import { usePoll } from "../hooks";
import { Empty, presenceOf, relTime } from "../components";
import type { Presence } from "../components";

// ── Layout constants (SVG user units) ─────────────────────────────────────
const PAD = 28;
const NODE_GAP = 28;
const NODE_W = 300;
const INNER_PAD_X = 16;
const COLS = 3;
const CELL_W = (NODE_W - INNER_PAD_X * 2) / COLS;
const MARKER = 22;
const ROW_H = 60;
const CH_HEADER = 24;
const NODE_HEADER = 40;
const NODE_BODY_TOP = NODE_HEADER + 10;
const CH_GAP = 10;
const NODE_BOTTOM_PAD = 18;
const OPERATOR_BAND = 104;
const MIN_REGION_H = 150;

interface Pt {
  x: number;
  y: number;
}

interface PlacedMarker {
  s: ActivitySession;
  x: number;
  y: number;
  presence: Presence;
  recent: boolean;
}

interface ChannelLabel {
  node: string;
  text: string;
  x: number;
  y: number;
}

interface NodeRegion {
  node: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Edge {
  channel: string;
  hub: Pt;
  spokes: Pt[];
}

/** A resolved likely-waiting arrow: `agent` → the party it waits on. */
interface WaitArrow {
  agent: string;
  on: string;
  since: number;
  snippet: string;
  from: Pt;
  to: Pt;
}

interface Layout {
  width: number;
  height: number;
  regions: NodeRegion[];
  markers: PlacedMarker[];
  channelLabels: ChannelLabel[];
  edges: Edge[];
  waits: WaitArrow[];
  operator: Pt;
}

interface HoverInfo {
  name: string;
  node: string;
  channel: string;
  state: string;
  pending: number;
  remote: boolean;
}

const OPERATOR_R = 15;

const presenceColor: Record<Presence, string> = {
  busy: "var(--live)",
  idle: "var(--idle)",
  off: "var(--offline)",
};

const presenceState: Record<Presence, string> = {
  busy: "busy · streaming",
  idle: "idle",
  off: "offline",
};

/** A chamfered square path (one machined corner, top-left) centered on (cx, cy). */
function chamferSquare(cx: number, cy: number, size: number, chamfer = 5): string {
  const h = size / 2;
  const l = cx - h;
  const r = cx + h;
  const t = cy - h;
  const b = cy + h;
  return `M ${l + chamfer} ${t} L ${r} ${t} L ${r} ${b} L ${l} ${b} L ${l} ${t + chamfer} Z`;
}

/** Group sessions by node, preserving first-seen order. */
function groupByNode(sessions: ActivitySession[]): [string, ActivitySession[]][] {
  const map = new Map<string, ActivitySession[]>();
  for (const s of sessions) {
    const list = map.get(s.node);
    if (list) list.push(s);
    else map.set(s.node, [s]);
  }
  return [...map.entries()];
}

/** Group a node's sessions by channel (repo), preserving first-seen order. */
function groupByChannel(sessions: ActivitySession[]): [string, ActivitySession[]][] {
  const map = new Map<string, ActivitySession[]>();
  for (const s of sessions) {
    const list = map.get(s.channel);
    if (list) list.push(s);
    else map.set(s.channel, [s]);
  }
  return [...map.entries()];
}

function computeLayout(
  sessions: ActivitySession[],
  channels: Channel[],
  recent: Set<string>,
  waiting: WaitingEdge[],
): Layout {
  const byNode = groupByNode(sessions);
  const nNodes = Math.max(1, byNode.length);

  // First pass: per-node channel groupings + content height.
  const nodeGroups = byNode.map(([node, list]) => ({
    node,
    channels: groupByChannel(list),
  }));

  const contentHeight = (chs: [string, ActivitySession[]][]): number => {
    let h = NODE_BODY_TOP;
    for (const [, list] of chs) {
      h += CH_HEADER;
      h += Math.ceil(list.length / COLS) * ROW_H;
      h += CH_GAP;
    }
    return h + NODE_BOTTOM_PAD;
  };

  const regionHeight = Math.max(
    MIN_REGION_H,
    ...nodeGroups.map((g) => contentHeight(g.channels)),
  );

  const regionTop = PAD + OPERATOR_BAND;
  const width = Math.max(
    420,
    PAD * 2 + nNodes * NODE_W + (nNodes - 1) * NODE_GAP,
  );
  const height = regionTop + regionHeight + PAD;

  const regions: NodeRegion[] = [];
  const markers: PlacedMarker[] = [];
  const channelLabels: ChannelLabel[] = [];

  // Matching indices for edge resolution.
  const byNameNode = new Map<string, Pt>();
  const byName = new Map<string, Pt>();

  nodeGroups.forEach((g, i) => {
    const x0 = PAD + i * (NODE_W + NODE_GAP);
    regions.push({ node: g.node, x: x0, y: regionTop, w: NODE_W, h: regionHeight });

    let y = regionTop + NODE_BODY_TOP;
    for (const [channel, list] of g.channels) {
      channelLabels.push({ node: g.node, text: channel, x: x0 + INNER_PAD_X, y });
      const bodyY = y + CH_HEADER;
      list.forEach((s, idx) => {
        const row = Math.floor(idx / COLS);
        const col = idx % COLS;
        const cx = x0 + INNER_PAD_X + col * CELL_W + CELL_W / 2;
        const cy = bodyY + row * ROW_H + MARKER / 2 + 2;
        const presence = presenceOf(s.live, s.turn_state);
        const isRecent =
          recent.has(s.name) || recent.has(`${s.name}@${s.node}`);
        markers.push({ s, x: cx, y: cy, presence, recent: isRecent });
        const pt: Pt = { x: cx, y: cy };
        byNameNode.set(`${s.name}@${s.node}`, pt);
        if (!byName.has(s.name)) byName.set(s.name, pt);
      });
      y = bodyY + Math.ceil(list.length / COLS) * ROW_H + CH_GAP;
    }
  });

  const operator: Pt = { x: width / 2, y: PAD + OPERATOR_BAND / 2 };

  // Resolve a channel member address to a marker position.
  const resolve = (member: string): Pt | null => {
    const m = member.replace(/^@/, "");
    if (m === "operator") return operator;
    if (m.includes("@")) return byNameNode.get(m) ?? null;
    return byName.get(m) ?? null;
  };

  const edges: Edge[] = [];
  for (const ch of channels) {
    if (ch.kind !== "custom") continue;
    const spokes: Pt[] = [];
    for (const member of ch.members) {
      const pt = resolve(member);
      if (pt) spokes.push(pt);
    }
    if (spokes.length < 2) continue;
    const hub: Pt = {
      x: spokes.reduce((a, p) => a + p.x, 0) / spokes.length,
      y: spokes.reduce((a, p) => a + p.y, 0) / spokes.length,
    };
    edges.push({ channel: ch.name, hub, spokes });
  }

  // Likely-waiting arrows — only drawn when BOTH endpoints resolve to markers.
  const waits: WaitArrow[] = [];
  for (const w of waiting) {
    const from = resolve(w.agent);
    const to = resolve(w.on);
    if (!from || !to || from === to) continue;
    waits.push({ agent: w.agent, on: w.on, since: w.since, snippet: w.snippet, from, to });
  }

  return { width, height, regions, markers, channelLabels, edges, waits, operator };
}

/** A gentle perpendicular-arced path from a spoke endpoint to the hub. */
function arcPath(from: Pt, hub: Pt): string {
  const dx = hub.x - from.x;
  const dy = hub.y - from.y;
  const len = Math.hypot(dx, dy) || 1;
  const nx = -dy / len;
  const ny = dx / len;
  const off = Math.min(40, len * 0.15);
  const cx = (from.x + hub.x) / 2 + nx * off;
  const cy = (from.y + hub.y) / 2 + ny * off;
  return `M ${from.x} ${from.y} Q ${cx} ${cy} ${hub.x} ${hub.y}`;
}

/**
 * Geometry for a waiting arrow: a shallow arc (bowed the OPPOSITE way from
 * channel arcs) trimmed clear of both markers, with an arrowhead at the
 * waited-on end and a midpoint label anchor.
 */
function waitGeom(from: Pt, to: Pt): {
  d: string;
  label: Pt;
  head: string;
} {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const len = Math.hypot(dx, dy) || 1;
  const ux = dx / len;
  const uy = dy / len;
  const trim = Math.min(MARKER * 0.85, len / 3);
  const a: Pt = { x: from.x + ux * trim, y: from.y + uy * trim };
  const b: Pt = { x: to.x - ux * trim, y: to.y - uy * trim };
  // Perpendicular bow — negative side, so it never overlays channel arcs.
  const nx = uy;
  const ny = -ux;
  const off = Math.min(36, len * 0.14);
  const c: Pt = { x: (a.x + b.x) / 2 + nx * off, y: (a.y + b.y) / 2 + ny * off };
  // Quadratic midpoint (t = 0.5) for the label.
  const label: Pt = {
    x: 0.25 * a.x + 0.5 * c.x + 0.25 * b.x,
    y: 0.25 * a.y + 0.5 * c.y + 0.25 * b.y,
  };
  // Arrowhead aligned with the curve's end tangent (c → b).
  const tdx = b.x - c.x;
  const tdy = b.y - c.y;
  const tlen = Math.hypot(tdx, tdy) || 1;
  const tx = tdx / tlen;
  const ty = tdy / tlen;
  const px = -ty;
  const py = tx;
  const hw = 3.6;
  const hl = 8;
  const head =
    `M ${b.x} ${b.y} ` +
    `L ${b.x - tx * hl + px * hw} ${b.y - ty * hl + py * hw} ` +
    `L ${b.x - tx * hl - px * hw} ${b.y - ty * hl - py * hw} Z`;
  return { d: `M ${a.x} ${a.y} Q ${c.x} ${c.y} ${b.x} ${b.y}`, label, head };
}

export default function MeshMap() {
  const nav = useNavigate();
  const activityPoll = usePoll<Activity>(api.activity, 2000);
  const channelsPoll = usePoll<Channel[]>(api.channels, 2000);
  const meshPoll = usePoll<MeshInfo>(api.mesh, 5000);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<HoverInfo | null>(null);
  const [mouse, setMouse] = useState<{ x: number; y: number }>({ x: 0, y: 0 });

  const sessions = activityPoll.data?.sessions ?? [];
  const channels = channelsPoll.data ?? [];
  const trail = activityPoll.data?.trail ?? [];
  const waiting = activityPoll.data?.waiting ?? [];
  const mesh = meshPoll.data;

  const recent = useMemo(() => {
    const set = new Set<string>();
    for (const m of trail.slice(-6)) set.add(m.sender.replace(/^@/, ""));
    return set;
  }, [trail]);

  const layout = useMemo(
    () => computeLayout(sessions, channels, recent, waiting),
    [sessions, channels, recent, waiting],
  );

  const nodeCount = new Set(sessions.map((s) => s.node)).size;
  const customCount = channels.filter((c) => c.kind === "custom").length;
  const lastTraffic = trail.length > 0 ? trail[trail.length - 1] : null;

  const target = (s: ActivitySession): string =>
    s.remote ? `${s.name}@${s.node}` : s.name;

  function openSession(s: ActivitySession) {
    nav(`/session/${encodeURIComponent(target(s))}`);
  }

  function onMove(e: React.MouseEvent) {
    const r = wrapRef.current?.getBoundingClientRect();
    if (!r) return;
    setMouse({ x: e.clientX - r.left, y: e.clientY - r.top });
  }

  return (
    <>
      <style>{MAP_CSS}</style>
      <div className="stage-head">
        <span className="t-display">Mesh Map</span>
        <span className="mono-meta">
          {nodeCount} {nodeCount === 1 ? "node" : "nodes"} · {sessions.length}{" "}
          {sessions.length === 1 ? "session" : "sessions"} · {customCount} custom{" "}
          {customCount === 1 ? "channel" : "channels"}
        </span>
        <span style={{ flex: 1 }} />
        {lastTraffic && (
          <span className="mono-meta">last traffic {relTime(lastTraffic.created_at)} ago</span>
        )}
        {activityPoll.error && (
          <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>offline</span>
        )}
      </div>

      <div className="stage-body">
        {mesh && (
          <div
            className="strip"
            style={{
              display: "flex",
              alignItems: "center",
              flexWrap: "wrap",
              gap: 10,
              marginBottom: "var(--sp-4)",
              padding: "8px 14px",
            }}
          >
            {mesh.in_mesh ? (
              <>
                <span className="label">Mesh</span>
                <span className="mono" style={{ color: "var(--text-hi)" }}>
                  mesh {mesh.mesh} · node {mesh.node}
                </span>
                {(mesh.peers ?? []).map((p) => (
                  <span
                    key={p.node}
                    className="chip"
                    title={p.url ?? undefined}
                    style={{ gap: 6 }}
                  >
                    <span
                      aria-hidden
                      style={{
                        width: 7,
                        height: 7,
                        borderRadius: "50%",
                        display: "inline-block",
                        background: p.link_up ? "var(--live)" : "var(--offline)",
                        boxShadow: p.link_up ? "0 0 5px var(--live)" : "none",
                      }}
                    />
                    {p.node}
                    <span style={{ color: p.link_up ? "var(--text-dim)" : "var(--offline)" }}>
                      {p.link_up ? "linked" : "unreachable"} · {p.agents}{" "}
                      {p.agents === 1 ? "agent" : "agents"}
                    </span>
                  </span>
                ))}
                {mesh.relay?.url && (
                  <span
                    className="chip"
                    title={mesh.relay.url}
                    style={{
                      color:
                        mesh.relay.connected_at != null ? "var(--live)" : "var(--sig-gate)",
                      borderColor:
                        mesh.relay.connected_at != null ? "var(--live)" : "var(--sig-gate)",
                    }}
                  >
                    {mesh.relay.connected_at != null
                      ? `relay · connected ${relTime(mesh.relay.connected_at)}`
                      : "relay · down"}
                  </span>
                )}
              </>
            ) : (
              <>
                <span style={{ color: "var(--text-mid)", fontSize: "0.8125rem" }}>
                  standalone node {mesh.node} — not joined to a mesh
                </span>
                <span className="mono-meta">aspen mesh init | join</span>
              </>
            )}
          </div>
        )}
        {sessions.length === 0 ? (
          <Empty mark="◈">No sessions in the mesh yet.</Empty>
        ) : (
          <div
            ref={wrapRef}
            style={{ position: "relative" }}
            onMouseMove={onMove}
          >
            <svg
              viewBox={`0 0 ${layout.width} ${layout.height}`}
              style={{ width: "100%", height: "auto", display: "block" }}
              role="img"
              aria-label="Mesh topology map"
            >
              {/* Node regions */}
              {layout.regions.map((rg) => (
                <g key={rg.node}>
                  <path
                    d={`M ${rg.x + 10} ${rg.y} L ${rg.x + rg.w} ${rg.y} L ${rg.x + rg.w} ${
                      rg.y + rg.h
                    } L ${rg.x} ${rg.y + rg.h} L ${rg.x} ${rg.y + 10} Z`}
                    fill="var(--bg-panel)"
                    stroke="var(--line)"
                    strokeWidth={1}
                  />
                  {/* engraved header strip */}
                  <path
                    d={`M ${rg.x + 10} ${rg.y} L ${rg.x + rg.w} ${rg.y} L ${rg.x + rg.w} ${
                      rg.y + NODE_HEADER
                    } L ${rg.x} ${rg.y + NODE_HEADER} L ${rg.x} ${rg.y + 10} Z`}
                    fill="var(--bg-strip)"
                    stroke="var(--line)"
                    strokeWidth={1}
                  />
                  <text
                    x={rg.x + INNER_PAD_X}
                    y={rg.y + NODE_HEADER / 2 + 5}
                    fill="var(--text-mid)"
                    style={{
                      font: "700 13px/1 var(--font-display)",
                      letterSpacing: "0.16em",
                      textTransform: "uppercase",
                    }}
                  >
                    {rg.node}
                  </text>
                  <text
                    x={rg.x + rg.w - INNER_PAD_X}
                    y={rg.y + NODE_HEADER / 2 + 5}
                    textAnchor="end"
                    fill="var(--text-dim)"
                    style={{ font: "500 11px/1 var(--font-mono)", letterSpacing: "0.04em" }}
                  >
                    NODE
                  </text>
                </g>
              ))}

              {/* Channel sub-labels */}
              {layout.channelLabels.map((c) => (
                <text
                  key={`${c.node}:${c.text}`}
                  x={c.x}
                  y={c.y + 12}
                  fill="var(--text-dim)"
                  style={{ font: "500 11px/1 var(--font-mono)", letterSpacing: "0.04em" }}
                >
                  #{c.text}
                </text>
              ))}

              {/* Custom-channel edges (the centerpiece) */}
              {layout.edges.map((e, i) => {
                const labelW = e.channel.length * 6.6 + 14;
                return (
                  <g key={`edge:${e.channel}:${i}`} className="mm-edge">
                    {e.spokes.map((sp, j) => (
                      <path
                        key={j}
                        d={arcPath(sp, e.hub)}
                        fill="none"
                        stroke="var(--sig-notice)"
                        strokeWidth={1.25}
                        strokeOpacity={0.7}
                      />
                    ))}
                    <circle cx={e.hub.x} cy={e.hub.y} r={3} fill="var(--sig-notice)" />
                    <rect
                      x={e.hub.x - labelW / 2}
                      y={e.hub.y - 22}
                      width={labelW}
                      height={16}
                      fill="var(--sig-notice-dim)"
                      stroke="var(--sig-notice)"
                      strokeWidth={1}
                    />
                    <text
                      x={e.hub.x}
                      y={e.hub.y - 10}
                      textAnchor="middle"
                      fill="var(--sig-notice)"
                      style={{ font: "600 10px/1 var(--font-display)", letterSpacing: "0.08em" }}
                    >
                      {e.channel}
                    </text>
                  </g>
                );
              })}

              {/* Likely-waiting arrows (dashed amber, agent → waited-on) */}
              {layout.waits.map((w, i) => {
                const g = waitGeom(w.from, w.to);
                return (
                  <g key={`wait:${w.agent}:${w.on}:${i}`}>
                    <title>{`@${w.agent.replace(/^@/, "")} likely waiting on ${
                      w.on === "operator" ? "@operator" : `@${w.on.replace(/^@/, "")}`
                    } for ${relTime(w.since)}${w.snippet ? ` — ${w.snippet}` : ""}`}</title>
                    <path
                      d={g.d}
                      fill="none"
                      stroke="var(--sig-normal)"
                      strokeWidth={1.25}
                      strokeOpacity={0.85}
                      strokeDasharray="5 4"
                    />
                    <path d={g.head} fill="var(--sig-normal)" fillOpacity={0.85} />
                    <text
                      x={g.label.x}
                      y={g.label.y - 5}
                      textAnchor="middle"
                      fill="var(--sig-normal)"
                      style={{ font: "400 10px/1.4 var(--font-mono)", letterSpacing: "0.01em" }}
                    >
                      waiting {relTime(w.since)}
                    </text>
                  </g>
                );
              })}

              {/* Session markers */}
              {layout.markers.map((m) => {
                const color = presenceColor[m.presence];
                const name = m.s.name;
                return (
                  <g
                    key={`${m.s.node}:${m.s.name}`}
                    className="mm-marker"
                    role="button"
                    tabIndex={0}
                    onClick={() => openSession(m.s)}
                    onKeyDown={(ev) => {
                      if (ev.key === "Enter" || ev.key === " ") {
                        ev.preventDefault();
                        openSession(m.s);
                      }
                    }}
                    onMouseEnter={() =>
                      setHover({
                        name,
                        node: m.s.node,
                        channel: m.s.channel,
                        state: presenceState[m.presence],
                        pending: m.s.pending,
                        remote: m.s.remote,
                      })
                    }
                    onMouseLeave={() => setHover(null)}
                    style={{ cursor: "pointer" }}
                  >
                    <title>{`@${name} · #${m.s.channel} · ${m.s.node} · ${presenceState[m.presence]}${
                      m.s.pending > 0 ? ` · ${m.s.pending} pending` : ""
                    }`}</title>

                    {/* busy pulse ring */}
                    {m.presence === "busy" && (
                      <rect
                        className="mm-pulse"
                        x={m.x - MARKER / 2}
                        y={m.y - MARKER / 2}
                        width={MARKER}
                        height={MARKER}
                        fill="none"
                        stroke={color}
                        strokeWidth={1.5}
                      />
                    )}

                    {/* glow underlay for busy */}
                    {m.presence === "busy" && (
                      <path
                        d={chamferSquare(m.x, m.y, MARKER + 8)}
                        fill={color}
                        opacity={0.18}
                      />
                    )}

                    {/* recent-traffic accent */}
                    {m.recent && (
                      <path
                        d={chamferSquare(m.x, m.y, MARKER + 6)}
                        fill="none"
                        stroke="var(--sig-normal)"
                        strokeWidth={1.25}
                        strokeOpacity={0.9}
                      />
                    )}

                    {/* the contact */}
                    <path
                      d={chamferSquare(m.x, m.y, MARKER)}
                      fill={color}
                      stroke="var(--line-hi)"
                      strokeWidth={1}
                    />

                    {/* pending badge */}
                    {m.s.pending > 0 && (
                      <>
                        <circle
                          cx={m.x + MARKER / 2}
                          cy={m.y - MARKER / 2}
                          r={8}
                          fill="var(--sig-gate)"
                          stroke="var(--bg-panel)"
                          strokeWidth={1.5}
                        />
                        <text
                          x={m.x + MARKER / 2}
                          y={m.y - MARKER / 2 + 3.5}
                          textAnchor="middle"
                          fill="var(--text-hi)"
                          style={{ font: "600 9px/1 var(--font-mono)" }}
                        >
                          {m.s.pending > 9 ? "9+" : m.s.pending}
                        </text>
                      </>
                    )}

                    {/* name label */}
                    <text
                      x={m.x}
                      y={m.y + MARKER / 2 + 15}
                      textAnchor="middle"
                      fill="var(--text-mid)"
                      style={{ font: "500 11px/1 var(--font-mono)" }}
                    >
                      @{name}
                      {m.s.remote ? "*" : ""}
                    </text>
                  </g>
                );
              })}

              {/* Operator marker (distinct, --sig-normal) */}
              <g>
                <title>@operator · the console</title>
                <path
                  d={`M ${layout.operator.x} ${layout.operator.y - OPERATOR_R} L ${
                    layout.operator.x + OPERATOR_R
                  } ${layout.operator.y} L ${layout.operator.x} ${
                    layout.operator.y + OPERATOR_R
                  } L ${layout.operator.x - OPERATOR_R} ${layout.operator.y} Z`}
                  fill="var(--sig-normal-dim)"
                  stroke="var(--sig-normal)"
                  strokeWidth={1.5}
                />
                <text
                  x={layout.operator.x}
                  y={layout.operator.y + OPERATOR_R + 16}
                  textAnchor="middle"
                  fill="var(--sig-normal)"
                  style={{ font: "600 11px/1 var(--font-mono)", letterSpacing: "0.04em" }}
                >
                  @operator
                </text>
                <text
                  x={layout.operator.x}
                  y={layout.operator.y - OPERATOR_R - 8}
                  textAnchor="middle"
                  fill="var(--text-dim)"
                  style={{
                    font: "600 9px/1 var(--font-display)",
                    letterSpacing: "0.16em",
                    textTransform: "uppercase",
                  }}
                >
                  Console
                </text>
              </g>
            </svg>

            {/* Legend */}
            <div
              className="strip"
              style={{
                position: "absolute",
                left: 0,
                bottom: 0,
                display: "flex",
                gap: 16,
                alignItems: "center",
                padding: "8px 12px",
                pointerEvents: "none",
              }}
            >
              <LegendDot color="var(--live)" label="busy" />
              <LegendDot color="var(--idle)" label="idle" />
              <LegendDot color="var(--offline)" label="offline" />
              <span className="micro" style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                <svg width={22} height={8} aria-hidden>
                  <path d="M 0 4 Q 11 -4 22 4" fill="none" stroke="var(--sig-notice)" strokeWidth={1.5} />
                </svg>
                <span style={{ color: "var(--text-dim)" }}>custom channel</span>
              </span>
              <span className="micro" style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                <svg width={22} height={8} aria-hidden>
                  <path
                    d="M 0 4 Q 11 -4 22 4"
                    fill="none"
                    stroke="var(--sig-normal)"
                    strokeWidth={1.5}
                    strokeDasharray="4 3"
                  />
                </svg>
                <span style={{ color: "var(--text-dim)" }}>likely waiting</span>
              </span>
            </div>

            {/* Hover tooltip */}
            {hover && (
              <div
                className="strip flat"
                style={{
                  position: "absolute",
                  left: mouse.x + 14,
                  top: mouse.y + 14,
                  pointerEvents: "none",
                  minWidth: 160,
                  zIndex: 5,
                  background: "var(--bg-strip-2)",
                }}
              >
                <div className="mono" style={{ fontWeight: 500, marginBottom: 4 }}>
                  @{hover.name}
                  {hover.remote && (
                    <span className="mono-meta" style={{ marginLeft: 6 }}>remote</span>
                  )}
                </div>
                <div className="mono-meta">#{hover.channel} · {hover.node}</div>
                <div className="mono-meta">{hover.state}</div>
                {hover.pending > 0 && (
                  <div className="mono-meta" style={{ color: "var(--sig-gate)" }}>
                    {hover.pending} pending
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </div>
    </>
  );
}

function LegendDot({ color, label }: { color: string; label: string }) {
  return (
    <span className="micro" style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
      <svg width={12} height={12} aria-hidden>
        <path
          d="M 3 0 L 12 0 L 12 12 L 0 12 L 0 3 Z"
          fill={color}
          stroke="var(--line-hi)"
          strokeWidth={1}
        />
      </svg>
      <span style={{ color: "var(--text-dim)" }}>{label}</span>
    </span>
  );
}

const MAP_CSS = `
.mm-pulse {
  transform-box: fill-box;
  transform-origin: center;
  animation: mm-pulse 1.8s var(--ease-seat, ease-out) infinite;
}
@keyframes mm-pulse {
  0%   { opacity: 0.55; transform: scale(1); }
  70%  { opacity: 0;    transform: scale(1.9); }
  100% { opacity: 0;    transform: scale(1.9); }
}
.mm-marker:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
@media (prefers-reduced-motion: reduce) {
  .mm-pulse { animation: none !important; display: none; }
}
`;
