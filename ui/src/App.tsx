import { createContext, useContext, useEffect, useState } from "react";
import { Navigate, NavLink, Route, Routes, useLocation } from "react-router-dom";
import { api, type Agent, type BusMessage, type NodeInfo } from "./api";
import { usePoll } from "./hooks";
import { Meter, presenceOf, useTheme } from "./components";
import Now from "./pages/Now";
import Conversations from "./pages/Conversations";
import Session from "./pages/Session";
import Mesh from "./pages/Mesh";
import Palette from "./Palette";
import { GlobalHotkeys, HotkeysProvider } from "./hotkeys";

export interface AppData {
  agents: Agent[];
  agentsError: string | null;
  agentsLoaded: boolean;
  refreshAgents: () => Promise<void>;
  inbox: BusMessage[];
  refreshInbox: () => Promise<void>;
  node: NodeInfo | null;
}

const AppDataContext = createContext<AppData>({
  agents: [],
  agentsError: null,
  agentsLoaded: false,
  refreshAgents: async () => {},
  inbox: [],
  refreshInbox: async () => {},
  node: null,
});

export function useAppData(): AppData {
  return useContext(AppDataContext);
}

const NAV: { to: string; key: string; label: string; end?: boolean }[] = [
  { to: "/", key: "N", label: "Now", end: true },
  { to: "/flow", key: "F", label: "Flow" },
  { to: "/mesh", key: "M", label: "Mesh" },
];

// ── working set: pinned + recently opened sessions (per browser) ──────────
const WS_KEY = "aspen.workingSet";
interface WorkingSet {
  pinned: string[];
  recent: string[];
}
function loadWorkingSet(): WorkingSet {
  try {
    const raw = localStorage.getItem(WS_KEY);
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
    localStorage.setItem(WS_KEY, JSON.stringify(ws));
  } catch {
    /* storage unavailable */
  }
}

/** The rail is the operator's working set, not a fourth copy of the fleet:
 *  pinned sessions, the last few opened, and the fleet pulse. The whole
 *  roster lives in Now. */
function MeshColumn() {
  const { agents, inbox } = useAppData();
  const location = useLocation();
  const [ws, setWs] = useState<WorkingSet>(loadWorkingSet);

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
        <span className="rail-body">
          <span className="rail-line1">
            <span className="mono rail-name">@{bare}</span>
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
    <nav className="mesh-col" aria-label="primary">
      {NAV.map((n) => (
        <NavLink key={n.to} to={n.to} end={n.end} className={({ isActive }) => `nav-item${isActive ? " active" : ""}`}>
          <span className="nav-key" style={{ color: "var(--text-dim)", width: 14 }}>{n.key}</span>
          <span>{n.label}</span>
          {n.label === "Now" && needs > 0 && <span className="badge-count">{needs}</span>}
        </NavLink>
      ))}
      <div className="nav-section label" style={{ marginTop: 12 }} title="busy / live / registered">
        Fleet · {busy} busy · {live}/{agents.length} live
      </div>
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
    </nav>
  );
}

/** Daemon (API) version next to the UI's own build stamp. They come from the
 *  same workspace version, so a difference means this page is stale — a
 *  cached bundle after `aspen update --restart` — and needs a reload. */
function VersionBadge({ node }: { node: NodeInfo | null }) {
  const ui = __ASPEN_UI_VERSION__;
  const uiSha = __ASPEN_UI_SHA__;
  if (!node) return null;
  const stale = node.version !== ui || (node.sha && uiSha !== "unknown" && node.sha !== uiSha);
  const title = `api ${node.version}${node.sha ? ` (${node.sha})` : ""} · ui ${ui} (${uiSha})${
    stale ? " — reload to pick up the new console" : ""
  }`;
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
    <span className="micro" title={title} style={{ color: "var(--text-dim)" }}>
      v{node.version}
      {node.sha ? ` ${node.sha}` : ""}
    </span>
  );
}

function FlowRedirect() {
  const location = useLocation();
  return <Navigate to={location.pathname.replace(/^\/conversations/, "/flow")} replace />;
}

function StatusBar() {
  const { agents, node } = useAppData();
  const [theme, toggleTheme] = useTheme();
  const busy = agents.filter((a) => a.live && a.turn_state === "busy").length;
  const idle = agents.filter((a) => a.live && a.turn_state !== "busy").length;
  const off = agents.filter((a) => !a.live).length;
  return (
    <header className="statusbar">
      <span className="brand">ASP<b>E</b>N</span>
      <span className="mono-meta">{node ? `node ${node.node}` : "connecting…"}</span>
      <span className="spacer" />
      <VersionBadge node={node} />
      <span className="micro" style={{ color: "var(--live)" }}>{busy} BUSY</span>
      <span className="micro" style={{ color: "var(--idle)" }}>{idle} IDLE</span>
      <span className="micro" style={{ color: "var(--offline)" }}>{off} OFF</span>
      <button className="btn ghost sm" onClick={toggleTheme} title={`theme: ${theme}`} aria-label="toggle theme">
        {theme === "dark" ? "◑" : theme === "light" ? "◐" : "◒"}
      </button>
    </header>
  );
}

export default function App() {
  const agentsPoll = usePoll(api.agents, 2000);
  const inboxPoll = usePoll(api.inbox, 5000);
  const nodePoll = usePoll(api.node, 30000);

  const data: AppData = {
    agents: agentsPoll.data ?? [],
    agentsError: agentsPoll.error,
    agentsLoaded: agentsPoll.data !== null,
    refreshAgents: agentsPoll.refresh,
    inbox: inboxPoll.data ?? [],
    refreshInbox: inboxPoll.refresh,
    node: nodePoll.data,
  };

  return (
    <AppDataContext.Provider value={data}>
      <HotkeysProvider>
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
              <Route path="/mesh" element={<Mesh />} />
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
      </HotkeysProvider>
    </AppDataContext.Provider>
  );
}
