import { useCallback, useEffect, useMemo, useState } from "react";
import { api, ApiError, type SkillEntry } from "./../api";
import { useAppData } from "./../App";

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

function errText(e: unknown): string {
  return e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e);
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

interface EntryListProps {
  heading: string;
  entries: SkillEntry[];
  selectedRel: string | null;
  onSelect: (entry: SkillEntry) => void;
}

function EntryList({ heading, entries, selectedRel, onSelect }: EntryListProps) {
  return (
    <div className="skill-group">
      <div className="sidebar-heading">{heading}</div>
      {entries.length === 0 && <div className="sidebar-empty">none</div>}
      {entries.map((e) => (
        <button
          key={e.rel}
          type="button"
          className={`skill-entry${e.rel === selectedRel ? " selected" : ""}`}
          onClick={() => onSelect(e)}
          title={e.rel}
        >
          <span className="mono skill-entry-name">{e.name}</span>
          <span className={`chip chip-${e.kind}`}>{e.kind}</span>
          {e.description && <span className="skill-entry-desc">{e.description}</span>}
        </button>
      ))}
    </div>
  );
}

export default function Skills() {
  const { agents } = useAppData();

  // Distinct repo paths from the roster (remote agents carry no repo).
  const repoOptions = useMemo(() => {
    const set = new Set<string>();
    for (const a of agents) {
      if (a.repo) set.add(a.repo);
    }
    return [...set].sort();
  }, [agents]);

  const [repo, setRepo] = useState<string>(loadLastRepo);
  const [repoDraft, setRepoDraft] = useState<string>(repo);

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

  function chooseRepo(path: string) {
    const trimmed = path.trim();
    if (!trimmed || trimmed === repo) {
      setRepoDraft(repo);
      return;
    }
    if (dirty && !window.confirm("Discard unsaved changes?")) {
      setRepoDraft(repo);
      return;
    }
    setRepo(trimmed);
    setRepoDraft(trimmed);
    saveLastRepo(trimmed);
    setSelected(null);
    setContent("");
    setSavedContent("");
    setLoadError(null);
    setSaveError(null);
    setSaveNote(null);
    setConfirmDelete(false);
  }

  async function select(entry: SkillEntry) {
    if (entry.rel === selected?.rel) return;
    if (dirty && !window.confirm("Discard unsaved changes?")) return;
    setSelected(entry);
    setContent("");
    setSavedContent("");
    setLoadError(null);
    setSaveError(null);
    setSaveNote(null);
    setConfirmDelete(false);
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

  async function create(kind: "skill" | "command") {
    if (creating) return;
    if (dirty && !window.confirm("Discard unsaved changes?")) return;
    const label = kind === "skill" ? "skill" : "command";
    const name = window
      .prompt(`New ${label} name (lowercase, dashes ok):`)
      ?.trim()
      .replace(/\s+/g, "-");
    if (!name) return;
    const rel =
      kind === "skill" ? `.claude/skills/${name}/SKILL.md` : `.claude/commands/${name}.md`;
    const body = kind === "skill" ? skillTemplate(name) : commandTemplate(name);
    setCreating(true);
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

  const skills = (entries ?? []).filter((e) => e.kind === "skill");
  const commands = (entries ?? []).filter((e) => e.kind === "command");

  return (
    <div className="page page-skills">
      <header className="page-head">
        <h1>Skills</h1>
        <span className="dim">edit a repo's skills + commands; saves reload live sessions</span>
      </header>

      <div className="skill-repo-bar">
        <label>
          repo
          <select
            className="mono"
            value={repoOptions.includes(repo) ? repo : ""}
            onChange={(e) => {
              if (e.target.value) chooseRepo(e.target.value);
            }}
          >
            <option value="">— pick from roster —</option>
            {repoOptions.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
        </label>
        <label className="grow-label">
          or type a path
          <input
            className="mono grow"
            value={repoDraft}
            onChange={(e) => setRepoDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") chooseRepo(repoDraft);
            }}
            onBlur={() => chooseRepo(repoDraft)}
            placeholder="/home/you/src/project"
          />
        </label>
      </div>

      {listError && <div className="error-inline">skills: {listError}</div>}

      {!repo ? (
        <div className="empty">pick a repo above to browse its skills and commands.</div>
      ) : (
        <div className="skills-layout">
          <div className="skills-list panel">
            <div className="skills-list-actions">
              <button type="button" disabled={creating} onClick={() => void create("skill")}>
                + skill
              </button>
              <button type="button" disabled={creating} onClick={() => void create("command")}>
                + command
              </button>
            </div>
            {entries === null && !listError && <div className="sidebar-empty">loading…</div>}
            {entries !== null && entries.length === 0 && !listError && (
              <div className="empty">
                no skills yet — create one with <span className="mono">+ skill</span>.
              </div>
            )}
            {entries !== null && entries.length > 0 && (
              <>
                <EntryList
                  heading="skills"
                  entries={skills}
                  selectedRel={selected?.rel ?? null}
                  onSelect={(e) => void select(e)}
                />
                <EntryList
                  heading="commands"
                  entries={commands}
                  selectedRel={selected?.rel ?? null}
                  onSelect={(e) => void select(e)}
                />
              </>
            )}
          </div>

          <div className="skill-editor panel">
            {!selected ? (
              <div className="empty">select an entry to edit it.</div>
            ) : (
              <>
                <div className="skill-editor-head">
                  <span className="mono skill-rel" title={selected.rel}>
                    {selected.rel}
                  </span>
                  {dirty && <span className="chip chip-busy">unsaved</span>}
                </div>
                {loadError && <div className="error-inline">read: {loadError}</div>}
                <textarea
                  className="skill-editor-text mono"
                  value={content}
                  onChange={(e) => setContent(e.target.value)}
                  disabled={loading || loadError !== null}
                  placeholder={loading ? "loading…" : ""}
                  spellCheck={false}
                />
                <div className="skill-editor-actions">
                  <button
                    type="button"
                    onClick={() => void save()}
                    disabled={!dirty || saving || loading || loadError !== null}
                  >
                    {saving ? "saving…" : "save + reload"}
                  </button>
                  {!confirmDelete ? (
                    <button
                      type="button"
                      className="btn-deny"
                      disabled={deleting}
                      onClick={() => setConfirmDelete(true)}
                    >
                      delete…
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        className="btn-deny"
                        disabled={deleting}
                        onClick={() => void remove()}
                      >
                        {deleting ? "deleting…" : "really delete"}
                      </button>
                      <button
                        type="button"
                        className="btn-quiet"
                        onClick={() => setConfirmDelete(false)}
                      >
                        cancel
                      </button>
                    </>
                  )}
                  {saveError && <span className="error-text">{saveError}</span>}
                  {saveNote && <span className="ok-inline skill-save-note">{saveNote}</span>}
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
