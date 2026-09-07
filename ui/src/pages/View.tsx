// The artifact viewer (PROPOSALS-2026-09 §3): a file an agent pointed at,
// served from the agent's home node — locally or over the mesh — and
// rendered by type: images inline, markdown rendered with a source
// toggle, text and code with line numbers, PDF in a frame, JSON pretty,
// anything else as a download.

import { useEffect, useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, ApiError, type FileStat } from "../api";
import { ErrorBar, relTime } from "../components";
import { linkifyPaths } from "../pathLinks";
import "./view.css";

type Mode = "rendered" | "source";

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function kindOf(media: string, name: string): "image" | "markdown" | "pdf" | "json" | "text" | "binary" {
  if (media.startsWith("image/")) return "image";
  if (media === "application/pdf") return "pdf";
  if (media === "text/markdown" || /\.(md|markdown)$/i.test(name)) return "markdown";
  if (media === "application/json" || /\.json$/i.test(name)) return "json";
  if (media.startsWith("text/") || media === "application/x-ndjson" || media === "application/toml" || media === "application/x-yaml" || media === "application/javascript" || media === "application/xml")
    return "text";
  return "binary";
}

export default function View() {
  const { name = "" } = useParams<{ name: string }>();
  const [params] = useSearchParams();
  const path = params.get("path") ?? "";
  const [stat, setStat] = useState<FileStat | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>("rendered");
  const [loading, setLoading] = useState(true);

  const node = name.includes("@") ? name.split("@").pop() : null;
  const kind = stat && stat.exists ? kindOf(stat.media_type ?? "", stat.name ?? "") : null;
  const raw = api.fileUrl(name, path);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    setStat(null);
    setText(null);
    (async () => {
      try {
        const st = await api.fileStat(name, path);
        if (cancelled) return;
        setStat(st);
        if (!st.exists) {
          setErr(`no such file on ${node ?? "this node"}: ${path}`);
          return;
        }
        const k = kindOf(st.media_type ?? "", st.name ?? "");
        if (k === "markdown" || k === "text" || k === "json") {
          if ((st.size ?? 0) > 4 * 1024 * 1024) {
            setErr("too large to render as text; download instead");
            return;
          }
          const t = await api.fileText(name, path);
          if (!cancelled) setText(t);
        }
      } catch (e) {
        if (!cancelled) setErr(e instanceof ApiError ? e.message : e instanceof Error ? e.message : "failed");
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [name, path, node]);

  // The document's directory, for relative references inside it.
  const dir = path.replace(/[\\/][^\\/]*$/, "");
  const joinRel = (rel: string) => (rel.startsWith("/") || /^[A-Za-z]:/.test(rel) ? rel : `${dir}/${rel.replace(/^\.\//, "")}`);
  const resolveRel = (src: string) =>
    /^(https?:|data:|blob:)/.test(src) ? src : api.fileUrl(name, joinRel(src));

  const pretty = useMemo(() => {
    if (kind !== "json" || text === null) return null;
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return text;
    }
  }, [kind, text]);

  const body = (() => {
    if (loading) return <div className="dim">loading from {node ?? "this node"}…</div>;
    if (err) return null;
    if (!stat || !stat.exists) return null;
    switch (kind) {
      case "image":
        return (
          <div className="view-image">
            <img src={raw} alt={stat.name ?? path} />
          </div>
        );
      case "pdf":
        return <iframe className="view-frame" src={raw} title={stat.name ?? path} />;
      case "markdown":
        return mode === "rendered" ? (
          <div className="md view-md">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{
                // Relative images and links inside the document resolve
                // against the document's directory, through the viewer.
                img: ({ src, alt }) => <img src={resolveRel(String(src ?? ""))} alt={alt ?? ""} />,
                a: ({ href, children }) => {
                  const h = String(href ?? "");
                  if (/^(https?:|mailto:|#)/.test(h) || h.startsWith("/view/")) {
                    return h.startsWith("/view/") ? <Link to={h}>{children}</Link> : <a href={h} target="_blank" rel="noreferrer">{children}</a>;
                  }
                  return <Link to={`/view/${encodeURIComponent(name)}?path=${encodeURIComponent(joinRel(h))}`}>{children}</Link>;
                },
              }}
            >
              {linkifyPaths(text ?? "", name)}
            </ReactMarkdown>
          </div>
        ) : (
          <Numbered text={text ?? ""} />
        );
      case "json":
        return <Numbered text={mode === "rendered" ? (pretty ?? "") : (text ?? "")} />;
      case "text":
        return <Numbered text={text ?? ""} />;
      default:
        return (
          <div className="dim">
            {stat.media_type ?? "binary"} — no inline view.{" "}
            <a href={api.fileUrl(name, path, { download: true })}>download {stat.name}</a>
          </div>
        );
    }
  })();

  return (
    <div className="view-page">
      <div className="view-head">
        <Link className="mono-meta" to={`/session/${encodeURIComponent(name)}`}>
          ← @{name}
        </Link>
        <span className="mono view-path" title={path}>
          {path}
        </span>
        {stat?.exists && (
          <span className="mono-meta">
            {node ? `on ${node} · ` : ""}
            {stat.media_type} · {fmtSize(stat.size ?? 0)}
            {stat.mtime ? ` · ${relTime(stat.mtime)} ago` : ""}
          </span>
        )}
        <span style={{ flex: 1 }} />
        {(kind === "markdown" || kind === "json") && (
          <span className="seg">
            <button className={mode === "rendered" ? "on" : ""} onClick={() => setMode("rendered")}>
              {kind === "json" ? "pretty" : "rendered"}
            </button>
            <button className={mode === "source" ? "on" : ""} onClick={() => setMode("source")}>
              source
            </button>
          </span>
        )}
        {stat?.exists && (
          <>
            <a className="btn sm" href={raw} target="_blank" rel="noreferrer">
              raw ↗
            </a>
            <a className="btn sm" href={api.fileUrl(name, path, { download: true })}>
              download
            </a>
          </>
        )}
      </div>
      <ErrorBar error={err} />
      <div className="view-body">{body}</div>
    </div>
  );
}

function Numbered({ text }: { text: string }) {
  const lines = text.split("\n");
  return (
    <pre className="view-code">
      {lines.map((l, i) => (
        <span key={i} className="view-line">
          <span className="view-ln">{i + 1}</span>
          {l}
          {"\n"}
        </span>
      ))}
    </pre>
  );
}
