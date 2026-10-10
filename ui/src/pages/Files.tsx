/* Files: the agent's file space, browsable (PROPOSALS-2026-10-R.md R-2).
 * The same space the viewer serves (the repo, the session's data, plans,
 * temp since the session started, its attachments, and files its tool
 * calls named elsewhere), one directory at a time, plus the newest files
 * in the repo however they were written. A file opens in the viewer;
 * each row can be downloaded, copied, shared, or dragged into a composer. */
import { useEffect, useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { api, type FileEntry, type FileListing, type RecentFile } from "../api";
import { useAppData } from "../App";
import { ErrorBar, relTime } from "../components";
import { canShareFiles, copyFile, onFileDragStart, qualifiedAgent, shareFile, type AspenFileRef } from "../fileActions";
import { viewHref } from "../pathLinks";
import { fmtBytes } from "./sessionExtras";
import "./files.css";

type Tab = "browse" | "recent" | "named";
type SortKey = "name" | "mtime" | "size";

/** The roots shown as chips, most used first; the rest follow. */
const ROOT_ORDER = ["repo", "attachments", "plans", "temp", "session data"];

function joinPath(dir: string, name: string): string {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
}

export default function Files() {
  const { name = "" } = useParams<{ name: string }>();
  const [params, setParams] = useSearchParams();
  const { agents } = useAppData();
  const agent = agents.find((a) => a.name === name);
  const node = agent?.node ?? null;
  const tab: Tab = (["browse", "recent", "named"].includes(params.get("tab") ?? "") ? params.get("tab") : "browse") as Tab;
  const dir = params.get("dir") ?? "";
  const showHidden = params.get("hidden") === "1";
  const [roots, setRoots] = useState<FileListing | null>(null);
  const [listing, setListing] = useState<FileListing | null>(null);
  const [recent, setRecent] = useState<RecentFile[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: "name", desc: false });
  const shareable = useMemo(canShareFiles, []);

  const set = (patch: Record<string, string | null>) => {
    const next = new URLSearchParams(params);
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === "") next.delete(k);
      else next.set(k, v);
    }
    setParams(next, { replace: false });
  };

  // Roots once; the directory on every change (the repo by default).
  useEffect(() => {
    api.files(name).then(setRoots).catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  }, [name]);
  const repoRoot = roots?.roots?.find((r) => r.label === "repo")?.path ?? "";
  const shownDir = dir || repoRoot;
  useEffect(() => {
    if (tab !== "browse" || !shownDir) return;
    setListing(null);
    setErr(null);
    api
      .files(name, shownDir)
      .then(setListing)
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  }, [name, shownDir, tab]);
  useEffect(() => {
    if (tab !== "recent") return;
    setRecent(null);
    api
      .filesRecent(name, 80)
      .then((r) => setRecent(r.files))
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  }, [name, tab]);

  const ref = (path: string, fname: string, media?: string): AspenFileRef => ({
    agent: qualifiedAgent(name, node),
    path,
    name: fname,
    media: media ?? guessMedia(fname),
  });
  async function doCopy(r: AspenFileRef, size: number | null) {
    setNote(null);
    try {
      const what = await copyFile(r, size);
      setNote(`copied ${r.name} (${what}; paste into an Aspen composer to attach the file)`);
    } catch (e) {
      setErr(`copy: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  async function doShare(r: AspenFileRef) {
    try {
      await shareFile(r);
    } catch (e) {
      if (e instanceof Error && e.name === "AbortError") return;
      setErr(`share: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  async function doCopyPath(p: string) {
    try {
      await navigator.clipboard.writeText(p);
      setNote(`copied the path`);
    } catch {
      setErr("the clipboard is not available here");
    }
  }

  const rootChips = useMemo(() => {
    const rs = (roots?.roots ?? []).filter((r) => r.exists);
    return rs.sort((a, b) => {
      const ia = ROOT_ORDER.indexOf(a.label);
      const ib = ROOT_ORDER.indexOf(b.label);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
    });
  }, [roots]);

  const entries = useMemo(() => {
    const list = (listing?.entries ?? []).filter((e) => (showHidden || !e.hidden) && (!q || e.name.toLowerCase().includes(q.toLowerCase())));
    const dirFirst = (a: FileEntry, b: FileEntry) => Number(b.is_dir) - Number(a.is_dir);
    const by = (a: FileEntry, b: FileEntry) => {
      const k = sort.key;
      const v = k === "name" ? a.name.toLowerCase().localeCompare(b.name.toLowerCase()) : ((a[k] ?? 0) as number) - ((b[k] ?? 0) as number);
      return sort.desc ? -v : v;
    };
    return list.slice().sort((a, b) => dirFirst(a, b) || by(a, b));
  }, [listing, showHidden, q, sort]);
  const hiddenCount = (listing?.entries ?? []).filter((e) => e.hidden).length;

  // Breadcrumbs: the root, then each folder under it.
  const crumbs = useMemo(() => {
    if (!listing?.dir || !listing.root) return [];
    const root = listing.root;
    const rel = listing.dir.slice(root.length).replace(/^[\\/]+/, "");
    const out = [{ label: listing.root_label ?? "root", path: root }];
    let acc = root;
    for (const seg of rel.split(/[\\/]/).filter(Boolean)) {
      acc = joinPath(acc, seg);
      out.push({ label: seg, path: acc });
    }
    return out;
  }, [listing]);

  const headerCell = (key: SortKey, label: string) => (
    <button className={`fl-sort${sort.key === key ? " on" : ""}`} onClick={() => setSort((s) => ({ key, desc: s.key === key ? !s.desc : key !== "name" }))}>
      {label}
      {sort.key === key ? (sort.desc ? " ↓" : " ↑") : ""}
    </button>
  );

  const actions = (r: AspenFileRef, size: number | null) => (
    <span className="fl-actions">
      <a className="btn ghost sm" href={api.filesViaTunnel() ? undefined : api.fileUrl(name, r.path, { download: true })} onClick={api.filesViaTunnel() ? (e) => { e.preventDefault(); void downloadViaBlob(name, r); } : undefined} title="download">
        download
      </a>
      <button className="btn ghost sm" onClick={() => void doCopy(r, size)} title="copy: the image, or the text, or the path; and the file itself for an Aspen composer">
        copy
      </button>
      {shareable && (
        <button className="btn ghost sm" onClick={() => void doShare(r)} title="send the file to another app">
          share
        </button>
      )}
      <button className="btn ghost sm" onClick={() => void doCopyPath(r.path)} title="copy the path">
        path
      </button>
    </span>
  );

  return (
    <div className="page files-page">
      <div className="view-head">
        <Link className="mono-meta" to={`/session/${encodeURIComponent(name)}`}>
          ← @{name.split("@")[0]}
        </Link>
        <span className="mono">files</span>
        {node && <span className="mono-meta">on {node}</span>}
        <span style={{ flex: 1 }} />
        <span className="seg" role="tablist">
          {(["browse", "recent", "named"] as Tab[]).map((t) => (
            <button key={t} className={tab === t ? "on" : ""} onClick={() => set({ tab: t === "browse" ? null : t })}>
              {t === "named" ? "named elsewhere" : t}
            </button>
          ))}
        </span>
      </div>
      <ErrorBar error={err} />
      {note && <div className="mono-meta fl-note">{note}</div>}

      {tab === "browse" && (
        <>
          <div className="fl-roots">
            {rootChips.map((r) => (
              <button key={r.path} className={`chip mono fl-root${listing?.root === r.path ? " on" : ""}`} onClick={() => set({ dir: r.label === "repo" ? null : r.path })} title={r.path}>
                {r.label}
              </button>
            ))}
            {listing?.root_label === "temp" && roots?.since && (
              <span className="mono-meta dim">temp shows what changed since this session started</span>
            )}
          </div>
          <div className="fl-bar">
            <span className="fl-crumbs mono">
              {crumbs.map((c, i) => (
                <span key={c.path}>
                  {i > 0 && <span className="dim"> / </span>}
                  {i === crumbs.length - 1 ? <b>{c.label}</b> : <button className="linklike" onClick={() => set({ dir: c.path })}>{c.label}</button>}
                </span>
              ))}
            </span>
            <span style={{ flex: 1 }} />
            <input className="fl-filter" type="search" placeholder="filter" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
            {hiddenCount > 0 && (
              <label className="mono-meta fl-check">
                <input type="checkbox" checked={showHidden} onChange={(e) => set({ hidden: e.target.checked ? "1" : null })} /> hidden ({hiddenCount})
              </label>
            )}
          </div>
          {!listing && !err && <div className="dim">loading…</div>}
          {listing && (
            <div className="fl-list" role="table">
              <div className="fl-row fl-headrow" role="row">
                {headerCell("name", "name")}
                {headerCell("size", "size")}
                {headerCell("mtime", "modified")}
                <span />
              </div>
              {listing.parent && (
                <div className="fl-row" role="row">
                  <button className="linklike fl-name" onClick={() => set({ dir: listing.parent ?? null })}>
                    ..
                  </button>
                  <span />
                  <span />
                  <span />
                </div>
              )}
              {entries.map((e) => {
                const full = joinPath(listing.dir ?? "", e.name);
                const r = ref(full, e.name);
                return (
                  <div key={e.name} className={`fl-row${e.hidden ? " hidden" : ""}${e.served ? "" : " unserved"}`} role="row" draggable={!e.is_dir && e.served} onDragStart={(ev) => onFileDragStart(ev, r)}>
                    {e.is_dir ? (
                      e.served ? (
                        <button className="linklike fl-name" onClick={() => set({ dir: full })}>
                          {e.name}/
                        </button>
                      ) : (
                        <span className="fl-name dim" title="a link out of the agent's file space">{e.name}/ ↗</span>
                      )
                    ) : e.served ? (
                      <Link className="fl-name" to={viewHref(name, full)}>
                        {e.name}
                      </Link>
                    ) : (
                      <span className="fl-name dim" title="a link out of the agent's file space">{e.name} ↗</span>
                    )}
                    <span className="mono-meta fl-size">{e.is_dir ? "" : fmtBytes(e.size ?? 0)}</span>
                    <span className="mono-meta fl-time" title={e.mtime ? new Date(e.mtime * 1000).toLocaleString() : ""}>{e.mtime ? `${relTime(e.mtime)} ago` : ""}</span>
                    {!e.is_dir && e.served ? actions(r, e.size) : <span />}
                  </div>
                );
              })}
              {entries.length === 0 && <div className="dim fl-empty">{q ? "nothing here matches" : "empty"}</div>}
              {listing.truncated && <div className="mono-meta dim fl-empty">showing the first 2,000 entries; filter to narrow</div>}
            </div>
          )}
        </>
      )}

      {tab === "recent" && (
        <div className="fl-list" role="table">
          {!recent && !err && <div className="dim">loading…</div>}
          {(recent ?? []).map((f) => {
            const fname = f.name ?? f.path.split(/[\\/]/).pop() ?? f.path;
            const r = ref(f.path, fname);
            const rel = repoRoot && f.path.startsWith(repoRoot) ? f.path.slice(repoRoot.length).replace(/^[\\/]+/, "") : f.path;
            return (
              <div key={f.path} className="fl-row" role="row" draggable onDragStart={(ev) => onFileDragStart(ev, r)}>
                <Link className="fl-name" to={viewHref(name, f.path)} title={f.path}>
                  {rel}
                </Link>
                <span className="mono-meta fl-size">{fmtBytes(f.size)}</span>
                <span className="mono-meta fl-time">{relTime(f.mtime)} ago</span>
                {actions(r, f.size)}
              </div>
            );
          })}
          {recent && recent.length === 0 && <div className="dim fl-empty">no files</div>}
        </div>
      )}

      {tab === "named" && (
        <div className="fl-list" role="table">
          <p className="dim" style={{ margin: "4px 0 8px" }}>Files this session's tool calls named outside its repo and the other roots.</p>
          {(roots?.named ?? []).map((f) => {
            const fname = f.path.split(/[\\/]/).pop() ?? f.path;
            const r = ref(f.path, fname);
            return (
              <div key={f.path} className="fl-row" role="row" draggable onDragStart={(ev) => onFileDragStart(ev, r)}>
                <Link className="fl-name" to={viewHref(name, f.path)} title={f.path}>
                  {f.path}
                </Link>
                <span className="mono-meta fl-size">{fmtBytes(f.stat.size ?? 0)}</span>
                <span className="mono-meta fl-time">{f.stat.mtime ? `${relTime(f.stat.mtime)} ago` : ""}</span>
                {actions(r, f.stat.size ?? null)}
              </div>
            );
          })}
          {roots && (roots.named ?? []).length === 0 && <div className="dim fl-empty">none</div>}
        </div>
      )}
    </div>
  );
}

/** Over the relay there is no URL for the file: fetch it and save it. */
async function downloadViaBlob(name: string, r: AspenFileRef) {
  const blob = await api.fileBlob(name, r.path);
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = r.name;
  a.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** A media type from the name, for the clipboard and share (the node's
 *  own answer comes with the bytes). */
function guessMedia(name: string): string {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  const m: Record<string, string> = {
    png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml",
    md: "text/markdown", txt: "text/plain", csv: "text/csv", json: "application/json", html: "text/html", pdf: "application/pdf",
    ts: "text/plain", tsx: "text/plain", js: "text/javascript", rs: "text/plain", py: "text/plain", toml: "text/plain", yml: "text/plain", yaml: "text/plain", log: "text/plain", sh: "text/plain",
  };
  return m[ext] ?? "application/octet-stream";
}
