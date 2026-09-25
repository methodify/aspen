/* A transcript read without resuming it (Mesh list → preview): the turns
 * as the session view shows them, read-only, with a way to resume under a
 * name once you know you want it. */
import { useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, type HistoryItem } from "../api";
import { ErrorBar } from "../components";
import { useTrustedStart } from "../trust";
import { slugify } from "../sessionRows";
import "./preview.css";

export default function Preview() {
  const [sp] = useSearchParams();
  const nav = useNavigate();
  const repo = sp.get("repo") ?? "";
  const session = sp.get("session") ?? "";
  const harness = sp.get("harness");
  const node = sp.get("node");
  const [items, setItems] = useState<HistoryItem[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [starting, setStarting] = useState(false);
  const trust = useTrustedStart();
  useEffect(() => {
    setItems(null);
    setErr(null);
    if (!repo || !session) {
      setErr("preview needs a repo and a session id");
      return;
    }
    api
      .sessionPreview({ repo, session, harness, node })
      .then((v) => setItems(v))
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)));
  }, [repo, session, harness, node]);
  const firstUser = items?.find((i) => i.role === "user" && !i.bus)?.text ?? "";
  const title = firstUser.split("\n")[0].slice(0, 120);
  const base = repo.split(/[\\/]/).filter(Boolean).pop() ?? repo;
  async function resume() {
    const n = name.trim() || slugify(title) || "main";
    setStarting(true);
    try {
      const a = await trust.start({ name: n, repo, resume: session, node: node ?? undefined, ...(harness ? { harness: harness as "claude" | "codex" } : {}) });
      if (a) nav(`/session/${encodeURIComponent(a.name)}`);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setStarting(false);
    }
  }
  return (
    <div className="page preview-page">
      <div className="preview-head">
        <Link to="/mesh" className="btn ghost sm">← mesh</Link>
        <span className="mono" style={{ color: "var(--text-hi)" }}>#{base}</span>
        {node && <span className="chip mono">{node}</span>}
        {harness && harness !== "claude" && <span className="chip mono">{harness}</span>}
        <span className="mono-meta" title={session}>{session.slice(0, 8)}</span>
        {items && <span className="mono-meta">{items.filter((i) => i.role === "user" && !i.bus).length} msgs</span>}
        <span className="chip mono" title="reading the transcript only; nothing is running">preview</span>
        <span style={{ flex: 1 }} />
        <input className="mono" value={name} onChange={(e) => setName(e.target.value)} placeholder={slugify(title) || "name"} style={{ width: 160 }} spellCheck={false} onKeyDown={(e) => e.key === "Enter" && void resume()} />
        <button className="btn primary sm" disabled={starting} onClick={() => void resume()} title="start a session under this name on this transcript">{starting ? "…" : "resume as"}</button>
      </div>
      {title && <h2 className="preview-title">{title}</h2>}
      <ErrorBar error={err} />
      {items === null && !err && <div className="dim">loading…</div>}
      {items && items.length === 0 && <div className="empty">this transcript has no turns</div>}
      <div className="preview-body">
        {items?.map((it, i) => (
          <div key={it.uuid ?? i} className={`pv-item ${it.role}${it.bus ? " pv-bus" : ""}`}>
            <div className="pv-who mono-meta">
              {it.role === "user" ? (it.bus ? "bus" : "you") : (it.model ?? "assistant")}
              {it.timestamp && <span title={it.timestamp}> · {new Date(it.timestamp).toLocaleString()}</span>}
            </div>
            {it.role === "assistant" ? (
              <div className="pv-text md">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{it.text}</ReactMarkdown>
              </div>
            ) : (
              <div className="pv-text pre">{it.text}</div>
            )}
            {(it.images?.length ?? 0) > 0 && <div className="mono-meta">{it.images!.length} image{it.images!.length === 1 ? "" : "s"}</div>}
            {(it.tools?.length ?? 0) > 0 && (
              <div className="pv-tools">
                {it.tools!.map((t, j) => (
                  <span key={j} className={`chip mono${t.is_error ? " gating" : ""}`} title={typeof t.result === "string" ? t.result.slice(0, 400) : undefined}>{t.name}</span>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
      {trust.dialog}
    </div>
  );
}
