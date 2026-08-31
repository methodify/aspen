import { createContext, useContext } from "react";
import { NavLink, Route, Routes } from "react-router-dom";
import { api, type Agent, type BusMessage, type NodeInfo } from "./api";
import { usePoll } from "./hooks";
import { Meter, presenceOf, useTheme } from "./components";
import Command from "./pages/Command";
import Conversations from "./pages/Conversations";
import Sessions from "./pages/Sessions";
import Session from "./pages/Session";
import MeshMap from "./pages/MeshMap";
import Library from "./pages/Library";
import Palette from "./Palette";

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
  { to: "/", key: "C", label: "Command", end: true },
  { to: "/conversations", key: "V", label: "Conversations" },
  { to: "/sessions", key: "S", label: "Sessions" },
  { to: "/map", key: "M", label: "Map" },
  { to: "/library", key: "L", label: "Library" },
];

function MeshColumn() {
  const { agents, inbox } = useAppData();
  const live = agents.filter((a) => a.live);
  return (
    <nav className="mesh-col" aria-label="primary">
      {NAV.map((n) => (
        <NavLink key={n.to} to={n.to} end={n.end} className={({ isActive }) => `nav-item${isActive ? " active" : ""}`}>
          <span className="nav-key" style={{ color: "var(--text-dim)", width: 14 }}>{n.key}</span>
          <span>{n.label}</span>
          {n.label === "Command" && inbox.length > 0 && (
            <span className="badge-count">{inbox.length}</span>
          )}
        </NavLink>
      ))}
      <div className="nav-section label" style={{ marginTop: 12 }}>
        Sessions · {live.length}/{agents.length}
      </div>
      {agents.length === 0 && (
        <div style={{ padding: "4px 16px", color: "var(--text-dim)", fontSize: 12 }}>none running</div>
      )}
      {agents.map((a) => (
        <NavLink
          key={a.name}
          to={`/session/${encodeURIComponent(a.name)}`}
          className={({ isActive }) => `nav-item${isActive ? " active" : ""}`}
          title={`${a.repo ?? `remote · ${a.node}`} · ${a.live ? (a.turn_state ?? "live") : "down"}`}
          style={{ gap: 10 }}
        >
          <Meter presence={presenceOf(a.live, a.turn_state)} />
          <span className="mono" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            @{a.name}
          </span>
          {a.pending > 0 && <span className="badge-count">{a.pending}</span>}
        </NavLink>
      ))}
    </nav>
  );
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
      <Palette />
      <div className="shell">
        <StatusBar />
        <div className="body-grid">
          <MeshColumn />
          <main className="stage">
            <Routes>
              <Route path="/" element={<Command />} />
              <Route path="/conversations" element={<Conversations />} />
              <Route path="/conversations/:channel" element={<Conversations />} />
              <Route path="/sessions" element={<Sessions />} />
              <Route path="/session/:name" element={<Session />} />
              <Route path="/map" element={<MeshMap />} />
              <Route path="/library" element={<Library />} />
            </Routes>
          </main>
        </div>
      </div>
    </AppDataContext.Provider>
  );
}
