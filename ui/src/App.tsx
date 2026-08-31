import { createContext, useContext } from "react";
import { NavLink, Route, Routes } from "react-router-dom";
import { api, type Agent, type BusMessage } from "./api";
import { usePoll } from "./hooks";
import Mesh from "./pages/Mesh";
import Session from "./pages/Session";
import Bus from "./pages/Bus";
import Inbox from "./pages/Inbox";
import Skills from "./pages/Skills";

export interface AppData {
  agents: Agent[];
  agentsError: string | null;
  agentsLoaded: boolean;
  refreshAgents: () => Promise<void>;
  inbox: BusMessage[];
  refreshInbox: () => Promise<void>;
}

const AppDataContext = createContext<AppData>({
  agents: [],
  agentsError: null,
  agentsLoaded: false,
  refreshAgents: async () => {},
  inbox: [],
  refreshInbox: async () => {},
});

export function useAppData(): AppData {
  return useContext(AppDataContext);
}

function turnDotClass(agent: Agent): string {
  if (!agent.live) return "dot dot-down";
  return agent.turn_state === "busy" ? "dot dot-busy" : "dot dot-idle";
}

function Sidebar() {
  const { agents, inbox } = useAppData();
  return (
    <nav className="sidebar" aria-label="primary">
      <div className="brand">
        <span className="brand-mark">aspen</span>
        <span className="brand-sub">operator console</span>
      </div>
      <div className="nav-links">
        <NavLink to="/" end>
          Mesh
        </NavLink>
        <NavLink to="/bus">Bus</NavLink>
        <NavLink to="/skills">Skills</NavLink>
        <NavLink to="/inbox">
          Inbox
          {inbox.length > 0 && <span className="badge badge-unread">{inbox.length}</span>}
        </NavLink>
      </div>
      <div className="sidebar-agents">
        <div className="sidebar-heading">agents</div>
        {agents.length === 0 && <div className="sidebar-empty">none planted</div>}
        {agents.map((a) => (
          <NavLink
            key={a.name}
            to={`/session/${encodeURIComponent(a.name)}`}
            className="sidebar-agent"
            title={`${a.repo ?? `remote · ${a.node}`} · ${a.live ? (a.turn_state ?? "live") : "down"}`}
          >
            <span className={turnDotClass(a)} />
            <span className="mono">@{a.name}</span>
            {a.pending > 0 && <span className="badge">{a.pending}</span>}
          </NavLink>
        ))}
      </div>
    </nav>
  );
}

export default function App() {
  const agentsPoll = usePoll(api.agents, 2000);
  const inboxPoll = usePoll(api.inbox, 5000);

  const data: AppData = {
    agents: agentsPoll.data ?? [],
    agentsError: agentsPoll.error,
    agentsLoaded: agentsPoll.data !== null,
    refreshAgents: agentsPoll.refresh,
    inbox: inboxPoll.data ?? [],
    refreshInbox: inboxPoll.refresh,
  };

  return (
    <AppDataContext.Provider value={data}>
      <div className="shell">
        <Sidebar />
        <main className="content">
          <Routes>
            <Route path="/" element={<Mesh />} />
            <Route path="/session/:name" element={<Session />} />
            <Route path="/bus" element={<Bus />} />
            <Route path="/skills" element={<Skills />} />
            <Route path="/inbox" element={<Inbox />} />
          </Routes>
        </main>
      </div>
    </AppDataContext.Provider>
  );
}
