// The mesh membership + ceremony panel, shown at the top of the Mesh page.
//
// The API's authority stops at trust: the console SEES the mesh (members,
// certs, link health, why a peer isn't linked), INSPECTS ceremony blobs,
// and AUTHORS changes into a proposal queue — but every mutation is run by
// a human in a shell with `aspen mesh apply`. Blobs are public material,
// so they travel between consoles as URL fragments (deep links).

import { useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { api, type BlobInfo, type MeshInfo, type MeshPeer } from "./api";
import { usePoll } from "./hooks";
import { ErrorBar, relTime } from "./components";

function Copy({ text, label = "copy" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="btn ghost sm"
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setDone(true);
          window.setTimeout(() => setDone(false), 1200);
        });
      }}
    >
      {done ? "copied" : label}
    </button>
  );
}

/** A deep link that opens another console's ceremony panel prefilled. The
 *  host is a guess (the default console port) — the operator adjusts it to
 *  that node's console; the fragment never reaches any server. */
function deepLink(kind: "enroll" | "join" | "cert", blob: string): string {
  return `http://127.0.0.1:7420/mesh#${kind}=${blob}`;
}

function PeerRow({ p, selfVersion }: { p: MeshPeer; selfVersion?: string }) {
  const h = p.health;
  const skew = h?.version && selfVersion && h.version !== selfVersion;
  return (
    <div className="mesh-peer">
      <span className={`dot ${p.link_up ? "dot-idle" : "dot-down"}`} aria-hidden />
      <span className="mono" style={{ color: "var(--text-hi)", minWidth: 120 }}>{p.node}</span>
      <span className="mono-meta">
        {p.link_up
          ? `linked${h?.last_up ? ` ${relTime(h.last_up)}` : ""}`
          : h?.last_down
            ? `down since ${relTime(h.last_down)} ago`
            : "not linked"}
      </span>
      <span className="mono-meta">{p.url ? `dials ${p.url}` : "inbound only (dials us)"}</span>
      <span className="mono-meta">{p.agents} agent{p.agents === 1 ? "" : "s"}</span>
      {p.fingerprint && <span className="mono-meta" title="cert key fingerprint">⌘ {p.fingerprint}</span>}
      {h?.version && (
        <span className="mono-meta" style={{ color: skew ? "var(--sig-normal)" : undefined }} title={h.sha ?? undefined}>
          v{h.version}{skew ? " (skew)" : ""}
        </span>
      )}
      <span style={{ flex: 1 }} />
      {h?.last_error && !p.link_up && (
        <span className="mono-meta" style={{ color: "var(--sig-gate)" }} title={h.last_error}>
          {h.last_error.length > 90 ? `${h.last_error.slice(0, 90)}…` : h.last_error}
          {h.last_error_at ? ` · ${relTime(h.last_error_at)} ago` : ""}
        </span>
      )}
    </div>
  );
}

export function MeshPanel() {
  const poll = usePoll<MeshInfo>(api.mesh, 5000);
  const mesh = poll.data;
  const location = useLocation();
  const nav = useNavigate();
  const [open, setOpen] = useState(false);
  const [blob, setBlob] = useState("");
  const [info, setInfo] = useState<BlobInfo | null>(null);
  const [inspectErr, setInspectErr] = useState<string | null>(null);
  const [url, setUrl] = useState(() => `ws://${window.location.hostname}:${window.location.port || "7420"}/api/federation/ws`);
  const [nodeName, setNodeName] = useState("");
  const [relayUrl, setRelayUrl] = useState("");
  const [err, setErr] = useState<string | null>(null);

  // Deep link: #enroll=… / #join=… / #cert=… opens the panel prefilled.
  useEffect(() => {
    const m = /^#(enroll|join|cert)=(.+)$/.exec(location.hash);
    if (!m) return;
    setBlob(decodeURIComponent(m[2]));
    setOpen(true);
    nav({ pathname: location.pathname, search: location.search, hash: "" }, { replace: true });
  }, [location.hash, location.pathname, location.search, nav]);

  // Inspect whatever is pasted.
  useEffect(() => {
    const b = blob.trim();
    if (!b) {
      setInfo(null);
      setInspectErr(null);
      return;
    }
    let live = true;
    api
      .meshInspect(b)
      .then((i) => live && (setInfo(i), setInspectErr(null)))
      .catch((e) => live && (setInfo(null), setInspectErr(e instanceof Error ? e.message : "not a recognized blob")));
    return () => {
      live = false;
    };
  }, [blob]);

  const pending = mesh?.pending;
  const proposals = pending?.proposals ?? [];
  const outcomes = useMemo(() => [...(pending?.outcomes ?? [])].reverse(), [pending]);

  async function propose(kind: string, args: Record<string, unknown>) {
    setErr(null);
    try {
      await api.meshPropose(kind, args);
      setBlob("");
      setInfo(null);
      await poll.refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "could not queue");
    }
  }

  const inMesh = mesh?.in_mesh === true;
  const me = mesh?.identity;
  const canCertify = inMesh && me?.has_root;
  const summary = mesh
    ? inMesh
      ? `mesh ${mesh.mesh} · node ${mesh.node} · ${(mesh.peers ?? []).filter((p) => p.link_up).length}/${(mesh.peers ?? []).length} peers linked${mesh.relay?.url ? ` · relay ${mesh.relay.connected_at ? "connected" : "down"}` : ""}`
      : me
        ? `not in a mesh yet — identity '${me.node}' enrolled, awaiting a join bundle`
        : "not in a mesh"
    : "…";

  return (
    <div className="strip mesh-panel">
      <div className="mesh-head">
        <span className="label">Mesh</span>
        <span className="mono">{summary}</span>
        {proposals.length > 0 && (
          <span className="chip mono" style={{ color: "var(--sig-normal)" }}>
            {proposals.length} queued — run <code>aspen mesh apply</code>
          </span>
        )}
        <span style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          {open ? "less ▴" : "membership & ceremony ▾"}
        </button>
      </div>

      {open && mesh && (
        <div className="mesh-body">
          <ErrorBar error={err} />

          {/* ── membership ─────────────────────────────────────────── */}
          {me && (
            <div className="mesh-section">
              <div className="mesh-row">
                <span className="mono" style={{ color: "var(--text-hi)" }}>this node · {me.node}</span>
                <span className="mono-meta" title="identity key fingerprint">⌘ {me.fingerprint}</span>
                {me.version && <span className="mono-meta">v{me.version} {me.sha}</span>}
                {me.has_root && <span className="chip mono" title="the mesh root key lives here — certify runs here">root key here</span>}
                <span style={{ flex: 1 }} />
                {me.cert_blob && <Copy text={me.cert_blob} label="copy my cert blob" />}
                {mesh.root_public && <Copy text={mesh.root_public} label="copy root public key" />}
              </div>
              {(mesh.peers ?? []).map((p) => (
                <PeerRow key={p.node} p={p} selfVersion={me.version} />
              ))}
              {inMesh && (mesh.peers ?? []).length === 0 && (
                <span className="mono-meta">no peers yet — add a node below.</span>
              )}
            </div>
          )}

          {/* ── ceremony ───────────────────────────────────────────── */}
          <div className="mesh-section">
            <span className="label">Ceremony — the console authors, a shell applies</span>
            <span className="micro" style={{ color: "var(--text-dim)" }}>
              Trust changes need the keys, which live on disk behind a shell. Queue a step here, then run{" "}
              <code>aspen mesh apply</code> on this node to review and execute it. Blobs are public: paste them, or open the
              deep link the other node's console shows.
            </span>

            {!inMesh && !me && (
              <div className="mesh-row">
                <span className="mono-meta">join a mesh: name this node, queue enroll, apply, then send the enroll blob to the root node.</span>
                <input className="mono" value={nodeName} onChange={(e) => setNodeName(e.target.value)} placeholder="node name (distinct per machine)" style={{ width: 220 }} />
                <button className="btn sm" disabled={!nodeName.trim()} onClick={() => void propose("enroll", { node: nodeName.trim() })}>
                  queue enroll
                </button>
              </div>
            )}
            {!inMesh && me?.enroll_blob && (
              <div className="mesh-row">
                <span className="mono-meta">enroll blob for '{me.node}' — give this to the root node (paste it there, or open the link on its console):</span>
                <Copy text={me.enroll_blob} label="copy enroll blob" />
                <Copy text={deepLink("enroll", me.enroll_blob)} label="copy deep link" />
              </div>
            )}

            <div className="mesh-row" style={{ alignItems: "stretch" }}>
              <textarea
                className="mono"
                value={blob}
                onChange={(e) => setBlob(e.target.value)}
                placeholder={
                  inMesh
                    ? "paste an enroll blob (to certify a new node here) or a cert blob (to register a peer)"
                    : "paste the join bundle the root node produced"
                }
                rows={2}
                style={{ flex: 1, minWidth: 0 }}
                spellCheck={false}
              />
            </div>
            {inspectErr && blob.trim() && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>{inspectErr}</span>}
            {info && (
              <div className="mesh-inspect">
                <div className="mesh-row">
                  <span className="chip mono">{info.kind}</span>
                  <span className="mono" style={{ color: "var(--text-hi)" }}>{info.node}</span>
                  <span className="mono-meta">⌘ {info.fingerprint}</span>
                  {info.mesh && <span className="mono-meta">mesh {info.mesh}</span>}
                  {info.certifier && (
                    <span className="mono-meta">
                      certified by {info.certifier} (⌘ {info.certifier_fingerprint}){info.certifier_url ? ` · dial ${info.certifier_url}` : " · no dial URL"}
                      {info.relay ? ` · relay ${info.relay}` : ""}
                    </span>
                  )}
                </div>
                {info.warnings.map((w) => (
                  <div key={w} className="mono-meta" style={{ color: "var(--sig-gate)" }}>⚠ {w}</div>
                ))}
                <div className="mesh-row">
                  <span className="mono-meta">{info.next}</span>
                  <span style={{ flex: 1 }} />
                  {info.kind === "enroll" && canCertify && (
                    <>
                      <input className="mono" value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: 300 }} title="how the new node will dial THIS node (leave empty if this node dials it)" />
                      <button className="btn primary sm" disabled={info.warnings.length > 0} onClick={() => void propose("certify", { blob: blob.trim(), url: url.trim() || null })}>
                        queue certify
                      </button>
                    </>
                  )}
                  {info.kind === "enroll" && inMesh && !canCertify && (
                    <span className="mono-meta" style={{ color: "var(--sig-normal)" }}>the root key isn't on this node — certify from the root node's console</span>
                  )}
                  {info.kind === "bundle" && !inMesh && (
                    <button className="btn primary sm" onClick={() => void propose("join", { blob: blob.trim() })}>queue join</button>
                  )}
                  {info.kind === "cert" && inMesh && (
                    <>
                      <input className="mono" value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: 300 }} title="dial URL for this peer (empty = it dials us)" />
                      <button className="btn primary sm" disabled={info.warnings.length > 0} onClick={() => void propose("peers_add", { blob: blob.trim(), url: url.trim() || null })}>
                        queue peers-add
                      </button>
                    </>
                  )}
                </div>
              </div>
            )}

            {inMesh && (
              <div className="mesh-row">
                <span className="mono-meta">relay</span>
                <input className="mono" value={relayUrl} onChange={(e) => setRelayUrl(e.target.value)} placeholder={mesh.relay?.url ?? "wss://relay.example/relay"} style={{ width: 300 }} />
                <button className="btn ghost sm" onClick={() => void propose("relay", { url: relayUrl.trim() || null })}>
                  queue {relayUrl.trim() ? "set" : "clear"}
                </button>
              </div>
            )}

            {/* queue + outcomes */}
            {proposals.length > 0 && (
              <div className="mesh-queue">
                <span className="label">Queued — run <code>aspen mesh apply</code> in a shell on this node</span>
                {proposals.map((p) => (
                  <div key={p.id} className="mesh-row">
                    <span className="chip mono">{p.kind}</span>
                    <span className="mono-meta">
                      {p.kind === "certify" && `certify ${String(p.args["blob"] ?? "").slice(0, 28)}…${p.args["url"] ? ` · dial ${p.args["url"]}` : ""}`}
                      {p.kind === "join" && `join with bundle ${String(p.args["blob"] ?? "").slice(0, 28)}…`}
                      {p.kind === "enroll" && `enroll as '${p.args["node"]}'`}
                      {p.kind === "peers_add" && `register peer${p.args["url"] ? ` · dial ${p.args["url"]}` : ""}`}
                      {p.kind === "relay" && `relay → ${p.args["url"] ?? "(clear)"}`}
                    </span>
                    <span className="mono-meta">{relTime(p.created_at)} ago</span>
                    <span style={{ flex: 1 }} />
                    <button className="btn ghost sm" onClick={() => void api.meshWithdraw(p.id).then(poll.refresh)}>withdraw</button>
                  </div>
                ))}
              </div>
            )}
            {outcomes.length > 0 && (
              <div className="mesh-queue">
                <div className="mesh-row">
                  <span className="label">Applied</span>
                  <span style={{ flex: 1 }} />
                  <button className="btn ghost sm" onClick={() => void api.meshClearOutcomes().then(poll.refresh)}>clear</button>
                </div>
                {outcomes.map((o) => (
                  <div key={o.id} className="mesh-outcome">
                    <div className="mesh-row">
                      <span className="chip mono" style={{ color: o.ok ? "var(--live)" : "var(--sig-gate)" }}>{o.ok ? "✓" : "✗"} {o.kind}</span>
                      <span className="mono-meta" style={{ flex: 1, minWidth: 0 }}>{o.message}</span>
                      <span className="mono-meta">{relTime(o.applied_at)} ago</span>
                    </div>
                    {o.artifact && (
                      <div className="mesh-row">
                        <span className="mono-meta">
                          {o.kind === "enroll" ? "enroll blob → give to the root node:" : o.kind === "certify" ? "join bundle → give to the new node:" : "artifact:"}
                        </span>
                        <Copy text={o.artifact} label="copy blob" />
                        {o.kind === "enroll" && <Copy text={deepLink("enroll", o.artifact)} label="copy deep link for the root node's console" />}
                        {o.kind === "certify" && <Copy text={deepLink("join", o.artifact)} label="copy deep link for the new node's console" />}
                        {o.kind === "certify" && <Copy text={`aspen mesh join ${o.artifact}`} label="copy join command" />}
                        {o.kind === "enroll" && <Copy text={`aspen mesh certify ${o.artifact}`} label="copy certify command" />}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
