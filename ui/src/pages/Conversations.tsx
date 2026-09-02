// Conversations — every conversation surface in one place. A left rail
// (full trail, auto #repo channels, custom channels, direct-message pairs)
// → a center pane (channel thread / DM thread / filterable trail) → a
// context pane. Channel posts expand into live per-recipient receipts so
// the operator can watch a message land.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import {
  api,
  type BusMessage,
  type Channel,
  type ChannelPost,
  type DmPair,
  type Urgency,
} from "../api";
import { usePoll } from "../hooks";
import { useAppData } from "../App";
import { Modal, NewChannelDialog } from "../channels";
import { Capsule, ClassBadge, ClassSelect, Empty, ErrorBar, MessageRow, relTime } from "../components";
import "./conversations.css";

/* ── Shared delivery-state fragment for a BusMessage ────────────────── */

function DeliveryState({ m }: { m: BusMessage }) {
  if (m.ingested_at !== null) {
    return <span className="dstate ok">✓ ingested {relTime(m.ingested_at)} ago</span>;
  }
  if (m.delivered_at !== null) {
    return (
      <span className="dstate">
        delivered{m.delivered_via ? ` via ${m.delivered_via}` : ""} {relTime(m.delivered_at)} ago
      </span>
    );
  }
  return <span className="dstate">pending</span>;
}

function whoLabel(name: string): ReactNode {
  if (name === "operator") return <span className="who op">@operator</span>;
  return <span className="who">@{name}</span>;
}

export default function Conversations() {
  const { channel } = useParams();
  const [params] = useSearchParams();
  const nav = useNavigate();
  const { agents } = useAppData();
  const channelsPoll = usePoll<Channel[]>(api.channels, 3000);
  const channels = useMemo(() => channelsPoll.data ?? [], [channelsPoll.data]);
  const dmsPoll = usePoll<DmPair[]>(api.dms, 5000);
  const dms = dmsPoll.data ?? [];
  const [newOpen, setNewOpen] = useState(false);

  const isTrail = params.get("view") === "trail";
  const dmParam = params.get("dm");
  const dmPair = useMemo(() => {
    if (!dmParam) return null;
    const parts = dmParam.split(",");
    if (parts.length !== 2 || !parts[0] || !parts[1]) return null;
    return [parts[0], parts[1]] as [string, string];
  }, [dmParam]);

  const selected = isTrail || dmPair ? null : (channel ?? channels[0]?.name ?? null);
  const current = channels.find((c) => c.name === selected) ?? null;

  const repoChannels = channels.filter((c) => c.kind === "repo");
  const customChannels = channels.filter((c) => c.kind === "custom");

  const openChannel = (name: string) => nav(`/conversations/${encodeURIComponent(name)}`);
  const openTrail = () => nav("/conversations?view=trail");
  const openDm = (a: string, b: string) =>
    nav(`/conversations?dm=${encodeURIComponent(a)},${encodeURIComponent(b)}`);

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Conversations</span>
        {isTrail && <span className="mono-meta">full trail</span>}
        {dmPair && <span className="mono-meta">@{dmPair[0]} ↔ @{dmPair[1]}</span>}
        {current && <span className="mono-meta">#{current.name}</span>}
        <span style={{ flex: 1 }} />
        <button className="btn primary sm" onClick={() => setNewOpen(true)}>+ new channel</button>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "220px 1fr 260px", height: "calc(100% - 61px)", minHeight: 0 }}>
        {/* Rail */}
        <div style={{ borderRight: "1px solid var(--line)", overflow: "auto", padding: "12px 0" }}>
          <div
            className={`nav-item${isTrail ? " active" : ""}`}
            onClick={openTrail}
            role="button"
            tabIndex={0}
            onKeyDown={(e) => e.key === "Enter" && openTrail()}
          >
            <span className="mono">≡ trail</span>
            <span style={{ flex: 1 }} />
            <span className="mono-meta">all</span>
          </div>

          <div className="nav-section label" style={{ marginTop: 12 }}>Repos</div>
          {repoChannels.length === 0 && <div style={{ padding: "2px 16px" }} className="mono-meta">none</div>}
          {repoChannels.map((c) => (
            <ChannelLink key={c.name} c={c} active={c.name === selected} onClick={() => openChannel(c.name)} />
          ))}
          <div className="nav-section label" style={{ marginTop: 12 }}>Custom</div>
          {customChannels.length === 0 && <div style={{ padding: "2px 16px" }} className="mono-meta">none yet</div>}
          {customChannels.map((c) => (
            <ChannelLink key={c.name} c={c} active={c.name === selected} onClick={() => openChannel(c.name)} />
          ))}

          <div className="nav-section label" style={{ marginTop: 12 }}>Direct</div>
          {dms.length === 0 && <div style={{ padding: "2px 16px" }} className="mono-meta">none yet</div>}
          {dms.map((d) => {
            const active =
              dmPair !== null &&
              ((dmPair[0] === d.a && dmPair[1] === d.b) || (dmPair[0] === d.b && dmPair[1] === d.a));
            return (
              <div
                key={`${d.a}|${d.b}`}
                className={`nav-item${active ? " active" : ""}`}
                onClick={() => openDm(d.a, d.b)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => e.key === "Enter" && openDm(d.a, d.b)}
                title={`last ${relTime(d.last_at)} ago`}
              >
                <span className="mono dm-pair-label">
                  {whoLabel(d.a)}
                  <span className="link">↔</span>
                  {whoLabel(d.b)}
                </span>
                <span style={{ flex: 1 }} />
                <span className="mono-meta">{d.messages}</span>
              </div>
            );
          })}
        </div>

        {/* Center pane */}
        {isTrail ? (
          <TrailView />
        ) : dmPair ? (
          <DmThread key={`${dmPair[0]}|${dmPair[1]}`} a={dmPair[0]} b={dmPair[1]} />
        ) : selected ? (
          <Thread key={selected} name={selected} allChannels={channels} />
        ) : (
          <Empty mark="#">No channels yet. Every repo with a running session forms one automatically, or create a custom channel.</Empty>
        )}

        {/* Context */}
        {current ? (
          <ContextPane channel={current} onChanged={channelsPoll.refresh} />
        ) : (
          <div style={{ borderLeft: "1px solid var(--line)", padding: 16 }}>
            {isTrail && (
              <div className="mono-meta">The full bus trail — every message on this node, filterable by sender, recipient, thread, record, class, and body.</div>
            )}
            {dmPair && (
              <div className="mono-meta">Direct traffic between @{dmPair[0]} and @{dmPair[1]}. Delivery and ingestion state shown per message.</div>
            )}
          </div>
        )}
      </div>

      {newOpen && (
        <NewChannelDialog
          agents={agents.map((a) => a.name)}
          onClose={() => setNewOpen(false)}
          onCreated={(name) => {
            setNewOpen(false);
            channelsPoll.refresh();
            openChannel(name);
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

/* ── Per-recipient receipts for a channel post — watch it land ──────── */

function ReceiptPanel({ post }: { post: string }) {
  const load = useMemo(() => () => api.postReceipts(post), [post]);
  const poll = usePoll<BusMessage[]>(load, 2000);
  const rows = poll.data;
  return (
    <div className="receipts" onClick={(e) => e.stopPropagation()}>
      {poll.error && <div className="dstate">{poll.error}</div>}
      {rows === null && !poll.error && <div className="dstate">loading receipts…</div>}
      {rows !== null && rows.length === 0 && <div className="dstate">no recipients recorded</div>}
      {(rows ?? []).map((r) => (
        <div key={r.id} className="receipt-row">
          <span className="who">{r.recipient === "operator" ? "@operator" : `@${r.recipient}`}</span>
          <DeliveryState m={r} />
        </div>
      ))}
    </div>
  );
}

/* ── Channel thread ─────────────────────────────────────────────────── */

function Thread({ name, allChannels }: { name: string; allChannels: Channel[] }) {
  const load = useMemo(() => () => api.channelLog(name, 100), [name]);
  const poll = usePoll<ChannelPost[]>(load, 2500);
  const posts = poll.data ?? [];
  const [body, setBody] = useState("");
  const [cls, setCls] = useState<Urgency>("normal");
  const [err, setErr] = useState<string | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [routePost, setRoutePost] = useState<ChannelPost | null>(null);
  const [openReceipts, setOpenReceipts] = useState<Set<string>>(new Set());

  function toggleReceipts(post: string) {
    setOpenReceipts((s) => {
      const n = new Set(s);
      if (n.has(post)) n.delete(post); else n.add(post);
      return n;
    });
  }

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
                <span
                  className="ticks clickable"
                  title="click for per-recipient receipts"
                  role="button"
                  tabIndex={0}
                  onClick={() => toggleReceipts(p.post)}
                  onKeyDown={(e) => e.key === "Enter" && toggleReceipts(p.post)}
                >
                  <span>{p.delivered}/{p.recipients} deliv</span>
                  {p.ingested > 0 && <b>· {p.ingested} ✓ read</b>}
                  <span className="mono-meta">· {relTime(p.created_at)}</span>
                </span>
              }
              actions={<button className="btn ghost sm" onClick={() => setRoutePost(p)}>route…</button>}
            >
              {p.body}
              {openReceipts.has(p.post) && <ReceiptPanel post={p.post} />}
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

/* ── Direct-message thread ──────────────────────────────────────────── */

function DmThread({ a, b }: { a: string; b: string }) {
  const load = useMemo(() => () => api.dmLog(a, b, 200), [a, b]);
  const poll = usePoll<BusMessage[]>(load, 2500);
  const msgs = poll.data ?? [];
  const other = a === "operator" ? b : b === "operator" ? a : null;
  const [body, setBody] = useState("");
  const [cls, setCls] = useState<Urgency>("normal");
  const [err, setErr] = useState<string | null>(null);

  async function send() {
    if (!other || !body.trim()) return;
    setErr(null);
    try {
      await api.busSend({ to: `@${other}`, body: body.trim(), urgency: cls });
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
        {msgs.length === 0 ? (
          <Empty mark="↔">No direct messages between @{a} and @{b} yet.</Empty>
        ) : (
          msgs.map((m) => (
            <MessageRow
              key={m.id}
              urgency={m.urgency}
              sender={m.sender}
              meta={
                <>
                  <DeliveryState m={m} />
                  <span className="mono-meta">· {relTime(m.created_at)}</span>
                </>
              }
            >
              {m.body}
            </MessageRow>
          ))
        )}
      </div>

      {other ? (
        <div style={{ borderTop: "1px solid var(--line)", padding: "12px 20px", background: "var(--bg-panel)" }}>
          <div style={{ display: "flex", gap: 10, alignItems: "flex-end" }}>
            <ClassSelect value={cls} onChange={setCls} />
            <textarea
              style={{ flex: 1 }}
              rows={2}
              placeholder={`message @${other} directly`}
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
      ) : (
        <div className="dm-observer">observer view — message either agent directly to join</div>
      )}
    </div>
  );
}

/* ── Full trail — the deep, filterable lookback ─────────────────────── */

function TrailView() {
  const [fSender, setFSender] = useState("");
  const [fRecipient, setFRecipient] = useState("");
  const [fThread, setFThread] = useState("");
  const [fRecord, setFRecord] = useState("");
  const [fUrg, setFUrg] = useState("");
  const [fQ, setFQ] = useState("");
  const [open, setOpen] = useState<Set<number>>(new Set());

  const fetcher = useMemo(() => {
    const filters = {
      ...(fSender.trim() ? { sender: fSender.trim() } : {}),
      ...(fRecipient.trim() ? { recipient: fRecipient.trim() } : {}),
      ...(fThread.trim() ? { thread: fThread.trim() } : {}),
      ...(fRecord.trim() ? { record: fRecord.trim() } : {}),
      ...(fUrg ? { urgency: fUrg } : {}),
      ...(fQ.trim() ? { q: fQ.trim() } : {}),
    };
    return () => api.busLog(200, filters);
  }, [fSender, fRecipient, fThread, fRecord, fUrg, fQ]);
  const poll = usePoll<BusMessage[]>(fetcher, 3000);
  const refresh = poll.refresh;
  useEffect(() => {
    void refresh();
  }, [fetcher, refresh]);

  const rows = [...(poll.data ?? [])].reverse();

  function toggle(id: number) {
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id); else n.add(id);
      return n;
    });
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", minHeight: 0 }}>
      <div className="trail-filters">
        <input placeholder="sender" value={fSender} onChange={(e) => setFSender(e.target.value)} />
        <input placeholder="recipient" value={fRecipient} onChange={(e) => setFRecipient(e.target.value)} />
        <input placeholder="thread" value={fThread} onChange={(e) => setFThread(e.target.value)} />
        <input placeholder="record" value={fRecord} onChange={(e) => setFRecord(e.target.value)} />
        <select value={fUrg} onChange={(e) => setFUrg(e.target.value)} aria-label="urgency filter">
          <option value="">any class</option>
          <option value="gating">gating</option>
          <option value="normal">normal</option>
          <option value="notice">notice</option>
        </select>
        <input className="wide" placeholder="body contains…" value={fQ} onChange={(e) => setFQ(e.target.value)} />
      </div>
      <div className="trail-rows">
        <ErrorBar error={poll.error} />
        {rows.length === 0 ? (
          <Empty mark="≡">No messages match.</Empty>
        ) : (
          rows.map((m) => (
            <div key={m.id} className="trail-row">
              <Capsule urgency={m.urgency} />
              <ClassBadge urgency={m.urgency} />
              <span className="mono addr" style={{ fontSize: 12 }}>@{m.sender}</span>
              <span className="mono-meta addr">→ {m.to_display}</span>
              <span
                className={`trail-body${open.has(m.id) ? " open" : ""}`}
                onClick={() => toggle(m.id)}
                title={open.has(m.id) ? "collapse" : "expand"}
              >
                {m.body}
              </span>
              <DeliveryState m={m} />
              <span className="mono-meta addr">{relTime(m.created_at)}</span>
            </div>
          ))
        )}
      </div>
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
  const nav = useNavigate();
  const [targets, setTargets] = useState<Set<string>>(new Set());
  const [cls, setCls] = useState<Urgency>(post.urgency);
  const [result, setResult] = useState<string[] | null>(null);
  const [routedChannels, setRoutedChannels] = useState<string[]>([]);
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
      setRoutedChannels([...targets].filter((t) => t.startsWith("#")).map((t) => t.slice(1)));
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
          {routedChannels.length > 0 && (
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 12, alignItems: "center" }}>
              <span className="mono-meta">watch it land:</span>
              {routedChannels.map((c) => (
                <button
                  key={c}
                  className="btn ghost sm"
                  onClick={() => {
                    onClose();
                    nav(`/conversations/${encodeURIComponent(c)}`);
                  }}
                >
                  watch in #{c}
                </button>
              ))}
            </div>
          )}
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

