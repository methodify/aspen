// Serve a folder for an agent (PROPOSALS-2026-11-C §C-3): for this agent,
// every agent of its repo, or every member of one of its projects. Used by
// the viewer when a file is refused and by the Files page's "+ add a folder".
import { useEffect, useState } from "react";
import { api, type Agent, type Board } from "./api";

/** The agent's projects (on the desk or the bench). */
function projectsOf(boards: Board[], name: string): Board[] {
  return boards.filter((b) => !b.deleted && !b.query && b.status !== "archived" && JSON.stringify([b.layout, b.bench ?? []]).includes(`"${name}`));
}

export async function addProjectRoot(b: Board, node: string, path: string) {
  const roots = (b.roots ?? []).filter((r) => !(r.node === node && r.path === path));
  await api.patchBoard(b.id, { roots: [...roots, { node, path }] });
}

export async function removeProjectRoot(boardId: string, node: string, path: string) {
  const b = (await api.boards()).find((x) => x.id === boardId);
  if (!b) return;
  await api.patchBoard(b.id, { roots: (b.roots ?? []).filter((r) => !(r.node === node && r.path === path)) });
}

export function ServeFolder({ name, agent, folder, onDone }: { name: string; agent?: Agent; folder: string; onDone: () => void }) {
  const [projects, setProjects] = useState<Board[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api
      .boards()
      .then((bs) => setProjects(projectsOf(bs, name)))
      .catch(() => setProjects([]));
  }, [name]);
  const run = (f: () => Promise<unknown>) => {
    setBusy(true);
    setErr(null);
    f()
      .then(onDone)
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };
  const bare = name.split("@")[0];
  return (
    <div className="serve-folder">
      <span className="mono-meta">
        serve <span className="mono">{folder}</span> for
      </span>
      <button type="button" className="btn sm" disabled={busy} onClick={() => run(() => api.addRoot(name, folder, "agent"))} title="this agent only">
        @{bare}
      </button>
      {agent?.channel && (
        <button type="button" className="btn ghost sm" disabled={busy} onClick={() => run(() => api.addRoot(name, folder, "repo"))} title={`every agent of #${agent.channel} on ${agent.node}`}>
          #{agent.channel}
        </button>
      )}
      {agent &&
        projects.map((b) => (
          <button key={b.id} type="button" className="btn ghost sm" disabled={busy} onClick={() => run(() => addProjectRoot(b, agent.node, folder))} title={`every member of ${b.name}, on ${agent.node}`}>
            {b.name}
          </button>
        ))}
      {err && <span className="error-text mono-meta">{err}</span>}
    </div>
  );
}
