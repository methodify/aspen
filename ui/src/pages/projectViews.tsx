// A project's views beside its desk (PROPOSALS-2026-11-A §A-6): the team,
// the charter and the operator's notes, and the activity.
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, type Agent, type Board, type ProjectActivityItem } from "../api";
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
  return a?.title ?? null;
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
  return (
    <div className="project-team">
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
            {!readOnly && (
              <span className="project-member-verbs">
                {m.benched ? (
                  <button type="button" className="btn ghost sm" onClick={() => onBringBack(m.agent)} title="back onto the desk">bring back</button>
                ) : (
                  m.pane && <button type="button" className="btn ghost sm" onClick={() => onBench(m.pane!)} title="off the desk for now, still on the team">bench</button>
                )}
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

/** The project's timeline on this node, newest first. */
export function ActivityView({ id }: { id: string }) {
  const [items, setItems] = useState<ProjectActivityItem[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  useEffect(() => {
    let stop = false;
    const load = () =>
      api
        .projectActivity(id)
        .then((r) => !stop && (setItems(r.items), setErr(null)))
        .catch((e) => !stop && setErr(e instanceof Error ? e.message : String(e)));
    void load();
    const t = window.setInterval(load, 10_000);
    return () => {
      stop = true;
      window.clearInterval(t);
    };
  }, [id]);
  if (err) return <div className="error-text mono-meta">{err}</div>;
  if (!items) return <div className="dim">loading…</div>;
  const list = [...items].reverse();
  return (
    <div className="project-activity">
      {list.length === 0 && <div className="empty">nothing yet: joins, the charter, status changes and the team's messages show here</div>}
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
              <span className="mono">@{bare(it.message.sender)}</span>
              <span className="mono-meta"> → {it.message.to_display ?? it.message.recipient}</span>
              <span className="project-act-msg">{it.message.body.length > 400 ? `${it.message.body.slice(0, 400)}…` : it.message.body}</span>
            </span>
          )}
        </div>
      ))}
    </div>
  );
}
