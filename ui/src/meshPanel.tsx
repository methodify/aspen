// The mesh panel: membership, health, and the ceremony — shaped by where
// this node is in its life.
//
//   solo      no identity, no mesh: a mesh is optional; start one here
//             (become the root) or join one (enroll).
//   enrolled  identity, not yet certified: hand the enroll blob to the root
//             holder, paste the bundle that comes back.
//   root      in a mesh, root key here: add nodes (certify), remove them,
//             keep the root key safe.
//   member    in a mesh, no root key: see the mesh, know where certify
//             happens and get sent there, register peers by cert, leave.
//
// The API's authority stops at trust: the console SEES, INSPECTS, and
// AUTHORS changes into a proposal queue; a human runs `aspen mesh apply` in
// a shell on this node. Blobs are public material and travel between
// consoles as URL fragments (deep links).

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { api, type BlobInfo, type MeshInfo, type MeshPeer } from "./api";
import { usePoll } from "./hooks";
import { useAppData } from "./App";
import { ErrorBar, relTime } from "./components";

type Stage = "solo" | "enrolled" | "root" | "member";

function stageOf(m: MeshInfo | null): Stage | null {
  if (!m) return null;
  if (m.in_mesh) return m.identity?.has_root ? "root" : "member";
  return m.identity ? "enrolled" : "solo";
}

function Copy({ text, label = "copy", primary = false }: { text: string; label?: string; primary?: boolean }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className={`btn ${primary ? "" : "ghost "}sm`}
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

/** A deep link that opens another console's panel prefilled. `base` is that
 *  console's address when known (a peer's dial URL tells us); otherwise the
 *  default console port, for the operator to adjust. The fragment never
 *  reaches any server. */
function deepLink(kind: "enroll" | "join" | "cert", blob: string, base?: string | null): string {
  return `${base ?? "http://127.0.0.1:7420"}/mesh#${kind}=${blob}`;
}

/** The install one-liner for the platform we're likely talking about. */
const INSTALL_SH = "curl -fsSL https://raw.githubusercontent.com/methodify/aspen/main/install.sh | sh";
const INSTALL_PS = "irm https://raw.githubusercontent.com/methodify/aspen/main/install.ps1 | iex";

function Step({ n, title, done, children }: { n: number; title: ReactNode; done?: boolean; children?: ReactNode }) {
  return (
    <div className="mesh-step">
      <span className={`mesh-step-n${done ? " done" : ""}`}>{done ? "✓" : n}</span>
      <div className="mesh-step-body">
        <div className="mesh-step-title">{title}</div>
        {children}
      </div>
    </div>
  );
}

function Orientation() {
  const [open, setOpen] = useState(false);
  return (
    <div className="mesh-section" style={{ borderTop: "none", paddingTop: 0 }}>
      <button className="btn ghost sm" style={{ alignSelf: "flex-start" }} onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        how a mesh works {open ? "▴" : "▾"}
      </button>
      {open && (
        <div className="mesh-inspect" style={{ gap: 8 }}>
          <div><b>A mesh</b> is the set of machines one console can see and drive. One machine is fine on its own — a mesh is for when you have two.</div>
          <div><b>The root key</b> is the mesh: one file (<code>root.key</code>) on the machine where you created it. It signs every node in. Back it up; never copy it to machines that don't certify.</div>
          <div><b>A node identity</b> is a keypair each machine makes for itself. Its <b>enroll blob</b> is the public half — safe to paste anywhere.</div>
          <div><b>Certify</b> happens where the root key lives: an enroll blob goes in, a <b>join bundle</b> comes out (the new node's cert, plus how to reach the certifier). Paste the bundle on the new node and it's in.</div>
          <div><b>The shell rule:</b> every step that touches keys runs from a shell on that machine (<code>aspen mesh apply</code>). The console queues the step, shows you what it will do, and hands blobs between consoles as links.</div>
        </div>
      )}
    </div>
  );
}

function PeerRow({ p, selfVersion, onRemove }: { p: MeshPeer; selfVersion?: string; onRemove?: () => void }) {
  const h = p.health;
  const skew = h?.version && selfVersion && h.version !== selfVersion;
  const [confirm, setConfirm] = useState(false);
  return (
    <div className="mesh-peer">
      <span className={`dot ${p.link_up ? "dot-idle" : "dot-down"}`} aria-hidden />
      <span className="mono" style={{ color: "var(--text-hi)", minWidth: 120 }}>{p.node}</span>
      {p.has_root && <span className="chip mono" title="the mesh's root key lives there — certify on that node">root key</span>}
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
      {p.console_url && (
        <a className="mono-meta" href={p.console_url} target="_blank" rel="noreferrer" title="that node's console (a guess from its dial URL)">
          console ↗
        </a>
      )}
      <span style={{ flex: 1 }} />
      {h?.last_error && !p.link_up && (
        <span className="mono-meta" style={{ color: "var(--sig-gate)" }} title={h.last_error}>
          {h.last_error.length > 90 ? `${h.last_error.slice(0, 90)}…` : h.last_error}
          {h.last_error_at ? ` · ${relTime(h.last_error_at)} ago` : ""}
        </span>
      )}
      {onRemove &&
        (confirm ? (
          <>
            <span className="mono-meta">forget {p.node} here?</span>
            <button className="btn danger sm" onClick={() => { setConfirm(false); onRemove(); }}>queue remove</button>
            <button className="btn ghost sm" onClick={() => setConfirm(false)}>cancel</button>
          </>
        ) : (
          <button className="btn ghost sm" onClick={() => setConfirm(true)} title="stop dialing/accepting this node here (its cert is not revoked — remove it on every node that lists it)">
            remove…
          </button>
        ))}
    </div>
  );
}

export function MeshPanel() {
  const poll = usePoll<MeshInfo>(api.mesh, 5000);
  const mesh = poll.data;
  const stage = stageOf(mesh);
  const location = useLocation();
  const nav = useNavigate();
  const [open, setOpen] = useState<boolean | null>(null);
  const [blob, setBlob] = useState("");
  const [info, setInfo] = useState<BlobInfo | null>(null);
  const [inspectErr, setInspectErr] = useState<string | null>(null);
  const { node: nodeInfo } = useAppData();
  // The dial URL other machines use for THIS node: its hostname (never
  // "localhost" — that is only where the browser is), on the port it
  // listens on. The operator swaps in a LAN IP or tailnet name if needed.
  const [url, setUrl] = useState("");
  const [urlTouched, setUrlTouched] = useState(false);
  useEffect(() => {
    if (urlTouched || !nodeInfo) return;
    const port = nodeInfo.listen?.split(":").pop() || window.location.port || "7420";
    const browserHost = window.location.hostname;
    const host =
      browserHost && browserHost !== "localhost" && browserHost !== "127.0.0.1" && browserHost !== "[::1]"
        ? browserHost
        : nodeInfo.hostname || browserHost || "this-host";
    setUrl(`ws://${host}:${port}/api/federation/ws`);
  }, [nodeInfo, urlTouched]);
  const setUrlByHand = (u: string) => {
    setUrlTouched(true);
    setUrl(u);
  };
  const reachability = nodeInfo?.loopback_only ? (
    <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>
      ⚠ this node listens on {nodeInfo.listen ?? "127.0.0.1"} — loopback only, so no other machine can dial it. Before certifying, on this
      machine: <code>aspen config listen 0.0.0.0:{nodeInfo.listen?.split(":").pop() || "7420"}</code> then <code>aspen restart</code> (allow it
      through the firewall; the console URL then carries a token — <code>aspen status</code> prints it). Or leave the dial URL empty and have
      this node dial the new one instead.
    </span>
  ) : null;
  const [nodeName, setNodeName] = useState("");
  const [meshName, setMeshName] = useState("");
  const [relayUrl, setRelayUrl] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [advanced, setAdvanced] = useState(false);
  const [confirmLeave, setConfirmLeave] = useState(false);

  // First-timers (no mesh yet) see the guide open; established nodes see
  // a one-line summary until they ask.
  const isOpen = open ?? (stage === "solo" || stage === "enrolled");

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
  const queued = (kind: string) => proposals.some((p) => p.kind === kind);

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

  const me = mesh?.identity;
  const peers = mesh?.peers ?? [];
  const rootPeer = peers.find((p) => p.has_root);
  const linked = peers.filter((p) => p.link_up).length;
  const summary = mesh
    ? stage === "root" || stage === "member"
      ? `mesh ${mesh.mesh} · node ${mesh.node} · ${linked}/${peers.length} peer${peers.length === 1 ? "" : "s"} linked${mesh.relay?.url ? ` · relay ${mesh.relay.connected_at ? "connected" : "down"}` : ""}${stage === "root" ? " · root key here" : rootPeer ? ` · root key on ${rootPeer.node}` : ""}`
      : stage === "enrolled"
        ? `not in a mesh yet — identity '${me?.node}' enrolled, waiting for its join bundle`
        : "not in a mesh — this machine on its own"
    : "…";
  const applyHint = <span className="mono-meta">then, in a shell on this machine: <code>aspen mesh apply</code></span>;
  const lastCertify = outcomes.find((o) => o.kind === "certify" && o.ok && o.artifact);
  const lastCertified = lastCertify && /certified '([^']+)'/.exec(lastCertify.message)?.[1];

  return (
    <div className="strip mesh-panel">
      <div className="mesh-head">
        <span className="label">Mesh</span>
        <span className="mono">{summary}</span>
        {stage && <span className="chip mono" title="where this node is in its mesh life">{stage === "solo" ? "solo" : stage === "enrolled" ? "enrolled" : stage === "root" ? "root node" : "member"}</span>}
        {proposals.length > 0 && (
          <span className="chip mono" style={{ color: "var(--sig-normal)" }}>
            {proposals.length} queued — run <code>aspen mesh apply</code>
          </span>
        )}
        <span style={{ flex: 1 }} />
        <button className="btn ghost sm" onClick={() => setOpen(!isOpen)} aria-expanded={isOpen}>
          {isOpen ? "less ▴" : stage === "root" || stage === "member" ? "members & add a node ▾" : "set up ▾"}
        </button>
      </div>

      {isOpen && mesh && (
        <div className="mesh-body">
          <ErrorBar error={err} />
          <Orientation />

          {/* ── solo: three ways forward ─────────────────────────────── */}
          {stage === "solo" && (
            <div className="mesh-choices">
              <div className="mesh-choice">
                <span className="label">Just this machine</span>
                <span className="micro" style={{ color: "var(--text-mid)" }}>
                  Nothing to do. Every feature works on one machine — sessions, repos, the bus, history. Come back here when you add a second one.
                </span>
              </div>
              <div className="mesh-choice">
                <span className="label">Start a mesh here</span>
                <span className="micro" style={{ color: "var(--text-mid)" }}>
                  This machine becomes the <b>root</b>: the root key is created here, and every other machine gets certified from this console. Pick the machine you'll keep.
                </span>
                <input className="mono" value={meshName} onChange={(e) => setMeshName(e.target.value)} placeholder="mesh name, e.g. home" spellCheck={false} aria-label="mesh name" />
                <input className="mono" value={nodeName} onChange={(e) => setNodeName(e.target.value)} placeholder={`this node's name, e.g. ${mesh.node}`} spellCheck={false} aria-label="node name" />
                <div className="mesh-row">
                  <button className="btn primary sm" disabled={!meshName.trim() || queued("init")} onClick={() => void propose("init", { mesh: meshName.trim(), node: nodeName.trim() || mesh.node })}>
                    {queued("init") ? "queued" : "queue: start mesh"}
                  </button>
                  {queued("init") && applyHint}
                </div>
              </div>
              <div className="mesh-choice">
                <span className="label">Join an existing mesh</span>
                <span className="micro" style={{ color: "var(--text-mid)" }}>
                  Another machine already holds a root key. This one enrolls: it makes its identity and hands you an enroll blob to take to that machine's console.
                </span>
                <input className="mono" value={nodeName} onChange={(e) => setNodeName(e.target.value)} placeholder={`this node's name, e.g. ${mesh.node}`} spellCheck={false} aria-label="node name" />
                <span className="micro" style={{ color: "var(--text-dim)" }}>names must be distinct per machine — a Windows box and its WSL share a hostname.</span>
                <div className="mesh-row">
                  <button className="btn primary sm" disabled={!nodeName.trim() || queued("enroll")} onClick={() => void propose("enroll", { node: nodeName.trim() })}>
                    {queued("enroll") ? "queued" : "queue: enroll"}
                  </button>
                  {queued("enroll") && applyHint}
                </div>
              </div>
            </div>
          )}

          {/* ── enrolled: the hand-off ───────────────────────────────── */}
          {stage === "enrolled" && me && (
            <div className="mesh-section">
              <Step n={1} title={<>identity <span className="mono">{me.node}</span> created (⌘ {me.fingerprint})</>} done />
              <Step n={2} title="give the enroll blob to the machine that holds the root key">
                <div className="mesh-row">
                  {me.enroll_blob && (
                    <>
                      <Copy text={me.enroll_blob} label="copy enroll blob" primary />
                      <Copy text={deepLink("enroll", me.enroll_blob)} label="copy link for its console" />
                      <Copy text={`aspen mesh certify ${me.enroll_blob}${url ? ` --url ${url}` : ""}`} label="copy certify command" />
                    </>
                  )}
                </div>
                <span className="micro" style={{ color: "var(--text-dim)" }}>
                  On that machine: open the link (or paste the blob into its Mesh panel), queue <b>certify</b>, run <code>aspen mesh apply</code> there. It answers with a <b>join bundle</b>.
                  The certify step asks how this node will dial that one — make sure that node listens beyond loopback (<code>aspen config listen 0.0.0.0:7420</code> there).
                </span>
              </Step>
              <Step n={3} title="paste the join bundle here">
                <textarea className="mono" value={blob} onChange={(e) => setBlob(e.target.value)} placeholder="aspen:bundle:…" rows={2} style={{ width: "100%" }} spellCheck={false} />
                {inspectErr && blob.trim() && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>{inspectErr}</span>}
                {info?.kind === "bundle" && (
                  <div className="mesh-row">
                    <span className="mono-meta">
                      bundle for <span className="mono">{info.node}</span> · mesh {info.mesh} · certified by {info.certifier}
                      {info.certifier_url ? ` · will dial ${info.certifier_url}` : " · no dial URL (that node must dial us)"}
                    </span>
                    {info.warnings.map((w) => (
                      <span key={w} className="mono-meta" style={{ color: "var(--sig-gate)" }}>⚠ {w}</span>
                    ))}
                    <button className="btn primary sm" disabled={queued("join")} onClick={() => void propose("join", { blob: blob.trim() })}>
                      {queued("join") ? "queued" : "queue: join"}
                    </button>
                    {queued("join") && applyHint}
                  </div>
                )}
                {info && info.kind !== "bundle" && (
                  <span className="mono-meta" style={{ color: "var(--sig-normal)" }}>that's a {info.kind} blob — this step wants the join bundle the root node produced.</span>
                )}
              </Step>
              <div className="mesh-row" style={{ marginTop: 6 }}>
                <span className="micro" style={{ color: "var(--text-dim)" }}>Changed your mind — this should be the root instead?</span>
                <input className="mono" value={meshName} onChange={(e) => setMeshName(e.target.value)} placeholder="mesh name" style={{ width: 160 }} spellCheck={false} />
                <button className="btn ghost sm" disabled={!meshName.trim() || queued("init")} onClick={() => void propose("init", { mesh: meshName.trim(), node: me.node })}>
                  {queued("init") ? "queued" : `start mesh here as ${me.node}`}
                </button>
              </div>
            </div>
          )}

          {/* ── in a mesh: members ───────────────────────────────────── */}
          {(stage === "root" || stage === "member") && me && (
            <div className="mesh-section">
              <div className="mesh-row">
                <span className="mono" style={{ color: "var(--text-hi)" }}>this node · {me.node}</span>
                <span className="mono-meta" title="identity key fingerprint">⌘ {me.fingerprint}</span>
                {me.version && <span className="mono-meta">v{me.version} {me.sha}</span>}
                {me.has_root && <span className="chip mono" title="the mesh root key lives here">root key</span>}
                <span style={{ flex: 1 }} />
                {me.cert_blob && <Copy text={me.cert_blob} label="copy my cert blob" />}
                {mesh.root_public && <Copy text={mesh.root_public} label="copy root public key" />}
              </div>
              {peers.map((p) => (
                <PeerRow key={p.node} p={p} selfVersion={me.version} onRemove={() => void propose("peers_remove", { node: p.node })} />
              ))}
              {peers.length === 0 && <span className="mono-meta">no other nodes yet — add one below.</span>}
              {stage === "root" && me.root_key_path && (
                <span className="micro" style={{ color: "var(--text-dim)" }}>
                  root key: <code>{me.root_key_path}</code> — it <b>is</b> the mesh. Back it up somewhere safe; if this disk dies, no new node can ever be certified.
                </span>
              )}
            </div>
          )}

          {/* ── root: add a node, guided ─────────────────────────────── */}
          {stage === "root" && (
            <div className="mesh-section">
              <span className="label">Add a node</span>
              <Step n={1} title="on the new machine: install aspen and enroll it">
                <div className="mesh-row">
                  <Copy text={INSTALL_SH} label="copy install (linux/mac)" />
                  <Copy text={INSTALL_PS} label="copy install (windows)" />
                  <Copy text={`aspen mesh enroll --node <name>`} label="copy enroll command" />
                </div>
                <span className="micro" style={{ color: "var(--text-dim)" }}>
                  or open its console → Mesh → <i>Join an existing mesh</i>. Either way it hands you an <b>enroll blob</b> (or a link that opens this panel).
                </span>
              </Step>
              <Step n={2} title="paste its enroll blob here and certify">
                <textarea className="mono" value={blob} onChange={(e) => setBlob(e.target.value)} placeholder="aspen:enroll:…  (a cert blob also works here: it registers a peer without certifying)" rows={2} style={{ width: "100%" }} spellCheck={false} />
                {inspectErr && blob.trim() && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>{inspectErr}</span>}
                {reachability}
                {info && <InspectResult info={info} blob={blob} url={url} setUrl={setUrlByHand} stage={stage} queued={queued} propose={propose} applyHint={applyHint} />}
              </Step>
              <Step n={3} title="send the join bundle back to the new machine" done={!!lastCertify}>
                {lastCertify ? (
                  <div className="mesh-row">
                    <span className="mono-meta">bundle for <span className="mono">{lastCertified ?? "the new node"}</span> · {relTime(lastCertify.applied_at)} ago</span>
                    <Copy text={deepLink("join", lastCertify.artifact!)} label="copy link for its console" primary />
                    <Copy text={lastCertify.artifact!} label="copy bundle" />
                    <Copy text={`aspen mesh join ${lastCertify.artifact}`} label="copy join command" />
                  </div>
                ) : (
                  <span className="micro" style={{ color: "var(--text-dim)" }}>appears here once <code>aspen mesh apply</code> has run the certify.</span>
                )}
              </Step>
            </div>
          )}

          {/* ── member: where things happen ──────────────────────────── */}
          {stage === "member" && (
            <div className="mesh-section">
              <span className="label">Add a node</span>
              <span className="micro" style={{ color: "var(--text-mid)" }}>
                Certifying needs the root key, which is{" "}
                {rootPeer ? (
                  <>
                    on <span className="mono">{rootPeer.node}</span>
                    {rootPeer.console_url ? (
                      <>
                        {" "}— <a href={rootPeer.console_url + "/mesh"} target="_blank" rel="noreferrer">open its console ↗</a>
                      </>
                    ) : null}
                  </>
                ) : (
                  "on a node that isn't linked right now"
                )}
                . Enroll the new machine, then take its blob there. This node can still <i>register</i> a certified peer by its cert blob:
              </span>
              <textarea className="mono" value={blob} onChange={(e) => setBlob(e.target.value)} placeholder="aspen:cert:…  (or an enroll blob: it becomes a link to the root node's console)" rows={2} style={{ width: "100%" }} spellCheck={false} />
              {inspectErr && blob.trim() && <span className="mono-meta" style={{ color: "var(--sig-gate)" }}>{inspectErr}</span>}
              {info?.kind === "enroll" && (
                <div className="mesh-row">
                  <span className="mono-meta">an enroll blob for <span className="mono">{info.node}</span> — certify it on the root node:</span>
                  <Copy text={deepLink("enroll", blob.trim(), rootPeer?.console_url)} label={`copy link for ${rootPeer?.node ?? "the root node"}'s console`} primary />
                  {rootPeer?.console_url && (
                    <a className="btn sm" href={deepLink("enroll", blob.trim(), rootPeer.console_url)} target="_blank" rel="noreferrer">open there ↗</a>
                  )}
                </div>
              )}
              {info && info.kind !== "enroll" && <InspectResult info={info} blob={blob} url={url} setUrl={setUrlByHand} stage={stage} queued={queued} propose={propose} applyHint={applyHint} />}
            </div>
          )}

          {/* ── relay + advanced ─────────────────────────────────────── */}
          {(stage === "root" || stage === "member") && (
            <div className="mesh-section">
              <div className="mesh-row">
                <span className="mono-meta" title="a rendezvous relay lets nodes with no direct path reach each other">relay</span>
                <input className="mono" value={relayUrl} onChange={(e) => setRelayUrl(e.target.value)} placeholder={mesh.relay?.url ?? "wss://relay.example/relay (optional)"} style={{ width: 300 }} spellCheck={false} />
                <button className="btn ghost sm" onClick={() => void propose("relay", { url: relayUrl.trim() || null })}>
                  queue {relayUrl.trim() ? "set" : "clear"}
                </button>
                <span style={{ flex: 1 }} />
                <button className="btn ghost sm" onClick={() => setAdvanced((v) => !v)} aria-expanded={advanced}>advanced {advanced ? "▴" : "▾"}</button>
              </div>
              {advanced && (
                <div className="mesh-inspect">
                  {stage === "root" && (
                    <div className="mesh-row">
                      <span className="mono-meta">register a certified peer by its cert blob (without certifying): paste it in the box above.</span>
                    </div>
                  )}
                  <div className="mesh-row">
                    <span className="mono-meta">
                      {stage === "root"
                        ? "leave the mesh: this node holds the root key, so leaving ENDS the mesh — every node it certified is orphaned."
                        : "leave the mesh: drops membership and this node's cert; the keypair stays for a future enroll."}
                    </span>
                    <span style={{ flex: 1 }} />
                    {confirmLeave ? (
                      <>
                        <button className="btn danger sm" disabled={queued("leave")} onClick={() => { setConfirmLeave(false); void propose("leave", stage === "root" ? { discard_root: true } : {}); }}>
                          queue: leave{stage === "root" ? " and discard root key" : ""}
                        </button>
                        <button className="btn ghost sm" onClick={() => setConfirmLeave(false)}>cancel</button>
                      </>
                    ) : (
                      <button className="btn ghost sm" disabled={queued("leave")} onClick={() => setConfirmLeave(true)}>{queued("leave") ? "leave queued" : "leave…"}</button>
                    )}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* ── queue + outcomes ─────────────────────────────────────── */}
          {proposals.length > 0 && (
            <div className="mesh-queue">
              <span className="label">Queued — run <code>aspen mesh apply</code> in a shell on this machine</span>
              {proposals.map((p) => (
                <div key={p.id} className="mesh-row">
                  <span className="chip mono">{p.kind}</span>
                  <span className="mono-meta">{describeProposal(p.kind, p.args)}</span>
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
                        {o.kind === "enroll" ? "enroll blob → give to the root node:" : o.kind === "certify" ? "join bundle → give to the new node:" : o.kind === "init" ? "this node's cert blob:" : "artifact:"}
                      </span>
                      <Copy text={o.artifact} label="copy blob" />
                      {o.kind === "enroll" && <Copy text={deepLink("enroll", o.artifact, rootPeer?.console_url)} label="copy link for the root node's console" />}
                      {o.kind === "enroll" && <Copy text={`aspen mesh certify ${o.artifact}`} label="copy certify command" />}
                      {o.kind === "certify" && <Copy text={deepLink("join", o.artifact)} label="copy link for the new node's console" />}
                      {o.kind === "certify" && <Copy text={`aspen mesh join ${o.artifact}`} label="copy join command" />}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function describeProposal(kind: string, args: Record<string, unknown>): string {
  const s = (k: string) => String(args[k] ?? "");
  switch (kind) {
    case "init":
      return `create mesh '${s("mesh")}' here as node '${s("node")}' — mints the root key on this machine`;
    case "enroll":
      return `enroll this node as '${s("node")}'`;
    case "certify":
      return `certify ${s("blob").slice(0, 28)}…${args["url"] ? ` · it will dial ${s("url")}` : " · it must dial us"}`;
    case "join":
      return `join with bundle ${s("blob").slice(0, 28)}…`;
    case "peers_add":
      return `register peer${args["url"] ? ` · dial ${s("url")}` : ""}`;
    case "peers_remove":
      return `forget peer '${s("node")}'`;
    case "relay":
      return `relay → ${args["url"] ?? "(clear)"}`;
    case "leave":
      return args["discard_root"] ? "LEAVE the mesh and discard the root key (ends the mesh)" : "leave the mesh";
    default:
      return kind;
  }
}

/** What a pasted blob is and the action it affords at this stage. */
function InspectResult({
  info,
  blob,
  url,
  setUrl,
  stage,
  queued,
  propose,
  applyHint,
}: {
  info: BlobInfo;
  blob: string;
  url: string;
  setUrl: (u: string) => void;
  stage: Stage;
  queued: (k: string) => boolean;
  propose: (kind: string, args: Record<string, unknown>) => Promise<void>;
  applyHint: ReactNode;
}) {
  return (
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
        {info.kind === "enroll" && stage === "root" && (
          <>
            <span className="mono-meta" title="an address the new machine can reach this one at: hostname, LAN IP, or tailnet name">{info.node} dials this node at</span>
            <input className="mono" value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: 320 }} placeholder="empty: this node will dial it instead" title="how the new node will dial THIS node — never localhost; empty if this node will dial it instead" spellCheck={false} />
            <button className="btn primary sm" disabled={info.warnings.length > 0 || queued("certify")} onClick={() => void propose("certify", { blob: blob.trim(), url: url.trim() || null })}>
              {queued("certify") ? "queued" : "queue: certify"}
            </button>
          </>
        )}
        {info.kind === "bundle" && (stage === "root" || stage === "member") && (
          <span className="mono-meta" style={{ color: "var(--sig-normal)" }}>a join bundle is for the node that enrolled — paste it on that machine.</span>
        )}
        {info.kind === "cert" && (
          <>
            <input className="mono" value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: 300 }} title="dial URL for this peer (empty = it dials us)" spellCheck={false} />
            <button className="btn primary sm" disabled={info.warnings.length > 0 || queued("peers_add")} onClick={() => void propose("peers_add", { blob: blob.trim(), url: url.trim() || null })}>
              {queued("peers_add") ? "queued" : "queue: register peer"}
            </button>
          </>
        )}
      </div>
      {(queued("certify") || queued("peers_add")) && <div className="mesh-row">{applyHint}</div>}
    </div>
  );
}
