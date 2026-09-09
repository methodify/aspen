// Search — the answer to "where did we decide X?" (PROPOSALS-2026-09-C.md
// §2). One query box; the node searches every session it holds (registered
// repos, both harnesses, replicas) and asks every up peer for theirs. Hits
// come grouped by session, newest first, each a snippet with the match
// marked; clicking opens the session at that line.

import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, type SearchResult, type SearchSession } from "../api";
import { useHotkeys } from "../hotkeys";
import { Empty, ErrorBar, relTime } from "../components";
import "./search.css";

function harnessChip(h?: string) {
  return h ? <span className="chip harness-chip">{h}</span> : null;
}

/** Where a hit opens: the owning agent's session at the line, or nothing
 *  clickable when no agent owns the session (the History viewer has no
 *  per-session page for that yet — the id is shown to attach by). */
function target(s: SearchSession, self: string, uuid: string | null): string | null {
  if (!s.agent) return null;
  const name = s.node === self ? s.agent : `${s.agent}@${s.node}`;
  return `/session/${encodeURIComponent(name)}${uuid ? `?at=${encodeURIComponent(uuid)}` : ""}`;
}

export default function Search() {
  const [params, setParams] = useSearchParams();
  const initial = params.get("q") ?? "";
  const [q, setQ] = useState(initial);
  const [res, setRes] = useState<SearchResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const self = res?.self ?? "";
  const inputRef = useRef<HTMLInputElement | null>(null);
  useHotkeys("search", [{ key: "/", handler: () => inputRef.current?.focus(), description: "focus the query box" }]);
  useEffect(() => {
    inputRef.current?.focus();
  }, []);
  const run = async (text: string) => {
    const t = text.trim();
    if (t.length < 2) return;
    setBusy(true);
    setErr(null);
    setParams({ q: t }, { replace: true });
    try {
      setRes(await api.search(t));
    } catch (e) {
      setErr(e instanceof Error ? e.message : "search failed");
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (initial.trim().length >= 2) void run(initial);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <>
      <div className="stage-head">
        <span className="label">Search</span>
        <form
          className="search-form"
          onSubmit={(e) => {
            e.preventDefault();
            void run(q);
          }}
        >
          <input
            ref={inputRef}
            className="mono search-input"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="a word or phrase from any session, anywhere on the mesh"
            aria-label="search text"
          />
          <button className="btn primary sm" type="submit" disabled={busy || q.trim().length < 2}>
            {busy ? "searching…" : "search"}
          </button>
        </form>
        {res && (
          <span className="mono-meta" title={`nodes asked: ${res.nodes.join(", ")}${res.nodes_failed.length ? ` · no answer from ${res.nodes_failed.join(", ")}` : ""}`}>
            {res.sessions.length} session{res.sessions.length === 1 ? "" : "s"} · {res.scanned} scanned · {res.nodes.length} node{res.nodes.length === 1 ? "" : "s"} · {res.took_ms} ms
            {res.nodes_failed.length ? <span className="error-text"> · {res.nodes_failed.length} did not answer</span> : null}
          </span>
        )}
      </div>
      <div className="stage-body">
        <ErrorBar error={err} />
        {!res && !busy && <Empty mark="?">Type what you remember — a name, a decision, an error — and search every transcript the mesh holds.</Empty>}
        {res && res.sessions.length === 0 && <Empty mark="—">Nothing mentions “{res.q}”.</Empty>}
        {res?.sessions.map((s) => {
          const open = target(s, self, null);
          return (
            <div className="search-session" key={`${s.node}:${s.session_id}`}>
              <div className="search-session-head">
                {open ? (
                  <Link to={open} className="mono search-agent">@{s.agent}{s.node !== self ? `@${s.node}` : ""}</Link>
                ) : (
                  <span className="mono search-agent dim" title="no agent owns this session; attach to it from its repo to open it">{s.title ?? s.session_id.slice(0, 8)}</span>
                )}
                {harnessChip(s.harness)}
                <span className="mono-meta">{s.repo_handle}</span>
                <span className="mono-meta">{s.node}{s.replica ? ` · replica of ${s.home}` : ""}</span>
                {s.title && open && <span className="search-title">{s.title}</span>}
                <span style={{ flex: 1 }} />
                <span className="mono-meta" title={new Date(s.modified * 1000).toLocaleString()}>{relTime(s.modified)} ago</span>
                <span className="mono-meta" title="session id">{s.session_id.slice(0, 8)}</span>
              </div>
              {s.hits.map((h, i) => {
                const to = target(s, self, h.uuid);
                const body = (
                  <>
                    <span className={`search-role mono-meta ${h.role ?? ""}`}>{h.role === "user" ? "you" : h.role === "assistant" ? "agent" : "·"}</span>
                    <span className="search-snippet">
                      {h.snippet.before}
                      <mark>{h.snippet.match}</mark>
                      {h.snippet.after}
                    </span>
                    {h.timestamp && <span className="mono-meta search-when">{relTime(Date.parse(h.timestamp) / 1000)} ago</span>}
                  </>
                );
                return to ? (
                  <Link key={i} to={to} className="search-hit" title="open the session at this line">{body}</Link>
                ) : (
                  <div key={i} className="search-hit">{body}</div>
                );
              })}
            </div>
          );
        })}
      </div>
    </>
  );
}
