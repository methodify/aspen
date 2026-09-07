// Boards (PROPOSALS-2026-09 §6): a named split-tree layout of panes —
// sessions from any node, artifact viewers, Now — stored on the node and
// synced across the mesh. Dividers drag; panes split, close, swap, zoom;
// one pane has focus and takes the keyboard; a pane needing attention is
// marked; broadcast sends one message to every session pane.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, type Agent, type Board, type BoardNode, type BoardPane, type OpenPrompt } from "../api";
import { useAppData } from "../App";
import { useHotkeys } from "../hotkeys";
import { ErrorBar } from "../components";
import { SessionView } from "./Session";
import View from "./View";
import "./board.css";

// ------------------------------------------------------------ layout ops

let paneSeq = 0;
function newId(): string {
  paneSeq += 1;
  return `${Date.now().toString(36)}-${paneSeq}`;
}

export function emptyPane(): BoardPane {
  return { kind: "empty", id: newId() };
}

/** Presets: split trees with empty panes. */
export const PRESETS: { key: string; label: string; make: () => BoardNode }[] = [
  { key: "1", label: "1", make: () => emptyPane() },
  { key: "1|2", label: "1 | 2", make: () => ({ kind: "split", id: newId(), dir: "row", sizes: [50, 50], children: [emptyPane(), emptyPane()] }) },
  { key: "1/2", label: "1 / 2", make: () => ({ kind: "split", id: newId(), dir: "col", sizes: [50, 50], children: [emptyPane(), emptyPane()] }) },
  {
    key: "1|2/3",
    label: "1 | 2/3",
    make: () => ({
      kind: "split",
      id: newId(),
      dir: "row",
      sizes: [50, 50],
      children: [emptyPane(), { kind: "split", id: newId(), dir: "col", sizes: [50, 50], children: [emptyPane(), emptyPane()] }],
    }),
  },
  {
    key: "2x2",
    label: "2 × 2",
    make: () => ({
      kind: "split",
      id: newId(),
      dir: "col",
      sizes: [50, 50],
      children: [
        { kind: "split", id: newId(), dir: "row", sizes: [50, 50], children: [emptyPane(), emptyPane()] },
        { kind: "split", id: newId(), dir: "row", sizes: [50, 50], children: [emptyPane(), emptyPane()] },
      ],
    }),
  },
  {
    key: "main+stack",
    label: "main + stack",
    make: () => ({
      kind: "split",
      id: newId(),
      dir: "row",
      sizes: [62, 38],
      children: [emptyPane(), { kind: "split", id: newId(), dir: "col", sizes: [50, 50], children: [emptyPane(), emptyPane()] }],
    }),
  },
];

function panesOf(n: BoardNode): BoardPane[] {
  return n.kind === "split" ? n.children.flatMap(panesOf) : [n];
}

function mapTree(n: BoardNode, f: (p: BoardPane) => BoardNode): BoardNode {
  return n.kind === "split" ? { ...n, children: n.children.map((c) => mapTree(c, f)) } : f(n);
}

/** Replace a pane by id with a split of it and a new empty pane. */
function splitPane(root: BoardNode, id: string, dir: "row" | "col"): BoardNode {
  return mapTree(root, (p) =>
    p.id === id ? { kind: "split", id: newId(), dir, sizes: [50, 50], children: [p, emptyPane()] } : p,
  );
}

/** Remove a pane; a split left with one child collapses to it. */
function removePane(root: BoardNode, id: string): BoardNode | null {
  if (root.kind !== "split") return root.id === id ? null : root;
  const kept: BoardNode[] = [];
  const sizes: number[] = [];
  root.children.forEach((c, i) => {
    const r = removePane(c, id);
    if (r) {
      kept.push(r);
      sizes.push(root.sizes[i] ?? 50);
    }
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0]!;
  const total = sizes.reduce((a, b) => a + b, 0) || 1;
  return { ...root, children: kept, sizes: sizes.map((s) => (s / total) * 100) };
}

function swapPanes(root: BoardNode, a: string, b: string): BoardNode {
  const pa = panesOf(root).find((p) => p.id === a);
  const pb = panesOf(root).find((p) => p.id === b);
  if (!pa || !pb) return root;
  return mapTree(root, (p) => (p.id === a ? { ...pb, id: a } : p.id === b ? { ...pa, id: b } : p));
}

function setSizes(root: BoardNode, splitId: string, sizes: number[]): BoardNode {
  if (root.kind !== "split") return root;
  if (root.id === splitId) return { ...root, sizes };
  return { ...root, children: root.children.map((c) => setSizes(c, splitId, sizes)) };
}

function setPane(root: BoardNode, id: string, pane: BoardPane): BoardNode {
  return mapTree(root, (p) => (p.id === id ? { ...pane, id } : p));
}

/** Auto layout for N panes: a grid by count, or main + stack. */
function autoLayout(panes: BoardPane[], style: "grid" | "main"): BoardNode {
  if (panes.length === 0) return emptyPane();
  if (panes.length === 1) return panes[0]!;
  if (style === "main") {
    const [main, ...rest] = panes;
    return {
      kind: "split",
      id: "auto-root",
      dir: "row",
      sizes: [60, 40],
      children: [main!, { kind: "split", id: "auto-stack", dir: "col", sizes: rest.map(() => 100 / rest.length), children: rest }],
    };
  }
  const cols = Math.ceil(Math.sqrt(panes.length));
  const rows: BoardNode[] = [];
  for (let i = 0; i < panes.length; i += cols) {
    const slice = panes.slice(i, i + cols);
    rows.push(
      slice.length === 1
        ? slice[0]!
        : { kind: "split", id: `auto-r${i}`, dir: "row", sizes: slice.map(() => 100 / slice.length), children: slice },
    );
  }
  return rows.length === 1 ? rows[0]! : { kind: "split", id: "auto-root", dir: "col", sizes: rows.map(() => 100 / rows.length), children: rows };
}

// ------------------------------------------------------------ dynamic

export interface DynamicQuery {
  node?: string;
  channel?: string;
  state?: "busy" | "live" | "attention" | "activity" | "any";
  name?: string;
  style?: "grid" | "main";
}

function matchQuery(a: Agent, q: DynamicQuery, attention: Set<string>): boolean {
  if (q.node && a.node !== q.node) return false;
  if (q.channel && a.channel !== q.channel) return false;
  if (q.name && !a.name.toLowerCase().includes(q.name.toLowerCase())) return false;
  switch (q.state ?? "live") {
    case "busy":
      return a.live && a.turn_state === "busy";
    case "live":
      return a.live;
    case "attention":
      return attention.has(a.name);
    case "activity":
      return (a.activities?.running ?? 0) > 0;
    default:
      return true;
  }
}

// ------------------------------------------------------------ the page

export default function BoardPage() {
  const { id = "" } = useParams<{ id: string }>();
  const nav = useNavigate();
  const { agents } = useAppData();
  const [board, setBoard] = useState<Board | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [focus, setFocus] = useState<string | null>(null);
  const [zoom, setZoom] = useState<string | null>(null);
  const [broadcast, setBroadcast] = useState(false);
  const [picker, setPicker] = useState<string | null>(null); // pane id being filled
  const [attention, setAttention] = useState<Set<string>>(new Set());
  const [narrow, setNarrow] = useState(() => window.innerWidth < 900);
  const [tab, setTab] = useState(0);
  const [renaming, setRenaming] = useState(false);
  const [bcastAsk, setBcastAsk] = useState(false);
  const [bcastArmed, setBcastArmed] = useState(false);
  const [deleteAsk, setDeleteAsk] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const saveTimer = useRef<number | null>(null);

  // Load.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const all = await api.boards();
        const b = all.find((x) => x.id === id);
        if (!cancelled) {
          if (b) {
            setBoard(b);
            setErr(null);
          } else setErr("no such board (it may live on a node that hasn't synced yet)");
        }
      } catch (e) {
        if (!cancelled) setErr(e instanceof Error ? e.message : "failed to load boards");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [id]);

  useEffect(() => {
    const onResize = () => setNarrow(window.innerWidth < 900);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Attention: prompts open anywhere on the fleet.
  useEffect(() => {
    let stop = false;
    const tick = async () => {
      try {
        const n = await api.needs();
        if (!stop) setAttention(new Set(n.prompts.map((p: OpenPrompt) => p.agent)));
      } catch {
        // keep the last set
      }
    };
    void tick();
    const t = window.setInterval(() => void tick(), 3000);
    return () => {
      stop = true;
      window.clearInterval(t);
    };
  }, []);

  // Persist (debounced) on every layout change.
  const update = useCallback(
    (f: (b: Board) => Board) => {
      setBoard((cur) => {
        if (!cur) return cur;
        const next = { ...f(cur), updated_at: Date.now() / 1000 };
        if (saveTimer.current) window.clearTimeout(saveTimer.current);
        saveTimer.current = window.setTimeout(() => {
          api.putBoard(next).catch((e) => setErr(`save: ${e instanceof Error ? e.message : "failed"}`));
        }, 400);
        return next;
      });
    },
    [],
  );

  // The effective tree: a dynamic board computes its panes from the fleet.
  const tree: BoardNode | null = useMemo(() => {
    if (!board) return null;
    const q = board.query;
    if (q) {
      const matched = agents.filter((a) => matchQuery(a, q, attention));
      const panes: BoardPane[] = matched.map((a) => ({ kind: "session", id: `dyn-${a.name}`, agent: a.name }));
      if (q.style === "main") panes.sort((x, y) => (attention.has(y.agent ?? "") ? 1 : 0) - (attention.has(x.agent ?? "") ? 1 : 0));
      return autoLayout(panes, q.style ?? "grid");
    }
    return board.layout;
  }, [board, agents, attention]);

  const panes = useMemo(() => (tree ? panesOf(tree) : []), [tree]);
  const sessionPanes = panes.filter((p) => p.kind === "session" && p.agent);

  // Focus follows the first pane until the operator chooses.
  useEffect(() => {
    if (!focus && panes.length) setFocus(panes[0]!.id);
    if (focus && !panes.some((p) => p.id === focus)) setFocus(panes[0]?.id ?? null);
  }, [panes, focus]);

  function moveFocus(dir: "left" | "right" | "up" | "down") {
    if (!focus) return;
    const cur = document.querySelector<HTMLElement>(`[data-pane="${focus}"]`);
    if (!cur) return;
    const r = cur.getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    let best: { id: string; d: number } | null = null;
    for (const p of panes) {
      if (p.id === focus) continue;
      const el = document.querySelector<HTMLElement>(`[data-pane="${p.id}"]`);
      if (!el) continue;
      const q = el.getBoundingClientRect();
      const qx = q.left + q.width / 2;
      const qy = q.top + q.height / 2;
      const ok =
        dir === "left" ? qx < cx - 10 : dir === "right" ? qx > cx + 10 : dir === "up" ? qy < cy - 10 : qy > cy + 10;
      if (!ok) continue;
      const d = Math.hypot(qx - cx, qy - cy);
      if (!best || d < best.d) best = { id: p.id, d };
    }
    if (best) setFocus(best.id);
  }

  function nextAttention() {
    const order = panes.filter((p) => p.kind === "session" && p.agent && attention.has(p.agent));
    if (order.length === 0) return;
    const i = order.findIndex((p) => p.id === focus);
    setFocus(order[(i + 1) % order.length]!.id);
  }

  useHotkeys("board", [
    { key: "alt+ArrowLeft", description: "focus pane to the left", handler: () => moveFocus("left") },
    { key: "alt+ArrowRight", description: "focus pane to the right", handler: () => moveFocus("right") },
    { key: "alt+ArrowUp", description: "focus pane above", handler: () => moveFocus("up") },
    { key: "alt+ArrowDown", description: "focus pane below", handler: () => moveFocus("down") },
    { key: "alt+.", description: "focus the next pane needing attention", handler: nextAttention },
    { key: "alt+z", description: "zoom the focused pane / back", handler: () => setZoom((z) => (z ? null : focus)) },
    ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => ({
      key: `alt+${n}`,
      description: `focus pane ${n}`,
      handler: () => {
        const p = panes[n - 1];
        if (p) setFocus(p.id);
      },
    })),
  ]);

  // Broadcast: after the focused pane sends, the same text goes to every
  // other session pane.
  const onSent = useCallback(
    (fromPane: string, text: string) => {
      if (!broadcast) return;
      for (const p of sessionPanes) {
        if (p.id !== fromPane && p.agent) api.sendMessage(p.agent, text).catch(() => {});
      }
    },
    [broadcast, sessionPanes],
  );

  if (err && !board) return <div className="page"><ErrorBar error={err} /><Link to="/boards">← boards</Link></div>;
  if (!board || !tree) return <div className="page dim">loading board…</div>;
  const dynamic = !!board.query;

  // ---- renderers
  const renderPane = (p: BoardPane, index: number) => {
    const focused = p.id === focus;
    const needs = p.kind === "session" && !!p.agent && attention.has(p.agent);
    const agent = p.kind === "session" ? agents.find((a) => a.name === p.agent) : undefined;
    const body = (() => {
      switch (p.kind) {
        case "session":
          return p.agent ? (
            <SessionView
              key={p.agent}
              name={p.agent}
              pane={{ id: `${board.id}:${p.id}`, focused, compact: zoom !== p.id, onFocus: () => setFocus(p.id), onSent: (t) => onSent(p.id, t) }}
            />
          ) : null;
        case "view":
          return p.agent && p.path ? <View key={`${p.agent}:${p.path}`} agent={p.agent} path={p.path} embedded /> : null;
        default:
          return (
            <div className="pane-empty">
              <button className="btn primary sm" onClick={() => setPicker(p.id)}>choose a session…</button>
              <span className="mono-meta">or drop one here</span>
            </div>
          );
      }
    })();
    return (
      <section
        key={p.id}
        data-pane={p.id}
        className={`pane${focused ? " focused" : ""}${needs ? " attention" : ""}${p.kind === "empty" ? " empty" : ""}`}
        onMouseDownCapture={() => setFocus(p.id)}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes("text/aspen-agent") || e.dataTransfer.types.includes("text/aspen-pane")) e.preventDefault();
        }}
        onDrop={(e) => {
          const other = e.dataTransfer.getData("text/aspen-pane");
          const ag = e.dataTransfer.getData("text/aspen-agent");
          if (other && other !== p.id && !dynamic) {
            e.preventDefault();
            update((b) => ({ ...b, layout: swapPanes(b.layout, other, p.id) }));
          } else if (ag && !dynamic) {
            e.preventDefault();
            update((b) => ({ ...b, layout: setPane(b.layout, p.id, { kind: "session", id: p.id, agent: ag }) }));
          }
        }}
      >
        <div
          className="pane-bar"
          draggable={!dynamic}
          onDragStart={(e) => {
            e.dataTransfer.setData("text/aspen-pane", p.id);
            e.dataTransfer.effectAllowed = "move";
          }}
          title="drag onto another pane to swap"
        >
          <span className="mono-meta pane-index">{index + 1}</span>
          <span className="mono pane-title">
            {p.kind === "session" && p.agent ? `@${p.agent}` : p.kind === "view" ? p.path?.split("/").pop() : "empty"}
          </span>
          {agent && (
            <span className={`dot ${agent.live ? (agent.turn_state === "busy" ? "dot-busy" : "dot-idle") : "dot-down"}`} />
          )}
          {needs && <span className="chip chip-error">needs you</span>}
          {(agent?.activities?.running ?? 0) > 0 && (
            <span className="chip mono activity-chip" title="background work">{agent!.activities!.running} bg</span>
          )}
          {broadcast && p.kind === "session" && <span className="chip chip-busy" title="broadcast is on: messages sent in any pane go to this one too">bcast</span>}
          <span style={{ flex: 1 }} />
          {p.kind === "session" && p.agent && (
            <Link className="pane-btn" to={`/session/${encodeURIComponent(p.agent)}`} title="open as a page">↗</Link>
          )}
          <button className="pane-btn" onClick={() => setZoom((z) => (z === p.id ? null : p.id))} title="zoom (alt+z)">{zoom === p.id ? "⤡" : "⤢"}</button>
          {!dynamic && (
            <>
              <button className="pane-btn" onClick={() => setPicker(p.id)} title="change what this pane shows">⋯</button>
              <button className="pane-btn" onClick={() => update((b) => ({ ...b, layout: splitPane(b.layout, p.id, "row") }))} title="split right">⫿</button>
              <button className="pane-btn" onClick={() => update((b) => ({ ...b, layout: splitPane(b.layout, p.id, "col") }))} title="split down">⫽</button>
              <button
                className="pane-btn"
                onClick={() => update((b) => ({ ...b, layout: removePane(b.layout, p.id) ?? emptyPane() }))}
                title="close pane"
              >
                ×
              </button>
            </>
          )}
        </div>
        <div className="pane-body">{body}</div>
      </section>
    );
  };

  let paneIndex = 0;
  const renderNode = (n: BoardNode): React.ReactNode => {
    if (n.kind !== "split") return renderPane(n, paneIndex++);
    return (
      <SplitBox key={n.id} node={n} onSizes={(sizes) => !dynamic && update((b) => ({ ...b, layout: setSizes(b.layout, n.id, sizes) }))} locked={dynamic}>
        {n.children.map((c) => renderNode(c))}
      </SplitBox>
    );
  };

  const zoomed = zoom ? panes.find((p) => p.id === zoom) : null;

  return (
    <div className="board-page">
      <div className="board-head">
        <Link className="mono-meta" to="/boards">boards</Link>
        <span className="mono-meta">/</span>
        {renaming ? (
          <input
            autoFocus
            className="board-name-input"
            value={nameDraft}
            onChange={(e) => setNameDraft(e.target.value)}
            onBlur={() => {
              setRenaming(false);
              if (nameDraft.trim()) update((b) => ({ ...b, name: nameDraft.trim() }));
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.currentTarget as HTMLInputElement).blur();
              if (e.key === "Escape") setRenaming(false);
            }}
          />
        ) : (
          <button className="board-name" onClick={() => { setNameDraft(board.name); setRenaming(true); }} title="rename">
            {board.name}
          </button>
        )}
        {dynamic && <span className="chip mono" title={JSON.stringify(board.query)}>dynamic · {panes.length}</span>}
        <span className="mono-meta">{sessionPanes.length} session{sessionPanes.length === 1 ? "" : "s"}</span>
        <span style={{ flex: 1 }} />
        <label className={`bcast-toggle${broadcast ? " on" : ""}`} title="send the focused pane's messages to every session pane on this board">
          <input
            type="checkbox"
            checked={broadcast}
            onChange={(e) => {
              if (e.target.checked && !bcastArmed) {
                setBcastAsk(true);
                return;
              }
              setBroadcast(e.target.checked);
            }}
          />
          broadcast
        </label>
        {bcastAsk && (
          <span className="inline-confirm">
            <span className="mono-meta">every message you send in any pane goes to all {sessionPanes.length} session panes —</span>
            <button className="btn sm primary" onClick={() => { setBcastArmed(true); setBroadcast(true); setBcastAsk(false); }}>turn on</button>
            <button className="btn sm" onClick={() => setBcastAsk(false)}>cancel</button>
          </span>
        )}
        {dynamic && (
          <button
            className="btn sm"
            onClick={() => update((b) => ({ ...b, query: undefined, layout: tree }))}
            title="freeze the current membership into an ordinary board"
          >
            pin
          </button>
        )}
        <button
          className="btn sm"
          onClick={() => {
            const data = JSON.stringify(board, null, 2);
            void navigator.clipboard?.writeText(data);
            setErr("board JSON copied to the clipboard");
          }}
          title="copy this board as JSON"
        >
          export
        </button>
        {deleteAsk ? (
          <span className="inline-confirm">
            <span className="mono-meta">delete "{board.name}"?</span>
            <button className="btn sm" onClick={async () => { await api.deleteBoard(board.id).catch(() => {}); nav("/boards"); }}>yes, delete</button>
            <button className="btn sm" onClick={() => setDeleteAsk(false)}>keep</button>
          </span>
        ) : (
          <button className="btn sm" onClick={() => setDeleteAsk(true)}>delete</button>
        )}
      </div>
      <ErrorBar error={err} />
      {narrow ? (
        <div className="board-tabs-wrap">
          <div className="board-tabs">
            {panes.map((p, i) => (
              <button key={p.id} className={i === tab ? "on" : ""} onClick={() => { setTab(i); setFocus(p.id); }}>
                {p.kind === "session" && p.agent ? `@${p.agent.split("@")[0]}` : p.kind === "view" ? "view" : "empty"}
                {p.kind === "session" && p.agent && attention.has(p.agent) ? " •" : ""}
              </button>
            ))}
          </div>
          <div className="board-body">{panes[tab] ? renderPane(panes[tab]!, tab) : null}</div>
        </div>
      ) : zoomed ? (
        <div className="board-body">{renderPane(zoomed, panes.indexOf(zoomed))}</div>
      ) : (
        <div className="board-body">{renderNode(tree)}</div>
      )}
      {picker && (
        <Picker
          agents={agents}
          attention={attention}
          onPick={(pane) => {
            update((b) => ({ ...b, layout: setPane(b.layout, picker, pane) }));
            setPicker(null);
          }}
          onClose={() => setPicker(null)}
        />
      )}
    </div>
  );
}

// ------------------------------------------------------------ split box

function SplitBox({
  node,
  children,
  onSizes,
  locked,
}: {
  node: Extract<BoardNode, { kind: "split" }>;
  children: React.ReactNode[];
  onSizes: (sizes: number[]) => void;
  locked: boolean;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const row = node.dir === "row";
  const sizes = node.sizes.length === children.length ? node.sizes : children.map(() => 100 / children.length);

  function startDrag(i: number, e: React.PointerEvent) {
    if (locked) return;
    e.preventDefault();
    const box = ref.current;
    if (!box) return;
    const rect = box.getBoundingClientRect();
    const total = row ? rect.width : rect.height;
    const start = row ? e.clientX : e.clientY;
    const a0 = sizes[i]!;
    const b0 = sizes[i + 1]!;
    const move = (ev: PointerEvent) => {
      const d = (((row ? ev.clientX : ev.clientY) - start) / total) * 100;
      const a = Math.max(10, Math.min(a0 + b0 - 10, a0 + d));
      const next = [...sizes];
      next[i] = a;
      next[i + 1] = a0 + b0 - a;
      onSizes(next);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  return (
    <div ref={ref} className={`split ${row ? "split-row" : "split-col"}`}>
      {children.map((c, i) => (
        <div key={i} className="split-cell" style={{ flexBasis: `${sizes[i]}%` }}>
          {c}
          {i < children.length - 1 && (
            <div
              className={`divider${locked ? " locked" : ""}`}
              onPointerDown={(e) => startDrag(i, e)}
              onDoubleClick={() => !locked && onSizes(children.map(() => 100 / children.length))}
              title={locked ? undefined : "drag to resize · double-click to equalize"}
            />
          )}
        </div>
      ))}
    </div>
  );
}

// ------------------------------------------------------------ picker

function Picker({
  agents,
  attention,
  onPick,
  onClose,
}: {
  agents: Agent[];
  attention: Set<string>;
  onPick: (p: BoardPane) => void;
  onClose: () => void;
}) {
  const [q, setQ] = useState("");
  const [viewPath, setViewPath] = useState("");
  const [viewAgent, setViewAgent] = useState("");
  const list = agents
    .filter((a) => !a.moved_to && (!q || a.name.toLowerCase().includes(q.toLowerCase()) || (a.title ?? "").toLowerCase().includes(q.toLowerCase())))
    .sort((x, y) => Number(y.live) - Number(x.live) || x.name.localeCompare(y.name));
  return (
    <div className="trust-backdrop" onClick={onClose} role="presentation">
      <div className="trust-panel picker" role="dialog" aria-label="choose a session" onClick={(e) => e.stopPropagation()}>
        <div className="trust-head">
          <span className="label">What goes in this pane?</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onClose}>esc</button>
        </div>
        <input autoFocus className="picker-search" placeholder="filter sessions by name or title" value={q} onChange={(e) => setQ(e.target.value)} />
        <div className="picker-list">
          {list.map((a) => (
            <button key={a.name} className="picker-row" onClick={() => onPick({ kind: "session", id: "", agent: a.name })}>
              <span className={`dot ${a.live ? (a.turn_state === "busy" ? "dot-busy" : "dot-idle") : "dot-down"}`} />
              <span className="mono">@{a.name}</span>
              {a.title && <span className="mono-meta">{a.title}</span>}
              <span style={{ flex: 1 }} />
              <span className="mono-meta">{a.node}</span>
              {attention.has(a.name) && <span className="chip chip-error">needs you</span>}
            </button>
          ))}
          {list.length === 0 && <div className="dim" style={{ padding: 8 }}>no sessions match</div>}
        </div>
        <div className="picker-view">
          <span className="mono-meta">or an artifact viewer:</span>
          <select value={viewAgent} onChange={(e) => setViewAgent(e.target.value)}>
            <option value="">agent…</option>
            {agents.map((a) => (
              <option key={a.name} value={a.name}>@{a.name}</option>
            ))}
          </select>
          <input className="mono" placeholder="/path/on/its/node/report.md" value={viewPath} onChange={(e) => setViewPath(e.target.value)} />
          <button className="btn sm" disabled={!viewAgent || !viewPath.trim()} onClick={() => onPick({ kind: "view", id: "", agent: viewAgent, path: viewPath.trim() })}>
            add viewer
          </button>
        </div>
      </div>
    </div>
  );
}

// ------------------------------------------------------------ boards list

export function BoardsPage() {
  const nav = useNavigate();
  const { agents } = useAppData();
  const [boards, setBoards] = useState<Board[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [preset, setPreset] = useState("1|2");
  const [name, setName] = useState("");
  const [dyn, setDyn] = useState<DynamicQuery>({ state: "live", style: "grid" });
  const [dynName, setDynName] = useState("");

  async function load() {
    try {
      setBoards(await api.boards());
    } catch (e) {
      setErr(e instanceof Error ? e.message : "failed");
    }
  }
  useEffect(() => {
    void load();
  }, []);

  async function create(layout: BoardNode, nm: string, query?: DynamicQuery) {
    const b: Board = { id: crypto.randomUUID().slice(0, 8), name: nm, layout, query, updated_at: Date.now() / 1000 };
    try {
      await api.putBoard(b);
      nav(`/board/${b.id}`);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "failed to create");
    }
  }
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState("");
  async function importJson() {
    const text = importText;
    if (!text.trim()) return;
    try {
      const b = JSON.parse(text) as Board;
      b.id = crypto.randomUUID().slice(0, 8);
      b.updated_at = Date.now() / 1000;
      await api.putBoard(b);
      setImportOpen(false);
      setImportText("");
      await load();
    } catch (e) {
      setErr(`import: ${e instanceof Error ? e.message : "bad JSON"}`);
    }
  }
  const nodes = Array.from(new Set(agents.map((a) => a.node))).sort();
  const channels = Array.from(new Set(agents.map((a) => a.channel))).sort();

  return (
    <div className="stage-body boards-page">
      <h1>Boards</h1>
      <p className="dim">
        Layouts of sessions across the estate. A board lives on the node and syncs across the mesh, so every console shows
        the same set. A session can sit in any number of boards.
      </p>
      <ErrorBar error={err} />
      <div className="boards-grid">
        {boards.map((b) => (
          <Link key={b.id} className="board-card" to={`/board/${b.id}`}>
            <span className="board-card-name">{b.name}</span>
            <span className="mono-meta">
              {b.query ? `dynamic · ${describeQuery(b.query)}` : `${panesOf(b.layout).length} panes`}
            </span>
          </Link>
        ))}
        {boards.length === 0 && <div className="dim">no boards yet</div>}
      </div>
      <h2>New board</h2>
      <div className="boards-new">
        <input placeholder="name" value={name} onChange={(e) => setName(e.target.value)} />
        <span className="seg">
          {PRESETS.map((p) => (
            <button key={p.key} className={preset === p.key ? "on" : ""} onClick={() => setPreset(p.key)}>{p.label}</button>
          ))}
        </span>
        <button className="btn primary sm" onClick={() => void create(PRESETS.find((p) => p.key === preset)!.make(), name.trim() || preset)}>
          create
        </button>
        <button className="btn sm" onClick={() => setImportOpen((o) => !o)}>import JSON</button>
      </div>
      {importOpen && (
        <div className="boards-new" style={{ marginTop: 8 }}>
          <textarea className="mono" rows={6} style={{ width: "100%", maxWidth: 720 }} placeholder="paste a board's JSON (from export)" value={importText} onChange={(e) => setImportText(e.target.value)} />
          <button className="btn primary sm" onClick={() => void importJson()} disabled={!importText.trim()}>import</button>
        </div>
      )}
      <h2>New dynamic board</h2>
      <p className="dim">Panes are whatever matches, laid out automatically; they come and go with the fleet.</p>
      <div className="boards-new">
        <input placeholder="name" value={dynName} onChange={(e) => setDynName(e.target.value)} />
        <select value={dyn.state ?? "live"} onChange={(e) => setDyn({ ...dyn, state: e.target.value as DynamicQuery["state"] })}>
          <option value="busy">busy now</option>
          <option value="live">live</option>
          <option value="attention">needs attention</option>
          <option value="activity">has background activity</option>
          <option value="any">any (incl. stopped)</option>
        </select>
        <select value={dyn.node ?? ""} onChange={(e) => setDyn({ ...dyn, node: e.target.value || undefined })}>
          <option value="">any node</option>
          {nodes.map((n) => <option key={n} value={n}>{n}</option>)}
        </select>
        <select value={dyn.channel ?? ""} onChange={(e) => setDyn({ ...dyn, channel: e.target.value || undefined })}>
          <option value="">any channel</option>
          {channels.map((c) => <option key={c} value={c}>#{c}</option>)}
        </select>
        <input placeholder="name contains…" value={dyn.name ?? ""} onChange={(e) => setDyn({ ...dyn, name: e.target.value || undefined })} />
        <span className="seg">
          <button className={(dyn.style ?? "grid") === "grid" ? "on" : ""} onClick={() => setDyn({ ...dyn, style: "grid" })}>grid</button>
          <button className={dyn.style === "main" ? "on" : ""} onClick={() => setDyn({ ...dyn, style: "main" })}>main + stack</button>
        </span>
        <button className="btn primary sm" onClick={() => void create(emptyPane(), dynName.trim() || describeQuery(dyn), dyn)}>
          create
        </button>
      </div>
    </div>
  );
}

function describeQuery(q: DynamicQuery): string {
  const parts = [q.state ?? "live", q.node ? `on ${q.node}` : "", q.channel ? `#${q.channel}` : "", q.name ? `~${q.name}` : ""].filter(Boolean);
  return parts.join(" · ");
}
