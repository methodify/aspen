// Attach (docs/RELAY.md §11): make this browser a mesh peer and reach a
// node through a relay, with no daemon of its own. Three steps: an
// identity (kept in this browser), a cert minted by the mesh root from
// the enroll blob shown here, and a relay + node to attach to (the join
// bundle carries the relay). The tunnel then carries every console
// request; the status bar shows it.

import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Tunnel, consoleNodeName, createIdentity, enrollBlob, installBlob, loadConfig, loadIdentity, saveConfig, tunnel, type ConsoleIdentity } from "../tunnel";
import { activate, activeConnection, addConnection, connectionSummary, directUrlProblem, hosted, listConnections, markActive, probeMesh, removeConnection, type Connection } from "../connections";
import { activeProfile, addProfile, listProfiles, onProfilesChange, profileName, readFrom, removeProfile, setProfileLabel, setProfileMesh, switchTo, type Profile } from "../profiles";
import { ErrorBar } from "../components";

export default function Attach() {
  const [params] = useSearchParams();
  const [id, setId] = useState<ConsoleIdentity | null>(() => loadIdentity());
  const [cfg, setCfg] = useState(() => {
    const c = loadConfig();
    return { relay: params.get("relay") ?? c.relay, node: params.get("node") ?? c.node, enabled: c.enabled };
  });
  const [blob, setBlob] = useState("");
  // F-8: the name this console goes by in the mesh — the cert carries it,
  // so it is chosen before the identity is made.
  const [consoleName, setConsoleName] = useState("");
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
        <span className="t-display">{hosted ? "Meshes" : "Attach through a relay"}</span>
        <span className="mono-meta">{hosted ? "the meshes this console is connected to, and how" : "this browser as a mesh peer — no local node needed"}</span>
      </div>
      <div className="stage-body attach-page">
        {hosted && <MeshCards />}
        {hosted && <div className="stage-head" style={{ padding: 0 }}><span className="label">Setting up: {activeProfile() ? profileName(activeProfile()!) : "this mesh"}</span></div>}
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
                  setId(createIdentity(consoleName));
                }}
                title="forget this identity and make a new one (the old cert stops working); the name below is used if given"
              >
                new identity
              </button>
            </div>
          ) : (
            <div className="attach-row">
              <input
                className="mono"
                value={consoleName}
                onChange={(e) => setConsoleName(e.target.value)}
                placeholder="name in this mesh, e.g. bryons-phone (optional)"
                spellCheck={false}
                style={{ flex: 1, minWidth: 220 }}
                title="how this console appears to the mesh's nodes: console-<name>; the cert carries it, so it is chosen now"
              />
              <span className="mono-meta">{consoleNodeName(consoleName)}</span>
              <button className="btn primary sm" onClick={() => setId(createIdentity(consoleName))}>create identity</button>
            </div>
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
                    if (next.cert) setProfileMesh(next.cert.mesh);
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
              {tunnel.state === "down" && (
                <button type="button" className="btn sm" onClick={() => tunnel.reconnect()} title="dial the relay again now — if another tab or device holds this console's connection, this one takes it over">
                  reconnect
                </button>
              )}
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
/** Direct paths (TLS.md §8): once this browser trusts the mesh CA, the
 *  console reaches a node straight over https when it can and keeps the
 *  relay for presence and fallback. This switch pins it to the relay. */
function PreferRelay() {
  const [v, setV] = useState(() => Tunnel.preferRelay());
  const [, setTick] = useState(0);
  useEffect(() => tunnel.onChange(() => setTick((n) => n + 1)), []);
  const direct = tunnel.directNodes;
  return (
    <div className="attach-row">
      <label className="mono-meta" style={{ display: "flex", alignItems: "center", gap: 6 }} title="when off, the console probes each node's https address and goes direct when this browser trusts the mesh certificate; the relay stays registered for presence, mail and fallback">
        <input type="checkbox" checked={v} onChange={(e) => { setV(e.target.checked); Tunnel.setPreferRelay(e.target.checked); }} />
        stay on the relay
      </label>
      <span className="mono-meta">
        {v ? "direct paths off" : direct.length ? `direct to ${direct.join(", ")}` : tunnel.state === "up" ? "no direct path yet (trust the mesh certificate — Meshes → certificate)" : ""}
      </span>
    </div>
  );
}

function Connections({ relay, node, certified }: { relay: string; node: string; certified: boolean }) {
  const [list, setList] = useState<Connection[]>(() => listConnections());
  const active = activeConnection();
  const [name, setName] = useState("");
  const [url, setUrl] = useState("http://127.0.0.1:7420");
  const [token, setToken] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const refresh = () => setList(listConnections());
  const problem = directUrlProblem(url.trim());
  // Hosted, a direct node is filed under the mesh it says it is in
  // (PROPOSALS-2026-09-F.md §2.3): a node from another mesh is refused
  // here and belongs in a profile of its own.
  async function addDirect() {
    setErr(null);
    const u = url.trim();
    const t = token.trim() || null;
    if (hosted) {
      const p = activeProfile();
      try {
        const got = await probeMesh(u, t);
        if (p?.mesh && got.mesh && got.mesh !== p.mesh) {
          setErr(`${got.node || u} is in mesh ${got.mesh}; this is ${p.mesh}. Use "connect to another mesh" for it.`);
          return;
        }
        if (p?.mesh && !got.mesh) {
          setErr(`${got.node || u} is in no mesh; this console's ${p.mesh} is one. Use "connect to another mesh" for it.`);
          return;
        }
        setProfileMesh(got.mesh, got.mesh ? undefined : got.node);
      } catch (e) {
        // Unreachable right now: keep it; the mesh is filled in when it
        // answers (the switcher asks). An https node the browser cannot
        // reach is most often a certificate it does not trust yet — the
        // browser tells JS nothing more than "failed to fetch".
        const why = e instanceof Error ? e.message : "could not reach the node";
        const tls = u.startsWith("https://")
          ? " If the node is up, this browser probably does not trust the mesh certificate yet: on this computer run `aspen tls trust`, or open the Meshes page from a node here and use Certificate → trust."
          : "";
        setErr(`${why} — saved anyway; its mesh is filled in when it answers.${tls}`);
      }
    }
    const c = addConnection({ name: name.trim(), kind: "direct", url: u, token: t });
    setName("");
    setToken("");
    refresh();
    if (!active) activate(c.id);
  }
  return (
    <section className="strip attach-step">
      <span className="label">Connections</span>
      <PreferRelay />
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
        <input className="mono" placeholder="http://127.0.0.1:7420 or https://<node>:7420" value={url} onChange={(e) => setUrl(e.target.value)} style={{ flex: 1, minWidth: 220 }} spellCheck={false} />
        <input className="mono" placeholder="token (from `aspen status`, if the node needs one)" value={token} onChange={(e) => setToken(e.target.value)} style={{ flex: 1, minWidth: 220 }} spellCheck={false} />
        <button
          className="btn primary sm"
          disabled={!name.trim() || !url.trim() || !!problem}
          onClick={() => void addDirect()}
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


/** The meshes this console holds (PROPOSALS-2026-09-F.md §2.3): one
 *  card each — its name, this console's identity there, how it gets in,
 *  whether this device is pushed from it — with switch, rename and
 *  remove; and the way to add another. The active one's setup steps
 *  are the rest of this page. */
function MeshCards() {
  const [, setTick] = useState(0);
  useEffect(() => onProfilesChange(() => setTick((n) => n + 1)), []);
  const active = activeProfile();
  const profiles = listProfiles();
  const [confirm, setConfirm] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const identityOf = (p: Profile): { node: string; mesh: string | null } | null => {
    try {
      const raw = readFrom(p.id, "aspen.console.identity");
      const id = raw ? (JSON.parse(raw) as { node: string; cert?: { mesh: string } | null }) : null;
      return id ? { node: id.node, mesh: id.cert?.mesh ?? null } : null;
    } catch {
      return null;
    }
  };
  return (
    <section className="strip attach-step">
      <span className="label">Meshes</span>
      {profiles.map((p) => {
        const isActive = p.id === active?.id;
        const ident = identityOf(p);
        const push = readFrom(p.id, "aspen.push.on") === "1";
        return (
          <div className="attach-row" key={p.id} style={{ alignItems: "baseline" }}>
            <span className={`chip mono ${isActive ? "op" : ""}`}>{isActive ? "looking at" : "mesh"}</span>
            {editing === p.id ? (
              <input
                className="mono"
                autoFocus
                value={label}
                placeholder={p.mesh ?? "label"}
                onChange={(e) => setLabel(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    setProfileLabel(p.id, label);
                    setEditing(null);
                  }
                  if (e.key === "Escape") setEditing(null);
                }}
                onBlur={() => {
                  setProfileLabel(p.id, label);
                  setEditing(null);
                }}
                style={{ width: 160 }}
              />
            ) : (
              <button
                type="button"
                className="btn ghost sm mono"
                style={{ padding: 0, fontSize: "inherit" }}
                title="rename (a label for this console only; the mesh keeps its name)"
                onClick={() => {
                  setLabel(p.label ?? "");
                  setEditing(p.id);
                }}
              >
                {profileName(p)}
                {p.label && p.mesh ? <span className="mono-meta"> · {p.mesh}</span> : null}
              </button>
            )}
            <span className="mono-meta">
              {ident ? `${ident.node}${ident.mesh ? " · certified" : " · not certified"}` : "no relay identity"} · {connectionSummary(p.id)}
              {push ? " · push on" : ""}
            </span>
            <span style={{ flex: 1 }} />
            {!isActive && (
              <button className="btn sm" onClick={() => switchTo(p.id, "/attach")} title="look at this mesh">
                switch
              </button>
            )}
            {confirm === p.id ? (
              <>
                <span className="mono-meta">forget this mesh and everything remembered about it here?</span>
                <button className="btn sm" style={{ color: "var(--sig-gate)" }} onClick={() => removeProfile(p.id)}>
                  forget
                </button>
                <button className="btn ghost sm" onClick={() => setConfirm(null)}>
                  keep
                </button>
              </>
            ) : (
              <button className="btn ghost sm" onClick={() => setConfirm(p.id)} title="forget this mesh: its identity, connections and everything remembered about it in this browser">
                ×
              </button>
            )}
          </div>
        );
      })}
      <div className="attach-row">
        <button className="btn sm" onClick={() => addProfile()} title="a new identity, certified by that mesh's root, or a node on this machine that is in it">
          connect to another mesh
        </button>
        <span className="mono-meta">each mesh gets its own identity here; a node is in one mesh, this console may look at several</span>
      </div>
    </section>
  );
}
