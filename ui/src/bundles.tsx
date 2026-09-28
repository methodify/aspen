/* Repo bundles (docs/BUNDLES.md; PROPOSALS-2026-09-O §1): export a repo
 * with its context to one file, import one as a new repo or on top of an
 * existing one. The file is built on (and read from) the repo's node; a
 * console with a direct connection can download and upload it, one on the
 * relay works with paths on the node (the relay cannot carry a file this
 * size — chunked transfer is on the backlog). */
import { useEffect, useMemo, useState } from "react";
import {
  api,
  bundleDownloadUrl,
  bundleUpload,
  type BundleExportPreflight,
  type BundleExportResult,
  type BundleImportPlan,
  type BundleImportReport,
  type BundleRepoMode,
} from "./api";
import { tunnel } from "./tunnel";
import { ErrorBar } from "./components";

export function fmtBytes(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1 << 10) return `${Math.round(n / (1 << 10))} KB`;
  return `${n} B`;
}

/** Can this console move a file straight to or from the node? Not over the
 *  relay, and not for a peer's node (the file lives there). */
function directTo(node?: string): boolean {
  return !tunnel.enabled && !node;
}

const MODES: { key: BundleRepoMode; label: string; hint: string }[] = [
  { key: "tracked", label: "tracked + history", hint: "the files git tracks, and .git (history, branches)" },
  { key: "untracked", label: "+ untracked", hint: "also new files not yet committed, honoring .gitignore" },
  { key: "all", label: "everything", hint: "the whole directory: dotfiles, ignored files, build output, .env files" },
  { key: "none", label: "no files", hint: "context only — for a repo the other side gets through git" },
];

export function ExportPanel({ path, node, onClose }: { path: string; node?: string; onClose: () => void }) {
  const [pf, setPf] = useState<BundleExportPreflight | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [mode, setMode] = useState<BundleRepoMode>("tracked");
  const [picked, setPicked] = useState<Set<string> | null>(null);
  const [sidecars, setSidecars] = useState(true);
  const [memory, setMemory] = useState(true);
  const [names, setNames] = useState(true);
  const [seal, setSeal] = useState(false);
  const [pass, setPass] = useState("");
  const [pass2, setPass2] = useState("");
  const [out, setOut] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<BundleExportResult | null>(null);
  useEffect(() => {
    api
      .bundleExportPreflight(path, node)
      .then((v) => {
        setPf(v);
        setPicked(new Set(v.sessions.map((s) => s.id)));
      })
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  }, [path, node]);
  const size = useMemo(() => {
    if (!pf) return 0;
    const repo = mode === "none" ? 0 : pf.modes[mode].bytes;
    const ctx = pf.sessions.filter((s) => picked?.has(s.id)).reduce((n, s) => n + s.bytes + (sidecars ? s.sidecar_bytes : 0), 0);
    return repo + ctx + (memory ? pf.memory_bytes : 0);
  }, [pf, mode, picked, sidecars, memory]);
  const passOk = !seal || (pass.length > 0 && pass === pass2);
  async function go() {
    if (!pf || !passOk) return;
    setBusy(true);
    setErr(null);
    try {
      const all = picked?.size === pf.sessions.length;
      const r = await api.bundleExport({
        path,
        node,
        out: out.trim() || undefined,
        repo_mode: mode,
        sessions: all ? null : [...(picked ?? [])],
        sidecars,
        memory,
        names,
        passphrase: seal ? pass : undefined,
      });
      setDone(r);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="bundle-panel">
      <div className="bundle-head">
        <span className="label">export with context</span>
        <span className="mono-meta">one file: the repo, every session with its paths made portable, memory, names</span>
        <span style={{ flex: 1 }} />
        <button type="button" className="btn ghost sm" onClick={onClose}>close</button>
      </div>
      <ErrorBar error={err} />
      {!pf && !err && <div className="dim">measuring…</div>}
      {pf && !done && (
        <>
          <div className="bundle-row">
            <span className="bundle-k">files</span>
            <div className="bundle-choices">
              {MODES.map((m) => (
                <label key={m.key} className={`bundle-choice${mode === m.key ? " on" : ""}`} title={m.hint}>
                  <input type="radio" name={`mode-${path}`} checked={mode === m.key} onChange={() => setMode(m.key)} />
                  <span>{m.label}</span>
                  <span className="mono-meta">{m.key === "none" ? "—" : `${pf.modes[m.key].files} · ${fmtBytes(pf.modes[m.key].bytes)}`}</span>
                </label>
              ))}
            </div>
          </div>
          {mode === "all" && (
            <div className="bundle-warn micro">
              everything includes ignored files — secrets in <code>.env</code>-style files come along.
              {pf.largest_dirs.length > 0 && (
                <> Largest: {pf.largest_dirs.slice(0, 4).map((d) => `${d.name} ${fmtBytes(d.bytes)}`).join(" · ")}.</>
              )}
            </div>
          )}
          <div className="bundle-row">
            <span className="bundle-k">sessions</span>
            <div className="bundle-sessions">
              {pf.sessions.length === 0 && <span className="mono-meta">none on disk for this repo</span>}
              {pf.sessions.map((s) => (
                <label key={s.id} className="bundle-session">
                  <input
                    type="checkbox"
                    checked={picked?.has(s.id) ?? false}
                    onChange={(e) =>
                      setPicked((cur) => {
                        const n = new Set(cur ?? []);
                        if (e.target.checked) n.add(s.id);
                        else n.delete(s.id);
                        return n;
                      })
                    }
                  />
                  <span className="mono">{s.name ? `@${s.name.split("@")[0]}` : "no name"}</span>
                  <span className="bundle-title">{s.title ?? ""}</span>
                  <span className="mono-meta">{s.harness !== "claude" ? `${s.harness} · ` : ""}{s.id.slice(0, 8)} · {fmtBytes(s.bytes + (sidecars ? s.sidecar_bytes : 0))}</span>
                </label>
              ))}
            </div>
          </div>
          <div className="bundle-row">
            <span className="bundle-k">with</span>
            <label className="bundle-flag" title="tool results, subagent and workflow transcripts — the bulk of the context">
              <input type="checkbox" checked={sidecars} onChange={(e) => setSidecars(e.target.checked)} /> sidecar folders
            </label>
            <label className="bundle-flag"><input type="checkbox" checked={memory} onChange={(e) => setMemory(e.target.checked)} /> memory ({fmtBytes(pf.memory_bytes)})</label>
            <label className="bundle-flag" title="the repo's Aspen names, charters, titles, bookmarks and lineage"><input type="checkbox" checked={names} onChange={(e) => setNames(e.target.checked)} /> names ({pf.names})</label>
          </div>
          <div className="bundle-row">
            <span className="bundle-k">seal</span>
            <label className="bundle-flag" title="encrypt the file with a passphrase (scrypt + XChaCha20-Poly1305); import asks for it">
              <input type="checkbox" checked={seal} onChange={(e) => setSeal(e.target.checked)} /> with a passphrase
            </label>
            {seal && (
              <>
                <input type="password" autoComplete="new-password" value={pass} onChange={(e) => setPass(e.target.value)} placeholder="passphrase" style={{ width: 170 }} />
                <input type="password" autoComplete="new-password" value={pass2} onChange={(e) => setPass2(e.target.value)} placeholder="again" style={{ width: 170 }} />
                {pass2 && pass !== pass2 && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>they differ</span>}
              </>
            )}
          </div>
          <div className="bundle-row">
            <span className="bundle-k">write to</span>
            <input className="mono" value={out} onChange={(e) => setOut(e.target.value)} placeholder={`${node ? `${node}: ` : ""}the node's exports folder (or a path / folder on the node)`} style={{ flex: 1, minWidth: 220 }} spellCheck={false} />
          </div>
          <div className="bundle-row">
            <span className="mono-meta">about {fmtBytes(size)} before compression</span>
            <span style={{ flex: 1 }} />
            <button type="button" className="btn primary sm" disabled={busy || !passOk || (mode === "none" && (picked?.size ?? 0) === 0)} onClick={() => void go()}>
              {busy ? "writing…" : "export"}
            </button>
          </div>
        </>
      )}
      {done && (
        <div className="bundle-done">
          <div className="mono">{done.out}</div>
          <div className="mono-meta">
            {fmtBytes(done.bytes)} · {done.files} files · {done.sessions} session{done.sessions === 1 ? "" : "s"}{done.sealed ? " · sealed" : ""}
          </div>
          {done.notes.map((n, i) => (
            <div key={i} className="micro dim">{n}</div>
          ))}
          <div className="bundle-row">
            {done.file && directTo(node) ? (
              <a className="btn primary sm" href={bundleDownloadUrl(done.file)} download={done.file}>download</a>
            ) : (
              <span className="micro dim">
                {tunnel.enabled ? "this console is on the relay, which cannot carry a file this size: " : node ? `the file is on ${node}: ` : ""}
                take it from that path on the node (scp, a sync folder, a drive), or <code>aspen repos import &lt;file&gt; --to &lt;path&gt;</code> where it lands.
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export function ImportPanel({ nodes, onDone, onClose }: { nodes: { node: string; me: boolean }[]; onDone: () => void; onClose: () => void }) {
  const [onNode, setOnNode] = useState(nodes.find((n) => n.me)?.node ?? "");
  const me = nodes.find((n) => n.node === onNode)?.me ?? true;
  const direct = directTo(me ? undefined : onNode);
  const [file, setFile] = useState("");
  const [upload, setUpload] = useState<File | null>(null);
  const [sent, setSent] = useState<number | null>(null);
  const [pass, setPass] = useState("");
  const [target, setTarget] = useState("");
  const [mode, setMode] = useState<"new" | "top_up">("new");
  const [missing, setMissing] = useState(false);
  const [names, setNames] = useState(true);
  const [busy, setBusy] = useState<null | string>(null);
  const [err, setErr] = useState<string | null>(null);
  const [plan, setPlan] = useState<BundleImportPlan | null>(null);
  const [report, setReport] = useState<BundleImportReport | null>(null);
  const req = (f: string) => ({
    file: f,
    node: me ? undefined : onNode,
    passphrase: pass || undefined,
    target: target.trim(),
    mode,
    repo_files: missing ? ("missing" as const) : ("none" as const),
    names,
  });
  async function preview() {
    setErr(null);
    setPlan(null);
    try {
      let f = file.trim();
      if (upload && direct) {
        setBusy("uploading…");
        setSent(0);
        f = (await bundleUpload(upload, (n) => setSent(n))).file;
        setFile(f);
        setUpload(null);
      }
      if (!f) throw new Error("choose a bundle file");
      if (!target.trim()) throw new Error("say where the repo goes on the node");
      setBusy("reading the bundle…");
      setPlan(await api.bundleImportPreflight(req(f)));
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
      setSent(null);
    }
  }
  async function go() {
    if (!plan) return;
    setBusy("importing…");
    setErr(null);
    try {
      setReport(await api.bundleImport({ ...req(file.trim()), staging_id: plan.staging_id }));
      onDone();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }
  return (
    <div className="bundle-panel strip">
      <div className="bundle-head">
        <span className="label">import a repo with its context</span>
        <span className="mono-meta">an .aspen-repo file: a new repo, or on top of one that is here</span>
        <span style={{ flex: 1 }} />
        <button type="button" className="btn ghost sm" onClick={onClose}>close</button>
      </div>
      <ErrorBar error={err} />
      {!report && (
        <>
          <div className="bundle-row">
            <span className="bundle-k">node</span>
            <select value={onNode} onChange={(e) => { setOnNode(e.target.value); setPlan(null); }} style={{ width: "auto" }}>
              {nodes.map((n) => (
                <option key={n.node} value={n.node}>{n.node}{n.me ? " · this node" : ""}</option>
              ))}
            </select>
          </div>
          <div className="bundle-row">
            <span className="bundle-k">bundle</span>
            {direct && (
              <input type="file" accept=".aspen-repo" onChange={(e) => { setUpload(e.target.files?.[0] ?? null); setPlan(null); }} />
            )}
            <input className="mono" value={file} onChange={(e) => { setFile(e.target.value); setUpload(null); setPlan(null); }} placeholder={direct ? "…or a path on the node" : "a path on the node (the relay cannot carry the file)"} style={{ flex: 1, minWidth: 220 }} spellCheck={false} />
          </div>
          <div className="bundle-row">
            <span className="bundle-k">passphrase</span>
            <input type="password" autoComplete="off" value={pass} onChange={(e) => setPass(e.target.value)} placeholder="if it is sealed" style={{ width: 200 }} />
          </div>
          <div className="bundle-row">
            <span className="bundle-k">into</span>
            <label className={`bundle-choice${mode === "new" ? " on" : ""}`}><input type="radio" checked={mode === "new"} onChange={() => { setMode("new"); setPlan(null); }} /> a new repo</label>
            <label className={`bundle-choice${mode === "top_up" ? " on" : ""}`} title="merge into a repo that is here: new sessions installed, longer copies replace shorter ones, diverged ones land as forks"><input type="radio" checked={mode === "top_up"} onChange={() => { setMode("top_up"); setPlan(null); }} /> top up a repo that is here</label>
            <input className="mono" value={target} onChange={(e) => { setTarget(e.target.value); setPlan(null); }} placeholder={mode === "new" ? "~/src/project (absent or empty)" : "~/src/project"} style={{ flex: 1, minWidth: 200 }} spellCheck={false} />
          </div>
          <div className="bundle-row">
            <span className="bundle-k">with</span>
            {mode === "top_up" && (
              <label className="bundle-flag" title="off: git carries the code and the repo's files are left alone"><input type="checkbox" checked={missing} onChange={(e) => { setMissing(e.target.checked); setPlan(null); }} /> write files this repo lacks</label>
            )}
            <label className="bundle-flag"><input type="checkbox" checked={names} onChange={(e) => { setNames(e.target.checked); setPlan(null); }} /> register the names (not started)</label>
            <span style={{ flex: 1 }} />
            <button type="button" className="btn sm" disabled={!!busy} onClick={() => void preview()}>
              {busy && !plan ? (sent != null ? `uploading ${fmtBytes(sent)}…` : busy) : "preview"}
            </button>
          </div>
          {plan && <PlanView plan={plan} />}
          {plan && (
            <div className="bundle-row">
              <span style={{ flex: 1 }} />
              <button type="button" className="btn primary sm" disabled={!!busy || plan.blockers.length > 0} onClick={() => void go()}>
                {busy ? busy : "import"}
              </button>
            </div>
          )}
        </>
      )}
      {report && (
        <div className="bundle-done">
          <div className="mono">{report.repo} · #{report.handle}</div>
          <div className="mono-meta">
            repo files {report.repo_files_written} · sessions installed {report.sessions_installed.length} · replaced {report.sessions_replaced.length} · forked {report.sessions_forked.length} · kept {report.sessions_kept.length} · memory {report.memory_written} · names {report.names.length}
          </div>
          {report.names.length > 0 && <div className="micro">names registered, not started: {report.names.map((n) => `@${n.split("@")[0]}`).join(", ")} — resume them from the repo's row.</div>}
          {report.memory_conflicts.map((c) => <div key={c} className="micro dim">memory kept beside: {c}</div>)}
          {report.residue > 0 && <div className="micro dim">{report.residue} path(s) from the source machine could not be rewritten (advisory)</div>}
          {report.notes.map((n, i) => <div key={i} className="micro dim">{n}</div>)}
        </div>
      )}
    </div>
  );
}

function PlanView({ plan }: { plan: BundleImportPlan }) {
  const m = plan.manifest;
  return (
    <div className="bundle-plan">
      <div className="mono-meta">
        from {m.source_node} · {m.repo.basename}{m.repo.branch ? ` (${m.repo.branch})` : ""} · {new Date(m.created_at * 1000).toLocaleString()} · aspen v{m.aspen_version}{m.sealed ? " · sealed" : ""}
      </div>
      <div className="mono-meta">
        {plan.mode === "new" ? `repo files: ${plan.repo.files}` : `repo files in the bundle: ${plan.repo.files} (${plan.repo.missing_here} missing here)`} · memory: {plan.memory.new} new{plan.memory.conflicts ? `, ${plan.memory.conflicts} conflicting (kept beside)` : ""}
      </div>
      {plan.sessions.map((s) => (
        <div key={s.id} className={`bundle-plan-row st-${s.status}`}>
          <span className="mono">{s.name ? `@${s.name.split("@")[0]}` : "no name"}</span>
          <span className="bundle-title">{s.title ?? ""}</span>
          <span className="mono-meta">{s.id.slice(0, 8)}</span>
          <span className="bundle-action">{s.action}</span>
        </div>
      ))}
      {plan.names.map((n) => (
        <div key={n.name} className="mono-meta">@{n.name} → @{n.as} ({n.action})</div>
      ))}
      {plan.warnings.map((w, i) => <div key={i} className="micro" style={{ color: "var(--sig-waiting)" }}>{w}</div>)}
      {plan.blockers.map((b, i) => <div key={i} className="micro" style={{ color: "var(--sig-gate)" }}>{b}</div>)}
    </div>
  );
}
