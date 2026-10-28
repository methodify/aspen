// A path on a node (PROPOSALS-2026-10-U §U-1): browse its directories and
// pick a file or a folder. Names only — nothing here opens a file.
import { useEffect, useState } from "react";
import { api, type FsList } from "./api";
import { relTime } from "./components";
import { fmtBytes } from "./pages/sessionExtras";
import "./pages/board.css";

export function PathPicker({
  node,
  mode,
  ext,
  start,
  title,
  onPick,
  onClose,
}: {
  /** The node, as the mesh names it; undefined for the console's own. */
  node?: string;
  /** A file (a click picks it) or a folder (the one shown is picked). */
  mode: "file" | "dir";
  /** Files with this ending come first; others are dimmed (file mode). */
  ext?: string;
  /** Where to open (a path the operator typed), else the starting points. */
  start?: string;
  title?: string;
  onPick: (path: string) => void;
  onClose: () => void;
}) {
  const [list, setList] = useState<FsList | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [hidden, setHidden] = useState(false);
  const open = (dir?: string | null) => {
    setErr(null);
    setQ("");
    api
      .fsList(node, dir ?? null)
      .then(setList)
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  };
  useEffect(() => {
    open(start?.trim() || null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [node]);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  const sep = list?.sep ?? "/";
  const join = (dir: string, name: string) => (dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`);
  const matches = (name: string) => !q || name.toLowerCase().includes(q.toLowerCase());
  const wanted = (name: string) => !ext || name.endsWith(ext);
  const entries = (list?.entries ?? [])
    .filter((e) => (hidden || !e.hidden) && matches(e.name))
    .sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || Number(wanted(b.name)) - Number(wanted(a.name)));
  return (
    <div className="trust-backdrop" onClick={onClose} role="presentation">
      <div className="trust-panel picker path-picker" role="dialog" aria-label={title ?? "choose a path"} onClick={(e) => e.stopPropagation()}>
        <div className="trust-head">
          <span className="label">{title ?? (mode === "dir" ? "Choose a folder" : "Choose a file")}</span>
          <span className="mono-meta">{node ? `on ${node}` : "on this node"} · names only, nothing is opened</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onClose}>esc</button>
        </div>
        <div className="path-starts">
          {(list?.starts ?? []).map((s) => (
            <button key={s.path} type="button" className="btn ghost sm mono" title={s.path} onClick={() => open(s.path)}>
              {s.label}
            </button>
          ))}
        </div>
        {list?.dir && (
          <div className="path-here">
            {list.parent && list.parent !== list.dir && (
              <button type="button" className="btn ghost sm" onClick={() => open(list.parent)} title="up one folder">↑</button>
            )}
            <span className="mono path-dir" title={list.dir}>{list.dir}</span>
            <span style={{ flex: 1 }} />
            {mode === "dir" && (
              <button type="button" className="btn primary sm" onClick={() => onPick(list.dir!)}>use this folder</button>
            )}
          </div>
        )}
        {err && <div className="error-text mono-meta" style={{ padding: "4px 12px" }}>{err}</div>}
        {list?.dir && (
          <div className="path-tools">
            <input className="picker-search" placeholder="filter" value={q} onChange={(e) => setQ(e.target.value)} autoFocus />
            <label className="mono-meta"><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> dotfiles</label>
          </div>
        )}
        <div className="picker-list">
          {!list && !err && <div className="dim" style={{ padding: 8 }}>loading…</div>}
          {list && !list.dir && <div className="dim" style={{ padding: 8 }}>start from one of the places above</div>}
          {list?.dir &&
            entries.map((e) => {
              const path = join(list.dir!, e.name);
              const pickable = e.is_dir || mode === "file";
              return (
                <button
                  key={e.name}
                  type="button"
                  className={`picker-row${!e.is_dir && mode === "dir" ? " dim" : ""}${!e.is_dir && ext && !wanted(e.name) ? " dim" : ""}`}
                  disabled={!pickable}
                  onClick={() => (e.is_dir ? open(path) : onPick(path))}
                  title={e.is_dir ? "open" : mode === "file" ? "choose this file" : undefined}
                >
                  <span className="mono">{e.is_dir ? "▸" : " "}</span>
                  <span className="mono">{e.name}{e.is_dir ? sep : ""}</span>
                  <span style={{ flex: 1 }} />
                  {e.size != null && <span className="mono-meta">{fmtBytes(e.size)}</span>}
                  {e.mtime != null && <span className="mono-meta">{relTime(e.mtime)}</span>}
                </button>
              );
            })}
          {list?.dir && entries.length === 0 && <div className="dim" style={{ padding: 8 }}>{q ? "nothing matches" : "empty"}</div>}
          {list?.truncated && <div className="mono-meta" style={{ padding: 8 }}>the first 2,000 entries; filter to narrow</div>}
        </div>
      </div>
    </div>
  );
}
