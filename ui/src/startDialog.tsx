// Start an agent where you are (PROPOSALS-2026-11-B §B-3): a name, a
// template eligible for this node and repo, a starter prompt, and a project
// to join; under "more", runtime, effort, charter, model, args and
// permissions. Used everywhere an agent starts: the Mesh view's repo rows,
// a project's add-a-member picker, and the new-session dialog.
import { useEffect, useMemo, useState } from "react";
import { api, templateFits, type Board, type Harness, type Template } from "./api";

const NAME_RE = /^[A-Za-z0-9_-]+$/;
/** The project choice that makes one. */
const NEW = "\u0000new";
export const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;

export interface StartChoice {
  name: string;
  template?: string;
  prompt?: string;
  project?: string;
  effort?: string;
  harness?: Harness;
  charter?: string;
  model?: string;
  extra_args?: string;
  skip_permissions?: boolean;
}

/** A choice as the start request carries it (trust.start). */
export function startFields(c: StartChoice) {
  return {
    name: c.name,
    ...(c.template ? { template: c.template } : {}),
    ...(c.prompt ? { prompt: c.prompt } : {}),
    ...(c.project ? { project: c.project } : {}),
    ...(c.effort ? { effort: c.effort } : {}),
    ...(c.harness ? { harness: c.harness } : {}),
    ...(c.charter ? { charter: c.charter } : {}),
    ...(c.model ? { model: c.model } : {}),
    ...(c.extra_args ? { extra_args: c.extra_args } : {}),
    ...(c.skip_permissions ? { skip_permissions: true } : {}),
  };
}

export function StartDialog({
  node,
  self,
  repo,
  project: fixedProject,
  initialTemplate,
  initialName,
  onStart,
  onCancel,
}: {
  /** The node, as the mesh names it. */
  node: string;
  /** It is the console's own node. */
  self: boolean;
  repo: { path: string; handle?: string | null };
  /** Started into this project (the board's picker): no project choice. */
  project?: { id: string; name: string };
  /** Preselect a template (id or name), when it fits here. */
  initialTemplate?: string;
  initialName?: string;
  onStart: (c: StartChoice) => void;
  onCancel: () => void;
}) {
  const [templates, setTemplates] = useState<Template[]>([]);
  const [projects, setProjects] = useState<Board[]>([]);
  const [name, setName] = useState(initialName ?? "");
  const [tid, setTid] = useState("");
  const [prompt, setPrompt] = useState("");
  const [project, setProject] = useState("");
  const [newProject, setNewProject] = useState("");
  const [newCharter, setNewCharter] = useState("");
  const [effort, setEffort] = useState("");
  const [harness, setHarness] = useState<Harness | "">("");
  const [charter, setCharter] = useState("");
  const [model, setModel] = useState("");
  const [args, setArgs] = useState("");
  const [skip, setSkip] = useState(false);
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
    setCharter(t.spec.charter ?? "");
    setModel(t.spec.model ?? "");
    setArgs(t.spec.extra_args ?? "");
    setSkip(t.spec.permission === "skip");
  };
  const [preset, setPreset] = useState(false);
  useEffect(() => {
    if (preset || !initialTemplate || fits.length === 0) return;
    const t = fits.find((x) => x.id === initialTemplate || x.name === initialTemplate);
    if (t) pick(t.id);
    setPreset(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialTemplate, fits]);
  const submit = async () => {
    const n = name.trim();
    if (!n) return setErr("a name is required");
    if (!NAME_RE.test(n)) return setErr("letters, digits, - and _ only");
    if (n === "operator") return setErr("“operator” is reserved");
    // A new project, made here first (an empty desk the agent lands on),
    // so it all happens at once.
    let projectId = fixedProject?.id ?? (project && project !== NEW ? project : undefined);
    if (!fixedProject && project === NEW) {
      const pname = newProject.trim();
      if (!pname) return setErr("the new project needs a name");
      const id = crypto.randomUUID().slice(0, 8);
      try {
        await api.putBoard({ id, name: pname, layout: { kind: "empty", id: "e1" }, updated_at: Date.now() / 1000 });
        if (newCharter.trim()) await api.patchBoard(id, { charter: newCharter.trim() });
      } catch (e) {
        return setErr(`the project was not made: ${e instanceof Error ? e.message : String(e)}`);
      }
      projectId = id;
    }
    onStart({
      name: n,
      template: tid || undefined,
      prompt: prompt.trim() ? prompt : undefined,
      project: projectId,
      effort: effort || undefined,
      harness: harness || undefined,
      // A template's own charter, model, args and permission come with it
      // (the node applies them); what is sent here is what was changed.
      charter: charter.trim() && charter !== (chosen?.spec.charter ?? "") ? charter.trim() : undefined,
      model: model.trim() && model !== (chosen?.spec.model ?? "") ? model.trim() : undefined,
      extra_args: args.trim() && args !== (chosen?.spec.extra_args ?? "") ? args.trim() : undefined,
      skip_permissions: skip || undefined,
    });
  };
  const chosen = fits.find((t) => t.id === tid);
  return (
    <form
      className="start-dialog"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
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
        {fixedProject ? (
          <span className="mono-meta">joins “{fixedProject.name}”</span>
        ) : (
        <label className="start-inline">
          <span className="label">join a project</span>
          <select value={project} onChange={(e) => setProject(e.target.value)} aria-label="project">
            <option value="">none</option>
            <option value={NEW}>a new project…</option>
            {projects.map((b) => (
              <option key={b.id} value={b.id}>
                {b.name}
                {b.status === "paused" ? " · paused" : ""}
              </option>
            ))}
          </select>
        </label>
        )}
        {!fixedProject && project === NEW && (
          <input className="new-project-name" value={newProject} onChange={(e) => setNewProject(e.target.value)} placeholder="the new project's name" aria-label="new project name" autoFocus />
        )}
        <button type="button" className="link mono-meta" onClick={() => setMore(!more)}>
          {more ? "less" : "runtime, effort, charter…"}
        </button>
      </div>
      {!fixedProject && project === NEW && (
        <label className="start-field">
          <span className="label">the project's charter (optional)</span>
          <textarea rows={2} value={newCharter} onChange={(e) => setNewCharter(e.target.value)} placeholder="what the project is for; every member reads it, this agent first" />
        </label>
      )}
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
          <label className="start-inline">
            <span className="label">model</span>
            <input className="mono" value={model} onChange={(e) => setModel(e.target.value)} placeholder="default" spellCheck={false} style={{ width: 140 }} />
          </label>
          <label className="start-inline">
            <input type="checkbox" checked={skip} onChange={(e) => setSkip(e.target.checked)} />
            <span className="mono-meta">skip permission prompts (dangerous)</span>
          </label>
        </div>
      )}
      {more && (
        <label className="start-field">
          <span className="label">charter</span>
          <textarea rows={3} value={charter} onChange={(e) => setCharter(e.target.value)} placeholder="what this agent is here to do (its own part; a project's charter comes before it)" />
        </label>
      )}
      {more && (
        <label className="start-field">
          <span className="label">runtime args</span>
          <input className="mono" value={args} onChange={(e) => setArgs(e.target.value)} placeholder="e.g. --chrome (after the harness defaults)" spellCheck={false} />
        </label>
      )}
      <div className="mono-meta">
        in {repo.handle ? `#${repo.handle}` : repo.path} on {self ? `${node} (this node)` : node}
      </div>
      {err && <div className="error-text mono-meta">{err}</div>}
    </form>
  );
}
