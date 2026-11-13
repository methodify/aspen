// Start an agent where you are (PROPOSALS-2026-11-B §B-3): a name, a
// template eligible for this node and repo, a starter prompt, and a project
// to join. Used by the Mesh view's repo rows.
import { useEffect, useMemo, useState } from "react";
import { api, templateFits, type Board, type Harness, type Template } from "./api";

const NAME_RE = /^[A-Za-z0-9_-]+$/;
export const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;

export interface StartChoice {
  name: string;
  template?: string;
  prompt?: string;
  project?: string;
  effort?: string;
  harness?: Harness;
}

export function StartDialog({
  node,
  self,
  repo,
  onStart,
  onCancel,
}: {
  /** The node, as the mesh names it. */
  node: string;
  /** It is the console's own node. */
  self: boolean;
  repo: { path: string; handle?: string | null };
  onStart: (c: StartChoice) => void;
  onCancel: () => void;
}) {
  const [templates, setTemplates] = useState<Template[]>([]);
  const [projects, setProjects] = useState<Board[]>([]);
  const [name, setName] = useState("");
  const [tid, setTid] = useState("");
  const [prompt, setPrompt] = useState("");
  const [project, setProject] = useState("");
  const [effort, setEffort] = useState("");
  const [harness, setHarness] = useState<Harness | "">("");
  const [more, setMore] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api.templates().then(setTemplates).catch(() => setTemplates([]));
    api
      .boards()
      .then((bs) => setProjects(bs.filter((b) => !b.deleted && !b.query && b.status !== "archived")))
      .catch(() => setProjects([]));
  }, []);
  const fits = useMemo(
    () =>
      templates
        .filter((t) => templateFits(t, node, repo))
        // This node's own first, then by name.
        .sort((a, b) => Number(!!b.spec.node) - Number(!!a.spec.node) || a.name.localeCompare(b.name)),
    [templates, node, repo],
  );
  const pick = (id: string) => {
    setTid(id);
    const t = fits.find((x) => x.id === id);
    if (!t) return;
    if (t.spec.name && !name.trim()) setName(t.spec.name);
    setPrompt(t.spec.prompt ?? "");
    setEffort(t.spec.effort ?? "");
    setHarness(t.spec.harness ?? "");
  };
  const submit = () => {
    const n = name.trim();
    if (!n) return setErr("a name is required");
    if (!NAME_RE.test(n)) return setErr("letters, digits, - and _ only");
    if (n === "operator") return setErr("“operator” is reserved");
    onStart({
      name: n,
      template: tid || undefined,
      prompt: prompt.trim() ? prompt : undefined,
      project: project || undefined,
      effort: effort || undefined,
      harness: harness || undefined,
    });
  };
  const chosen = fits.find((t) => t.id === tid);
  return (
    <form
      className="start-dialog"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="start-row">
        <input className="mono" value={name} onChange={(e) => setName(e.target.value)} placeholder="agent name" aria-label="agent name" autoFocus spellCheck={false} />
        <select value={tid} onChange={(e) => pick(e.target.value)} aria-label="template" title="templates made for this node or anywhere, and for this repo or any">
          <option value="">no template</option>
          {fits.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name}
              {t.spec.node ? " · this node's" : ""}
              {t.spec.plugins?.length ? ` · ${t.spec.plugins.length} plugin${t.spec.plugins.length === 1 ? "" : "s"}` : ""}
              {t.spec.prompt ? " · starter" : ""}
            </option>
          ))}
        </select>
        <button type="submit" className="btn primary sm">start</button>
        <button type="button" className="btn ghost sm" onClick={onCancel}>cancel</button>
      </div>
      {chosen && (
        <div className="mono-meta">
          {[chosen.spec.harness, chosen.spec.model, chosen.spec.effort && `effort ${chosen.spec.effort}`, chosen.spec.permission === "skip" && "skip permissions", chosen.spec.plugins?.map((p) => p.plugin).join(", ")].filter(Boolean).join(" · ") || "the template's defaults"}
          {chosen.spec.charter ? " · has a charter" : ""}
        </div>
      )}
      <label className="start-field">
        <span className="label">starter prompt</span>
        <textarea
          rows={4}
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="optional: its first message, e.g. “We're joining {{project}}. Read the skills you have, then wait for me.”"
        />
        <span className="mono-meta">sent once it is up · {"{{name}} {{node}} {{repo}} {{project}} {{charter}}"} are filled in</span>
      </label>
      <div className="start-row">
        <label className="start-inline">
          <span className="label">join a project</span>
          <select value={project} onChange={(e) => setProject(e.target.value)} aria-label="project">
            <option value="">none</option>
            {projects.map((b) => (
              <option key={b.id} value={b.id}>
                {b.name}
                {b.status === "paused" ? " · paused" : ""}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="link mono-meta" onClick={() => setMore(!more)}>
          {more ? "less" : "runtime, effort…"}
        </button>
      </div>
      {more && (
        <div className="start-row">
          <label className="start-inline">
            <span className="label">runtime</span>
            <select value={harness} onChange={(e) => setHarness(e.target.value as Harness | "")}>
              <option value="">repo default</option>
              <option value="claude">claude</option>
              <option value="codex">codex</option>
            </select>
          </label>
          <label className="start-inline">
            <span className="label">effort</span>
            <select value={effort} onChange={(e) => setEffort(e.target.value)}>
              <option value="">default</option>
              {EFFORTS.map((x) => (
                <option key={x} value={x}>{x}</option>
              ))}
            </select>
          </label>
        </div>
      )}
      <div className="mono-meta">
        in {repo.handle ? `#${repo.handle}` : repo.path} on {self ? `${node} (this node)` : node}
      </div>
      {err && <div className="error-text mono-meta">{err}</div>}
    </form>
  );
}
