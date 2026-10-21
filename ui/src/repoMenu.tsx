// The repo menu (PROPOSALS-2026-10-T §2): the session bar's repo segment
// opens the repo — its agents with their state and the usual verbs, a new
// agent there, and on a board, ways to bring a sibling onto the board.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, type Agent } from "./api";
import { useAppData } from "./App";
import { Meter, presenceOf, relTime } from "./components";
import { useTrustedStart } from "./trust";

/** A board's verbs for a pane's repo menu: this pane, or beside it. */
export interface TeamVerbs {
  showHere: (agent: string) => void;
  addBeside: (agent: string) => void;
}

const bare = (n: string) => n.split("@")[0];

/** The agents of one repo, the way the menu orders them: busy, then live
 *  and idle, then stopped; most recently active first within each. */
export function repoAgents(agents: Agent[], channel: string, node: string): { here: Agent[]; elsewhere: Agent[] } {
  const rank = (a: Agent) => (a.live ? (a.turn_state === "busy" ? 0 : 1) : 2);
  const order = (x: Agent, y: Agent) => rank(x) - rank(y) || (y.last_active ?? 0) - (x.last_active ?? 0);
  const same = agents.filter((a) => a.channel === channel && !a.moved_to);
  return {
    here: same.filter((a) => a.node === node).sort(order),
    elsewhere: same.filter((a) => a.node !== node).sort(order),
  };
}

export function RepoMenu({ agent, team, onClose }: { agent: Agent; team?: TeamVerbs; onClose: () => void }) {
  const nav = useNavigate();
  const { agents, waiting, refreshAgents, node: nodeInfo } = useAppData();
  const trust = useTrustedStart();
  const ref = useRef<HTMLDivElement>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [showElsewhere, setShowElsewhere] = useState(false);
  // A remote agent's row carries no repo path, and a local one no node
  // name (on a node outside a mesh); the mesh's repo list has both, as
  // the Mesh list keys them.
  const [repoPath, setRepoPath] = useState<string | null>(agent.repo);
  const [meshNode, setMeshNode] = useState<string | null>(agent.remote ? agent.node : null);
  useEffect(() => {
    let live = true;
    api
      .meshRepos()
      .then((r) => {
        const n = agent.remote ? r.nodes.find((x) => x.node === agent.node) : r.nodes.find((x) => x.self);
        if (!live || !n) return;
        setMeshNode(n.node);
        const repo = n.repos.find((x) => x.handle === agent.channel);
        if (!agent.repo && repo) setRepoPath(repo.path);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [agent.repo, agent.node, agent.remote, agent.channel]);
  const nodeLabel = agent.remote ? agent.node : nodeInfo?.node ?? meshNode ?? "";

  // Fixed, under the bar where it opened (a pane clips anything absolute),
  // kept inside the window.
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  useLayoutEffect(() => {
    const anchor = ref.current?.parentElement?.closest(".session-bar") ?? ref.current?.parentElement;
    const r = anchor?.getBoundingClientRect();
    if (!r) return;
    const w = Math.min(640, window.innerWidth - 16);
    setPos({ top: Math.round(r.bottom + 4), left: Math.round(Math.max(8, Math.min(r.left + 8, window.innerWidth - w - 8))) });
  }, []);

  const { here, elsewhere } = useMemo(() => repoAgents(agents, agent.channel, agent.node), [agents, agent.channel, agent.node]);
  const liveCount = here.filter((a) => a.live).length;

  // Close on a click outside or Escape.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Element;
      // The repo button toggles on its own click.
      if (t.closest?.(".session-repo")) return;
      if (ref.current && !ref.current.contains(t)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const open = (n: string) => {
    onClose();
    nav(`/session/${encodeURIComponent(n)}`);
  };
  const revive = async (n: string) => {
    setErr(null);
    try {
      await api.revive(n);
      await refreshAgents();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  async function startNew() {
    const n = name.trim();
    if (!n || !repoPath) return;
    setBusy(true);
    setErr(null);
    try {
      const a = await trust.start({ name: n, repo: repoPath, node: agent.remote ? agent.node : undefined });
      if (a) {
        await refreshAgents();
        if (team) {
          team.showHere(a.name);
          onClose();
        } else open(a.name);
      }
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const row = (a: Agent) => {
    const self = a.name === agent.name;
    return (
      <div key={a.name} className={`repo-menu-row${self ? " self" : ""}`}>
        <Meter presence={presenceOf(a.live, a.turn_state)} />
        <span className="mono repo-menu-name">@{a.bare ?? bare(a.name)}</span>
        {waiting.has(a.name) && <span className="rail-wait-pip" title="waiting on you" aria-label="waiting on you" />}
        <span className="repo-menu-title dim" title={a.title ?? undefined}>{a.title ?? ""}</span>
        <span className="mono-meta">{self ? "this one" : a.live ? (a.turn_state === "busy" ? "busy" : "idle") : "stopped"}</span>
        {a.last_active ? <span className="mono-meta repo-menu-when">{relTime(a.last_active)}</span> : <span className="repo-menu-when" />}
        <span className="repo-menu-verbs">
          {!self && team && (
            <>
              <button type="button" className="btn sm" onClick={() => { team.showHere(a.name); onClose(); }} title="show it in this pane; the one here goes to the bench">show here</button>
              <button type="button" className="btn ghost sm" onClick={() => { team.addBeside(a.name); onClose(); }} title="split this pane and put it beside">add to board</button>
            </>
          )}
          {!self && !team && (a.live ? (
            <button type="button" className="btn sm" onClick={() => open(a.name)}>open</button>
          ) : (
            <button type="button" className="btn sm" onClick={() => void revive(a.name)} title="start it again on its transcript">revive</button>
          ))}
          {!self && team && !a.live && (
            <button type="button" className="btn ghost sm" onClick={() => void revive(a.name)} title="start it again on its transcript">revive</button>
          )}
        </span>
      </div>
    );
  };

  return (
    <div ref={ref} className="session-menu repo-menu" role="dialog" aria-label={`agents in #${agent.channel}`} style={pos ?? { visibility: "hidden" }}>
      <div className="repo-menu-head">
        <span className="mono">#{agent.channel}</span>
        {nodeLabel && <span className="mono-meta">{nodeLabel} ·</span>}
        <span className="mono-meta repo-menu-path" title={repoPath ?? undefined}>{repoPath ?? ""}</span>
        <span style={{ flex: 1 }} />
        <span className="mono-meta">{here.length} agent{here.length === 1 ? "" : "s"} · {liveCount} live</span>
      </div>
      <div className="repo-menu-rows">{here.map(row)}</div>
      {elsewhere.length > 0 && (
        <>
          <button type="button" className="link mono-meta repo-menu-fold" aria-expanded={showElsewhere} onClick={() => setShowElsewhere((x) => !x)}>
            {showElsewhere ? "▾" : "▸"} on other nodes ({elsewhere.length})
          </button>
          {showElsewhere && (
            <div className="repo-menu-rows">
              {elsewhere.map((a) => (
                <div key={a.name} className="repo-menu-row">
                  <Meter presence={presenceOf(a.live, a.turn_state)} />
                  <span className="mono repo-menu-name">@{a.bare ?? bare(a.name)}</span>
                  <span className="chip mono">{a.node}</span>
                  <span className="repo-menu-title dim">{a.title ?? ""}</span>
                  <span className="repo-menu-verbs">
                    {team ? (
                      <button type="button" className="btn ghost sm" onClick={() => { team.addBeside(a.name); onClose(); }}>add to board</button>
                    ) : (
                      <button type="button" className="btn sm" onClick={() => open(a.name)}>open</button>
                    )}
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
      <form
        className="repo-menu-new"
        onSubmit={(e) => {
          e.preventDefault();
          void startNew();
        }}
      >
        <span className="mono-meta">new agent here</span>
        <span className="mono-meta">@</span>
        <input className="mono" value={name} onChange={(e) => setName(e.target.value.replace(/[^A-Za-z0-9_-]/g, ""))} placeholder="name" aria-label="new agent name" disabled={!repoPath} />
        <button type="submit" className="btn primary sm" disabled={!name.trim() || !repoPath || busy}>{busy ? "starting…" : "start"}</button>
        {team && <span className="mono-meta">(opens in this pane)</span>}
      </form>
      {err && <div className="error-text mono-meta">{err}</div>}
      <div className="repo-menu-foot">
        {repoPath && meshNode && (
          <button type="button" className="link mono-meta" onClick={() => { onClose(); nav(`/mesh?node=${encodeURIComponent(meshNode)}&repo=${encodeURIComponent(repoPath)}`); }}>
            all transcripts (Mesh) →
          </button>
        )}
        <button type="button" className="link mono-meta" onClick={() => { onClose(); nav(`/session/${encodeURIComponent(agent.name)}/files`); }}>
          files →
        </button>
      </div>
      {trust.dialog}
    </div>
  );
}
