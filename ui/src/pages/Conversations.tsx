// Conversations — channels as a first-class place. A channel rail (auto
// #repo, custom, cross-node) → a thread with inline delivery/ingest ticks →
// a context pane (members, add/remove, route). New Channel mixes agents,
// repos, and nodes. Route… fans a message across channels.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { api, type Channel, type ChannelPost, type Urgency } from "../api";
import { usePoll } from "../hooks";
import { useAppData } from "../App";
import { ClassBadge, ClassSelect, Empty, ErrorBar, MessageRow, relTime } from "../components";

export default function Conversations() {
  const { channel } = useParams();
  const nav = useNavigate();
  const { agents } = useAppData();
  const channelsPoll = usePoll<Channel[]>(api.channels, 3000);
  const channels = useMemo(() => channelsPoll.data ?? [], [channelsPoll.data]);
  const [newOpen, setNewOpen] = useState(false);

  const selected = channel ?? channels[0]?.name ?? null;
  const current = channels.find((c) => c.name === selected) ?? null;

  const repoChannels = channels.filter((c) => c.kind === "repo");
  const customChannels = channels.filter((c) => c.kind === "custom");

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Conversations</span>
        {current && <span className="mono-meta">#{current.name}</span>}
        <span style={{ flex: 1 }} />
        <button className="btn primary sm" onClick={() => setNewOpen(true)}>+ new channel</button>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "220px 1fr 260px", height: "calc(100% - 61px)", minHeight: 0 }}>
        {/* Channel rail */}
        <div style={{ borderRight: "1px solid var(--line)", overflow: "auto", padding: "12px 0" }}>
          <div className="nav-section label">Repos</div>
          {repoChannels.length === 0 && <div style={{ padding: "2px 16px" }} className="mono-meta">none</div>}
          {repoChannels.map((c) => (
            <ChannelLink key={c.name} c={c} active={c.name === selected} onClick={() => nav(`/conversations/${encodeURIComponent(c.name)}`)} />
          ))}
          <div className="nav-section label" style={{ marginTop: 12 }}>Custom</div>
          {customChannels.length === 0 && <div style={{ padding: "2px 16px" }} className="mono-meta">none yet</div>}
          {customChannels.map((c) => (
            <ChannelLink key={c.name} c={c} active={c.name === selected} onClick={() => nav(`/conversations/${encodeURIComponent(c.name)}`)} />
          ))}
        </div>

        {/* Thread */}
        {selected ? (
          <Thread key={selected} name={selected} allChannels={channels} />
        ) : (
          <Empty mark="#">No channels yet. Every repo with a running session forms one automatically, or create a custom channel.</Empty>
        )}

        {/* Context */}
        {current ? (
          <ContextPane channel={current} onChanged={channelsPoll.refresh} />
        ) : (
          <div style={{ borderLeft: "1px solid var(--line)" }} />
        )}
      </div>

      {newOpen && (
        <NewChannelDialog
          agents={agents.map((a) => a.name)}
          onClose={() => setNewOpen(false)}
          onCreated={(name) => {
            setNewOpen(false);
            channelsPoll.refresh();
            nav(`/conversations/${encodeURIComponent(name)}`);
          }}
        />
      )}
    </>
  );
}

function ChannelLink({ c, active, onClick }: { c: Channel; active: boolean; onClick: () => void }) {
  return (
    <div className={`nav-item${active ? " active" : ""}`} onClick={onClick} role="button" tabIndex={0} onKeyDown={(e) => e.key === "Enter" && onClick()}>
      <span className="mono">#{c.name}</span>
      <span style={{ flex: 1 }} />
      <span className="mono-meta">{c.members.length}</span>
    </div>
  );
}

function Thread({ name, allChannels }: { name: string; allChannels: Channel[] }) {
  const load = useMemo(() => () => api.channelLog(name, 100), [name]);
  const poll = usePoll<ChannelPost[]>(load, 2500);
  const posts = poll.data ?? [];
  const [body, setBody] = useState("");
  const [cls, setCls] = useState<Urgency>("normal");
  const [err, setErr] = useState<string | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [routePost, setRoutePost] = useState<ChannelPost | null>(null);

  async function send() {
    if (!body.trim()) return;
    setErr(null);
    try {
      const res = await api.busSend({ to: `#${name}`, body: body.trim(), urgency: cls });
      setNotes(res.notes);
      setBody("");
      poll.refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "send failed");
    }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", minHeight: 0 }}>
      <div style={{ flex: 1, overflow: "auto", padding: "16px 20px" }}>
        <ErrorBar error={poll.error} />
        {posts.length === 0 ? (
          <Empty mark="◇">No messages in #{name} yet. Send the first below.</Empty>
        ) : (
          posts.map((p) => (
            <MessageRow
              key={p.post}
              urgency={p.urgency}
              sender={p.sender}
              meta={
                <span className="ticks" title={`${p.delivered}/${p.recipients} delivered, ${p.ingested} ingested`}>
                  <span>{p.delivered}/{p.recipients} deliv</span>
                  {p.ingested > 0 && <b>· {p.ingested} ✓ read</b>}
                  <span className="mono-meta">· {relTime(p.created_at)}</span>
                </span>
              }
              actions={<button className="btn ghost sm" onClick={() => setRoutePost(p)}>route…</button>}
            >
              {p.body}
            </MessageRow>
          ))
        )}
      </div>

      {/* Composer */}
      <div style={{ borderTop: "1px solid var(--line)", padding: "12px 20px", background: "var(--bg-panel)" }}>
        {notes.length > 0 && (
          <div className="mono-meta" style={{ marginBottom: 8 }}>
            {notes.map((n, i) => <div key={i}>{n}</div>)}
          </div>
        )}
        <div style={{ display: "flex", gap: 10, alignItems: "flex-end" }}>
          <ClassSelect value={cls} onChange={setCls} />
          <textarea
            style={{ flex: 1 }}
            rows={2}
            placeholder={`message #${name} — the body is the message`}
            value={body}
            onChange={(e) => setBody(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
            }}
          />
          <button className="btn primary" onClick={send} disabled={!body.trim()}>send</button>
        </div>
        {err && <div className="error-bar" style={{ marginTop: 8 }}>{err}</div>}
      </div>

      {routePost && (
        <RouteDialog
          post={routePost}
          channels={allChannels.filter((c) => c.name !== name)}
          onClose={() => setRoutePost(null)}
        />
      )}
    </div>
  );
}

function ContextPane({ channel, onChanged }: { channel: Channel; onChanged: () => void }) {
  const { agents } = useAppData();
  const [adding, setAdding] = useState("");
  const [busy, setBusy] = useState(false);
  const isCustom = channel.kind === "custom";

  async function addMember() {
    const m = adding.trim();
    if (!m) return;
    setBusy(true);
    try {
      await api.addChannelMember(channel.name, m);
      setAdding("");
      onChanged();
    } finally {
      setBusy(false);
    }
  }
  async function removeMember(m: string) {
    await api.removeChannelMember(channel.name, m);
    onChanged();
  }
  async function del() {
    if (!confirm(`Delete channel #${channel.name}? Members and topic are removed; the message trail is kept.`)) return;
    await api.deleteChannel(channel.name);
    onChanged();
  }

  const suggestions = agents.map((a) => a.name);

  return (
    <div style={{ borderLeft: "1px solid var(--line)", overflow: "auto", padding: 16 }}>
      <div className="label">Channel</div>
      <div className="t-display" style={{ marginBottom: 4 }}>#{channel.name}</div>
      <div className="mono-meta" style={{ marginBottom: 16 }}>
        {channel.kind === "repo" ? "auto · repo channel" : "custom · spans repos & nodes"}
      </div>
      {channel.topic && <div style={{ marginBottom: 16, fontSize: 13, color: "var(--text-mid)" }}>{channel.topic}</div>}

      <div className="label" style={{ marginBottom: 8 }}>Members · {channel.members.length}</div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 12 }}>
        {channel.members.map((m) => (
          <span key={m} className={`chip${m === "operator" ? " op" : ""}`}>
            {m === "operator" ? "@operator" : m.includes("@") ? m : `@${m}`}
            {isCustom && <span className="rm" onClick={() => removeMember(m)} title="remove">×</span>}
          </span>
        ))}
        {channel.members.length === 0 && <span className="mono-meta">no members</span>}
      </div>

      {isCustom && (
        <>
          <div style={{ display: "flex", gap: 6, marginBottom: 8 }}>
            <input
              list="member-suggestions"
              style={{ flex: 1 }}
              placeholder="@agent, name@node, @operator"
              value={adding}
              onChange={(e) => setAdding(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && addMember()}
            />
            <button className="btn sm" onClick={addMember} disabled={busy || !adding.trim()}>add</button>
          </div>
          <datalist id="member-suggestions">
            <option value="@operator" />
            {suggestions.map((s) => <option key={s} value={`@${s}`} />)}
          </datalist>
          <button className="btn ghost sm danger" onClick={del} style={{ marginTop: 16 }}>delete channel</button>
        </>
      )}
      {!isCustom && (
        <div className="mono-meta">Repo channels self-maintain from the sessions running in the repo.</div>
      )}
    </div>
  );
}

function RouteDialog({ post, channels, onClose }: { post: ChannelPost; channels: Channel[]; onClose: () => void }) {
  const { agents } = useAppData();
  const [targets, setTargets] = useState<Set<string>>(new Set());
  const [cls, setCls] = useState<Urgency>(post.urgency);
  const [result, setResult] = useState<string[] | null>(null);
  const [err, setErr] = useState<string | null>(null);

  function toggle(addr: string) {
    setTargets((t) => {
      const n = new Set(t);
      if (n.has(addr)) n.delete(addr); else n.add(addr);
      return n;
    });
  }

  async function route() {
    setErr(null);
    const notes: string[] = [];
    try {
      for (const to of targets) {
        const res = await api.busSend({ to, body: post.body, urgency: cls, record: post.record ?? undefined });
        notes.push(...res.notes);
      }
      setResult(notes);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "route failed");
    }
  }

  return (
    <Modal onClose={onClose} title="Route message">
      <div className="mono-meta" style={{ marginBottom: 12, whiteSpace: "pre-wrap", background: "var(--bg-well)", padding: 10, border: "1px solid var(--line)" }}>
        <ClassBadge urgency={post.urgency} /> @{post.sender}: {post.body.slice(0, 200)}
      </div>
      <div className="label" style={{ marginBottom: 8 }}>Send to</div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 12 }}>
        {channels.map((c) => (
          <button key={c.name} className={`chip${targets.has(`#${c.name}`) ? " op" : ""}`} onClick={() => toggle(`#${c.name}`)}>#{c.name}</button>
        ))}
        {agents.map((a) => (
          <button key={a.name} className={`chip${targets.has(`@${a.name}`) ? " op" : ""}`} onClick={() => toggle(`@${a.name}`)}>@{a.name}</button>
        ))}
        <button className={`chip${targets.has("@operator") ? " op" : ""}`} onClick={() => toggle("@operator")}>@operator</button>
      </div>
      <div style={{ display: "flex", gap: 10, alignItems: "center", marginBottom: 12 }}>
        <span className="label">as</span>
        <ClassSelect value={cls} onChange={setCls} />
      </div>
      {err && <div className="error-bar">{err}</div>}
      {result ? (
        <>
          <div className="mono-meta" style={{ marginBottom: 12 }}>{result.map((n, i) => <div key={i}>{n}</div>)}</div>
          <button className="btn primary" onClick={onClose}>done</button>
        </>
      ) : (
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <button className="btn ghost" onClick={onClose}>cancel</button>
          <button className="btn primary" onClick={route} disabled={targets.size === 0}>route to {targets.size}</button>
        </div>
      )}
    </Modal>
  );
}

function NewChannelDialog({ agents, onClose, onCreated }: { agents: string[]; onClose: () => void; onCreated: (name: string) => void }) {
  const [name, setName] = useState("");
  const [topic, setTopic] = useState("");
  const [members, setMembers] = useState<Set<string>>(new Set(["@operator"]));
  const [err, setErr] = useState<string | null>(null);
  const [custom, setCustom] = useState("");

  function toggle(addr: string) {
    setMembers((m) => {
      const n = new Set(m);
      if (n.has(addr)) n.delete(addr); else n.add(addr);
      return n;
    });
  }

  async function create() {
    const clean = name.trim().replace(/^#/, "");
    if (!clean) { setErr("name required"); return; }
    setErr(null);
    try {
      await api.createChannel(clean, topic.trim() || undefined, [...members]);
      onCreated(clean);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "create failed");
    }
  }

  return (
    <Modal onClose={onClose} title="New channel">
      <div className="label" style={{ marginBottom: 4 }}>Name</div>
      <input style={{ width: "100%", marginBottom: 12 }} placeholder="release-train" value={name} onChange={(e) => setName(e.target.value)} autoFocus />
      <div className="label" style={{ marginBottom: 4 }}>Topic (optional)</div>
      <input style={{ width: "100%", marginBottom: 12 }} placeholder="what this channel coordinates" value={topic} onChange={(e) => setTopic(e.target.value)} />
      <div className="label" style={{ marginBottom: 8 }}>Members — mix agents, nodes, and the operator</div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 10 }}>
        <button className={`chip${members.has("@operator") ? " op" : ""}`} onClick={() => toggle("@operator")}>@operator</button>
        {agents.map((a) => (
          <button key={a} className={`chip${members.has(`@${a}`) ? " op" : ""}`} onClick={() => toggle(`@${a}`)}>@{a}</button>
        ))}
      </div>
      <div style={{ display: "flex", gap: 6, marginBottom: 12 }}>
        <input style={{ flex: 1 }} placeholder="add by address: name@node" value={custom} onChange={(e) => setCustom(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && custom.trim()) { toggle(custom.trim()); setCustom(""); } }} />
        <button className="btn sm" onClick={() => { if (custom.trim()) { toggle(custom.trim()); setCustom(""); } }}>add</button>
      </div>
      <div style={{ marginBottom: 12 }}>
        {[...members].map((m) => (
          <span key={m} className="chip op" style={{ marginRight: 6 }}>{m}<span className="rm" onClick={() => toggle(m)}>×</span></span>
        ))}
      </div>
      {err && <div className="error-bar">{err}</div>}
      <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
        <button className="btn ghost" onClick={onClose}>cancel</button>
        <button className="btn primary" onClick={create}>create channel</button>
      </div>
    </Modal>
  );
}

function Modal({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);
  return (
    <div
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 50 }}
      onClick={onClose}
    >
      <div className="strip" style={{ width: 480, maxWidth: "90vw", maxHeight: "85vh", overflow: "auto", background: "var(--bg-panel)" }} onClick={(e) => e.stopPropagation()}>
        <div style={{ display: "flex", alignItems: "center", marginBottom: 16 }}>
          <span className="t-display">{title}</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onClose}>esc</button>
        </div>
        {children}
      </div>
    </div>
  );
}
