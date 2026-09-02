// Custom-channel creation: the one dialog for wiring a bus connection
// between agents/nodes/operator, used from Conversations and the Map.

import { useEffect, useState, type ReactNode } from "react";
import { api } from "./api";

/** Create a custom channel: a bus connection between any mix of agents
 *  (local or name@node), nodes, and the operator. Shared by Conversations
 *  and the Map (which pre-fills the selected agents). */
export function NewChannelDialog({
  agents,
  initialMembers,
  onClose,
  onCreated,
}: {
  agents: string[];
  /** Pre-selected member addresses (e.g. from a map selection). */
  initialMembers?: string[];
  onClose: () => void;
  onCreated: (name: string) => void;
}) {
  const [name, setName] = useState("");
  const [topic, setTopic] = useState("");
  const [members, setMembers] = useState<Set<string>>(
    () => new Set(["@operator", ...(initialMembers ?? [])]),
  );
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


export function Modal({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
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
