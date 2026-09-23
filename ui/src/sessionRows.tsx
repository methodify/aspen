// The Mesh list's session rows and the session's own "transcripts" panel
// share these (PROPOSALS-2026-09-J §4.3, §4.5): one row per transcript,
// led by the name on it and its state, one primary verb, the rest in ⋯.
import { useEffect, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { api, type SessionInfo } from "./api";
import { relTime } from "./components";
import { suggestBranchName } from "./branchCard";
import { useTrustedStart } from "./trust";

/** Derive a valid agent name from a session title. */
export function slugify(title: string | null): string {
  const base = (title ?? "")
    .trim()
    .replace(/[^A-Za-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return base || "session";
}

/** How the console addresses an agent on `node`: the local key on this
 *  node, `key@node` on a peer. */
export function agentAddr(key: string, node: string, isSelf: (n: string) => boolean): string {
  return isSelf(node) ? key : `${key}@${node}`;
}

/** What a session row leads with: the name on the transcript and whether
 *  it is that name's current transcript or one it left, or where it was
 *  branched from, or "no name" (PROPOSALS-2026-09-J §4.1). */
export function sessionLead(s: SessionInfo): ReactNode {
  const bare = (k: string) => k.split("@")[0];
  if (s.agent && s.state === "current") {
    const more = (s.agents ?? []).slice(1);
    return (
      <>
        <span className="mono sess-name">@{bare(s.agent)}{more.map((m) => `, @${bare(m)}`).join("")}</span>
        <span className="sess-state">current{s.agent_live ? " · running" : ""}{s.branch_of && bare(s.branch_of) !== bare(s.agent) ? <> · branched from <span className="mono">@{bare(s.branch_of)}</span></> : null}</span>
        {more.length > 0 && <span className="chip mono" title="more than one name points at this transcript; pick one to keep and move the others" style={{ color: "var(--sig-gate)" }}>two names</span>}
      </>
    );
  }
  if (s.agent && s.state === "earlier") {
    return (
      <>
        <span className="mono sess-name">@{bare(s.agent)}</span>
        <span className="sess-state">earlier{s.label ? ` · “${s.label}”` : ""}</span>
      </>
    );
  }
  if (s.branch_of) {
    return (
      <>
        <span className="sess-state">branch of <span className="mono">@{bare(s.branch_of)}</span> · no name</span>
      </>
    );
  }
  return <span className="sess-state">no name</span>;
}

export type SessionVerbs = {
  node: string;
  isSelf: (n: string) => boolean;
  /** Every name in this repo (bare), for "move @x here". */
  names: string[];
  onResumeNew: (s: SessionInfo, name: string) => void;
  onOpen: (addr: string) => void;
  onRevive: (addr: string) => void;
  onMove: (agentKey: string, s: SessionInfo) => void;
  onIgnore: (s: SessionInfo) => void;
  onForget: (s: SessionInfo) => void;
};

/** One transcript. One primary verb; the rest behind ⋯. `resume…` opens a
 *  chooser of the branch card's shape: a new name, or move a name here. */
export function SessionRow({ session: s, v, indent }: { session: SessionInfo; v: SessionVerbs; indent?: boolean }) {
  const [chooser, setChooser] = useState(false);
  const [more, setMore] = useState(false);
  const [newName, setNewName] = useState("");
  const [moveName, setMoveName] = useState("");
  const bare = (k: string) => k.split("@")[0];
  const addr = s.agent ? agentAddr(s.agent, v.node, v.isSelf) : null;
  const isCurrent = !!s.agent && s.state === "current";
  // Who can be moved here: every name in the repo; an earlier row's own
  // name first (moving it back), a branch's parent name first.
  const own = s.state === "earlier" && s.agent ? bare(s.agent) : s.branch_of ? bare(s.branch_of) : null;
  const moveCandidates = isCurrent ? [] : [...new Set([...(own ? [own] : []), ...v.names])];
  const defaultMove = moveCandidates[0] ?? "";
  function openChooser() {
    setNewName(own ? suggestBranchName(own, v.names) : slugify(s.mcc_name ?? s.title));
    setMoveName(defaultMove);
    setChooser(true);
    setMore(false);
  }
  const title = s.mcc_name || s.title;
  const hasMore = !!s.adoption_id || (s.state === "earlier" && !!s.bookmark_id);
  return (
    <div className={`strip flat sess-row${indent ? " sess-indent" : ""}`}>
      <div className="sess-main">
        <span className="sess-lead">{sessionLead(s)}</span>
        <span style={{ flex: 1 }} />
        <span className="sess-meta">
          {s.harness && s.harness !== "claude" && <span className="chip mono harness-chip" title={`a ${s.harness} session`}>{s.harness}</span>}
          {s.entrypoint && s.entrypoint !== "aspen" && <span className="chip mono" title="which program wrote it">{s.entrypoint}</span>}
          <span className="mono-meta">{s.user_messages} msg{s.user_messages === 1 ? "" : "s"}</span>
          <span className="mono-meta" title={new Date(s.modified * 1000).toLocaleString()}>{relTime(s.modified)}</span>
          <span className="mono-meta" title={s.session_id}>{s.session_id.slice(0, 8)}</span>
        </span>
        {!chooser && isCurrent && addr && (s.agent_live ? (
          <button type="button" className="btn sm" onClick={() => v.onOpen(addr)}>open</button>
        ) : (
          <button type="button" className="btn sm" onClick={() => v.onRevive(addr)} title="start the name again on this transcript">revive</button>
        ))}
        {!chooser && !isCurrent && (
          <button type="button" className="btn sm" onClick={openChooser}>resume…</button>
        )}
        {hasMore && (
          <button type="button" className="btn ghost sm" aria-label="more" title="more" onClick={() => setMore((m) => !m)}>⋯</button>
        )}
      </div>
      {(s.pending?.length ?? 0) > 0 && (
        <div className="sess-pending mono-meta" title="branches of this transcript that have not taken a turn — open one and send something to start its transcript; stopping one loses nothing">
          waiting for a first turn: {s.pending!.map((p, i) => (
            <span key={p}>{i > 0 ? ", " : ""}<button type="button" className="link mono" onClick={() => v.onOpen(agentAddr(p, v.node, v.isSelf))}>@{bare(p)}</button></span>
          ))}
        </div>
      )}
      {title && <div className="sess-title mono-meta" title={s.title ?? undefined}>{title}{s.mcc_name && s.title && s.title !== s.mcc_name ? <span className="dim"> · {s.title}</span> : null}{s.mcc_args ? <span className="dim"> · {s.mcc_args}</span> : null}</div>}
      {more && (
        <div className="sess-more">
          {s.adoption_id && <button type="button" className="btn ghost sm" onClick={() => { setMore(false); v.onIgnore(s); }} title="stop asking about this branch; it stays listed as no name">ignore this branch</button>}
          {s.state === "earlier" && s.bookmark_id && <button type="button" className="btn ghost sm" onClick={() => { setMore(false); v.onForget(s); }} title="drop the earlier point; the transcript stays on disk as no name">forget this earlier point</button>}
        </div>
      )}
      {chooser && (
        <form
          className="sess-chooser"
          onSubmit={(e) => {
            e.preventDefault();
          }}
        >
          <label className="sess-choice">
            <span className="sess-choice-head">as a new agent</span>
            <span className="sess-choice-field">
              <span className="mono-meta">@</span>
              <input className="mono" value={newName} onChange={(e) => setNewName(e.target.value)} autoFocus aria-label="new agent name" placeholder="name" />
              <button type="button" className="btn primary sm" disabled={!newName.trim()} onClick={() => { setChooser(false); v.onResumeNew(s, newName.trim()); }}>
                start @{newName.trim() || "…"}
              </button>
            </span>
          </label>
          {moveCandidates.length > 0 && (
            <label className="sess-choice">
              <span className="sess-choice-head">move a name here <span className="dim">(restarts it on a branch of this transcript; where it was is kept as an earlier point)</span></span>
              <span className="sess-choice-field">
                <select value={moveName} onChange={(e) => setMoveName(e.target.value)} aria-label="which name to move here">
                  {moveCandidates.map((n) => <option key={n} value={n}>@{n}</option>)}
                </select>
                <button type="button" className="btn sm" disabled={!moveName} onClick={() => { setChooser(false); v.onMove(moveName, s); }}>
                  move @{moveName || "…"} here
                </button>
              </span>
            </label>
          )}
          <button type="button" className="btn ghost sm" onClick={() => setChooser(false)}>cancel</button>
        </form>
      )}
    </div>
  );
}

/** A repo's transcripts grouped by name: each name's current transcript
 *  with its earlier ones under it, then branches nobody named, then the
 *  rest folded into one "no name" row (PROPOSALS-2026-09-J §4.3). */
export function SessionList({ sessions, v }: { sessions: SessionInfo[]; v: SessionVerbs }) {
  const [showUnnamed, setShowUnnamed] = useState(false);
  const byName = new Map<string, { current: SessionInfo[]; earlier: SessionInfo[] }>();
  const branches: SessionInfo[] = [];
  const unnamed: SessionInfo[] = [];
  for (const s of sessions) {
    if (s.agent) {
      const g = byName.get(s.agent) ?? { current: [], earlier: [] };
      (s.state === "current" ? g.current : g.earlier).push(s);
      byName.set(s.agent, g);
    } else if (s.branch_of) branches.push(s);
    else unnamed.push(s);
  }
  const groups = [...byName.entries()].sort((a, b) => {
    const am = Math.max(...[...a[1].current, ...a[1].earlier].map((x) => x.modified));
    const bm = Math.max(...[...b[1].current, ...b[1].earlier].map((x) => x.modified));
    return bm - am;
  });
  return (
    <>
      {groups.map(([agent, g]) => (
        <div key={agent} className="sess-group">
          {g.current.map((s) => <SessionRow key={s.session_id} session={s} v={v} />)}
          {g.earlier.map((s) => <SessionRow key={s.session_id} session={s} v={v} indent={g.current.length > 0} />)}
        </div>
      ))}
      {branches.map((s) => <SessionRow key={s.session_id} session={s} v={v} />)}
      {unnamed.length > 0 && (
        <div className="sess-group">
          <button type="button" className="btn ghost sm sess-unnamed-toggle" onClick={() => setShowUnnamed((x) => !x)} aria-expanded={showUnnamed}>
            {showUnnamed ? "▾" : "▸"} no name · {unnamed.length}
          </button>
          {showUnnamed && unnamed.map((s) => <SessionRow key={s.session_id} session={s} v={v} />)}
        </div>
      )}
    </>
  );
}


/** The session's own transcripts (S-5): the name's current transcript,
 *  the ones it left, and branches of them nobody named — the Mesh row's
 *  format and verbs, inside the ⋯ menu's history drawer. */
export function TranscriptsPanel({ name, repo, node, selfNode, onError }: { name: string; repo: string | null; node: string; selfNode: string; onError: (e: string) => void }) {
  const nav = useNavigate();
  const trust = useTrustedStart();
  const [rows, setRows] = useState<SessionInfo[] | null>(null);
  const [gen, setGen] = useState(0);
  const isSelf = (n: string) => n === selfNode || n === "";
  const key = name.split("@").length > 2 ? name.split("@").slice(0, 2).join("@") : name;
  useEffect(() => {
    if (!repo) {
      setRows([]);
      return;
    }
    let dead = false;
    api
      .sessions(repo, isSelf(node) ? undefined : node)
      .then((list) => {
        if (dead) return;
        const bare = (k: string) => k.split("@")[0];
        const mine = list.filter((s) => (s.agent && s.agent === key) || (s.branch_of && bare(s.branch_of) === bare(key) && !s.agent));
        setRows(mine.sort((a, b) => (a.state === "current" ? -1 : b.state === "current" ? 1 : b.modified - a.modified)));
      })
      .catch((e: unknown) => {
        if (!dead) onError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      dead = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repo, node, key, gen]);
  const reload = () => setGen((g) => g + 1);
  const fail = (e: unknown) => onError(e instanceof Error ? e.message : String(e));
  const addrOf = (k: string) => agentAddr(k, node, isSelf);
  if (rows === null) return <div className="dim">loading…</div>;
  if (rows.length === 0) return <div className="empty" style={{ padding: "12px 0" }}>only this transcript so far — branch to leave an earlier point</div>;
  return (
    <SessionList
      sessions={rows}
      v={{
        node,
        isSelf,
        names: [...new Set(rows.filter((x) => x.agent).map((x) => x.agent!.split("@")[0]))],
        onResumeNew: (s, newName) => {
          void (async () => {
            try {
              const a = await trust.start({ name: newName, repo: repo!, resume: s.session_id, node: isSelf(node) ? undefined : node, ...(s.harness ? { harness: s.harness } : {}) });
              if (a) nav(`/session/${encodeURIComponent(a.name)}`);
            } catch (e) {
              fail(e);
            }
          })();
        },
        onOpen: (addr) => nav(`/session/${encodeURIComponent(addr)}`),
        onRevive: (addr) => {
          void api.revive(addr).then(() => reload()).catch(fail);
        },
        onMove: (agentKey, s) => {
          const full = rows.find((x) => x.agent && x.agent.split("@")[0] === agentKey)?.agent ?? `${agentKey}@${key.split("@")[1] ?? ""}`;
          void api.moveTo(addrOf(full), s.session_id).then((a) => nav(`/session/${encodeURIComponent(a.name)}`)).catch(fail);
        },
        onIgnore: (s) => {
          if (!s.adoption_id) return;
          void api.resolveAdoption(s.adoption_id, "ignore", undefined, isSelf(node) ? undefined : node).then(reload).catch(fail);
        },
        onForget: (s) => {
          if (!s.agent || !s.bookmark_id) return;
          void api.deleteBookmark(addrOf(s.agent), s.bookmark_id).then(reload).catch(fail);
        },
      }}
    />
  );
}
