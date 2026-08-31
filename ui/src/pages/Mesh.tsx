import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Agent } from "./../api";
import { useAppData } from "./../App";

function AgentCard({ agent }: { agent: Agent }) {
  const state = !agent.live ? "down" : (agent.turn_state ?? "live");
  return (
    <Link to={`/session/${encodeURIComponent(agent.name)}`} className="agent-card">
      <div className="agent-card-head">
        <span
          className={
            !agent.live ? "dot dot-down" : agent.turn_state === "busy" ? "dot dot-busy" : "dot dot-idle"
          }
        />
        <span className="mono agent-name">@{agent.name}</span>
        <span className={`chip chip-${state}`}>{state}</span>
        {agent.pending > 0 && (
          <span className="badge" title="undelivered bus messages held for it">
            {agent.pending} pending
          </span>
        )}
      </div>
      <div className="agent-card-repo mono">
        {agent.repo ?? `remote · ${agent.node}`}
      </div>
      <div className="agent-card-meta">
        <span className="mono">#{agent.channel}</span>
        {agent.charter && <span className="agent-card-charter">{agent.charter}</span>}
      </div>
    </Link>
  );
}

function PlantForm() {
  const { refreshAgents } = useAppData();
  const [name, setName] = useState("");
  const [repo, setRepo] = useState("");
  const [charter, setCharter] = useState("");
  const [model, setModel] = useState("");
  const [resume, setResume] = useState("");
  const [allowAll, setAllowAll] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [planted, setPlanted] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setError(null);
    setPlanted(null);
    if (!name.trim() || !repo.trim()) {
      setError("name and repo are required");
      return;
    }
    setBusy(true);
    try {
      const agent = await api.plantAgent({
        name: name.trim(),
        repo: repo.trim(),
        ...(charter.trim() ? { charter: charter.trim() } : {}),
        ...(model.trim() ? { model: model.trim() } : {}),
        ...(resume.trim() ? { resume: resume.trim() } : {}),
        ...(allowAll ? { allow_all: true } : {}),
      });
      setPlanted(agent.name);
      setName("");
      setCharter("");
      setModel("");
      setResume("");
      setAllowAll(false);
      await refreshAgents();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="panel plant-form" onSubmit={submit}>
      <h2>plant an agent</h2>
      <div className="form-row">
        <label>
          name
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="arch"
            className="mono"
            required
          />
        </label>
        <label>
          repo path
          <input
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="/home/you/src/project"
            className="mono grow"
            required
          />
        </label>
      </div>
      <label>
        charter <span className="dim">(optional — who this agent is on the bus)</span>
        <textarea
          value={charter}
          onChange={(e) => setCharter(e.target.value)}
          rows={2}
          placeholder="you are the architect for this repo; impl sessions report findings to you"
        />
      </label>
      <div className="form-row">
        <label>
          model <span className="dim">(optional)</span>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="default"
            className="mono"
          />
        </label>
        <label>
          resume session id <span className="dim">(optional)</span>
          <input
            value={resume}
            onChange={(e) => setResume(e.target.value)}
            placeholder=""
            className="mono grow"
          />
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={allowAll}
            onChange={(e) => setAllowAll(e.target.checked)}
          />
          allow_all
        </label>
        <button type="submit" disabled={busy}>
          {busy ? "planting…" : "plant"}
        </button>
      </div>
      {error && <div className="error-inline">{error}</div>}
      {planted && (
        <div className="ok-inline">
          planted <span className="mono">@{planted}</span> —{" "}
          <Link to={`/session/${encodeURIComponent(planted)}`}>open session</Link>
        </div>
      )}
    </form>
  );
}

export default function Mesh() {
  const { agents, agentsError, agentsLoaded } = useAppData();
  return (
    <div className="page">
      <header className="page-head">
        <h1>Mesh</h1>
        <span className="dim">
          {agents.length} agent{agents.length === 1 ? "" : "s"} ·{" "}
          {agents.filter((a) => a.live).length} live
        </span>
      </header>
      {agentsError && <div className="error-inline">roster: {agentsError}</div>}
      <div className="agent-grid">
        {agents.map((a) => (
          <AgentCard key={a.name} agent={a} />
        ))}
        {agentsLoaded && agents.length === 0 && (
          <div className="empty">no agents yet — plant one below.</div>
        )}
      </div>
      <PlantForm />
    </div>
  );
}
