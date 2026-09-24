import { createContext, useContext, useEffect, useMemo, useState } from "react";
import { scoped } from "./profiles";
import { Navigate, NavLink, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { api, type Agent, type Board, type BoardNode, type BusMessage, type NodeInfo } from "./api";
import { usePoll } from "./hooks";
import { Meter, presenceOf, useTheme } from "./components";
import Now from "./pages/Now";
import Conversations from "./pages/Conversations";
import Session from "./pages/Session";
import View from "./pages/View";
import BoardPage, { BoardsPage } from "./pages/Board";
import Plugins from "./pages/Plugins";
import Usage from "./pages/Usage";
import Attach from "./pages/Attach";
import { tunnel } from "./tunnel";
import Mesh from "./pages/Mesh";
import History from "./pages/History";
import Search from "./pages/Search";
import Palette from "./Palette";
import { NoticesBell, NoticesProvider } from "./notices";
import { GlobalHotkeys, HotkeysProvider } from "./hotkeys";
import { evictStaleTranscripts } from "./transcript";
import { pwa, setBadge } from "./pwa";
import { activeConnection, connectionSummary, hosted } from "./connections";
import { activeProfile, addProfile, listProfiles, onProfilesChange, profileName, setProfileMesh, switchTo } from "./profiles";
import { peek } from "./peek";
import { MenuGroup, MenuRow, SessionMenu } from "./sessionBar";

export interface AppData {
  agents: Agent[];
  agentsError: string | null;
  agentsLoaded: boolean;
  refreshAgents: () => Promise<void>;
  inbox: BusMessage[];
  refreshInbox: () => Promise<void>;
  /** Agents with a permission prompt or question open for the operator
   *  (`/api/needs` prompts), fleet-wide. */
  waiting: Set<string>;
  node: NodeInfo | null;
  refreshNode: () => Promise<void>;
}

const AppDataContext = createContext<AppData>({
  agents: [],
  agentsError: null,
  agentsLoaded: false,
  refreshAgents: async () => {},
  inbox: [],
  refreshInbox: async () => {},
  waiting: new Set(),
  node: null,
  refreshNode: async () => {},
});

/** Housekeeping for the browser-side transcript cache (transcript.ts). */
function useTranscriptEviction() {
  useEffect(() => {
    void evictStaleTranscripts();
    const t = window.setInterval(() => void evictStaleTranscripts(), 3600 * 1000);
    return () => window.clearInterval(t);
  }, []);
}

export function useAppData(): AppData {
  return useContext(AppDataContext);
}

// `phone`: on the bottom bar at phone width; the rest are in the More
// sheet there (PROPOSALS-2026-09-I.md §2.1). Nothing is hidden.
const NAV: { to: string; key: string; label: string; end?: boolean; phone?: boolean }[] = [
  { to: "/", key: "N", label: "Now", end: true, phone: true },
  { to: "/flow", key: "F", label: "Flow", phone: true },
  { to: "/mesh", key: "M", label: "Mesh", phone: true },
  { to: "/history", key: "H", label: "History" },
  { to: "/search", key: "S", label: "Search", phone: true },
  { to: "/boards", key: "B", label: "Boards", phone: true },
  { to: "/plugins", key: "P", label: "Plugins" },
  { to: "/usage", key: "U", label: "Usage" },
];

/** The phone's More sheet: every rail item that is not on the bottom bar,
 *  the palette, and the status bar's tail (meshes, notifications, install,
 *  theme) — reachable by tap, in the desktop's words. */
function MoreSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const nav = useNavigate();
  const [theme, toggleTheme] = useTheme();
  const [, setTick] = useState(0);
  useEffect(() => pwa.onChange(() => setTick((n) => n + 1)), []);
  const go = (to: string) => {
    onClose();
    nav(to);
  };
  return (
    <SessionMenu open={open} onClose={onClose} anchor={null}>
      <div className="menu-head">
        <span className="mono">more</span>
      </div>
      <MenuGroup label="surfaces">
        {NAV.filter((n) => !n.phone).map((n) => (
          <MenuRow key={n.to} label={`${n.key}  ${n.label}`} onClick={() => go(n.to)} />
        ))}
        <MenuRow label="⌘  command palette" hint="every session control and surface, by name" onClick={() => { onClose(); window.dispatchEvent(new Event("aspen:palette")); }} />
      </MenuGroup>
      <MenuGroup label="this console">
        {hosted && <MenuRow label="meshes · connect" hint="the meshes this console is connected to, and how" onClick={() => go("/attach")} />}
        <MenuRow label="notifications & push" hint="notices, push to this device, outbound hooks" onClick={() => { onClose(); window.dispatchEvent(new Event("aspen:notices")); }} />
        {pwa.canInstall && <MenuRow label="install as app" hint="its own window, a dock icon with the needs-you count" onClick={() => { onClose(); void pwa.install(); }} />}
        {pwa.needRefresh && <MenuRow label="new console — reload" onClick={() => pwa.reload()} />}
        <MenuRow label={`theme: ${theme}`} hint="cycle system · light · dark" onClick={() => toggleTheme()} />
      </MenuGroup>
    </SessionMenu>
  );
}

// ── working set: pinned + recently opened sessions (per browser) ──────────
const WS_KEY = "aspen.workingSet";
interface WorkingSet {
  pinned: string[];
  recent: string[];
}
function loadWorkingSet(): WorkingSet {
  try {
    const raw = localStorage.getItem(scoped(WS_KEY));
    if (raw) {
      const v = JSON.parse(raw) as Partial<WorkingSet>;
      return { pinned: v.pinned ?? [], recent: v.recent ?? [] };
    }
  } catch {
    /* storage unavailable */
  }
  return { pinned: [], recent: [] };
}
function saveWorkingSet(ws: WorkingSet) {
  try {
    localStorage.setItem(scoped(WS_KEY), JSON.stringify(ws));
  } catch {
    /* storage unavailable */
  }
}

/** The rail is the operator's working set, not a fourth copy of the fleet:
 *  pinned sessions, the last few opened, and the fleet pulse. The whole
 *  roster lives in Now. */
function MeshColumn() {
  const [boards, setBoards] = useState<Board[]>([]);
  const loc = useLocation();
  // The rail collapses to a narrow strip of keys and dots (per browser).
  const [narrow, setNarrowState] = useState<boolean>(() => {
    try {
      return localStorage.getItem("aspen.rail") === "narrow";
    } catch {
      return false;
    }
  });
  const setNarrow = (f: (n: boolean) => boolean) =>
    setNarrowState((cur) => {
      const next = f(cur);
      try {
        localStorage.setItem("aspen.rail", next ? "narrow" : "wide");
      } catch {
        // fine
      }
      document.body.classList.toggle("rail-narrow", next);
      return next;
    });
  useEffect(() => {
    document.body.classList.toggle("rail-narrow", narrow);
  }, [narrow]);
  useEffect(() => {
    const onToggle = () => setNarrow((n) => !n);
    window.addEventListener("aspen:rail", onToggle);
    return () => window.removeEventListener("aspen:rail", onToggle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    let stop = false;
    const load = () => api.boards().then((b) => !stop && setBoards(b)).catch(() => {});
    void load();
    const t = window.setInterval(() => void load(), 15000);
    const onChange = () => void load();
    window.addEventListener("aspen:boards", onChange);
    return () => {
      stop = true;
      window.clearInterval(t);
      window.removeEventListener("aspen:boards", onChange);
    };
    // reload when navigating (a board was just created/deleted)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loc.pathname.startsWith("/board")]);
  const { agents, inbox, waiting } = useAppData();
  const location = useLocation();
  const [ws, setWs] = useState<WorkingSet>(loadWorkingSet);
  const [moreOpen, setMoreOpen] = useState(false);
  // A board's pip is the net of its panes' (the operator's ask): any
  // session pane waiting on the operator lights the board.
  const boardWaiting = (b: Board): number => {
    if (b.query) return 0;
    let n = 0;
    const walk = (node: BoardNode) => {
      if (node.kind === "split") node.children.forEach(walk);
      else if (node.kind === "session" && node.agent && waiting.has(node.agent)) n++;
    };
    walk(b.layout);
    return n;
  };

  // Visiting a session adds it to the recents.
  useEffect(() => {
    const m = /^\/session\/(.+)$/.exec(location.pathname);
    if (!m) return;
    const name = decodeURIComponent(m[1]);
    setWs((cur) => {
      const recent = [name, ...cur.recent.filter((n) => n !== name)].slice(0, 8);
      const next = { ...cur, recent };
      saveWorkingSet(next);
      return next;
    });
  }, [location.pathname]);

  function togglePin(name: string) {
    setWs((cur) => {
      const pinned = cur.pinned.includes(name)
        ? cur.pinned.filter((n) => n !== name)
        : [...cur.pinned, name];
      const next = { ...cur, pinned };
      saveWorkingSet(next);
      return next;
    });
  }

  const byName = new Map(agents.map((a) => [a.name, a] as const));
  const busy = agents.filter((a) => a.live && a.turn_state === "busy").length;
  const live = agents.filter((a) => a.live).length;
  const needs = inbox.length;
  const pinnedRows = ws.pinned.map((n) => byName.get(n)).filter((a): a is Agent => !!a);
  const recentRows = ws.recent
    .filter((n) => !ws.pinned.includes(n))
    .map((n) => byName.get(n))
    .filter((a): a is Agent => !!a);

  const row = (a: Agent, pinned: boolean) => {
    const bare = a.bare ?? a.name.split("@")[0];
    const identity = [a.title, `#${a.channel}`, a.remote ? a.node : null].filter(Boolean).join(" · ");
    return (
      <NavLink
        key={a.name}
        to={`/session/${encodeURIComponent(a.name)}`}
        className={({ isActive }) => `nav-item rail-session${isActive ? " active" : ""}`}
        title={`${a.repo ?? `remote · ${a.node}`} · ${a.live ? (a.turn_state ?? "live") : "down"}`}
      >
        <Meter presence={presenceOf(a.live, a.turn_state)} />
        <span className={`rail-glyph ${presenceOf(a.live, a.turn_state)}`} aria-hidden>
          {bare.slice(0, 2).toUpperCase()}
        </span>
        <span className="rail-body">
          <span className="rail-line1">
            <span className="mono rail-name">@{bare}</span>
            {waiting.has(a.name) && (
              <span className="rail-wait-pip" title="waiting on you — a permission prompt or question is open" aria-label="waiting on you" />
            )}
            {(a.activities?.running ?? 0) > 0 && (
              <span
                className="rail-activity-pip"
                title={`${a.activities!.running} background ${a.activities!.running === 1 ? "activity" : "activities"} running — subagents, tasks, workflows`}
                aria-label="background activity running"
              />
            )}
            {a.pending > 0 && <span className="badge-count">{a.pending}</span>}
          </span>
          <span className="rail-line2">{identity}</span>
        </span>
        <button
          type="button"
          className="rail-pin"
          title={pinned ? "unpin" : "pin to the rail"}
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            togglePin(a.name);
          }}
        >
          {pinned ? "●" : "○"}
        </button>
      </NavLink>
    );
  };

  return (
    <nav className={`mesh-col${narrow ? " narrow" : ""}`} aria-label="primary">
      <button
        className="rail-toggle"
        onClick={() => setNarrow((n) => !n)}
        title={narrow ? "expand the rail" : "collapse the rail to icons"}
        aria-label={narrow ? "expand navigation" : "collapse navigation"}
      >
        {narrow ? "»" : "«"}
      </button>
      {NAV.map((n) => (
        <NavLink key={n.to} to={n.to} end={n.end} className={({ isActive }) => `nav-item${isActive ? " active" : ""}${n.phone ? " nav-phone" : ""}`} title={n.label}>
          <span className="nav-key" style={{ color: "var(--text-dim)", width: 14 }}>{n.key}</span>
          <span>{n.label}</span>
          {n.label === "Now" && needs > 0 && <span className="badge-count">{needs}</span>}
        </NavLink>
      ))}
      <button type="button" className="nav-item nav-more" onClick={() => setMoreOpen(true)} title="More" aria-haspopup="menu" aria-expanded={moreOpen}>
        <span className="nav-key" style={{ color: "var(--text-dim)", width: 14 }}>⋯</span>
        <span>More</span>
      </button>
      <MoreSheet open={moreOpen} onClose={() => setMoreOpen(false)} />
      <div className="nav-section label" style={{ marginTop: 12 }} title="busy / live / registered">
        Fleet · {busy} busy · {live}/{agents.length} live
      </div>
      <div className="rail-scroll">
      {boards.length > 0 && (
        <>
          <div className="nav-section label" style={{ marginTop: 6 }}>Boards</div>
          {boards.map((b) => (
            <NavLink key={b.id} to={`/board/${b.id}`} className={({ isActive }) => `nav-item${isActive ? " active" : ""}`} title={b.name}>
              <span className="nav-key" style={{ color: "var(--text-dim)", width: 14 }}>▦</span>
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{b.name}</span>
              {boardWaiting(b) > 0 && (
                <span className="rail-wait-pip" title={`${boardWaiting(b)} session${boardWaiting(b) === 1 ? "" : "s"} on this board waiting on you`} aria-label="a session on this board is waiting on you" />
              )}
            </NavLink>
          ))}
        </>
      )}
      {pinnedRows.length > 0 && (
        <>
          <div className="nav-section label" style={{ marginTop: 6 }}>Pinned</div>
          {pinnedRows.map((a) => row(a, true))}
        </>
      )}
      <div className="nav-section label" style={{ marginTop: 6 }}>Recent</div>
      {recentRows.length === 0 && (
        <div style={{ padding: "4px 16px", color: "var(--text-dim)", fontSize: 12 }}>
          sessions you open show up here — pin the ones you live in
        </div>
      )}
      {recentRows.map((a) => row(a, false))}
      </div>
    </nav>
  );
}

/** The oldest node this console works against (hosted builds). */
const MIN_NODE_VERSION = "0.25.0";
function cmpVersion(a: string, b: string): number {
  const pa = a.split(".").map((n) => parseInt(n, 10) || 0);
  const pb = b.split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < 3; i++) {
    if ((pa[i] ?? 0) !== (pb[i] ?? 0)) return (pa[i] ?? 0) - (pb[i] ?? 0);
  }
  return 0;
}

/** Daemon (API) version next to the UI's own build stamp. They come from the
 *  same workspace version, so a difference means this page is stale — a
 *  cached bundle after `aspen update --restart` — and needs a reload. */
function VersionBadge({ node }: { node: NodeInfo | null }) {
  const ui = __ASPEN_UI_VERSION__;
  const uiSha = __ASPEN_UI_SHA__;
  const nav = useNavigate();
  const { refreshNode } = useAppData();
  const [checking, setChecking] = useState<null | "…" | string>(null);
  if (!node) return null;
  // Hosted: the console and the node are on separate release trains
  // (PROPOSALS-2026-09-D.md §3); skew is normal, too old is a banner.
  if (hosted) {
    const old = cmpVersion(node.version, MIN_NODE_VERSION) < 0;
    return (
      <button
        type="button"
        className="btn ghost sm"
        onClick={() => nav("/mesh?view=list#nodes")}
        title={old ? `this node runs v${node.version}; this console needs v${MIN_NODE_VERSION} or newer — update the node` : `node v${node.version} · console v${ui} (${uiSha})`}
        style={old ? { color: "var(--sig-gate)" } : undefined}
      >
        {old ? `node v${node.version} — too old for this console` : `node v${node.version} · console v${ui}`}
      </button>
    );
  }
  async function checkNow() {
    setChecking("…");
    try {
      const r = await api.checkUpdatesAll();
      await refreshNode();
      const n = Object.keys(r.results).length;
      setChecking(r.behind > 0 ? `v available on ${r.behind} of ${n}` : `up to date (${n} node${n === 1 ? "" : "s"})`);
      if (r.behind > 0) nav("/mesh?view=list#nodes");
    } catch (e) {
      setChecking(e instanceof Error ? e.message : "check failed");
    }
    window.setTimeout(() => setChecking(null), 4000);
  }
  const stale = node.version !== ui || (node.sha && uiSha !== "unknown" && node.sha !== uiSha);
  const title = `api ${node.version}${node.sha ? ` (${node.sha})` : ""} · ui ${ui} (${uiSha})${
    stale ? " — reload to pick up the new console" : ""
  }`;
  const available = node.update_available && !node.update_skipped ? node.update_available : null;
  const servicing = node.service_state && node.service_state !== "ready" ? node.service_state : null;
  if (!stale && (available || servicing)) {
    return (
      <button
        type="button"
        className="btn ghost sm"
        onClick={() => nav("/mesh?view=list#nodes")}
        title={
          servicing
            ? `this node is ${servicing}${node.service_detail ? ` — ${node.service_detail}` : ""}`
            : `v${available} is available — open Nodes to update`
        }
        style={{ color: "var(--sig-normal)" }}
      >
        ● v{node.version}
        {servicing ? ` · ${servicing}` : ` → v${available}`}
      </button>
    );
  }
  return stale ? (
    <button
      type="button"
      className="btn ghost sm"
      onClick={() => window.location.reload()}
      title={title}
      style={{ color: "var(--sig-normal)" }}
    >
      api {node.version} · ui {ui} — reload
    </button>
  ) : (
    <button
      type="button"
      className="btn ghost sm"
      onClick={() => void checkNow()}
      disabled={checking === "…"}
      title={`${title} — click to check for updates on every node`}
      style={{ color: "var(--text-dim)" }}
    >
      {checking === "…"
        ? "checking…"
        : checking
          ? checking
          : `v${node.version}${node.sha ? ` ${node.sha}` : ""}`}
    </button>
  );
}

function FlowRedirect() {
  const location = useLocation();
  return <Navigate to={location.pathname.replace(/^\/conversations/, "/flow")} replace />;
}

/** "via relay → node" while the console is attached through a relay
 *  (RELAY.md §11); click opens the attach page. */
function TunnelPill() {
  const [, setTick] = useState(0);
  useEffect(() => tunnel.onChange(() => setTick((n) => n + 1)), []);
  const nav = useNavigate();
  const conn = activeConnection();
  // An attached tunnel is what requests ride, whatever connection is
  // marked active: say that first.
  if (conn?.kind === "direct" && !tunnel.enabled) {
    return (
      <button className="btn ghost sm mono" onClick={() => nav("/attach")} title={`talking to ${conn.url} directly`}>
        {conn.name}
      </button>
    );
  }
  if (!tunnel.enabled && tunnel.state === "off") return null;
  const color = tunnel.state === "up" ? "var(--live)" : tunnel.state === "down" ? "var(--sig-gate)" : "var(--sig-normal)";
  return (
    <button className="btn ghost sm mono" style={{ color: tunnel.error ? "var(--sig-gate)" : color }} onClick={() => nav("/attach")} title={tunnel.error ?? `console attached through ${tunnel.config.relay}`}>
      {tunnel.directNode ? `direct → ${tunnel.directNode} · relay standing by` : `via relay → ${tunnel.config.node} · ${tunnel.state}`}{tunnel.error ? " · trouble" : ""}{!tunnel.directNode && tunnel.directNodes.length > 0 ? ` · direct to ${tunnel.directNodes.join(", ")}` : ""}
    </button>
  );
}

/** Which mesh this console is looking at (PROPOSALS-2026-09-F.md §2.2):
 *  a label always — served by a node, that node's mesh; hosted, the
 *  active profile's — and, hosted with more than one mesh, a menu that
 *  switches. The node's answer also names a profile that reached it
 *  directly before it knew where it was. */
function MeshSwitcher({ node }: { node: NodeInfo | null }) {
  const meshPoll = usePoll(api.mesh, 60000);
  const [open, setOpen] = useState(false);
  const [, setTick] = useState(0);
  useEffect(() => onProfilesChange(() => setTick((n) => n + 1)), []);
  // Peek (F-6): needs-you counts for the meshes not in view.
  useEffect(() => peek.onChange(() => setTick((n) => n + 1)), []);
  const info = meshPoll.data;
  // The node's answer names a profile that does not know its mesh yet;
  // a profile that does keeps its name — a node in another mesh is a
  // misfiled connection, said in the label, not a rename.
  useEffect(() => {
    if (!hosted || !info) return;
    if (info.in_mesh && info.mesh) setProfileMesh(info.mesh, undefined, true);
    else if (!info.in_mesh) setProfileMesh(null, info.node, true);
  }, [info]);
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);
  const nodeLabel = node ? `node ${node.node}` : "connecting…";
  if (!hosted) {
    const mesh = info ? (info.in_mesh && info.mesh ? info.mesh : "no mesh") : null;
    return <span className="mono-meta" title={mesh ? `this node is in mesh ${mesh}` : undefined}>{mesh ? `${mesh} · ${nodeLabel}` : nodeLabel}</span>;
  }
  const active = activeProfile();
  const profiles = listProfiles();
  const name = active ? profileName(active) : "no mesh";
  const actual = info ? (info.in_mesh && info.mesh ? info.mesh : "no mesh") : null;
  const misfiled = active?.mesh && actual && actual !== active.mesh ? ` (in ${actual})` : "";
  return (
    <span className="mesh-switch" onClick={(e) => e.stopPropagation()}>
      <button
        type="button"
        className="btn ghost sm mono"
        onClick={() => setOpen((o) => !o)}
        title={profiles.length > 1 ? "switch mesh" : "connect to another mesh"}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {name} · {nodeLabel}{misfiled} ▾
      </button>
      {open && (
        <div className="mesh-switch-menu" role="menu">
          {profiles.map((p) => {
            const conn = connectionSummary(p.id);
            const isActive = p.id === active?.id;
            const pk = isActive ? undefined : peek.get(p.id);
            return (
              <button
                key={p.id}
                type="button"
                role="menuitem"
                className={`mesh-switch-row${isActive ? " active" : ""}`}
                onClick={() => {
                  if (!isActive) switchTo(p.id);
                  setOpen(false);
                }}
                title={isActive ? "the mesh you are looking at" : "switch to this mesh"}
              >
                <span className="mono">
                  {profileName(p)}
                  {!isActive && (pk?.needs ?? 0) > 0 && <span className="badge-count" title={`${pk!.needs} waiting on you there`}>{pk!.needs}</span>}
                </span>
                <span className="mono-meta">
                  {isActive ? `${conn} · here` : conn}
                  {!isActive && pk?.state === "down" ? ` · unreachable${pk.error ? `: ${pk.error}` : ""}` : ""}
                  {!isActive && pk?.state === "up" && pk.needs === 0 ? " · nothing waiting" : ""}
                </span>
              </button>
            );
          })}
          <button type="button" role="menuitem" className="mesh-switch-row" onClick={() => addProfile()} title="connect this console to another mesh">
            <span>connect to another mesh…</span>
          </button>
        </div>
      )}
    </span>
  );
}

/** The status bar in zones: brand · context | spacer | the presence
 *  strip | utilities (version, connection, install/reload, the bell).
 *  Theme lives in the palette and the phone's More sheet. */
function StatusBar() {
  const { agents, node } = useAppData();
  const busy = agents.filter((a) => a.live && a.turn_state === "busy").length;
  const idle = agents.filter((a) => a.live && a.turn_state !== "busy").length;
  const off = agents.filter((a) => !a.live).length;
  return (
    <header className="statusbar">
      <span className="sb-zone">
        <span className="brand"><img className="brand-mark" src={`${import.meta.env.BASE_URL}aspen-mark.svg`} alt="" width="18" height="18" />ASP<b>E</b>N</span>
        <button type="button" className="btn ghost sm palette-btn" onClick={() => window.dispatchEvent(new Event("aspen:palette"))} title="command palette (⌘K / ctrl+K)" aria-label="open the command palette">⌘</button>
        <MeshSwitcher node={node} />
      </span>
      <span className="spacer" />
      <span className="presence-strip" title={`${busy} in a turn · ${idle} live and idle · ${off} not running`} aria-label="fleet presence">
        <span><i className="presence-dot busy" /><b className="n">{busy}</b> busy</span>
        <span><i className="presence-dot idle" /><b className="n">{idle}</b> idle</span>
        <span className="off-count"><i className="presence-dot off" />{off} off</span>
      </span>
      <span className="sb-zone sb-utilities">
        <VersionBadge node={node} />
        <TunnelPill />
        <PwaPill />
        <NoticesBell />
      </span>
    </header>
  );
}

/** The service worker's word (pwa.ts): a new console build is ready, or
 *  the app can be installed. */
function PwaPill() {
  const [, setTick] = useState(0);
  useEffect(() => pwa.onChange(() => setTick((n) => n + 1)), []);
  if (pwa.needRefresh) {
    return (
      <button className="btn sm" onClick={() => pwa.reload()} title="a newer console build is ready; reload to take it">
        new console — reload
      </button>
    );
  }
  if (pwa.canInstall) {
    return (
      <button className="btn ghost sm" onClick={() => void pwa.install()} title="install Aspen as an app: its own window, a dock icon with the needs-you count">
        install
      </button>
    );
  }
  return null;
}

// For a browser-console check of the attached tunnel (docs/TLS.md §8,
// MAINTAINING): `aspenTunnel.directNodes`, `aspenTunnel.probeDirect(true)`.
(window as unknown as { aspenTunnel?: unknown }).aspenTunnel = tunnel;

/** `web+aspen://…` links (the manifest's protocol handler) land here as
 *  `/open?u=`: session/<agent> opens the session; connect?relay=&node=
 *  goes to the attach page with those filled in. */
function OpenLink() {
  const nav = useNavigate();
  const { search } = useLocation();
  useEffect(() => {
    const u = new URLSearchParams(search).get("u") ?? "";
    const m = /^web\+aspen:\/\/([^/?]+)\/?([^?]*)(\?.*)?$/.exec(u);
    if (!m) {
      nav("/", { replace: true });
      return;
    }
    const [, kind, rest, qs] = m;
    if (kind === "session" && rest) nav(`/session/${encodeURIComponent(decodeURIComponent(rest))}`, { replace: true });
    else if (kind === "connect") nav(`/attach${qs ?? ""}`, { replace: true });
    else nav("/", { replace: true });
  }, [search, nav]);
  return null;
}

export default function App() {
  const agentsPoll = usePoll(api.agents, 2000);
  const inboxPoll = usePoll(api.inbox, 5000);
  const needsPoll = usePoll(api.needs, 3000);
  const waiting = useMemo(() => new Set((needsPoll.data?.prompts ?? []).map((p) => p.agent)), [needsPoll.data]);
  const nodePoll = usePoll(api.node, 15000);
  // The dock badge is the needs-you count (PROPOSALS-2026-09-D.md §1).
  const needsCount = inboxPoll.data?.length ?? 0;
  useEffect(() => setBadge(needsCount), [needsCount]);
  // Hosted with nothing to talk to: the front door is the connect page.
  const loc = useLocation();
  const navTo = useNavigate();
  useEffect(() => {
    // Attached through steps 1–3 counts as connected, saved or not.
    if (hosted && !activeConnection() && !tunnel.enabled && loc.pathname !== "/attach" && loc.pathname !== "/open") navTo("/attach", { replace: true });
  }, [loc.pathname, navTo]);

  const data: AppData = {
    agents: agentsPoll.data ?? [],
    agentsError: agentsPoll.error,
    agentsLoaded: agentsPoll.data !== null,
    refreshAgents: agentsPoll.refresh,
    inbox: inboxPoll.data ?? [],
    refreshInbox: inboxPoll.refresh,
    waiting,
    node: nodePoll.data,
    refreshNode: nodePoll.refresh,
  };

  useTranscriptEviction();
  return (
    <AppDataContext.Provider value={data}>
      <HotkeysProvider>
        <NoticesProvider>
        <GlobalHotkeys />
        <Palette />
        <div className="shell">
        <StatusBar />
        <div className="body-grid">
          <MeshColumn />
          <main className="stage">
            <Routes>
              <Route path="/" element={<Now />} />
              <Route path="/flow" element={<Conversations />} />
              <Route path="/flow/:channel" element={<Conversations />} />
              <Route path="/session/:name" element={<Session />} />
              <Route path="/session/:name/agent/:agentId" element={<Session />} />
              <Route path="/view/:name" element={<View />} />
              <Route path="/boards" element={<BoardsPage />} />
              <Route path="/plugins" element={<Plugins />} />
              <Route path="/usage" element={<Usage />} />
              <Route path="/attach" element={<Attach />} />
              <Route path="/board/:id" element={<BoardPage />} />
              <Route path="/mesh" element={<Mesh />} />
              <Route path="/history" element={<History />} />
              <Route path="/search" element={<Search />} />
              <Route path="/open" element={<OpenLink />} />
              {/* old surfaces → their new homes */}
              <Route path="/command" element={<Navigate to="/" replace />} />
              <Route path="/sessions" element={<Navigate to="/" replace />} />
              <Route path="/conversations" element={<Navigate to="/flow" replace />} />
              <Route path="/conversations/:channel" element={<FlowRedirect />} />
              <Route path="/map" element={<Navigate to="/mesh" replace />} />
              <Route path="/library" element={<Navigate to="/mesh?view=list" replace />} />
            </Routes>
          </main>
          </div>
        </div>
        </NoticesProvider>
      </HotkeysProvider>
    </AppDataContext.Provider>
  );
}
