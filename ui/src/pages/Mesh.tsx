import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Agent } from "./../api";
import { usePoll } from "./../hooks";
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

function NewSessionForm() {
  const { refreshAgents } = useAppData();
  const repoPoll = usePoll(api.repos, 5000);
  const repoPaths = (repoPoll.data ?? []).map((r) => r.path);
  const [name, setName] = useState("");
  const [repo, setRepo] = useState("");
  const [charter, setCharter] = useState("");
  const [model, setModel] = useState("");
  const [resume, setResume] = useState("");
  const [allowAll, setAllowAll] = useState(false);
  const [skipPermissions, setSkipPermissions] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setError(null);
    setStarted(null);
    if (!name.trim() || !repo.trim()) {
      setError("name and repo are required");
      return;
    }
    setBusy(true);
    try {
      const agent = await api.startAgent({
        name: name.trim(),
        repo: repo.trim(),
        ...(charter.trim() ? { charter: charter.trim() } : {}),
        ...(model.trim() ? { model: model.trim() } : {}),
        ...(resume.trim() ? { resume: resume.trim() } : {}),
        ...(allowAll ? { allow_all: true } : {}),
        // Only send when checked, so an unchecked box uses the repo default.
        ...(skipPermissions ? { skip_permissions: true } : {}),
      });
      setStarted(agent.name);
      setName("");
      setCharter("");
      setModel("");
      setResume("");
      setAllowAll(false);
      setSkipPermissions(false);
      await refreshAgents();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="panel session-form" onSubmit={submit}>
      <h2>New session</h2>
      <datalist id="mesh-repo-paths">
        {repoPaths.map((p) => (
          <option key={p} value={p} />
        ))}
      </datalist>
      <div className="form-row">
        <label>
          agent name
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="arch"
            className="mono"
            required
          />
        </label>
        <label>
          repository path
          <input
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="/home/you/src/project"
            className="mono grow"
            list="mesh-repo-paths"
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
        <label className="check">
          <input
            type="checkbox"
            checked={skipPermissions}
            onChange={(e) => setSkipPermissions(e.target.checked)}
          />
          Skip permission prompts (dangerous)
        </label>
        <button type="submit" disabled={busy}>
          {busy ? "starting…" : "Start"}
        </button>
      </div>
      {error && <div className="error-inline">{error}</div>}
      {started && (
        <div className="ok-inline">
          started <span className="mono">@{started}</span> —{" "}
          <Link to={`/session/${encodeURIComponent(started)}`}>open session</Link>
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
          <div className="empty">No agents yet. Start one below.</div>
        )}
      </div>
      <NewSessionForm />
    </div>
  );
}
