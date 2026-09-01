// Library — the merged Repositories + Skills surface. One stage, two sections
// behind a segmented control: "Repositories" (remembered repos, their
// discovered sessions, resume / new-session) and "Skills" (edit a repo's
// skills + commands; saves reload live sessions). Reskin + merge of the old
// Repos and Skills pages — same endpoints, same flows, no native prompts.

import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { api, ApiError, type Repo, type SessionInfo, type SkillEntry } from "../api";
import { usePoll, type Poll } from "../hooks";
import { Empty, ErrorBar, relTime } from "../components";
import { useTrustedStart } from "../trust";

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

type Section = "repos" | "skills";

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
          {session.title || "(untitled)"}
        </span>
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
          initial={slugify(session.title)}
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
  onNewSession: (repo: string, name: string) => void;
  onError: (msg: string) => void;
}) {
  const [skipBusy, setSkipBusy] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const [namingNew, setNamingNew] = useState(false);

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
        <span className="mono-meta">
          {repo.sessions} session{repo.sessions === 1 ? "" : "s"}
        </span>
        <span
          className="mono-meta"
          style={{ color: repo.live_agents > 0 ? "var(--live)" : undefined }}
        >
          {repo.live_agents} live
        </span>
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

      {confirmForget && (
        <span className="micro" style={{ color: "var(--text-dim)" }}>
          forgetting removes it from this list only; sessions on disk are kept.
        </span>
      )}
    </div>
  );
}

function RepositoriesSection({ reposPoll }: { reposPoll: Poll<Repo[]> }) {
  const navigate = useNavigate();
  const trust = useTrustedStart();
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

  async function newSession(repo: string, name: string) {
    setActionError(null);
    try {
      const agent = await trust.start({ name, repo });
      if (agent === null) return; // operator declined the trust review
      navigate(`/session/${encodeURIComponent(name)}`);
    } catch (e) {
      setActionError(errText(e));
    }
  }

  async function resumeSession(repo: string, s: SessionInfo, name: string) {
    setActionError(null);
    try {
      const agent = await trust.start({ name, repo, resume: s.session_id });
      if (agent === null) return;
      navigate(`/session/${encodeURIComponent(name)}`);
    } catch (e) {
      setActionError(errText(e));
    }
  }

  function refresh() {
    void reposPoll.refresh();
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      {trust.dialog}
      <ErrorBar error={reposPoll.error ? `repos: ${reposPoll.error}` : null} />
      <ErrorBar error={actionError} />

      <AddRepoForm onAdded={refresh} />

      {reposPoll.data !== null && repos.length === 0 ? (
        <Empty mark="◦">
          No repositories remembered. Start a session or add a path above.
        </Empty>
      ) : (
        <div className="grid">
          {repos.map((r) => (
            <div key={r.path} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <RepoStrip
                repo={r}
                selected={selected === r.path}
                onSelect={() => setSelected(selected === r.path ? null : r.path)}
                onChanged={refresh}
                onForgotten={() => {
                  refresh();
                  if (selected === r.path) setSelected(null);
                }}
                onNewSession={(repo, name) => void newSession(repo, name)}
                onError={setActionError}
              />

              {selected === r.path && (
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
                  {sessionsError && (
                    <ErrorBar error={`sessions: ${sessionsError}`} />
                  )}
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
                      onResume={(sess, name) => void resumeSession(r.path, sess, name)}
                    />
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
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

export default function Library() {
  const reposPoll = usePoll(api.repos, 3000);
  const repos = reposPoll.data ?? [];
  const [section, setSection] = useState<Section>("repos");

  const tabs: { id: Section; label: string }[] = [
    { id: "repos", label: "Repositories" },
    { id: "skills", label: "Skills" },
  ];

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Library</span>
        <div className="class-select" role="tablist" aria-label="library section">
          {tabs.map((t) => {
            const active = section === t.id;
            return (
              <button
                key={t.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => setSection(t.id)}
                style={{
                  background: active ? "var(--bg-strip-2)" : "var(--bg-well)",
                  color: active ? "var(--text-hi)" : "var(--text-dim)",
                }}
              >
                {t.label}
              </button>
            );
          })}
        </div>
        <span style={{ flex: 1 }} />
        <span className="mono-meta">
          {repos.length} repo{repos.length === 1 ? "" : "s"} remembered
        </span>
      </div>
      <div className="stage-body">
        {section === "repos" ? (
          <RepositoriesSection reposPoll={reposPoll} />
        ) : (
          <SkillsSection repos={repos} />
        )}
      </div>
    </>
  );
}
