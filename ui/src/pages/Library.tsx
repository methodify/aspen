// Library — the merged Repositories + Skills surface. One stage, two sections
// behind a segmented control: "Repositories" (remembered repos, their
// discovered sessions, resume / new-session) and "Skills" (edit a repo's
// skills + commands; saves reload live sessions). Reskin + merge of the old
// Repos and Skills pages — same endpoints, same flows, no native prompts.

import { useCallback, useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { api, ApiError, type MeshRepoNode, type Repo, type SessionInfo, type SkillEntry } from "../api";
import { usePoll, type Poll } from "../hooks";
import { Empty, ErrorBar, relTime } from "../components";
import { useTrustedStart } from "../trust";
import { MeshPanel } from "../meshPanel";
import { ServicingPanel, useMeshVersions, versionLabel } from "../servicing";

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


const REPO_STORAGE_KEY = "aspen.skills.repo";

function loadLastRepo(): string {
  try {
    return window.localStorage.getItem(REPO_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

function saveLastRepo(repo: string): void {
  try {
    window.localStorage.setItem(REPO_STORAGE_KEY, repo);
  } catch {
    // private mode / storage disabled — a convenience only, ignore.
  }
}

function skillTemplate(name: string): string {
  return `---
name: ${name}
description: TODO — one line on what this skill is for and when to use it
---

# ${name}

TODO: write the instructions this skill loads.
`;
}

function commandTemplate(name: string): string {
  return `---
description: TODO — one line on what /${name} does
---

TODO: write the prompt that /${name} runs.
`;
}

// ── Small shared inline "name this agent" composer ─────────────────────────

function NameComposer({
  label,
  initial,
  submitLabel,
  onSubmit,
  onCancel,
}: {
  label: string;
  initial: string;
  submitLabel: string;
  onSubmit: (name: string) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial);
  function submit(e: FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    onSubmit(trimmed);
  }
  return (
    <form
      onSubmit={submit}
      style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}
    >
      <input
        autoFocus
        className="mono"
        value={name}
        placeholder={label}
        onChange={(e) => setName(e.target.value)}
        style={{ minWidth: 180 }}
      />
      <button type="submit" className="btn primary sm" disabled={!name.trim()}>
        {submitLabel}
      </button>
      <button type="button" className="btn ghost sm" onClick={onCancel}>
        cancel
      </button>
    </form>
  );
}

// ── Repositories section ───────────────────────────────────────────────────

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
    <form
      className="strip flat"
      onSubmit={submit}
      style={{ display: "flex", flexDirection: "column", gap: 10, padding: 14 }}
    >
      <span className="label">Add repository</span>
      <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
        <input
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="/home/you/src/project"
          className="mono"
          style={{ flex: 1, minWidth: 240 }}
        />
        <label
          className="micro"
          style={{ display: "flex", alignItems: "center", gap: 6, color: "var(--text-mid)" }}
        >
          <input
            type="checkbox"
            checked={skip}
            onChange={(e) => setSkip(e.target.checked)}
            style={{ width: "auto" }}
          />
          skip permissions by default
        </label>
        <button type="submit" className="btn sm" disabled={busy}>
          {busy ? "adding…" : "add"}
        </button>
      </div>
      <ErrorBar error={error} />
    </form>
  );
}

function HarnessDefaults() {
  const [args, setArgs] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .settings()
      .then((s) => {
        if (!live) return;
        setArgs(s.harness?.claude?.args ?? "");
        setLoaded(true);
      })
      .catch((e) => live && setError(errText(e)));
    return () => {
      live = false;
    };
  }, []);

  async function save() {
    if (busy) return;
    setError(null);
    setSaved(false);
    setBusy(true);
    try {
      await api.saveSettings({ harness: { claude: { args: args.trim() } } });
      setSaved(true);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="strip flat"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
      style={{ display: "flex", flexDirection: "column", gap: 10, padding: 14 }}
    >
      <span className="label">Claude defaults</span>
      <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
        <input
          value={args}
          disabled={!loaded}
          onChange={(e) => {
            setArgs(e.target.value);
            setSaved(false);
          }}
          placeholder="extra CLI args for every claude session, e.g. --chrome"
          className="mono"
          style={{ flex: 1, minWidth: 240 }}
          spellCheck={false}
          aria-label="default claude args"
        />
        <button type="submit" className="btn sm" disabled={busy || !loaded}>
          {busy ? "saving…" : saved ? "saved" : "save"}
        </button>
      </div>
      <span className="micro" style={{ color: "var(--text-mid)" }}>
        appended to every session of this harness; per-session args come after these.
      </span>
      <ErrorBar error={error} />
    </form>
  );
}

function SessionRow({
  session,
  onResume,
}: {
  session: SessionInfo;
  onResume: (s: SessionInfo, name: string) => void;
}) {
  const [naming, setNaming] = useState(false);
  return (
    <div
      className="strip flat"
      style={{ display: "flex", flexDirection: "column", gap: 8, padding: "10px 12px" }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
        <span className="mono" style={{ fontWeight: 500, color: "var(--text-hi)" }}>
          {session.mcc_name || session.title || "(untitled)"}
        </span>
        {session.mcc_name && (
          <span className="chip mono" title={session.title ?? undefined}>
            mcc
          </span>
        )}
        {session.mcc_args && (
          <span className="chip mono" title="args configured in mcc; applied on resume">
            {session.mcc_args}
          </span>
        )}
        <span style={{ flex: 1 }} />
        <span
          className="mono-meta"
          title={new Date(session.modified * 1000).toLocaleString()}
        >
          {relTime(session.modified)} ago
        </span>
        {session.entrypoint && <span className="chip mono">{session.entrypoint}</span>}
        <span className="mono-meta">
          {session.user_messages} msg{session.user_messages === 1 ? "" : "s"}
        </span>
        {!naming && (
          <button type="button" className="btn sm" onClick={() => setNaming(true)}>
            resume
          </button>
        )}
      </div>
      {naming && (
        <NameComposer
          label="agent name"
          initial={slugify(session.mcc_name ?? session.title)}
          submitLabel="resume"
          onSubmit={(name) => {
            setNaming(false);
            onResume(session, name);
          }}
          onCancel={() => setNaming(false)}
        />
      )}
    </div>
  );
}

function RepoStrip({
  repo,
  node,
  selected,
  onSelect,
  onChanged,
  onForgotten,
  onNewSession,
  onError,
}: {
  repo: Repo;
  /** The owning peer for a remote repo; undefined = this node. Every
   *  control acts on the owning node over the mesh. */
  node?: string;
  selected: boolean;
  onSelect: () => void;
  onChanged: () => void;
  onForgotten: () => void;
  onNewSession: (repo: string, name: string) => void;
  onError: (msg: string) => void;
}) {
  const [skipBusy, setSkipBusy] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const [namingNew, setNamingNew] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [handleDraft, setHandleDraft] = useState(repo.handle ?? "");
  const [renameBusy, setRenameBusy] = useState(false);

  async function rename() {
    const next = handleDraft.trim();
    if (renameBusy || !next || next === repo.handle) {
      setRenaming(false);
      return;
    }
    setRenameBusy(true);
    try {
      await api.renameRepo(repo.path, next, node);
      setRenaming(false);
      onChanged();
    } catch (e) {
      onError(errText(e));
    } finally {
      setRenameBusy(false);
    }
  }

  async function toggleSkip(next: boolean) {
    if (skipBusy) return;
    setSkipBusy(true);
    try {
      await api.setRepoSkip(repo.path, next, node);
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
      await api.forgetRepo(repo.path, node);
      setConfirmForget(false);
      onForgotten();
    } catch (e) {
      onError(errText(e));
    } finally {
      setForgetting(false);
    }
  }

  return (
    <div
      className="strip"
      style={{ display: "flex", flexDirection: "column", gap: 10, cursor: "default" }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap" }}>
        <button
          type="button"
          onClick={onSelect}
          aria-expanded={selected}
          style={{
            background: "transparent",
            border: "none",
            padding: 0,
            cursor: "pointer",
            color: "var(--text-hi)",
            display: "flex",
            alignItems: "center",
            gap: 8,
            textAlign: "left",
            flex: 1,
            minWidth: 0,
          }}
        >
          <span className="micro" style={{ color: "var(--text-dim)" }}>
            {selected ? "▾" : "▸"}
          </span>
          <span
            className="mono"
            style={{ fontWeight: 500, overflow: "hidden", textOverflow: "ellipsis" }}
          >
            {repo.path}
          </span>
        </button>
        {/* The handle: address segment (`arch@<handle>`) and channel name. */}
        {renaming ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void rename();
            }}
            style={{ display: "flex", gap: 6, alignItems: "center" }}
          >
            <span className="mono-meta">#</span>
            <input
              className="mono"
              value={handleDraft}
              onChange={(e) => setHandleDraft(e.target.value)}
              autoFocus
              spellCheck={false}
              style={{ width: 160 }}
              aria-label="repo handle"
              onKeyDown={(e) => e.key === "Escape" && setRenaming(false)}
            />
            <button type="submit" className="btn sm" disabled={renameBusy}>
              {renameBusy ? "…" : "rename"}
            </button>
          </form>
        ) : (
          <button
            type="button"
            className="chip mono"
            onClick={() => {
              setHandleDraft(repo.handle ?? "");
              setRenaming(true);
            }}
            title="repo handle: agents here are addressed as name@handle, and #handle is its channel — click to rename (stop its sessions first)"
          >
            #{repo.handle ?? "?"}
          </button>
        )}
        <span className="mono-meta">
          {repo.sessions} session{repo.sessions === 1 ? "" : "s"}
        </span>
        {(() => {
          const live = repo.live_agents ?? repo.live ?? 0;
          return (
            <span
              className="mono-meta"
              style={{ color: live > 0 ? "var(--live)" : undefined }}
            >
              {live} live
            </span>
          );
        })()}
      </div>

      <div style={{ display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap" }}>
        <label
          className="micro"
          style={{ display: "flex", alignItems: "center", gap: 6, color: "var(--text-mid)" }}
        >
          <input
            type="checkbox"
            checked={repo.skip_permissions}
            disabled={skipBusy}
            onChange={(e) => void toggleSkip(e.target.checked)}
            style={{ width: "auto" }}
          />
          skip permissions
        </label>
        <span className="micro" style={{ color: "var(--text-dim)" }}>
          runs sessions with --dangerously-skip-permissions
        </span>
        <span style={{ flex: 1 }} />
        {!namingNew && (
          <button type="button" className="btn sm" onClick={() => setNamingNew(true)}>
            new session
          </button>
        )}
        {!node && (
          <button
            type="button"
            className="btn ghost sm"
            aria-expanded={skillsOpen}
            onClick={() => setSkillsOpen((v) => !v)}
            title="this repo's skills and commands (.claude/*.md); saves reload live sessions"
          >
            skills {skillsOpen ? "▴" : "▾"}
          </button>
        )}
        {!confirmForget ? (
            <button
              type="button"
              className="btn ghost sm"
              onClick={() => setConfirmForget(true)}
            >
              forget…
            </button>
          ) : (
            <>
              <button
                type="button"
                className="btn danger sm"
                disabled={forgetting}
                onClick={() => void forget()}
              >
                {forgetting ? "forgetting…" : "really forget"}
              </button>
              <button
                type="button"
                className="btn ghost sm"
                onClick={() => setConfirmForget(false)}
              >
                cancel
              </button>
            </>
          )}
      </div>

      {namingNew && (
        <NameComposer
          label="agent name"
          initial=""
          submitLabel="start"
          onSubmit={(name) => {
            setNamingNew(false);
            onNewSession(repo.path, name);
          }}
          onCancel={() => setNamingNew(false)}
        />
      )}

      {skillsOpen && (
        <div style={{ borderTop: "1px solid var(--line)", paddingTop: 10 }}>
          <SkillsSection repos={[repo]} />
        </div>
      )}

      {confirmForget && (
        <span className="micro" style={{ color: "var(--text-dim)" }}>
          forgetting removes it from {node ? `${node}'s` : "this"} registry only; sessions on disk are kept.
        </span>
      )}
    </div>
  );
}

function RepositoriesSection({
  reposPoll,
  meshPoll,
}: {
  reposPoll: Poll<Repo[]>;
  meshPoll: Poll<{ nodes: MeshRepoNode[] }>;
}) {
  const navigate = useNavigate();
  const trust = useTrustedStart();
  const nodes = meshPoll.data?.nodes ?? [];
  const versions = useMeshVersions();

  // Selection is node-qualified: two nodes can hold the same path.
  const [selKey, setSelKey] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [openNodes, setOpenNodes] = useState<Record<string, boolean>>({});
  const [discovering, setDiscovering] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const q = filter.trim().toLowerCase();
  const matches = (path: string) => q === "" || path.toLowerCase().includes(q);

  const key = (node: string, path: string) => `${node}\u0000${path}`;
  const selected = selKey ? { node: selKey.split("\u0000")[0], path: selKey.split("\u0000")[1] } : null;

  // Load the selected repo's sessions (from its owning node) on change.
  useEffect(() => {
    if (!selected) {
      setSessions(null);
      setSessionsError(null);
      return;
    }
    let disposed = false;
    setSessions(null);
    setSessionsError(null);
    const selfNode = nodes.find((n) => n.self)?.node;
    const remote = selected.node !== selfNode ? selected.node : undefined;
    api
      .sessions(selected.path, remote)
      .then((list) => {
        if (disposed) return;
        setSessions(
          list.filter((s) => s.user_messages > 0).sort((a, b) => b.modified - a.modified),
        );
      })
      .catch((e: unknown) => {
        if (disposed) return;
        setSessions([]);
        setSessionsError(errText(e));
      });
    return () => {
      disposed = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selKey]);

  const selfNode = nodes.find((n) => n.self)?.node;
  const isSelf = (node: string) => node === selfNode;

  async function newSession(node: string, repo: string, name: string) {
    setActionError(null);
    try {
      const agent = await trust.start({
        name,
        repo,
        node: isSelf(node) ? undefined : node,
      });
      if (agent === null) return; // operator declined the trust review
      navigate(`/session/${encodeURIComponent(agent.name)}`);
    } catch (e) {
      setActionError(errText(e));
    }
  }

  async function resumeSession(node: string, repo: string, s: SessionInfo, name: string) {
    setActionError(null);
    try {
      const agent = await trust.start({
        name,
        repo,
        resume: s.session_id,
        node: isSelf(node) ? undefined : node,
        // mcc register carry-over: name → title, args ride, skip maps to us.
        title: s.mcc_name ?? undefined,
        extra_args: s.mcc_args ?? undefined,
        skip_permissions: s.mcc_skip ? true : undefined,
      });
      if (agent === null) return;
      navigate(`/session/${encodeURIComponent(agent.name)}`);
    } catch (e) {
      setActionError(errText(e));
    }
  }

  async function discover(node: string) {
    setActionError(null);
    setDiscovering(node);
    try {
      await api.discoverRepos(isSelf(node) ? undefined : node);
      await meshPoll.refresh();
    } catch (e) {
      setActionError(errText(e));
    } finally {
      setDiscovering(null);
    }
  }

  function refresh() {
    void meshPoll.refresh();
    void reposPoll.refresh();
  }

  const meshed = nodes.length > 1;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      {trust.dialog}
      <ErrorBar error={meshPoll.error ? `repos: ${meshPoll.error}` : null} />
      <ErrorBar error={actionError} />

      <MeshPanel />
      <ServicingPanel />
      <AddRepoForm onAdded={refresh} />
      <HarnessDefaults />
      <LinksSection />

      {nodes.reduce((acc, n) => acc + n.repos.length, 0) > 8 && (
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter repositories across all nodes…"
          className="mono"
          spellCheck={false}
          aria-label="filter repositories"
        />
      )}

      {nodes.map((n) => {
        // A single (local) node stays flat — no section chrome. With peers,
        // each node gets a collapsible header; remote sections start closed
        // so a large remote registry doesn't bury the local one.
        const visible = n.repos.filter((r) => matches(r.path));
        // A live filter opens every section so hits are never hidden.
        const open = q !== "" ? true : meshed ? (openNodes[n.node] ?? n.self) : true;
        if (q !== "" && visible.length === 0) return null;
        const body = (
          <div className="grid">
            {visible.length === 0 ? (
              <Empty mark="◦">
                {n.reachable
                  ? "No repositories here yet — discover, or start a session."
                  : "Node unreachable; its repositories are hidden until the link is back."}
              </Empty>
            ) : (
              visible.map((r) => {
                const k = key(n.node, r.path);
                return (
                  <div key={k} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                    <RepoStrip
                      repo={r}
                      node={n.self ? undefined : n.node}
                      selected={selKey === k}
                      onSelect={() => setSelKey(selKey === k ? null : k)}
                      onChanged={refresh}
                      onForgotten={() => {
                        refresh();
                        if (selKey === k) setSelKey(null);
                      }}
                      onNewSession={(repo, name) => void newSession(n.node, repo, name)}
                      onError={setActionError}
                    />
                    {selKey === k && (
                      <div
                        style={{
                          display: "flex",
                          flexDirection: "column",
                          gap: 6,
                          paddingLeft: 12,
                          borderLeft: "1px solid var(--line)",
                          marginLeft: 8,
                        }}
                      >
                        {sessionsError && <ErrorBar error={`sessions: ${sessionsError}`} />}
                        {sessions === null && !sessionsError && (
                          <span className="mono-meta">loading…</span>
                        )}
                        {sessions !== null && sessions.length === 0 && !sessionsError && (
                          <Empty mark="—">No prior sessions in this repository.</Empty>
                        )}
                        {sessions?.map((s) => (
                          <SessionRow
                            key={s.session_id}
                            session={s}
                            onResume={(sess, name) =>
                              void resumeSession(n.node, r.path, sess, name)
                            }
                          />
                        ))}
                      </div>
                    )}
                  </div>
                );
              })
            )}
          </div>
        );

        if (!meshed) return <div key={n.node}>{body}</div>;
        return (
          <div key={n.node} style={{ display: "flex", flexDirection: "column", gap: 10 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <button
                type="button"
                onClick={() => setOpenNodes((o) => ({ ...o, [n.node]: !open }))}
                aria-expanded={open}
                style={{
                  background: "transparent",
                  border: "none",
                  padding: 0,
                  cursor: "pointer",
                  color: "var(--text-hi)",
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                }}
              >
                <span className="micro" style={{ color: "var(--text-dim)" }}>
                  {open ? "▾" : "▸"}
                </span>
                <span className="label" style={{ margin: 0 }}>
                  {n.node}
                </span>
                <span className="chip mono">{n.self ? "this node" : "peer"}</span>
                {versionLabel(versions[n.node]) && (
                  <span
                    className="mono-meta"
                    style={{ color: versions[n.node]?.available || versions[n.node]?.skew ? "var(--sig-normal)" : undefined }}
                    title="aspen version this node runs"
                  >
                    {versionLabel(versions[n.node])}
                  </span>
                )}
                {!n.reachable && (
                  <span className="chip mono" style={{ color: "var(--sig-gate)" }}>
                    unreachable
                  </span>
                )}
                <span className="mono-meta">
                  {q !== "" && visible.length !== n.repos.length
                    ? `${visible.length} of ${n.repos.length}`
                    : `${n.repos.length} repo${n.repos.length === 1 ? "" : "s"}`}
                </span>
              </button>
              <span style={{ flex: 1 }} />
              {n.reachable && (
                <button
                  type="button"
                  className="btn ghost sm"
                  disabled={discovering === n.node}
                  onClick={() => void discover(n.node)}
                  title={
                    n.self
                      ? "find repos from this machine's Claude Code sessions"
                      : `run discovery on ${n.node} — its repos register there`
                  }
                >
                  {discovering === n.node ? "discovering…" : "discover"}
                </button>
              )}
            </div>
            {open && body}
          </div>
        );
      })}
    </div>
  );
}
// ── Skills section ─────────────────────────────────────────────────────────

interface EntryListProps {
  heading: string;
  entries: SkillEntry[];
  selectedRel: string | null;
  onSelect: (entry: SkillEntry) => void;
}

function EntryList({ heading, entries, selectedRel, onSelect }: EntryListProps) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 12 }}>
      <div className="label" style={{ marginBottom: 4 }}>
        {heading}
      </div>
      {entries.length === 0 && <span className="mono-meta">none</span>}
      {entries.map((e) => {
        const active = e.rel === selectedRel;
        return (
          <button
            key={e.rel}
            type="button"
            className="strip flat"
            onClick={() => onSelect(e)}
            title={e.rel}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "8px 10px",
              cursor: "pointer",
              textAlign: "left",
              border: "none",
              background: active ? "var(--bg-strip-2)" : undefined,
              boxShadow: active ? "inset 2px 0 0 var(--sig-normal)" : undefined,
            }}
          >
            <span className="mono" style={{ fontWeight: 500, color: "var(--text-hi)" }}>
              {e.name}
            </span>
            <span style={{ flex: 1 }} />
            {e.description && (
              <span
                className="mono-meta"
                style={{
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                  maxWidth: 180,
                }}
              >
                {e.description}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

function SkillsSection({ repos }: { repos: Repo[] }) {
  const repoOptions = useMemo(() => repos.map((r) => r.path).sort(), [repos]);

  const [repo, setRepo] = useState<string>(loadLastRepo);

  const [entries, setEntries] = useState<SkillEntry[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);

  const [selected, setSelected] = useState<SkillEntry | null>(null);
  const [content, setContent] = useState<string>("");
  const [savedContent, setSavedContent] = useState<string>("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveNote, setSaveNote] = useState<string | null>(null);

  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [creating, setCreating] = useState(false);
  const [creatingKind, setCreatingKind] = useState<"skill" | "command" | null>(null);

  // Inline discard-guard: the deferred action runs only if the operator OKs it.
  const [pendingDiscard, setPendingDiscard] = useState<(() => void) | null>(null);

  const dirty = selected !== null && content !== savedContent;

  const refreshList = useCallback(async (repoPath: string): Promise<SkillEntry[]> => {
    const list = await api.repoSkills(repoPath);
    setEntries(list);
    setListError(null);
    return list;
  }, []);

  // (Re)load the listing whenever the chosen repo changes.
  useEffect(() => {
    if (!repo) {
      setEntries(null);
      setListError(null);
      return;
    }
    let disposed = false;
    setEntries(null);
    api
      .repoSkills(repo)
      .then((list) => {
        if (disposed) return;
        setEntries(list);
        setListError(null);
      })
      .catch((e: unknown) => {
        if (disposed) return;
        setEntries([]);
        setListError(errText(e));
      });
    return () => {
      disposed = true;
    };
  }, [repo]);

  /** Run `action` now, or stage it behind a discard-confirm when dirty. */
  function guard(action: () => void) {
    if (dirty) {
      setPendingDiscard(() => action);
      return;
    }
    action();
  }

  function chooseRepo(path: string) {
    const trimmed = path.trim();
    if (!trimmed || trimmed === repo) return;
    guard(() => {
      setRepo(trimmed);
      saveLastRepo(trimmed);
      setSelected(null);
      setContent("");
      setSavedContent("");
      setLoadError(null);
      setSaveError(null);
      setSaveNote(null);
      setConfirmDelete(false);
      setCreatingKind(null);
    });
  }

  async function loadEntry(entry: SkillEntry) {
    setSelected(entry);
    setContent("");
    setSavedContent("");
    setLoadError(null);
    setSaveError(null);
    setSaveNote(null);
    setConfirmDelete(false);
    setCreatingKind(null);
    setLoading(true);
    try {
      const { content: text } = await api.readSkill(repo, entry.rel);
      setContent(text);
      setSavedContent(text);
    } catch (e) {
      setLoadError(errText(e));
    } finally {
      setLoading(false);
    }
  }

  function select(entry: SkillEntry) {
    if (entry.rel === selected?.rel) return;
    guard(() => void loadEntry(entry));
  }

  async function save() {
    if (!selected || !dirty || saving) return;
    setSaving(true);
    setSaveError(null);
    setSaveNote(null);
    try {
      const res = await api.writeSkill(repo, selected.rel, content, true);
      setSavedContent(content);
      setSaveNote(
        `saved · reloaded ${res.reloaded_sessions} live session${res.reloaded_sessions === 1 ? "" : "s"}`,
      );
      // Descriptions may have changed; refresh the listing quietly.
      void refreshList(repo).catch(() => {});
    } catch (e) {
      setSaveError(`save: ${errText(e)}`);
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (!selected || deleting) return;
    setDeleting(true);
    setSaveError(null);
    setSaveNote(null);
    try {
      await api.deleteSkill(repo, selected.rel);
      setSelected(null);
      setContent("");
      setSavedContent("");
      setConfirmDelete(false);
      await refreshList(repo).catch((e: unknown) => setListError(errText(e)));
    } catch (e) {
      setSaveError(`delete: ${errText(e)}`);
    } finally {
      setDeleting(false);
    }
  }

  async function create(kind: "skill" | "command", rawName: string) {
    if (creating) return;
    const name = rawName.trim().replace(/\s+/g, "-");
    if (!name) return;
    const rel =
      kind === "skill" ? `.claude/skills/${name}/SKILL.md` : `.claude/commands/${name}.md`;
    const body = kind === "skill" ? skillTemplate(name) : commandTemplate(name);
    setCreating(true);
    setCreatingKind(null);
    setSaveError(null);
    setSaveNote(null);
    try {
      const res = await api.writeSkill(repo, rel, body, true);
      const list = await refreshList(repo);
      const entry =
        list.find((e) => e.rel === rel) ?? { name, rel, kind, description: null };
      setSelected(entry);
      setContent(body);
      setSavedContent(body);
      setLoadError(null);
      setConfirmDelete(false);
      setSaveNote(
        `created · reloaded ${res.reloaded_sessions} live session${res.reloaded_sessions === 1 ? "" : "s"}`,
      );
    } catch (e) {
      setSaveError(`create: ${errText(e)}`);
    } finally {
      setCreating(false);
    }
  }

  function startCreate(kind: "skill" | "command") {
    guard(() => setCreatingKind(kind));
  }

  const skills = (entries ?? []).filter((e) => e.kind === "skill");
  const commands = (entries ?? []).filter((e) => e.kind === "command");

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <div
        className="strip flat"
        style={{ display: "flex", alignItems: "center", gap: 10, padding: 12, flexWrap: "wrap" }}
      >
        <span className="label">repo</span>
        <select
          className="mono"
          value={repoOptions.includes(repo) ? repo : ""}
          onChange={(e) => chooseRepo(e.target.value)}
          style={{ minWidth: 260 }}
        >
          <option value="">— pick a remembered repo —</option>
          {repoOptions.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        {repo && !repoOptions.includes(repo) && (
          <span className="mono-meta">using {repo}</span>
        )}
      </div>

      {pendingDiscard && (
        <div className="error-bar" style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span style={{ flex: 1 }}>Discard unsaved changes?</span>
          <button
            type="button"
            className="btn danger sm"
            onClick={() => {
              const run = pendingDiscard;
              setPendingDiscard(null);
              run();
            }}
          >
            discard
          </button>
          <button
            type="button"
            className="btn ghost sm"
            onClick={() => setPendingDiscard(null)}
          >
            keep editing
          </button>
        </div>
      )}

      <ErrorBar error={listError ? `skills: ${listError}` : null} />

      {!repo ? (
        <Empty mark="◦">Pick a repo above to browse its skills and commands.</Empty>
      ) : (
        <div
          style={{
            display: "grid",
            gap: 20,
            gridTemplateColumns: "minmax(260px, 1fr) minmax(360px, 1.6fr)",
            alignItems: "start",
          }}
        >
          {/* listing */}
          <section>
            <div style={{ display: "flex", gap: 8, marginBottom: 12, flexWrap: "wrap" }}>
              <button
                type="button"
                className="btn sm"
                disabled={creating}
                onClick={() => startCreate("skill")}
              >
                + skill
              </button>
              <button
                type="button"
                className="btn sm"
                disabled={creating}
                onClick={() => startCreate("command")}
              >
                + command
              </button>
            </div>

            {creatingKind && (
              <div style={{ marginBottom: 12 }}>
                <NameComposer
                  label={`new ${creatingKind} name (lowercase, dashes ok)`}
                  initial=""
                  submitLabel="create"
                  onSubmit={(name) => void create(creatingKind, name)}
                  onCancel={() => setCreatingKind(null)}
                />
              </div>
            )}

            {entries === null && !listError && <span className="mono-meta">loading…</span>}
            {entries !== null && entries.length === 0 && !listError && (
              <Empty mark="—">
                No skills yet — create one with <span className="mono">+ skill</span>.
              </Empty>
            )}
            {entries !== null && entries.length > 0 && (
              <>
                <EntryList
                  heading="skills"
                  entries={skills}
                  selectedRel={selected?.rel ?? null}
                  onSelect={select}
                />
                <EntryList
                  heading="commands"
                  entries={commands}
                  selectedRel={selected?.rel ?? null}
                  onSelect={select}
                />
              </>
            )}
          </section>

          {/* editor */}
          <section>
            {!selected ? (
              <Empty mark="◦">Select an entry to edit it.</Empty>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                  <span className="mono" style={{ color: "var(--text-mid)" }} title={selected.rel}>
                    {selected.rel}
                  </span>
                  {dirty && <span className="chip">unsaved</span>}
                </div>
                {loadError && <ErrorBar error={`read: ${loadError}`} />}
                <textarea
                  className="mono"
                  value={content}
                  onChange={(e) => setContent(e.target.value)}
                  disabled={loading || loadError !== null}
                  placeholder={loading ? "loading…" : ""}
                  spellCheck={false}
                  rows={22}
                  style={{ width: "100%", minHeight: 360 }}
                />
                <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                  <button
                    type="button"
                    className="btn primary sm"
                    onClick={() => void save()}
                    disabled={!dirty || saving || loading || loadError !== null}
                  >
                    {saving ? "saving…" : "save + reload"}
                  </button>
                  {!confirmDelete ? (
                    <button
                      type="button"
                      className="btn ghost sm"
                      disabled={deleting}
                      onClick={() => setConfirmDelete(true)}
                    >
                      delete…
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        className="btn danger sm"
                        disabled={deleting}
                        onClick={() => void remove()}
                      >
                        {deleting ? "deleting…" : "really delete"}
                      </button>
                      <button
                        type="button"
                        className="btn ghost sm"
                        onClick={() => setConfirmDelete(false)}
                      >
                        cancel
                      </button>
                    </>
                  )}
                  <span style={{ flex: 1 }} />
                  {saveError && (
                    <span className="micro" style={{ color: "var(--sig-gate)" }}>
                      {saveError}
                    </span>
                  )}
                  {saveNote && (
                    <span className="micro" style={{ color: "var(--live)" }}>
                      {saveNote}
                    </span>
                  )}
                </div>
              </div>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

// ── Library shell ──────────────────────────────────────────────────────────

/** The Mesh's list view: repos by node, editable in place; skills open
 *  as a drawer on a repo. (The map is the same data, drawn.) */
function humanEndpoint(ep: string): string {
  if (ep === "operator") return "@operator";
  if (ep.startsWith("agent:")) return `@${ep.slice(6)}`;
  if (ep.startsWith("repo:")) return `#${ep.slice(5)}`;
  if (ep.startsWith("node:")) return `node ${ep.slice(5)}`;
  return ep;
}

/** Declared links, as a list (the map draws them). */
function LinksSection() {
  const poll = usePoll(api.links, 5000);
  const links = poll.data ?? [];
  const [err, setErr] = useState<string | null>(null);
  if (links.length === 0) return null;
  return (
    <div className="strip flat" style={{ display: "flex", flexDirection: "column", gap: 6, padding: 14 }}>
      <span className="label">Links · {links.length}</span>
      <span className="micro" style={{ color: "var(--text-dim)" }}>
        declared pathways; the purpose is what the agents on the from-side are told. Draw new ones on the map.
      </span>
      <ErrorBar error={err} />
      {links.map((l) => (
        <div key={l.id} style={{ display: "flex", alignItems: "center", gap: 10, padding: "4px 0" }}>
          <span className="mono" style={{ color: "var(--text-hi)" }}>{humanEndpoint(l.src)}</span>
          <span className="mono-meta">{l.two_way ? "↔" : "→"}</span>
          <span className="mono" style={{ color: "var(--text-hi)" }}>{humanEndpoint(l.dst)}</span>
          {l.purpose && <span style={{ color: "var(--text-mid)" }}>— {l.purpose}</span>}
          {l.urgency && <span className="chip mono">{l.urgency}</span>}
          <span style={{ flex: 1 }} />
          <button
            type="button"
            className="btn ghost sm"
            onClick={() => void api.deleteLink(l.id).then(poll.refresh).catch((e) => setErr(errText(e)))}
          >
            remove
          </button>
        </div>
      ))}
    </div>
  );
}

export default function MeshList({ toggle }: { toggle: ReactNode }) {
  const reposPoll = usePoll(api.repos, 15000);
  const meshPoll = usePoll(api.meshRepos, 15000);
  const repos = reposPoll.data ?? [];
  const nodeCount = meshPoll.data?.nodes.length ?? 1;

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Mesh</span>
        {toggle}
        <span style={{ flex: 1 }} />
        <span className="mono-meta">
          {repos.length} local repo{repos.length === 1 ? "" : "s"}
          {nodeCount > 1 ? ` · ${nodeCount} nodes` : ""}
        </span>
      </div>
      <div className="stage-body">
        <RepositoriesSection reposPoll={reposPoll} meshPoll={meshPoll} />
      </div>
    </>
  );
}
