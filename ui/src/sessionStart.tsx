// The new-session dialog (PROPOSALS-2026-11-B §B-3): one way to start an
// agent from anywhere. Choose the node, then a repo there (or another
// folder, made if missing), then the shared start dialog: a template that
// fits, a starter prompt, a project, and the rest under "more".
//
// Opened from the title bar's +, Now's button and its `n` key, the palette
// (`start …`, `new session from template …`) and a template's *start…*
// (`/?new=<id>`), all through `openNewSession`.
import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, templateFits, type MeshRepoNode, type Template } from "./api";
import { useAppData } from "./App";
import { PathPicker } from "./pathPicker";
import { StartDialog, startFields } from "./startDialog";
import { useTrustedStart } from "./trust";

export interface NewSessionInit {
  node?: string;
  /** A repo path on that node. */
  repo?: string;
  name?: string;
  /** A template id or name. */
  template?: string;
}

const EVENT = "aspen:new-session";

/** Open the new-session dialog, wherever the console is. */
export function openNewSession(init: NewSessionInit = {}) {
  window.dispatchEvent(new CustomEvent<NewSessionInit>(EVENT, { detail: init }));
}

/** Mounted once (App): listens for `openNewSession` and shows the dialog. */
export function NewSessionHost() {
  const [init, setInit] = useState<NewSessionInit | null>(null);
  useEffect(() => {
    const on = (e: Event) => setInit((e as CustomEvent<NewSessionInit>).detail ?? {});
    window.addEventListener(EVENT, on);
    return () => window.removeEventListener(EVENT, on);
  }, []);
  if (!init) return null;
  return <NewSessionDialog init={init} onClose={() => setInit(null)} />;
}

const OTHER = "\u0000other";

function NewSessionDialog({ init, onClose }: { init: NewSessionInit; onClose: () => void }) {
  const nav = useNavigate();
  const { refreshAgents } = useAppData();
  const trust = useTrustedStart();
  const [nodes, setNodes] = useState<MeshRepoNode[] | null>(null);
  const [templates, setTemplates] = useState<Template[]>([]);
  const [node, setNode] = useState(init.node ?? "");
  const [repo, setRepo] = useState(init.repo ?? "");
  const [folder, setFolder] = useState("");
  const [browse, setBrowse] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const load = () =>
    api
      .meshRepos()
      .then((r) => setNodes(r.nodes.filter((n) => n.self || n.reachable)))
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  useEffect(() => {
    void load();
    api.templates().then(setTemplates).catch(() => setTemplates([]));
  }, []);
  // A template names where it belongs: its node, and its repo there.
  const tpl = templates.find((t) => !!init.template && (t.id === init.template || t.name === init.template));
  useEffect(() => {
    if (!nodes) return;
    const self = nodes.find((n) => n.self)?.node ?? nodes[0]?.node ?? "";
    setNode((cur) => cur || tpl?.spec.node || self);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodes, tpl?.id]);
  const here = nodes?.find((n) => n.node === node);
  useEffect(() => {
    if (!here || repo) return;
    const fit = tpl ? here.repos.find((r) => templateFits(tpl, here.node, r) && !!tpl.spec.repo) : undefined;
    if (fit) setRepo(fit.path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [here?.node, tpl?.id]);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && !browse && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose, browse]);
  const chosen = useMemo(() => here?.repos.find((r) => r.path === repo), [here, repo]);
  const addFolder = async () => {
    const p = folder.trim();
    if (!p || !here) return;
    setBusy("adding the folder…");
    setErr(null);
    try {
      const r = await api.addRepo(p, undefined, here.self ? undefined : here.node, { create: true });
      await load();
      setRepo(r.path ?? p);
      setFolder("");
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="trust-backdrop" onClick={onClose} role="presentation">
      <div className="trust-panel new-session" role="dialog" aria-label="new session" onClick={(e) => e.stopPropagation()}>
        <div className="trust-head">
          <span className="label">New session</span>
          {tpl && <span className="mono-meta" style={{ marginLeft: 8 }}>from {tpl.name}</span>}
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onClose}>esc</button>
        </div>
        <div className="start-row new-session-where">
          <label className="start-inline">
            <span className="label">node</span>
            <select
              value={node}
              onChange={(e) => {
                setNode(e.target.value);
                setRepo("");
              }}
              aria-label="node"
            >
              {(nodes ?? []).map((n) => (
                <option key={n.node} value={n.node}>
                  {n.node}
                  {n.self ? " · this node" : ""}
                </option>
              ))}
            </select>
          </label>
          <label className="start-inline new-session-repo">
            <span className="label">in</span>
            <select value={repo} onChange={(e) => setRepo(e.target.value)} aria-label="repository">
              <option value="">choose a repo…</option>
              {(here?.repos ?? []).map((r) => (
                <option key={r.path} value={r.path}>
                  #{r.handle ?? r.path.split(/[\\/]/).pop()} · {r.path}
                </option>
              ))}
              <option value={OTHER}>another folder…</option>
            </select>
          </label>
        </div>
        {repo === OTHER && here && (
          <form
            className="start-row"
            onSubmit={(e) => {
              e.preventDefault();
              void addFolder();
            }}
          >
            <input className="mono" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder={`a folder on ${here.node} (made if it is not there)`} autoFocus spellCheck={false} />
            <button type="button" className="btn ghost sm" onClick={() => setBrowse(true)}>browse…</button>
            <button type="submit" className="btn sm" disabled={!folder.trim() || !!busy}>use it</button>
          </form>
        )}
        {busy && <div className="mono-meta">{busy}</div>}
        {err && <div className="error-text mono-meta">{err}</div>}
        {!nodes && <div className="dim">loading…</div>}
        {here && chosen && (
          <StartDialog
            key={`${here.node}\u0000${chosen.path}`}
            node={here.node}
            self={here.self}
            repo={{ path: chosen.path, handle: chosen.handle }}
            initialTemplate={init.template}
            initialName={init.name}
            onCancel={onClose}
            onStart={(c) => {
              setBusy(`starting @${c.name}…`);
              setErr(null);
              trust
                .start({ repo: chosen.path, node: here.self ? undefined : here.node, ...startFields(c) })
                .then(async (a) => {
                  setBusy(null);
                  if (!a) return;
                  await refreshAgents();
                  onClose();
                  nav(`/session/${encodeURIComponent(a.name)}`);
                })
                .catch((e) => {
                  setBusy(null);
                  setErr(e instanceof Error ? e.message : String(e));
                });
            }}
          />
        )}
        {trust.dialog}
        {browse && here && (
          <PathPicker
            node={here.self ? undefined : here.node}
            mode="dir"
            start={folder}
            title="A folder for the new session"
            onPick={(p) => {
              setFolder(p);
              setBrowse(false);
            }}
            onClose={() => setBrowse(false)}
          />
        )}
      </div>
    </div>
  );
}
