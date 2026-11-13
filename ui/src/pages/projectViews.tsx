// A project's views beside its desk (PROPOSALS-2026-11-A §A-6): the team,
// the charter and the operator's notes, and the activity.
import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, type Agent, type Board, type ProjectActivityItem, type ProjectDecision, type ProjectFile, type ProjectFiles, type ProjectSearch, type SearchSession } from "../api";
import { Meter, presenceOf, relTime } from "../components";

export interface TeamMember {
  agent: string;
  /** The desk pane it sits in; absent for a benched member. */
  pane?: string;
  benched: boolean;
}

const bare = (n: string) => n.split("@")[0];

function lookup(agents: Agent[], name: string): Agent | undefined {
  const hit = agents.find((a) => a.name === name);
  if (hit) return hit;
  const p = name.split("@");
  return p.length === 3 ? agents.find((a) => a.name === `${p[0]}@${p[1]}` && !a.remote) : undefined;
}

/** The first line of an agent's charter: its role on the team. */
function roleOf(a: Agent | undefined): string | null {
  const c = a?.charter?.trim();
  if (c) return c.split("\n")[0]!.replace(/^#+\s*/, "").slice(0, 140);
  return a?.role ?? a?.title ?? null;
}

export function TeamView({
  members,
  agents,
  attention,
  readOnly,
  onBench,
  onBringBack,
  onRemove,
  onAdd,
}: {
  members: TeamMember[];
  agents: Agent[];
  attention: Set<string>;
  readOnly: boolean;
  onBench: (paneId: string) => void;
  onBringBack: (agent: string) => void;
  onRemove: (m: TeamMember) => void;
  onAdd: () => void;
}) {
  const [confirm, setConfirm] = useState<string | null>(null);
  const onDesk = members.filter((m) => !m.benched).length;
  return (
    <div className="project-team">
      {members.length > 0 && (
        <div className="project-team-head mono-meta">
          <span>{members.length} on the team · {onDesk} on the desk</span>
          <span style={{ flex: 1 }} />
          <span title="on the desk: shown as a pane; off: benched, still on the team (it keeps its briefings and the team thread)">desk</span>
        </div>
      )}
      {members.length === 0 && <div className="empty">no one on this project yet</div>}
      {members.map((m) => {
        const a = lookup(agents, m.agent);
        const state = m.benched ? "benched" : a?.live ? (a.turn_state === "busy" ? "busy" : "idle") : "stopped";
        return (
          <div key={m.agent} className={`project-member${m.benched ? " benched" : ""}`}>
            <Meter presence={presenceOf(a?.live ?? false, a?.turn_state ?? null)} />
            <Link className="mono project-member-name" to={`/session/${encodeURIComponent(m.agent)}`}>@{bare(m.agent)}</Link>
            {attention.has(m.agent) && <span className="rail-wait-pip" title="waiting on you" aria-label="waiting on you" />}
            <span className="project-member-role" title={a?.charter ?? undefined}>{roleOf(a) ?? <span className="dim">no charter</span>}</span>
            <span className="chip mono">{a?.node ?? m.agent.split("@")[2] ?? "this node"}</span>
            <span className="mono-meta project-member-state">{state}</span>
            <span className="mono-meta project-member-when">{a?.last_active ? relTime(a.last_active) : ""}</span>
            <label className="project-member-desk" title={m.benched ? "benched: on the team, not on the desk — check to put it back" : "on the desk — uncheck to bench it (still on the team)"}>
              <input
                type="checkbox"
                checked={!m.benched}
                disabled={readOnly || (!m.benched && !m.pane)}
                onChange={(e) => (e.target.checked ? onBringBack(m.agent) : m.pane && onBench(m.pane))}
                aria-label={`@${bare(m.agent)} on the desk`}
              />
            </label>
            {!readOnly && (
              <span className="project-member-verbs">
                {confirm === m.agent ? (
                  <>
                    <button type="button" className="btn sm" onClick={() => { setConfirm(null); onRemove(m); }}>remove from team</button>
                    <button type="button" className="btn ghost sm" onClick={() => setConfirm(null)}>keep</button>
                  </>
                ) : (
                  <button type="button" className="btn ghost sm" onClick={() => setConfirm(m.agent)} title="no longer on this project's team (told on its next turn)">remove…</button>
                )}
              </span>
            )}
          </div>
        );
      })}
      {!readOnly && (
        <div className="project-team-add">
          <button type="button" className="btn sm" onClick={onAdd}>+ add a member</button>
          <span className="mono-meta">an agent from anywhere on the mesh, or a new one</span>
        </div>
      )}
    </div>
  );
}

/** The charter (what every member reads) and the operator's notes (what
 *  only the operator reads), edited in place. */
export function CharterNotesView({ board, readOnly, onSave }: { board: Board; readOnly: boolean; onSave: (field: "charter" | "notes", value: string | null) => void }) {
  const [charter, setCharter] = useState(board.charter ?? "");
  const [notes, setNotes] = useState(board.notes ?? "");
  // A save elsewhere replaces what is shown, unless it is being edited.
  const [editing, setEditing] = useState<null | "charter" | "notes">(null);
  useEffect(() => {
    if (editing !== "charter") setCharter(board.charter ?? "");
    if (editing !== "notes") setNotes(board.notes ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [board.charter, board.notes]);
  const field = (k: "charter" | "notes", value: string, set: (v: string) => void, title: string, hint: string, placeholder: string) => {
    const saved = (k === "charter" ? board.charter : board.notes) ?? "";
    const dirty = value !== saved;
    return (
      <div className="project-text">
        <div className="project-text-head">
          <span className="label">{title}</span>
          <span className="mono-meta">{hint}</span>
          <span style={{ flex: 1 }} />
          {dirty && !readOnly && (
            <>
              <button type="button" className="btn ghost sm" onClick={() => { set(saved); setEditing(null); }}>discard</button>
              <button type="button" className="btn primary sm" onClick={() => { onSave(k, value.trim() ? value : null); setEditing(null); }}>save</button>
            </>
          )}
        </div>
        <textarea
          className="project-textarea"
          value={value}
          readOnly={readOnly}
          placeholder={placeholder}
          onFocus={() => setEditing(k)}
          onChange={(e) => set(e.target.value)}
          rows={k === "charter" ? 12 : 8}
        />
      </div>
    );
  };
  return (
    <div className="project-texts">
      {field("charter", charter, setCharter, "charter", "every member reads this at spawn; a change reaches them on their next turn (it never wakes them)", "What this project is for, how the team works, what done means. Each agent's own charter (its role) comes after it.")}
      {field("notes", notes, setNotes, "your notes", "only you: never sent to an agent", "Anything for your own reference: links, decisions, what to check next.")}
    </div>
  );
}

const EVENT_TEXT: Record<string, (d: Record<string, unknown>) => string> = {
  project_joined: (d) => `@${bare(String(d.agent ?? "?"))} joined the team`,
  project_left: (d) => `@${bare(String(d.agent ?? "?"))} left the team`,
  project_charter: (d) => (d.charter ? "the charter changed" : "the charter was cleared"),
  project_status: (d) => (d.to === "active" ? "resumed" : `${String(d.to)}`),
  project_renamed: (d) => `renamed from “${String(d.from)}”`,
  project_deleted: () => "deleted",
};

const who = (by: string) => (by === "operator" || by.startsWith("(") ? by : `@${bare(by)}`);

/** The decisions the team pinned, newest first; unpinned ones folded. */
function Decisions({ board, readOnly, onChanged }: { board: Board; readOnly: boolean; onChanged: () => void }) {
  const [showOld, setShowOld] = useState(false);
  const all: ProjectDecision[] = board.decisions ?? [];
  const pinned = all.filter((d) => d.pinned !== false).reverse();
  const old = all.filter((d) => d.pinned === false).reverse();
  if (all.length === 0) return null;
  const pin = (d: ProjectDecision, on: boolean) => void api.projectDecisionPin(board.id, d.id, on).then(onChanged).catch(() => {});
  return (
    <div className="project-decisions">
      <div className="project-text-head">
        <span className="label">decisions</span>
        <span className="mono-meta">pinned here and in every new member's briefing</span>
      </div>
      {pinned.map((d) => (
        <div key={d.id} className="project-decision">
          <span className="project-decision-text">{d.text}</span>
          <span className="mono-meta">{who(d.by)} · {relTime(d.at)}</span>
          {!readOnly && <button type="button" className="btn ghost sm" onClick={() => pin(d, false)} title="no longer pinned (kept below)">unpin</button>}
        </div>
      ))}
      {old.length > 0 && (
        <button type="button" className="link mono-meta" onClick={() => setShowOld(!showOld)}>
          {showOld ? "hide" : "show"} unpinned ({old.length})
        </button>
      )}
      {showOld &&
        old.map((d) => (
          <div key={d.id} className="project-decision old">
            <span className="project-decision-text">{d.text}</span>
            <span className="mono-meta">{who(d.by)} · {relTime(d.at)}</span>
            {!readOnly && <button type="button" className="btn ghost sm" onClick={() => pin(d, true)}>pin again</button>}
          </div>
        ))}
    </div>
  );
}

/** The project's timeline (events here, the team thread across the mesh),
 *  newest first, under its decisions and a box to post to the team. */
export function ActivityView({ board, readOnly, onChanged }: { board: Board; readOnly: boolean; onChanged: () => void }) {
  const id = board.id;
  const [items, setItems] = useState<ProjectActivityItem[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const load = useCallback(
    () =>
      api
        .projectActivity(id)
        .then((r) => (setItems(r.items), setErr(null)))
        .catch((e) => setErr(e instanceof Error ? e.message : String(e))),
    [id],
  );
  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 10_000);
    return () => window.clearInterval(t);
  }, [load]);
  const send = (kind: "message" | "decision") => {
    const body = draft.trim();
    if (!body) return;
    setSending(true);
    setNote(null);
    api
      .projectPost(id, body, kind)
      .then((r) => {
        setDraft("");
        setNote(r.notes.join("; "));
        void load();
        if (kind === "decision") onChanged();
      })
      .catch((e) => setNote(e instanceof Error ? e.message : String(e)))
      .finally(() => setSending(false));
  };
  const list = [...(items ?? [])].reverse();
  return (
    <div className="project-activity">
      <Decisions board={board} readOnly={readOnly} onChanged={onChanged} />
      {!readOnly && (
        <div className="project-compose">
          <textarea
            className="project-textarea"
            rows={3}
            value={draft}
            placeholder="To the whole team, on the project's thread (an idle member wakes to it)."
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                send("message");
              }
            }}
          />
          <div className="project-compose-row">
            <span className="mono-meta">{note}</span>
            <span style={{ flex: 1 }} />
            <button type="button" className="btn ghost sm" disabled={sending || !draft.trim()} onClick={() => send("decision")} title="post it and pin it as a decision">
              post as a decision
            </button>
            <button type="button" className="btn primary sm" disabled={sending || !draft.trim()} onClick={() => send("message")} title="ctrl+enter">
              post to the team
            </button>
          </div>
        </div>
      )}
      {err && <div className="error-text mono-meta">{err}</div>}
      {!items && !err && <div className="dim">loading…</div>}
      {items && list.length === 0 && <div className="empty">nothing yet: joins, the charter, status changes and the team's messages show here</div>}
      {list.map((it, i) => (
        <div key={i} className={`project-act ${it.kind}`}>
          <span className="mono-meta project-act-when" title={new Date(it.ts * 1000).toLocaleString()}>{relTime(it.ts)}</span>
          {it.kind === "event" ? (
            <span className="project-act-body">
              {(EVENT_TEXT[it.event] ?? ((d) => `${it.event} ${JSON.stringify(d)}`))(it.detail)}
              {it.event === "project_charter" && typeof it.detail.charter === "string" && (
                <>
                  {" "}
                  <button type="button" className="link mono-meta" onClick={() => setOpen(open === i ? null : i)}>{open === i ? "hide" : "show"}</button>
                  {open === i && <pre className="project-act-pre">{it.detail.charter}</pre>}
                </>
              )}
            </span>
          ) : (
            <span className="project-act-body">
              <span className="mono">{who(it.message.sender)}</span>
              <span className="mono-meta"> → {it.message.to_display ?? it.message.recipient}</span>
              <span className="project-act-msg">{it.message.body.length > 400 ? `${it.message.body.slice(0, 400)}…` : it.message.body}</span>
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

const size = (n: number) => (n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / (1024 * 1024)).toFixed(1)} MB`);

function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** One file opened: text shown (and edited), an image shown, anything
 *  else offered as a download. */
function FilePreview({ board, file, version, readOnly, onSaved }: { board: Board; file: ProjectFile; version: string | null; readOnly: boolean; onSaved: () => void }) {
  const [text, setText] = useState<string | null>(null);
  const [img, setImg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [edit, setEdit] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let url: string | null = null;
    let stop = false;
    setText(null);
    setImg(null);
    setErr(null);
    setEdit(null);
    api
      .projectFileBlob(board.id, file.path, version ?? undefined)
      .then(async (b) => {
        if (stop) return;
        if (b.type.startsWith("image/")) {
          url = URL.createObjectURL(b);
          setImg(url);
        } else if (b.type.startsWith("text/") || b.type.startsWith("application/json")) {
          setText(await b.text());
        } else setErr(`binary (${size(b.size)}): download it to open`);
      })
      .catch((e) => !stop && setErr(e instanceof Error ? e.message : String(e)));
    return () => {
      stop = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [board.id, file.path, file.sha, version]);
  const save = () => {
    if (edit === null) return;
    setSaving(true);
    api
      .projectFileUpload(board.id, file.path, new Blob([edit], { type: "text/plain" }))
      .then(() => {
        setEdit(null);
        onSaved();
      })
      .catch((e) => setErr(e instanceof Error ? e.message : String(e)))
      .finally(() => setSaving(false));
  };
  return (
    <div className="project-file-preview">
      {version && <div className="mono-meta">an earlier version ({version.slice(0, 10)})</div>}
      {err && <div className="mono-meta">{err}</div>}
      {img && <img src={img} alt={file.path} className="project-file-img" />}
      {text !== null && edit === null && (
        <>
          <pre className="project-act-pre project-file-text">{text}</pre>
          {!readOnly && !version && (
            <button type="button" className="btn ghost sm" onClick={() => setEdit(text)}>
              edit
            </button>
          )}
        </>
      )}
      {edit !== null && (
        <>
          <textarea className="project-textarea mono" rows={16} value={edit} onChange={(e) => setEdit(e.target.value)} />
          <div className="project-compose-row">
            <span style={{ flex: 1 }} />
            <button type="button" className="btn ghost sm" onClick={() => setEdit(null)}>discard</button>
            <button type="button" className="btn primary sm" disabled={saving || edit === text} onClick={save}>save</button>
          </div>
        </>
      )}
    </div>
  );
}

/** The project's shared files (§5): every member's node keeps them, and
 *  each member's repo links them at `.aspen/projects/<slug>/`. */
export function FilesView({ board, readOnly }: { board: Board; readOnly: boolean }) {
  const [data, setData] = useState<ProjectFiles | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState<{ path: string; version: string | null; history: boolean } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [newPath, setNewPath] = useState<string | null>(null);
  const [drag, setDrag] = useState(false);
  const input = useRef<HTMLInputElement | null>(null);
  const load = useCallback(
    () =>
      api
        .projectFiles(board.id)
        .then((d) => (setData(d), setErr(null)))
        .catch((e) => setErr(e instanceof Error ? e.message : String(e))),
    [board.id],
  );
  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 10_000);
    return () => window.clearInterval(t);
  }, [load]);
  const upload = async (files: FileList | File[]) => {
    for (const f of Array.from(files)) {
      setBusy(`uploading ${f.name}…`);
      try {
        await api.projectFileUpload(board.id, f.name, f);
      } catch (e) {
        setErr(`${f.name}: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    setBusy(null);
    void load();
  };
  const files = [...(data?.files ?? [])].sort((a, b) => a.path.localeCompare(b.path));
  return (
    <div
      className={`project-files${drag ? " drag" : ""}`}
      onDragOver={(e) => {
        if (readOnly) return;
        e.preventDefault();
        setDrag(true);
      }}
      onDragLeave={() => setDrag(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDrag(false);
        if (!readOnly && e.dataTransfer.files.length) void upload(e.dataTransfer.files);
      }}
    >
      <div className="project-text-head">
        <span className="label">files</span>
        <span className="mono-meta">
          {data ? `${files.length} · ${size(data.total)} of ${size(data.limit)}` : ""}
          {data?.folder ? ` · here at ${data.folder}` : ""}
        </span>
        <span style={{ flex: 1 }} />
        {busy && <span className="mono-meta">{busy}</span>}
        {!readOnly && (
          <>
            <button type="button" className="btn ghost sm" onClick={() => setNewPath(newPath === null ? "notes.md" : null)}>new text file</button>
            <button type="button" className="btn sm" onClick={() => input.current?.click()}>upload</button>
            <input ref={input} type="file" multiple hidden onChange={(e) => e.target.files && void upload(e.target.files)} />
          </>
        )}
      </div>
      <div className="mono-meta project-files-hint">
        Every member's node keeps these; each member finds them in its repo at <span className="mono">.aspen/projects/…</span> (kept out of git), and what anyone changes there is shared within seconds.
      </div>
      {newPath !== null && (
        <form
          className="project-compose-row"
          onSubmit={(e) => {
            e.preventDefault();
            const p = newPath.trim();
            if (!p) return;
            void api
              .projectFileUpload(board.id, p, new Blob([""], { type: "text/plain" }))
              .then(() => {
                setNewPath(null);
                setOpen({ path: p, version: null, history: false });
                return load();
              })
              .catch((er) => setErr(er instanceof Error ? er.message : String(er)));
          }}
        >
          <input className="input mono" value={newPath} onChange={(e) => setNewPath(e.target.value)} placeholder="specs/api.md" autoFocus />
          <button type="submit" className="btn primary sm">create</button>
          <button type="button" className="btn ghost sm" onClick={() => setNewPath(null)}>cancel</button>
        </form>
      )}
      {err && <div className="error-text mono-meta">{err}</div>}
      {data && files.length === 0 && <div className="empty">no files yet: upload or drop some here, or a member writes one</div>}
      {files.map((f) => {
        const isOpen = open?.path === f.path;
        return (
          <div key={f.path} className={`project-file${isOpen ? " open" : ""}`}>
            <div className="project-file-row">
              <button type="button" className="link mono project-file-path" onClick={() => setOpen(isOpen ? null : { path: f.path, version: null, history: false })}>
                {f.path}
              </button>
              {f.conflict && <span className="chip mono" title="written apart from the version at its own name; both kept, nothing lost">both kept</span>}
              {f.here === false && <span className="mono-meta" title={`on ${f.node}; fetched when opened`}>not here yet</span>}
              <span className="mono-meta">{size(f.size)}</span>
              <span className="mono-meta">{who(f.by)}</span>
              <span className="mono-meta" title={new Date(f.at * 1000).toLocaleString()}>{relTime(f.at)}</span>
              <span style={{ flex: 1 }} />
              <button type="button" className="btn ghost sm" onClick={() => void api.projectFileBlob(board.id, f.path).then((b) => download(b, f.path.split("/").pop() ?? "file")).catch((e) => setErr(String(e)))}>download</button>
              {f.history.length > 0 && (
                <button type="button" className="btn ghost sm" onClick={() => setOpen({ path: f.path, version: null, history: !(isOpen && open?.history) })}>
                  history · {f.history.length}
                </button>
              )}
              {!readOnly &&
                (confirm === f.path ? (
                  <>
                    <button type="button" className="btn sm" onClick={() => void api.projectFileDelete(board.id, f.path).then(() => (setConfirm(null), load())).catch((e) => setErr(String(e)))}>delete</button>
                    <button type="button" className="btn ghost sm" onClick={() => setConfirm(null)}>keep</button>
                  </>
                ) : (
                  <button type="button" className="btn ghost sm" onClick={() => setConfirm(f.path)} title="every member's copy goes; its history stays for restore">delete…</button>
                ))}
            </div>
            {isOpen && open?.history && (
              <div className="project-file-history">
                {f.history.map((h) => (
                  <div key={h.sha + h.at} className="project-file-row">
                    <button type="button" className="link mono-meta" onClick={() => setOpen({ path: f.path, version: h.sha, history: true })}>
                      {new Date(h.at * 1000).toLocaleString()}
                    </button>
                    <span className="mono-meta">{size(h.size)}</span>
                    <span className="mono-meta">{who(h.by)}</span>
                    <span style={{ flex: 1 }} />
                    {!readOnly && (
                      <button type="button" className="btn ghost sm" onClick={() => void api.projectFileRestore(board.id, f.path, h.sha).then(() => (setOpen({ path: f.path, version: null, history: false }), load())).catch((e) => setErr(String(e)))}>
                        restore
                      </button>
                    )}
                  </div>
                ))}
              </div>
            )}
            {isOpen && <FilePreview board={board} file={f} version={open?.version ?? null} readOnly={readOnly} onSaved={() => void load()} />}
          </div>
        );
      })}
    </div>
  );
}

/** Search within the project: its members' transcripts, its files and its
 *  team thread (A-8). */
export function SearchView({ board, members }: { board: Board; members: string[] }) {
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [res, setRes] = useState<{ q: string; mine: ProjectSearch; sessions: SearchSession[]; self: string } | null>(null);
  const run = (e: React.FormEvent) => {
    e.preventDefault();
    const needle = q.trim();
    if (needle.length < 2) return;
    setBusy(true);
    setErr(null);
    // A member is `bare@repo@node` (or `bare@repo` on this node).
    const keys = new Set(members);
    Promise.all([api.projectSearch(board.id, needle), api.search(needle, { limit: 200 })])
      .then(([mine, all]) => {
        const sessions = all.sessions.filter((s) => s.agent && (keys.has(`${s.agent}@${s.node}`) || keys.has(s.agent)));
        setRes({ q: needle, mine, sessions, self: all.self });
      })
      .catch((er) => setErr(er instanceof Error ? er.message : String(er)))
      .finally(() => setBusy(false));
  };
  const nothing = res && res.mine.files.length === 0 && res.mine.messages.length === 0 && res.sessions.length === 0;
  return (
    <div className="project-search">
      <form className="project-compose-row" onSubmit={run}>
        <input className="input" value={q} onChange={(e) => setQ(e.target.value)} placeholder="search the members' transcripts, the files and the team thread" autoFocus />
        <button type="submit" className="btn primary sm" disabled={busy || q.trim().length < 2}>{busy ? "searching…" : "search"}</button>
      </form>
      {err && <div className="error-text mono-meta">{err}</div>}
      {nothing && <div className="empty">nothing in this project mentions “{res.q}”</div>}
      {res && res.mine.files.length > 0 && (
        <div className="project-search-group">
          <div className="label">files</div>
          {res.mine.files.map((f) => (
            <div key={f.path} className="project-search-hit">
              <span className="mono">{f.path}</span>
              {f.lines.map((l) => (
                <div key={l.n} className="mono-meta">
                  {l.n}: {l.text}
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
      {res && res.mine.messages.length > 0 && (
        <div className="project-search-group">
          <div className="label">team thread</div>
          {res.mine.messages.map((m, i) => (
            <div key={i} className="project-search-hit">
              <span className="mono">{who(m.sender)}</span> <span className="mono-meta">{relTime(m.created_at)}</span>
              <div>{m.body.length > 300 ? `${m.body.slice(0, 300)}…` : m.body}</div>
            </div>
          ))}
        </div>
      )}
      {res && res.sessions.length > 0 && (
        <div className="project-search-group">
          <div className="label">transcripts</div>
          {res.sessions.map((s) => {
            const name = s.node === res.self ? s.agent! : `${s.agent}@${s.node}`;
            return (
              <div key={`${s.node}:${s.session_id}`} className="project-search-hit">
                <Link className="mono" to={`/session/${encodeURIComponent(name)}`}>@{bare(s.agent!)}</Link> <span className="mono-meta">{s.title ?? ""}</span>
                {s.hits.slice(0, 3).map((h, i) => (
                  <Link key={i} className="project-search-line" to={`/session/${encodeURIComponent(name)}${h.uuid ? `?at=${encodeURIComponent(h.uuid)}` : ""}`}>
                    {h.snippet.before}
                    <mark>{h.snippet.match}</mark>
                    {h.snippet.after}
                  </Link>
                ))}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
