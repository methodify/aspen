// The new-session panel: name + repo + charter/model/args/skip, through
// the trust gate. Shared by Now and the Mesh list.
import { useState } from "react";
import { type StartAgentRequest } from "./api";
import { type TrustedStart } from "./trust";
import { ErrorBar } from "./components";

const NAME_RE = /^[A-Za-z0-9_-]+$/;

export function NewSessionPanel({
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
