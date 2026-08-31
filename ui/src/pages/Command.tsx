// Command — the triage cockpit. NEEDS YOU (the operator inbox, actionable in
// place) over IN FLIGHT (the live activity feed: every session's derived
// presence, and the recent bus traffic). This is the first screen.

import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, type Activity } from "../api";
import { usePoll } from "../hooks";
import { useAppData } from "../App";
import {
  Capsule,
  ClassBadge,
  ClassSelect,
  Empty,
  ErrorBar,
  Meter,
  MessageRow,
  presenceOf,
  relTime,
} from "../components";
import type { Urgency } from "../api";

export default function Command() {
  const nav = useNavigate();
  const { inbox, refreshInbox } = useAppData();
  const activityPoll = usePoll<Activity>(api.activity, 2000);
  const activity = activityPoll.data;
  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [replyText, setReplyText] = useState("");
  const [replyClass, setReplyClass] = useState<Urgency>("normal");
  const [err, setErr] = useState<string | null>(null);

  async function sendReply(to: string) {
    if (!replyText.trim()) return;
    setErr(null);
    try {
      await api.busSend({ to: `@${to}`, body: replyText.trim(), urgency: replyClass });
      setReplyTo(null);
      setReplyText("");
      refreshInbox();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "send failed");
    }
  }

  async function clearInbox() {
    await api.markInboxRead();
    refreshInbox();
  }

  const sessions = activity?.sessions ?? [];
  const busy = sessions.filter((s) => s.live && s.turn_state === "busy");
  const idle = sessions.filter((s) => s.live && s.turn_state !== "busy");
  const down = sessions.filter((s) => !s.live);

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Command</span>
        <span className="mono-meta">
          {busy.length} busy · {idle.length} idle · {down.length} down
        </span>
      </div>
      <div className="stage-body" style={{ display: "grid", gap: 24, gridTemplateColumns: "minmax(320px, 1fr) minmax(340px, 1.3fr)", alignItems: "start" }}>
        {/* NEEDS YOU */}
        <section>
          <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
            <span className="label">Needs you</span>
            <span style={{ flex: 1 }} />
            {inbox.length > 0 && (
              <button className="btn ghost sm" onClick={clearInbox}>clear all</button>
            )}
          </div>
          <ErrorBar error={err} />
          {inbox.length === 0 ? (
            <Empty mark="—">Nothing needs you. The mesh is quiet.</Empty>
          ) : (
            inbox.map((m) => (
              <MessageRow
                key={m.id}
                urgency={m.urgency}
                sender={m.sender}
                meta={<span className="mono-meta">{relTime(m.created_at)} ago</span>}
                actions={
                  replyTo === `${m.id}` ? (
                    <div style={{ width: "100%", display: "flex", flexDirection: "column", gap: 8 }}>
                      <textarea
                        autoFocus
                        rows={2}
                        placeholder={`reply to @${m.sender}`}
                        value={replyText}
                        onChange={(e) => setReplyText(e.target.value)}
                      />
                      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                        <ClassSelect value={replyClass} onChange={setReplyClass} />
                        <span style={{ flex: 1 }} />
                        <button className="btn ghost sm" onClick={() => setReplyTo(null)}>cancel</button>
                        <button className="btn primary sm" onClick={() => sendReply(m.sender)}>send</button>
                      </div>
                    </div>
                  ) : (
                    <>
                      <button className="btn sm" onClick={() => { setReplyTo(`${m.id}`); setReplyText(""); }}>reply</button>
                      {!m.sender.includes("@") && (
                        <button className="btn sm" onClick={() => nav(`/session/${encodeURIComponent(m.sender)}`)}>
                          open @{m.sender}
                        </button>
                      )}
                    </>
                  )
                }
              >
                {m.body}
              </MessageRow>
            ))
          )}
        </section>

        {/* IN FLIGHT */}
        <section>
          <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
            <span className="label">In flight</span>
            <span style={{ flex: 1 }} />
            {activityPoll.error && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>offline</span>}
          </div>
          {sessions.length === 0 ? (
            <Empty mark="◦">No sessions running. Start one from Sessions or Library.</Empty>
          ) : (
            <div className="grid" style={{ marginBottom: 20 }}>
              {sessions.map((s) => {
                const p = presenceOf(s.live, s.turn_state);
                return (
                  <div
                    key={s.name}
                    className="strip"
                    style={{ display: "flex", alignItems: "center", gap: 12, cursor: "pointer" }}
                    onClick={() => nav(`/session/${encodeURIComponent(s.name)}`)}
                    role="button"
                    tabIndex={0}
                    onKeyDown={(e) => e.key === "Enter" && nav(`/session/${encodeURIComponent(s.name)}`)}
                  >
                    <Meter presence={p} />
                    <span className="mono" style={{ fontWeight: 500 }}>@{s.name}</span>
                    <span className="mono-meta">#{s.channel}</span>
                    <span style={{ flex: 1 }} />
                    <span className="micro" style={{ color: p === "busy" ? "var(--live)" : p === "off" ? "var(--offline)" : "var(--idle)" }}>
                      {p === "busy" ? "streaming" : p === "off" ? "offline" : "idle"}
                    </span>
                    {s.pending > 0 && <span className="badge-count">{s.pending}</span>}
                  </div>
                );
              })}
            </div>
          )}

          <div className="label" style={{ marginBottom: 8 }}>Recent traffic</div>
          <div className="strip flat" style={{ padding: "8px 12px", maxHeight: 320, overflow: "auto" }}>
            {(activity?.trail ?? []).length === 0 ? (
              <div className="mono-meta">no traffic yet</div>
            ) : (
              [...(activity?.trail ?? [])].reverse().map((m) => (
                <div key={m.id} style={{ display: "flex", alignItems: "center", gap: 8, padding: "3px 0" }}>
                  <Capsule urgency={m.urgency} />
                  <ClassBadge urgency={m.urgency} />
                  <span className="mono" style={{ fontSize: 12 }}>@{m.sender}</span>
                  <span className="mono-meta">→ {m.to_display}</span>
                  <span className="mono-meta" style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {m.body}
                  </span>
                  <span className="mono-meta">{relTime(m.created_at)}</span>
                </div>
              ))
            )}
          </div>
        </section>
      </div>
    </>
  );
}
