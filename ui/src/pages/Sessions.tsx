// Sessions — the flight board. Every agent in the mesh as a horizontal strip,
// ranked by presence (busy → idle → down), each a jump into its session. The
// board is live off useAppData()'s 2s agent poll; the "+ new session" panel is
// the one place the operator hand-spins up a new agent.

import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, type Agent, type StartAgentRequest } from "../api";
import { usePoll } from "../hooks";
import { useAppData } from "../App";
import { useHotkeys } from "../hotkeys";
import { useTrustedStart, type TrustedStart } from "../trust";
import { Empty, ErrorBar, Meter, presenceOf } from "../components";

const NAME_RE = /^[A-Za-z0-9_-]+$/;

function presenceRank(a: Agent): number {
  const p = presenceOf(a.live, a.turn_state);
  return p === "busy" ? 0 : p === "idle" ? 1 : 2;
}

function stateWord(p: "busy" | "idle" | "off"): string {
  return p === "busy" ? "streaming" : p === "off" ? "offline" : "idle";
}

function stateColor(p: "busy" | "idle" | "off"): string {
  return p === "busy" ? "var(--live)" : p === "off" ? "var(--offline)" : "var(--idle)";
}

export default function Sessions() {
  const trust = useTrustedStart();
  const nav = useNavigate();
  const { agents, refreshAgents } = useAppData();
  const reposPoll = usePoll(api.repos, 15000);
  const repos = reposPoll.data ?? [];

  const [query, setQuery] = useState("");
  const [panelOpen, setPanelOpen] = useState(false);

  useHotkeys("sessions", [
    {
      key: "n",
      description: "new session",
      handler: () => setPanelOpen((v) => !v),
    },
  ]);
  const [rowErr, setRowErr] = useState<string | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);
  const [focused, setFocused] = useState<string | null>(null);
  const [confirmStop, setConfirmStop] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<string | null>(null);

  const busy = agents.filter((a) => a.live && a.turn_state === "busy").length;
  const idle = agents.filter((a) => a.live && a.turn_state !== "busy").length;
  const down = agents.filter((a) => !a.live).length;

  const filtered = useMemo(() => {
    const q = query.trim().replace(/^[#@]/, "").toLowerCase();
    const list = q
      ? agents.filter(
          (a) => a.name.toLowerCase().includes(q) || a.channel.toLowerCase().includes(q),
        )
      : agents.slice();
    return list.sort((a, b) => presenceRank(a) - presenceRank(b) || a.name.localeCompare(b.name));
  }, [agents, query]);

  async function runAction(name: string, fn: () => Promise<unknown>) {
    setRowErr(null);
    setPendingAction(name);
    try {
      await fn();
      await refreshAgents();
    } catch (e) {
      setRowErr(e instanceof Error ? e.message : "action failed");
    } finally {
      setPendingAction(null);
    }
  }

  function open(name: string) {
    nav(`/session/${encodeURIComponent(name)}`);
  }

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Sessions</span>
        <span className="mono-meta">
          {busy} busy · {idle} idle · {down} down
        </span>
        <span style={{ flex: 1 }} />
        <input
          type="search"
          placeholder="filter @name or #channel"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label="filter sessions"
          style={{ width: 220 }}
        />
        <button
          className="btn primary"
          onClick={() => setPanelOpen((v) => !v)}
          aria-expanded={panelOpen}
        >
          + new session
        </button>
      </div>

      <div className="stage-body">
        {trust.dialog}
        {panelOpen && (
          <NewSessionPanel
            startFn={trust.start}
            repoPaths={repos.map((r) => r.path)}
            existing={agents.map((a) => a.name)}
            onClose={() => setPanelOpen(false)}
            onStarted={async (name) => {
              setPanelOpen(false);
              await refreshAgents();
              open(name);
            }}
          />
        )}

        <ErrorBar error={rowErr} />

        {agents.length === 0 ? (
          <Empty mark="◦">
            No sessions in the mesh yet. Spin one up with{" "}
            <button className="btn ghost sm" onClick={() => setPanelOpen(true)}>
              + new session
            </button>
            .
          </Empty>
        ) : filtered.length === 0 ? (
          <Empty mark="—">Nothing matches “{query}”.</Empty>
        ) : (
          <div className="grid">
            {filtered.map((a) => {
              const p = presenceOf(a.live, a.turn_state);
              const reveal = hovered === a.name || focused === a.name || confirmStop === a.name;
              const disabled = pendingAction === a.name;
              const canResume = !a.live && !a.remote && a.repo != null;
              const where = a.repo ?? `remote · ${a.node}`;
              return (
                <div
                  key={a.name}
                  className="strip"
                  style={{ display: "flex", alignItems: "center", gap: 12, cursor: "pointer" }}
                  role="link"
                  tabIndex={0}
                  aria-label={`open session ${a.name}`}
                  onClick={() => open(a.name)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") open(a.name);
                  }}
                  onMouseEnter={() => setHovered(a.name)}
                  onMouseLeave={() => setHovered((h) => (h === a.name ? null : h))}
                  onFocus={() => setFocused(a.name)}
                  onBlur={(e) => {
                    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
                      setFocused((f) => (f === a.name ? null : f));
                      setConfirmStop((c) => (c === a.name ? null : c));
                    }
                  }}
                >
                  <Meter presence={p} />
                  <span className="mono" style={{ fontWeight: 500 }}>
                    @{a.name}
                  </span>
                  <span className="mono-meta">#{a.channel}</span>
                  <span
                    className="mono-meta"
                    title={where}
                    style={{
                      flex: 1,
                      minWidth: 0,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {where}
                  </span>

                  {reveal ? (
                    <div
                      style={{ display: "flex", gap: 6, alignItems: "center" }}
                      onClick={(e) => e.stopPropagation()}
                    >
                      <button
                        className="btn sm"
                        disabled={disabled}
                        onClick={(e) => {
                          e.stopPropagation();
                          open(a.name);
                        }}
                      >
                        open
                      </button>
                      {p === "busy" && (
                        <button
                          className="btn sm"
                          disabled={disabled}
                          onClick={(e) => {
                            e.stopPropagation();
                            void runAction(a.name, () => api.interrupt(a.name));
                          }}
                        >
                          interrupt
                        </button>
                      )}
                      {canResume && (
                        <button
                          className="btn sm"
                          disabled={disabled}
                          onClick={(e) => {
                            e.stopPropagation();
                            void runAction(a.name, () =>
                              trust.start({
                                name: a.name,
                                repo: a.repo as string,
                                resume: a.session_id,
                              }),
                            );
                          }}
                        >
                          resume
                        </button>
                      )}
                      {confirmStop === a.name ? (
                        <>
                          <button
                            className="btn danger sm"
                            disabled={disabled}
                            onClick={(e) => {
                              e.stopPropagation();
                              setConfirmStop(null);
                              void runAction(a.name, () => api.deleteAgent(a.name));
                            }}
                          >
                            confirm stop
                          </button>
                          <button
                            className="btn ghost sm"
                            onClick={(e) => {
                              e.stopPropagation();
                              setConfirmStop(null);
                            }}
                          >
                            cancel
                          </button>
                        </>
                      ) : (
                        <button
                          className="btn ghost sm"
                          disabled={disabled}
                          onClick={(e) => {
                            e.stopPropagation();
                            setConfirmStop(a.name);
                          }}
                        >
                          stop
                        </button>
                      )}
                    </div>
                  ) : (
                    <>
                      <span className="micro" style={{ color: stateColor(p) }}>
                        {stateWord(p)}
                      </span>
                      {a.pending > 0 && <span className="badge-count">{a.pending}</span>}
                    </>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </>
  );
}

function NewSessionPanel({
  startFn,
  repoPaths,
  existing,
  onClose,
  onStarted,
}: {
  startFn: TrustedStart;
  repoPaths: string[];
  existing: string[];
  onClose: () => void;
  onStarted: (name: string) => void | Promise<void>;
}) {
  const [name, setName] = useState("");
  const [repo, setRepo] = useState("");
  const [charter, setCharter] = useState("");
  const [model, setModel] = useState("");
  const [extraArgs, setExtraArgs] = useState("");
  const [skip, setSkip] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  function validate(): string | null {
    const n = name.trim();
    if (!n) return "agent name is required";
    if (!NAME_RE.test(n)) return "name may use only letters, digits, - and _";
    if (n === "operator") return "“operator” is reserved";
    if (existing.includes(n)) return `@${n} already exists`;
    if (!repo.trim()) return "repository path is required";
    return null;
  }

  async function submit() {
    const problem = validate();
    if (problem) {
      setErr(problem);
      return;
    }
    setErr(null);
    setBusy(true);
    const req: StartAgentRequest = { name: name.trim(), repo: repo.trim() };
    if (charter.trim()) req.charter = charter.trim();
    if (model.trim()) req.model = model.trim();
    if (extraArgs.trim()) req.extra_args = extraArgs.trim();
    if (skip) req.skip_permissions = true;
    try {
      const agent = await startFn(req);
      if (agent === null) {
        // Operator declined the trust review.
        setBusy(false);
        return;
      }
      await onStarted(agent.name);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "failed to start session");
      setBusy(false);
    }
  }

  return (
    <form
      className="strip"
      style={{ display: "grid", gap: 12, marginBottom: 20 }}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div style={{ display: "flex", alignItems: "center" }}>
        <span className="label">New session</span>
        <span style={{ flex: 1 }} />
        <button type="button" className="btn ghost sm" onClick={onClose}>
          close
        </button>
      </div>

      <ErrorBar error={err} />

      <div className="grid cols">
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Agent name</span>
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. west"
            spellCheck={false}
            aria-label="agent name"
          />
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Repository path</span>
          <input
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="/path/to/repo"
            list="known-repos"
            spellCheck={false}
            aria-label="repository path"
          />
          <datalist id="known-repos">
            {repoPaths.map((p) => (
              <option key={p} value={p} />
            ))}
          </datalist>
        </label>
      </div>

      <label style={{ display: "grid", gap: 4 }}>
        <span className="label">Charter (optional)</span>
        <textarea
          rows={3}
          value={charter}
          onChange={(e) => setCharter(e.target.value)}
          placeholder="what this agent is here to do"
          aria-label="charter"
        />
      </label>

      <div className="grid cols">
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Model (optional)</span>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="default"
            spellCheck={false}
            aria-label="model"
          />
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Runtime args (optional)</span>
          <input
            value={extraArgs}
            onChange={(e) => setExtraArgs(e.target.value)}
            placeholder="e.g. --chrome (after harness defaults)"
            spellCheck={false}
            aria-label="runtime args"
          />
        </label>
        <label style={{ display: "flex", alignItems: "center", gap: 8, alignSelf: "end", paddingBottom: 6 }}>
          <input
            type="checkbox"
            checked={skip}
            onChange={(e) => setSkip(e.target.checked)}
            style={{ width: "auto" }}
          />
          <span className="mono" style={{ color: skip ? "var(--sig-gate)" : "var(--text-mid)" }}>
            skip permission prompts (dangerous)
          </span>
        </label>
      </div>

      <div style={{ display: "flex", gap: 8 }}>
        <span style={{ flex: 1 }} />
        <button type="button" className="btn ghost" onClick={onClose} disabled={busy}>
          cancel
        </button>
        <button type="submit" className="btn primary" disabled={busy}>
          {busy ? "starting…" : "start session"}
        </button>
      </div>
    </form>
  );
}
