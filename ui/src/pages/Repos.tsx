import { useEffect, useState, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { api, ApiError, type Repo, type SessionInfo } from "./../api";
import { usePoll } from "./../hooks";

function errText(e: unknown): string {
  return e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e);
}

/** Derive a valid agent name from a session title. */
function slugify(title: string | null): string {
  const base = (title ?? "")
    .trim()
    .replace(/[^A-Za-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return base || "session";
}

/** Format an epoch-seconds timestamp as a compact relative string. */
function fmtWhen(epochSeconds: number): string {
  const ms = epochSeconds * 1000;
  const diff = Date.now() - ms;
  const sec = Math.round(diff / 1000);
  if (sec < 45) return "just now";
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.round(hr / 24);
  if (day < 7) return `${day}d ago`;
  return new Date(ms).toLocaleDateString();
}

function AddRepoForm({ onAdded }: { onAdded: () => void }) {
  const [path, setPath] = useState("");
  const [skip, setSkip] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setError(null);
    if (!path.trim()) {
      setError("path is required");
      return;
    }
    setBusy(true);
    try {
      await api.addRepo(path.trim(), skip ? true : undefined);
      setPath("");
      setSkip(false);
      onAdded();
    } catch (err) {
      setError(errText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="panel session-form" onSubmit={submit}>
      <h2>Add repository</h2>
      <div className="form-row">
        <label className="grow-label">
          repository path
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="/home/you/src/project"
            className="mono grow"
            required
          />
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={skip}
            onChange={(e) => setSkip(e.target.checked)}
          />
          skip permissions by default
        </label>
        <button type="submit" disabled={busy}>
          {busy ? "adding…" : "Add"}
        </button>
      </div>
      {error && <div className="error-inline">{error}</div>}
    </form>
  );
}

function SessionRow({
  session,
  onResume,
}: {
  session: SessionInfo;
  onResume: (s: SessionInfo) => void;
}) {
  return (
    <div className="repo-session">
      <div className="repo-session-main">
        <span className="repo-session-title">{session.title || "(untitled)"}</span>
        <div className="repo-session-meta">
          <span className="dim" title={new Date(session.modified * 1000).toLocaleString()}>
            {fmtWhen(session.modified)}
          </span>
          {session.entrypoint && <span className="chip mono">{session.entrypoint}</span>}
          <span className="dim mono">
            {session.user_messages} msg{session.user_messages === 1 ? "" : "s"}
          </span>
        </div>
      </div>
      <button type="button" onClick={() => onResume(session)}>
        Resume
      </button>
    </div>
  );
}

function RepoCard({
  repo,
  selected,
  onSelect,
  onChanged,
  onForgotten,
  onNewSession,
  onError,
}: {
  repo: Repo;
  selected: boolean;
  onSelect: () => void;
  onChanged: () => void;
  onForgotten: () => void;
  onNewSession: (repo: string) => void;
  onError: (msg: string) => void;
}) {
  const [skipBusy, setSkipBusy] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [forgetting, setForgetting] = useState(false);

  async function toggleSkip(next: boolean) {
    if (skipBusy) return;
    setSkipBusy(true);
    try {
      await api.setRepoSkip(repo.path, next);
      onChanged();
    } catch (e) {
      onError(errText(e));
    } finally {
      setSkipBusy(false);
    }
  }

  async function forget() {
    if (forgetting) return;
    setForgetting(true);
    try {
      await api.forgetRepo(repo.path);
      setConfirmForget(false);
      onForgotten();
    } catch (e) {
      onError(errText(e));
    } finally {
      setForgetting(false);
    }
  }

  return (
    <div className={`repo-card${selected ? " selected" : ""}`}>
      <button type="button" className="repo-card-head" onClick={onSelect}>
        <span className="mono repo-path">{repo.path}</span>
        <span className="repo-counts dim mono">
          {repo.sessions} session{repo.sessions === 1 ? "" : "s"} · {repo.live_agents} live
        </span>
      </button>
      <div className="repo-card-controls">
        <label className="check repo-skip">
          <input
            type="checkbox"
            checked={repo.skip_permissions}
            disabled={skipBusy}
            onChange={(e) => void toggleSkip(e.target.checked)}
          />
          skip permissions
        </label>
        <button type="button" onClick={() => onNewSession(repo.path)}>
          New session
        </button>
        {!confirmForget ? (
          <button
            type="button"
            className="btn-deny"
            onClick={() => setConfirmForget(true)}
          >
            forget…
          </button>
        ) : (
          <>
            <button
              type="button"
              className="btn-deny"
              disabled={forgetting}
              onClick={() => void forget()}
            >
              {forgetting ? "forgetting…" : "really forget"}
            </button>
            <button
              type="button"
              className="btn-quiet"
              onClick={() => setConfirmForget(false)}
            >
              cancel
            </button>
          </>
        )}
      </div>
      <div className="repo-skip-caption dim">
        runs sessions with --dangerously-skip-permissions
      </div>
      {confirmForget && (
        <div className="repo-forget-note dim">
          forgetting removes it from this list only; sessions on disk are untouched.
        </div>
      )}
    </div>
  );
}

export default function Repos() {
  const navigate = useNavigate();
  const reposPoll = usePoll(api.repos, 3000);
  const repos = reposPoll.data ?? [];

  const [selected, setSelected] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  // Load a repo's discovered sessions whenever the selection changes.
  useEffect(() => {
    if (!selected) {
      setSessions(null);
      setSessionsError(null);
      return;
    }
    let disposed = false;
    setSessions(null);
    setSessionsError(null);
    api
      .sessions(selected)
      .then((list) => {
        if (disposed) return;
        const rows = list
          .filter((s) => s.user_messages > 0)
          .sort((a, b) => b.modified - a.modified);
        setSessions(rows);
      })
      .catch((e: unknown) => {
        if (disposed) return;
        setSessions([]);
        setSessionsError(errText(e));
      });
    return () => {
      disposed = true;
    };
  }, [selected]);

  async function newSession(repo: string) {
    const name = window.prompt("Agent name:")?.trim();
    if (!name) return;
    setActionError(null);
    try {
      await api.startAgent({ name, repo });
      navigate(`/session/${encodeURIComponent(name)}`);
    } catch (e) {
      setActionError(errText(e));
    }
  }

  async function resumeSession(repo: string, s: SessionInfo) {
    const name = window.prompt("Agent name:", slugify(s.title))?.trim();
    if (!name) return;
    setActionError(null);
    try {
      await api.startAgent({ name, repo, resume: s.session_id });
      navigate(`/session/${encodeURIComponent(name)}`);
    } catch (e) {
      setActionError(errText(e));
    }
  }

  function refresh() {
    void reposPoll.refresh();
  }

  return (
    <div className="page">
      <header className="page-head">
        <h1>Repos</h1>
        <span className="dim">
          {repos.length} remembered
        </span>
      </header>

      {reposPoll.error && <div className="error-inline">repos: {reposPoll.error}</div>}
      {actionError && <div className="error-inline">{actionError}</div>}

      <AddRepoForm onAdded={refresh} />

      {reposPoll.data !== null && repos.length === 0 && (
        <div className="empty">
          No repositories remembered. Start a session or add a path above.
        </div>
      )}

      <div className="repo-list">
        {repos.map((r) => (
          <RepoCard
            key={r.path}
            repo={r}
            selected={selected === r.path}
            onSelect={() => setSelected(selected === r.path ? null : r.path)}
            onChanged={refresh}
            onForgotten={() => {
              refresh();
              if (selected === r.path) setSelected(null);
            }}
            onNewSession={(repo) => void newSession(repo)}
            onError={setActionError}
          />
        ))}
      </div>

      {selected && (
        <div className="panel repo-sessions">
          <h2>
            sessions in <span className="mono">{selected}</span>
          </h2>
          {sessionsError && <div className="error-inline">sessions: {sessionsError}</div>}
          {sessions === null && !sessionsError && (
            <div className="sidebar-empty">loading…</div>
          )}
          {sessions !== null && sessions.length === 0 && !sessionsError && (
            <div className="empty">No prior sessions in this repository.</div>
          )}
          {sessions?.map((s) => (
            <SessionRow
              key={s.session_id}
              session={s}
              onResume={(sess) => void resumeSession(selected, sess)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
