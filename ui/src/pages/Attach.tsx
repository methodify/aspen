// Attach (docs/RELAY.md §11): make this browser a mesh peer and reach a
// node through a relay, with no daemon of its own. Three steps: an
// identity (kept in this browser), a cert minted by the mesh root from
// the enroll blob shown here, and a relay + node to attach to (the join
// bundle carries the relay). The tunnel then carries every console
// request; the status bar shows it.

import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { createIdentity, enrollBlob, installBlob, loadConfig, loadIdentity, saveConfig, tunnel, type ConsoleIdentity } from "../tunnel";
import { activate, activeConnection, addConnection, directUrlProblem, hosted, listConnections, markActive, removeConnection, type Connection } from "../connections";
import { ErrorBar } from "../components";

export default function Attach() {
  const [params] = useSearchParams();
  const [id, setId] = useState<ConsoleIdentity | null>(() => loadIdentity());
  const [cfg, setCfg] = useState(() => {
    const c = loadConfig();
    return { relay: params.get("relay") ?? c.relay, node: params.get("node") ?? c.node, enabled: c.enabled };
  });
  const [blob, setBlob] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [, setTick] = useState(0);
  useEffect(() => tunnel.onChange(() => setTick((n) => n + 1)), []);
  const [copied, setCopied] = useState(false);

  function apply(next: Partial<typeof cfg>) {
    const c = { ...cfg, ...next };
    setCfg(c);
    saveConfig(c);
  }
  async function connect() {
    setErr(null);
    apply({ enabled: true });
    // Connecting is saving: the relay and node become a connection and
    // the active one, so the rest of the console (and the next visit)
    // knows where it is talking to. No reload — the tunnel is starting
    // right here.
    if (cfg.relay && cfg.node) {
      const existing = listConnections().find((c) => c.kind === "relay" && c.relay === cfg.relay && c.node === cfg.node);
      const c = existing ?? addConnection({ name: `${cfg.node} via relay`, kind: "relay", relay: cfg.relay, node: cfg.node });
      markActive(c.id);
    }
    try {
      await tunnel.start();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }
  function disconnect() {
    apply({ enabled: false });
    tunnel.stop();
  }

  return (
    <>
      <div className="stage-head">
        <span className="t-display">{hosted ? "Connect" : "Attach through a relay"}</span>
        <span className="mono-meta">{hosted ? "this console talks to the node or relay you point it at" : "this browser as a mesh peer — no local node needed"}</span>
      </div>
      <div className="stage-body attach-page">
        <Connections relay={cfg.relay} node={cfg.node} certified={!!id?.cert} />
        <p className="dim" style={{ maxWidth: "80ch" }}>
          Reach a node you cannot dial directly. This browser keeps its own keypair; the mesh root certifies it once; then every console request rides the relay to the node you attach to, sealed end to end (the relay reads nothing). A console peer may read and control sessions, never spawn or trust.
        </p>
        <ErrorBar error={err ?? tunnel.error} />

        <section className="strip attach-step">
          <span className="label">1 · Identity (this browser)</span>
          {id ? (
            <div className="attach-row">
              <span className="mono">{id.node}</span>
              <span className="mono-meta">{id.cert ? `certified in mesh ${id.cert.mesh}` : "not certified yet"}</span>
              <span style={{ flex: 1 }} />
              <button
                className="btn ghost sm"
                onClick={() => {
                  if (tunnel.enabled) disconnect();
                  setId(createIdentity());
                }}
                title="forget this identity and make a new one (the old cert stops working)"
              >
                new identity
              </button>
            </div>
          ) : (
            <button className="btn primary sm" onClick={() => setId(createIdentity())}>create identity</button>
          )}
        </section>

        {id && !id.cert && (
          <section className="strip attach-step">
            <span className="label">2 · Certify (on the node that holds the root key)</span>
            <p className="dim">Copy this enroll blob and run it where the mesh's root key lives; paste the bundle it prints below.</p>
            <pre className="attach-blob mono">aspen mesh certify {enrollBlob(id)} --url ws://&lt;that node&gt;:&lt;port&gt;/api/federation/ws</pre>
            <div className="attach-row">
              <button
                className="btn sm"
                onClick={() => {
                  navigator.clipboard?.writeText(enrollBlob(id)).then(() => setCopied(true)).catch(() => {});
                }}
              >
                {copied ? "copied" : "copy enroll blob"}
              </button>
            </div>
            <textarea className="mono" rows={4} placeholder="aspen:bundle:… (or aspen:cert:…)" value={blob} onChange={(e) => setBlob(e.target.value)} spellCheck={false} />
            <div className="attach-row">
              <button
                className="btn primary sm"
                disabled={!blob.trim()}
                onClick={() => {
                  setErr(null);
                  try {
                    const next = installBlob(id, blob);
                    setId(next);
                    setBlob("");
                    const first = Object.keys(next.known ?? {})[0];
                    apply({ relay: cfg.relay || next.relay || "", node: cfg.node || first || "" });
                  } catch (e) {
                    setErr(e instanceof Error ? e.message : String(e));
                  }
                }}
              >
                install
              </button>
            </div>
          </section>
        )}

        {id?.cert && (
          <section className="strip attach-step">
            <span className="label">3 · Relay and node</span>
            <div className="grid cols">
              <label style={{ display: "grid", gap: 4 }}>
                <span className="label">Relay URL</span>
                <input className="mono" value={cfg.relay} onChange={(e) => apply({ relay: e.target.value })} placeholder="wss://relay.example/relay or ws://host:7420/api/federation/relay" spellCheck={false} />
              </label>
              <label style={{ display: "grid", gap: 4 }}>
                <span className="label">Attach to node</span>
                <input className="mono" value={cfg.node} onChange={(e) => apply({ node: e.target.value })} placeholder="node name" spellCheck={false} list="attach-present" />
                <datalist id="attach-present">
                  {tunnel.present.filter((p) => !p.startsWith("console-")).map((p) => (
                    <option key={p} value={p} />
                  ))}
                </datalist>
              </label>
            </div>
            <div className="attach-row">
              <span className={`chip mono attach-state s-${tunnel.state}`}>{tunnel.state}</span>
              {tunnel.present.length > 0 && <span className="mono-meta">on the relay: {tunnel.present.filter((p) => !p.startsWith("console-")).join(", ") || "no nodes"}</span>}
              <span style={{ flex: 1 }} />
              {tunnel.enabled && tunnel.state !== "off" ? (
                <button className="btn sm" onClick={disconnect}>disconnect</button>
              ) : (
                <button className="btn primary sm" disabled={!cfg.relay || !cfg.node} onClick={() => void connect()}>
                  connect
                </button>
              )}
            </div>
            <p className="dim">
              While attached, this console is the attached node's: its sessions, its peers' sessions through it, boards and everything else, over the relay. Files served as bytes (the artifact viewer's images) may not load through the tunnel.
            </p>
          </section>
        )}
      </div>
    </>
  );
}

/** The named connections this console keeps (connections.ts): a node on
 *  this machine reached directly, or a node reached through a relay as
 *  a mesh peer. Hosted, this is the front door; served by a node, it is
 *  a way to look at another node from this page. */
function Connections({ relay, node, certified }: { relay: string; node: string; certified: boolean }) {
  const [list, setList] = useState<Connection[]>(() => listConnections());
  const active = activeConnection();
  const [name, setName] = useState("");
  const [url, setUrl] = useState("http://127.0.0.1:7420");
  const [token, setToken] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const refresh = () => setList(listConnections());
  const problem = directUrlProblem(url.trim());
  return (
    <section className="strip attach-step">
      <span className="label">Connections</span>
      {list.length === 0 && <p className="dim">{hosted ? "None yet. Add the node on this machine below, or attach through a relay (steps 1–3) and save it as a connection." : "None saved. This page is served by a node and talks to it; a saved connection points it elsewhere."}</p>}
      {list.map((c) => (
        <div className="attach-row" key={c.id}>
          <span className={`chip mono ${active?.id === c.id ? "op" : ""}`}>{active?.id === c.id ? "active" : c.kind}</span>
          <span className="mono">{c.name}</span>
          <span className="mono-meta">{c.kind === "direct" ? c.url : `via ${c.relay} → ${c.node}`}</span>
          <span style={{ flex: 1 }} />
          {active?.id !== c.id && (
            <button className="btn sm" onClick={() => activate(c.id)} title="use this connection (the page reloads)">use</button>
          )}
          {active?.id === c.id && hosted && (
            <button className="btn ghost sm" onClick={() => activate(null)} title="disconnect">stop</button>
          )}
          <button className="btn ghost sm" onClick={() => { removeConnection(c.id); refresh(); }} title="forget this connection">×</button>
        </div>
      ))}
      <div className="attach-row" style={{ flexWrap: "wrap", gap: 8 }}>
        <input className="mono" placeholder="name" value={name} onChange={(e) => setName(e.target.value)} style={{ width: 140 }} />
        <input className="mono" placeholder="http://127.0.0.1:7420" value={url} onChange={(e) => setUrl(e.target.value)} style={{ flex: 1, minWidth: 220 }} spellCheck={false} />
        <input className="mono" placeholder="token (from `aspen status`, if the node needs one)" value={token} onChange={(e) => setToken(e.target.value)} style={{ flex: 1, minWidth: 220 }} spellCheck={false} />
        <button
          className="btn primary sm"
          disabled={!name.trim() || !url.trim() || !!problem}
          onClick={() => {
            setErr(null);
            const c = addConnection({ name: name.trim(), kind: "direct", url: url.trim(), token: token.trim() || null });
            setName("");
            setToken("");
            refresh();
            if (!active) activate(c.id);
          }}
          title="add a node reached directly"
        >
          add node
        </button>
        {relay && node && certified && (
          <button
            className="btn sm"
            onClick={() => {
              const c = addConnection({ name: name.trim() || `${node} via relay`, kind: "relay", relay, node });
              setName("");
              refresh();
              if (!active) activate(c.id);
            }}
            title="save the relay and node from steps 1–3 as a connection"
          >
            save relay → {node}
          </button>
        )}
      </div>
      {problem && url.trim() && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>{problem}</span>}
      {hosted && <p className="dim">A node on another machine has no certificate a browser trusts, so this page cannot call it directly; reach it through a relay, or open the console that node serves itself.</p>}
      <ErrorBar error={err} />
    </section>
  );
}
