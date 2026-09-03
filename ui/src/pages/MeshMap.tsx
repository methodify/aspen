// MeshMap — the signature spatial view: "the mesh as a place." A live
// topological map rendered as inline SVG. Sessions are tracked contacts,
// grouped into node regions and, within a node, by #channel (repo). Custom
// channels are drawn as edges arcing between their member contacts — the
// "conversations across repos/nodes" made visible. Everything is polled every
// 2s; color comes only from the Switchboard tokens.

import { useMemo, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import type { Activity, ActivitySession, Channel, Link, MeshInfo, WaitingEdge } from "../api";
import { usePoll } from "../hooks";
import { Empty, presenceOf, relTime } from "../components";
import { Modal, NewChannelDialog } from "../channels";
import { MeshPanel } from "../meshPanel";
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
        // Remote sessions arrive already node-qualified (`far@beta`);
        // qualifying again produced `far@beta@beta` and channel members
        // never resolved to their marker.
        const qualified = s.name.includes("@") ? s.name : `${s.name}@${s.node}`;
        byNameNode.set(qualified, pt);
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

export default function MeshMap({ toggle }: { toggle?: ReactNode }) {
  const nav = useNavigate();
  const activityPoll = usePoll<Activity>(api.activity, 2000);
  const channelsPoll = usePoll<Channel[]>(api.channels, 2000);
  const meshPoll = usePoll<MeshInfo>(api.mesh, 5000);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<HoverInfo | null>(null);
  // Connect: pick agents on the map, then create a channel joining them.
  // Shift/Ctrl-click always selects; the header toggle makes plain clicks
  // select too.
  const [connectMode, setConnectMode] = useState(false);
  // Selection is an ORDERED list of endpoints: `agent:key[@node]`,
  // `repo:handle@node`, `node:name`, `operator`. Two → link (from → to in
  // selection order); three or more agents → channel.
  const [selected, setSelected] = useState<string[]>([]);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [linkDialog, setLinkDialog] = useState(false);
  const [linkTwoWay, setLinkTwoWay] = useState(false);
  const [linkPurpose, setLinkPurpose] = useState("");
  const [linkUrgency, setLinkUrgency] = useState("");
  const [linkErr, setLinkErr] = useState<string | null>(null);
  const [selectedLink, setSelectedLink] = useState<Link | null>(null);
  const linksPoll = usePoll<Link[]>(api.links, 5000);
  const links = linksPoll.data ?? [];
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

  // /api/activity already returns remote sessions as `name@node`; appending
  // the node again produced `main@anindor-win@anindor-win` (bug).
  const target = (s: ActivitySession): string => s.name;

  function openSession(s: ActivitySession) {
    nav(`/session/${encodeURIComponent(target(s))}`);
  }

  const selfNode = mesh?.node ?? null;
  const agentEndpoint = (s: ActivitySession) =>
    s.remote ? `agent:${s.name}` : `agent:${s.name}`;
  const repoEndpoint = (node: string, handle: string) => `repo:${handle}@${node}`;

  function toggleEndpoint(ep: string) {
    setSelectedLink(null);
    setSelected((cur) => (cur.includes(ep) ? cur.filter((x) => x !== ep) : [...cur, ep]));
  }

  function toggleSelect(s: ActivitySession) {
    toggleEndpoint(agentEndpoint(s));
  }

  function markerClick(s: ActivitySession, e: React.MouseEvent | React.KeyboardEvent) {
    if (connectMode || e.shiftKey || e.ctrlKey || e.metaKey) toggleSelect(s);
    else openSession(s);
  }

  const allAgents = selected.every((ep) => ep.startsWith("agent:"));
  const humanEp = (ep: string) => {
    if (ep === "operator") return "@operator";
    if (ep.startsWith("agent:")) return `@${ep.slice(6)}`;
    if (ep.startsWith("repo:")) return `#${ep.slice(5)}`;
    if (ep.startsWith("node:")) return `node ${ep.slice(5)}`;
    return ep;
  };

  async function createLink() {
    if (selected.length !== 2) return;
    setLinkErr(null);
    try {
      await api.addLink({
        from: selected[0],
        to: selected[1],
        two_way: linkTwoWay,
        purpose: linkPurpose.trim() || undefined,
        urgency: linkUrgency || undefined,
      });
      setLinkDialog(false);
      setLinkPurpose("");
      setLinkUrgency("");
      setLinkTwoWay(false);
      setSelected([]);
      setConnectMode(false);
      await linksPoll.refresh();
    } catch (e) {
      setLinkErr(e instanceof Error ? e.message : "link failed");
    }
  }

  async function removeLink(l: Link) {
    try {
      await api.deleteLink(l.id);
      setSelectedLink(null);
      await linksPoll.refresh();
    } catch (e) {
      setLinkErr(e instanceof Error ? e.message : "remove failed");
    }
  }

  /** Anchor point for an endpoint on the current layout. */
  function anchor(ep: string): Pt | null {
    if (ep === "operator") return layout.operator;
    if (ep.startsWith("agent:")) {
      const key = ep.slice(6);
      const m = layout.markers.find((mk) => (mk.s.name.includes("@") && key === mk.s.name) || key === `${mk.s.name}@${mk.s.node}` || key === mk.s.name);
      return m ? { x: m.x, y: m.y } : null;
    }
    if (ep.startsWith("repo:")) {
      const rest = ep.slice(5);
      const [handle, node] = rest.includes("@") ? [rest.slice(0, rest.lastIndexOf("@")), rest.slice(rest.lastIndexOf("@") + 1)] : [rest, selfNode ?? ""];
      const c = layout.channelLabels.find((cl) => cl.text === handle && (cl.node === node || !node));
      return c ? { x: c.x + 24, y: c.y + 8 } : null;
    }
    if (ep.startsWith("node:")) {
      const rg = layout.regions.find((r) => r.node === ep.slice(5));
      return rg ? { x: rg.x + rg.w / 2, y: rg.y + 14 } : null;
    }
    return null;
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
        <span className="t-display">Mesh</span>
        {toggle}
        <span className="mono-meta">
          {nodeCount} {nodeCount === 1 ? "node" : "nodes"} · {sessions.length}{" "}
          {sessions.length === 1 ? "session" : "sessions"} · {customCount} custom{" "}
          {customCount === 1 ? "channel" : "channels"}
        </span>
        <span style={{ flex: 1 }} />
        {selectedLink ? (
          <>
            <span className="mono-meta">
              link {humanEp(selectedLink.src)} {selectedLink.two_way ? "↔" : "→"} {humanEp(selectedLink.dst)}
              {selectedLink.purpose ? ` — ${selectedLink.purpose}` : ""}
            </span>
            <button className="btn danger sm" onClick={() => void removeLink(selectedLink)}>remove link</button>
            <button className="btn ghost sm" onClick={() => setSelectedLink(null)}>cancel</button>
          </>
        ) : selected.length > 0 ? (
          <>
            <span className="mono-meta">
              {selected.length} selected · {selected.map(humanEp).join(" ")}
            </span>
            {selected.length === 2 && (
              <button className="btn primary sm" onClick={() => setLinkDialog(true)} title="declare a directed pathway from the first selected to the second">
                link {humanEp(selected[0])} → {humanEp(selected[1])}
              </button>
            )}
            {selected.length >= 2 && allAgents && (
              <button className={`btn sm${selected.length === 2 ? " ghost" : " primary"}`} onClick={() => setDialogOpen(true)}>
                new channel
              </button>
            )}
            {selected.length > 2 && !allAgents && (
              <span className="mono-meta" style={{ color: "var(--sig-normal)" }}>a link has two ends; channels take agents</span>
            )}
            <button className="btn ghost sm" onClick={() => setSelected([])}>clear</button>
          </>
        ) : (
          <button
            className={`btn sm${connectMode ? " primary" : " ghost"}`}
            onClick={() => setConnectMode((v) => !v)}
            title="pick two endpoints (agents, repos, nodes, the operator) for a link, or agents for a channel — shift-click also selects"
          >
            {connectMode ? "connecting: click endpoints…" : "connect"}
          </button>
        )}
        {lastTraffic && (
          <span className="mono-meta">last traffic {relTime(lastTraffic.created_at)} ago</span>
        )}
        {activityPoll.error && (
          <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>offline</span>
        )}
      </div>

      {dialogOpen && (
        <NewChannelDialog
          agents={sessions.map((s) => target(s))}
          initialMembers={selected.filter((e) => e.startsWith("agent:")).map((e) => `@${e.slice(6)}`)}
          onClose={() => setDialogOpen(false)}
          onCreated={(name) => {
            setDialogOpen(false);
            setSelected([]);
            setConnectMode(false);
            nav(`/flow/${encodeURIComponent(name)}`);
          }}
        />
      )}

      {linkDialog && selected.length === 2 && (
        <Modal title="New link" onClose={() => setLinkDialog(false)}>
          <div className="mono" style={{ marginBottom: 10, display: "flex", alignItems: "center", gap: 8 }}>
            <span>{humanEp(selected[0])}</span>
            <button
              className="btn ghost sm"
              onClick={() => setLinkTwoWay((v) => !v)}
              title="toggle direction: one-way (from → to; replies always allowed) or two-way"
            >
              {linkTwoWay ? "↔" : "→"}
            </button>
            <span>{humanEp(selected[1])}</span>
            <button className="btn ghost sm" onClick={() => setSelected([selected[1], selected[0]])} title="swap ends">
              swap
            </button>
          </div>
          <div className="label" style={{ marginBottom: 4 }}>
            Purpose — what the {linkTwoWay ? "agents on both ends are" : `agents at ${humanEp(selected[0])} are`} told this link is for
          </div>
          <input
            style={{ width: "100%", marginBottom: 10 }}
            placeholder="e.g. file task-tracker bugs and questions here"
            value={linkPurpose}
            onChange={(e) => setLinkPurpose(e.target.value)}
            autoFocus
            onKeyDown={(e) => e.key === "Enter" && void createLink()}
          />
          <div className="label" style={{ marginBottom: 4 }}>Default urgency (optional)</div>
          <select value={linkUrgency} onChange={(e) => setLinkUrgency(e.target.value)} style={{ marginBottom: 12 }}>
            <option value="">none</option>
            <option value="normal">normal</option>
            <option value="gating">gating</option>
            <option value="notice">notice</option>
          </select>
          {linkErr && <div className="error-bar">{linkErr}</div>}
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
            <button className="btn ghost" onClick={() => setLinkDialog(false)}>cancel</button>
            <button className="btn primary" onClick={() => void createLink()}>declare link</button>
          </div>
        </Modal>
      )}

      <div className="stage-body">
        <MeshPanel />
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
              // Layout units are pixels: the map has an intrinsic 1:1 scale
              // (22px markers, 300px node panels) and only ever SHRINKS to
              // fit the viewport — stretching a small mesh up to poster
              // size made a one-node map fill the whole screen.
              style={{
                width: layout.width,
                maxWidth: "100%",
                height: "auto",
                display: "block",
              }}
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
                    fill={selected.includes(`node:${rg.node}`) ? "var(--sig-notice)" : "var(--text-mid)"}
                    style={{
                      font: "700 13px/1 var(--font-display)",
                      letterSpacing: "0.16em",
                      textTransform: "uppercase",
                      cursor: "pointer",
                    }}
                    onClick={() => toggleEndpoint(`node:${rg.node}`)}
                  >
                    <title>{`node ${rg.node} — click to select every agent here as a link endpoint`}</title>
                    {selected.includes(`node:${rg.node}`) ? "▣ " : ""}{rg.node}
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
              {layout.channelLabels.map((c) => {
                const ep = repoEndpoint(c.node, c.text);
                const sel = selected.includes(ep);
                return (
                  <text
                    key={`${c.node}:${c.text}`}
                    x={c.x}
                    y={c.y + 12}
                    fill={sel ? "var(--sig-notice)" : "var(--text-dim)"}
                    style={{ font: `${sel ? 700 : 500} 11px/1 var(--font-mono)`, letterSpacing: "0.04em", cursor: "pointer" }}
                    onClick={() => toggleEndpoint(ep)}
                  >
                    <title>{`#${c.text} on ${c.node} — click to select the repo as a link endpoint`}</title>
                    {sel ? "▣ " : ""}#{c.text}
                  </text>
                );
              })}

              {/* Declared links: directed pathways between endpoints */}
              {links.map((l) => {
                const a = anchor(l.src);
                const b = anchor(l.dst);
                if (!a || !b) return null;
                const g = waitGeom(a, b);
                const sel = selectedLink?.id === l.id;
                return (
                  <g
                    key={`link:${l.id}`}
                    className="mm-link"
                    style={{ cursor: "pointer" }}
                    onClick={() => setSelectedLink(sel ? null : l)}
                  >
                    <title>{`${humanEp(l.src)} ${l.two_way ? "↔" : "→"} ${humanEp(l.dst)}${l.purpose ? ` — ${l.purpose}` : ""} (click to remove)`}</title>
                    <path d={g.d} fill="none" stroke="transparent" strokeWidth={12} />
                    <path
                      d={g.d}
                      fill="none"
                      stroke={sel ? "var(--sig-gate)" : "var(--text-hi)"}
                      strokeWidth={sel ? 2.5 : 2}
                      strokeOpacity={0.85}
                    />
                    <path d={g.head} fill={sel ? "var(--sig-gate)" : "var(--text-hi)"} />
                    {l.two_way && <circle cx={a.x} cy={a.y} r={3.5} fill="var(--text-hi)" />}
                    {(() => {
                      const text = (l.purpose ?? "link").slice(0, 28) + ((l.purpose ?? "").length > 28 ? "…" : "");
                      const w = text.length * 6.2 + 12;
                      return (
                        <>
                          <rect x={g.label.x - w / 2} y={g.label.y - 9} width={w} height={16} fill="var(--bg-panel)" stroke="var(--text-hi)" strokeWidth={1} />
                          <text x={g.label.x} y={g.label.y + 3} textAnchor="middle" fill="var(--text-hi)" style={{ font: "500 10px/1 var(--font-mono)" }}>
                            {text}
                          </text>
                        </>
                      );
                    })()}
                  </g>
                );
              })}

              {/* Custom-channel edges (the centerpiece) */}
              {layout.edges.map((e, i) => {
                const labelW = e.channel.length * 6.6 + 14;
                return (
                  <g
                    key={`edge:${e.channel}:${i}`}
                    className="mm-edge"
                    role="link"
                    style={{ cursor: "pointer" }}
                    onClick={() => nav(`/flow/${encodeURIComponent(e.channel)}`)}
                  >
                    <title>{`#${e.channel} — open in Conversations`}</title>
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
                    onClick={(ev) => markerClick(m.s, ev)}
                    onKeyDown={(ev) => {
                      if (ev.key === "Enter" || ev.key === " ") {
                        ev.preventDefault();
                        markerClick(m.s, ev);
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

                    {/* selection ring (connect) */}
                    {selected.includes(agentEndpoint(m.s)) && (
                      <path
                        d={chamferSquare(m.x, m.y, MARKER + 10)}
                        fill="none"
                        stroke="var(--sig-notice)"
                        strokeWidth={2}
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
              <g style={{ cursor: "pointer" }} onClick={() => toggleEndpoint("operator")}>
                <title>@operator · the console — click to select as a link endpoint</title>
                {selected.includes("operator") && (
                  <path
                    d={`M ${layout.operator.x} ${layout.operator.y - OPERATOR_R - 6} L ${
                      layout.operator.x + OPERATOR_R + 6
                    } ${layout.operator.y} L ${layout.operator.x} ${
                      layout.operator.y + OPERATOR_R + 6
                    } L ${layout.operator.x - OPERATOR_R - 6} ${layout.operator.y} Z`}
                    fill="none"
                    stroke="var(--sig-notice)"
                    strokeWidth={2}
                  />
                )}
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
                  <path d="M 0 4 L 16 4" fill="none" stroke="var(--text-hi)" strokeWidth={2} />
                  <path d="M 22 4 L 15 1 L 15 7 Z" fill="var(--text-hi)" />
                </svg>
                <span style={{ color: "var(--text-dim)" }}>link</span>
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
