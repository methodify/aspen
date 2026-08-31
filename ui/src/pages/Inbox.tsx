import { useState } from "react";
import { api } from "./../api";
import { useAppData } from "./../App";
import { BusRow } from "./Bus";

export default function Inbox() {
  const { inbox, refreshInbox } = useAppData();
  const [marking, setMarking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function markRead() {
    if (marking) return;
    setMarking(true);
    setError(null);
    try {
      await api.markInboxRead();
      await refreshInbox();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setMarking(false);
    }
  }

  return (
    <div className="page">
      <header className="page-head">
        <h1>Inbox</h1>
        <span className="dim">
          messages to <span className="mono">@operator</span> · {inbox.length} unread
        </span>
        {inbox.length > 0 && (
          <button onClick={() => void markRead()} disabled={marking}>
            {marking ? "marking…" : "mark read"}
          </button>
        )}
      </header>
      {error && <div className="error-inline">{error}</div>}
      <div className="bus-log">
        {inbox.length === 0 && <div className="empty">No unread messages.</div>}
        {inbox.map((m) => (
          <BusRow key={m.id} m={m} />
        ))}
      </div>
    </div>
  );
}
