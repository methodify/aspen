import { useState, type FormEvent } from "react";
import { api, ApiError, type BusMessage, type Urgency } from "./../api";
import { fmtTime, usePoll } from "./../hooks";

export function UrgencyBadge({ urgency }: { urgency: Urgency }) {
  return <span className={`urgency urgency-${urgency}`}>{urgency}</span>;
}

export function DeliveryState({ m }: { m: BusMessage }) {
  if (m.ingested_at !== null) {
    return (
      <span className="delivery delivery-ingested" title="runtime replay-ack: proof of ingestion">
        ingested {fmtTime(m.ingested_at)}
      </span>
    );
  }
  if (m.delivered_at !== null) {
    return (
      <span className="delivery delivery-delivered">
        delivered{m.delivered_via ? ` via ${m.delivered_via}` : ""} {fmtTime(m.delivered_at)}
      </span>
    );
  }
  return <span className="delivery delivery-pending">pending</span>;
}

export function BusRow({ m }: { m: BusMessage }) {
  return (
    <div className={`bus-row bus-row-${m.urgency}`}>
      <div className="bus-row-head">
        <span className="dim mono">{fmtTime(m.created_at)}</span>
        <span className="mono bus-route">
          @{m.sender} <span className="dim">→</span> {m.to_display}
        </span>
        <UrgencyBadge urgency={m.urgency} />
        {m.thread && <span className="chip mono">thread {m.thread}</span>}
        {m.record && (
          <span className="chip mono" title="durable record ref">
            rec {m.record}
          </span>
        )}
        <DeliveryState m={m} />
      </div>
      <div className="bus-body">{m.body}</div>
    </div>
  );
}

function BusComposer() {
  const [to, setTo] = useState("");
  const [body, setBody] = useState("");
  const [urgency, setUrgency] = useState<Urgency>("normal");
  const [thread, setThread] = useState("");
  const [record, setRecord] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<string[]>([]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (sending) return;
    setError(null);
    setNotes([]);
    if (!to.trim() || !body.trim()) {
      setError("to and body are required");
      return;
    }
    setSending(true);
    try {
      const res = await api.busSend({
        to: to.trim(),
        body: body.trim(),
        urgency,
        ...(thread.trim() ? { thread: thread.trim() } : {}),
        ...(record.trim() ? { record: record.trim() } : {}),
      });
      setNotes(res.notes);
      setBody("");
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSending(false);
    }
  }

  return (
    <form className="panel bus-composer" onSubmit={submit}>
      <h2>
        send as <span className="mono">@operator</span>
      </h2>
      <div className="form-row">
        <label>
          to
          <input
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="@agent, #channel, @operator"
            className="mono"
            required
          />
        </label>
        <label>
          urgency
          <select value={urgency} onChange={(e) => setUrgency(e.target.value as Urgency)}>
            <option value="normal">normal</option>
            <option value="gating">gating</option>
            <option value="notice">notice</option>
          </select>
        </label>
        <label>
          thread <span className="dim">(optional)</span>
          <input
            value={thread}
            onChange={(e) => setThread(e.target.value)}
            placeholder="t-1"
            className="mono"
          />
        </label>
        <label>
          record <span className="dim">(optional)</span>
          <input
            value={record}
            onChange={(e) => setRecord(e.target.value)}
            placeholder="docs/decisions/07.md"
            className="mono"
          />
        </label>
      </div>
      <label>
        body
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          rows={3}
          required
          placeholder="message body"
        />
      </label>
      <div className="form-row">
        <button type="submit" disabled={sending}>
          {sending ? "sending…" : "send"}
        </button>
      </div>
      {error && <div className="error-inline">{error}</div>}
      {notes.length > 0 && (
        <ul className="delivery-notes">
          {notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      )}
    </form>
  );
}

export default function Bus() {
  const { data, error } = usePoll(() => api.busLog(200), 2000);
  const log = data ?? [];
  return (
    <div className="page">
      <header className="page-head">
        <h1>Bus</h1>
        <span className="dim">{log.length} messages</span>
      </header>
      {error && <div className="error-inline">bus log: {error}</div>}
      <BusComposer />
      <div className="bus-log">
        {log.length === 0 && data !== null && <div className="empty">no traffic yet.</div>}
        {log.map((m) => (
          <BusRow key={m.id} m={m} />
        ))}
      </div>
    </div>
  );
}
