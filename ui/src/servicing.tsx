// Servicing — updates, drain state, inventory, rollout (docs/SERVICING.md).
//
// One hook over two polls (this node's GET /api/update, the mesh's peer
// health) yields one row per node; the Now card and the Mesh list's Nodes
// panel are two views of those rows. Two verbs on purpose: "update" waits
// for the node to be quiet; "update now" interrupts and says so first.

import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  api,
  type Agent,
  type Inventory,
  type MeshInfo,
  type Rollout,
  type UpdateOutcome,
  type UpdatePolicy,
  type UpdateStatus,
} from "./api";
import { usePoll } from "./hooks";
import { useAppData } from "./App";
import { ErrorBar, relTime } from "./components";
import { Modal } from "./channels";

export interface NodeRow {
  node: string;
  self: boolean;
  link_up: boolean;
  version: string | null;
  available: string | null;
  state: "ready" | "draining" | "updating" | null;
  detail: string | null;
  policy: string | null;
  inventory: Inventory | null;
  last_outcome: Partial<UpdateOutcome> | null;
}

export interface FleetServicing {
  self: UpdateStatus | null;
  mesh: MeshInfo | null;
  rows: NodeRow[];
  /** Nodes with a newer release known to them (not snoozed on self). */
  behind: NodeRow[];
  rollout: Rollout | null;
  refresh: () => Promise<void>;
  error: string | null;
}

export function useFleetServicing(ms = 10000): FleetServicing {
  const selfPoll = usePoll<UpdateStatus>(api.update, ms);
  const meshPoll = usePoll<MeshInfo>(api.mesh, ms);
  const self = selfPoll.data;
  const mesh = meshPoll.data;
  const rows = useMemo<NodeRow[]>(() => {
    const out: NodeRow[] = [];
    if (self) {
      out.push({
        node: mesh?.node ?? "this node",
        self: true,
        link_up: true,
        version: self.current,
        available: self.available && !self.skipped ? self.available.version : null,
        state: self.state.state,
        detail: self.state.state === "draining" ? draining(self.state) : self.state.state === "updating" ? "updater running" : null,
        policy: self.policy.mode ?? "notify",
        inventory: self.inventory,
        last_outcome: self.last_outcome,
      });
    }
    for (const p of mesh?.peers ?? []) {
      const h = p.health;
      out.push({
        node: p.node,
        self: false,
        link_up: p.link_up,
        version: h?.version ?? null,
        available: h?.update_available ?? null,
        state: h?.service_state ?? null,
        detail: h?.service_detail ?? null,
        policy: h?.policy ?? null,
        inventory: h?.inventory ?? null,
        last_outcome: h?.last_outcome ?? null,
      });
    }
    return out;
  }, [self, mesh]);
  const behind = rows.filter((r) => r.available);
  return {
    self,
    mesh,
    rows,
    behind,
    rollout: self?.rollout ?? null,
    refresh: async () => {
      await Promise.all([selfPoll.refresh(), meshPoll.refresh()]);
    },
    error: selfPoll.error ?? meshPoll.error,
  };
}

function draining(s: Extract<UpdateStatus["state"], { state: "draining" }>): string {
  const w = s.waiting_on.length ? `waiting on: ${s.waiting_on.join(", ")}` : "about to start";
  return `${s.overdue ? "overdue · " : ""}${s.when === "now" ? "now · " : ""}${w}`;
}

export function fmtUptime(startedAt: number | null | undefined): string {
  if (!startedAt) return "";
  const s = Date.now() / 1000 - startedAt;
  if (s < 3600) return `${Math.max(1, Math.floor(s / 60))}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
}

/** Sessions on a node that would lose their turn to an immediate restart. */
function busyOn(agents: Agent[], row: NodeRow): Agent[] {
  return agents.filter(
    (a) => a.live && a.turn_state === "busy" && (row.self ? !a.remote : a.remote && a.node === row.node),
  );
}

function StateChip({ row }: { row: NodeRow }) {
  if (!row.state || row.state === "ready") return null;
  const color = row.detail?.startsWith("overdue") ? "var(--sig-gate)" : "var(--sig-normal)";
  return (
    <span className="chip mono" style={{ color }} title={row.detail ?? undefined}>
      {row.state}
      {row.detail ? ` · ${row.detail.length > 70 ? `${row.detail.slice(0, 70)}…` : row.detail}` : ""}
    </span>
  );
}

function OutcomeChip({ o }: { o: Partial<UpdateOutcome> | null }) {
  if (!o || !o.finished_at) return null;
  if (Date.now() / 1000 - o.finished_at > 7 * 86400) return null;
  const label = o.rolled_back ? "rolled back" : o.ok ? "updated" : "update failed";
  const color = o.ok && !o.rolled_back ? "var(--live)" : "var(--sig-gate)";
  return (
    <span className="mono-meta" style={{ color }} title={o.error ?? undefined}>
      {label} {o.from} → {o.to} · {relTime(o.finished_at)} ago
      {o.error ? ` — ${o.error.length > 80 ? `${o.error.slice(0, 80)}…` : o.error}` : ""}
    </span>
  );
}

/** "Update now" interrupts turns; say which before doing it. */
function ConfirmNow({
  row,
  busy,
  onConfirm,
  onClose,
}: {
  row: NodeRow;
  busy: Agent[];
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Modal title={`Update ${row.node} now`} onClose={onClose}>
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <span>
          This restarts the node without waiting for sessions to go idle. Every session is revived afterwards, but a
          turn in flight is lost — its last tool call does not complete.
        </span>
        {busy.length > 0 ? (
          <span className="mono" style={{ color: "var(--sig-gate)" }}>
            busy right now: {busy.map((a) => `@${a.bare ?? a.name}`).join(", ")}
          </span>
        ) : (
          <span className="mono-meta">no session on {row.node} is mid-turn right now.</span>
        )}
        <div className="mesh-row">
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onClose}>cancel</button>
          <button className="btn primary sm" onClick={onConfirm}>update now</button>
        </div>
      </div>
    </Modal>
  );
}

function LogsDrawer({ node, self, onClose }: { node: string; self: boolean; onClose: () => void }) {
  const [lines, setLines] = useState<string[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  async function load() {
    try {
      const r = await api.logs(self ? undefined : node, 300);
      setLines(r.lines);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "could not read log");
    }
  }
  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [node]);
  return (
    <Modal title={`aspen.log · ${node}`} onClose={onClose}>
      <div style={{ display: "flex", flexDirection: "column", gap: 8, minWidth: "min(80vw, 900px)" }}>
        <ErrorBar error={err} />
        <pre className="svc-log">{lines === null ? "…" : lines.length ? lines.join("\n") : "(empty)"}</pre>
        <div className="mesh-row">
          <span className="mono-meta">last 300 lines</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={() => void load()}>refresh</button>
        </div>
      </div>
    </Modal>
  );
}

function PolicyEditor({
  initial,
  nodeName,
  meshed,
  onSaved,
}: {
  initial: UpdatePolicy;
  nodeName: string;
  meshed: boolean;
  onSaved: () => void;
}) {
  const [mode, setMode] = useState<"notify" | "auto">(initial.mode === "auto" ? "auto" : "notify");
  const [window_, setWindow] = useState(initial.window ?? "");
  const [soak, setSoak] = useState(initial.soak ?? "");
  const [check, setCheck] = useState(initial.check !== false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const policy = (): UpdatePolicy => ({
    mode,
    window: window_.trim() || null,
    soak: soak.trim() || null,
    skip: initial.skip ?? null,
    check: check ? null : false,
  });
  async function save(all: boolean) {
    setBusy(true);
    setErr(null);
    setMsg(null);
    try {
      const r = await api.setUpdatePolicy(policy(), all ? "*" : undefined);
      if (r.results) {
        const bad = Object.entries(r.results).filter(([, v]) => !v.ok);
        setMsg(bad.length ? `applied; failed on ${bad.map(([n, v]) => `${n} (${v.error})`).join(", ")}` : `applied to ${Object.keys(r.results).length} nodes`);
      } else {
        setMsg("saved");
      }
      onSaved();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "could not save");
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="mesh-inspect">
      <div className="mesh-row">
        <span className="label">Update policy · {nodeName}</span>
      </div>
      <div className="mesh-row">
        <span className="mono-meta" style={{ minWidth: 60 }}>mode</span>
        <div className="class-select" role="radiogroup" aria-label="update mode">
          {(["notify", "auto"] as const).map((m) => (
            <button
              key={m}
              type="button"
              role="radio"
              aria-checked={mode === m}
              onClick={() => setMode(m)}
              style={{
                background: mode === m ? "var(--bg-strip-2)" : "var(--bg-well)",
                color: mode === m ? "var(--text-hi)" : "var(--text-dim)",
              }}
            >
              {m}
            </button>
          ))}
        </div>
        <span className="micro" style={{ color: "var(--text-dim)" }}>
          {mode === "auto"
            ? "apply a newer release once every session has been idle 5 min, nothing is pending, and nothing just spawned"
            : "check and show; never apply on its own"}
        </span>
      </div>
      <div className="mesh-row">
        <span className="mono-meta" style={{ minWidth: 60 }}>window</span>
        <input className="mono" value={window_} onChange={(e) => setWindow(e.target.value)} placeholder="02:00-06:00 (local time)" style={{ width: 200 }} spellCheck={false} />
        <span className="mono-meta" style={{ minWidth: 60 }}>soak</span>
        <input className="mono" value={soak} onChange={(e) => setSoak(e.target.value)} placeholder="24h" style={{ width: 90 }} spellCheck={false} />
        <label className="mono-meta" style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <input type="checkbox" checked={check} onChange={(e) => setCheck(e.target.checked)} /> check the release channel
        </label>
      </div>
      <div className="mesh-row">
        {initial.skip && <span className="mono-meta">snoozed: v{initial.skip}</span>}
        <span style={{ flex: 1 }} />
        {msg && <span className="mono-meta" style={{ color: "var(--live)" }}>{msg}</span>}
        <button className="btn sm" disabled={busy} onClick={() => void save(false)}>save</button>
        {meshed && (
          <button className="btn sm" disabled={busy} onClick={() => void save(true)} title="set the same policy on every linked node">
            apply to all nodes
          </button>
        )}
      </div>
      <ErrorBar error={err} />
    </div>
  );
}

function NodeRowView({
  row,
  agents,
  onAct,
  showPolicy,
  onTogglePolicy,
}: {
  row: NodeRow;
  agents: Agent[];
  onAct: (label: string, f: () => Promise<unknown>) => Promise<void>;
  showPolicy: boolean;
  onTogglePolicy: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [logs, setLogs] = useState(false);
  const { node: me } = useAppData();
  const selfVersion = me?.version;
  const skew = row.version && selfVersion && row.version !== selfVersion;
  const target = row.self ? undefined : row.node;
  const canAct = row.self || row.link_up;
  const draining = row.state === "draining";
  const updating = row.state === "updating";
  return (
    <>
      <div className="mesh-peer">
        <span className={`dot ${row.link_up ? "dot-idle" : "dot-down"}`} aria-hidden />
        <span className="mono" style={{ color: "var(--text-hi)", minWidth: 120 }}>
          {row.node}
          {row.self ? <span className="mono-meta"> · this node</span> : null}
        </span>
        <span className="mono-meta" style={{ color: skew ? "var(--sig-normal)" : undefined }}>
          {row.version ? `v${row.version}` : "v?"}
          {skew ? " (skew)" : ""}
        </span>
        {row.available && (
          <span className="chip mono" style={{ color: "var(--sig-normal)" }} title="a newer release is published">
            v{row.available} available
          </span>
        )}
        {row.inventory?.claude_version && (
          <span className="mono-meta" title="harness version on this node">claude {row.inventory.claude_version.split(" ")[0]}</span>
        )}
        {row.inventory && (
          <span className="mono-meta" title={`${row.inventory.os}/${row.inventory.arch} · pid ${row.inventory.pid}`}>
            up {fmtUptime(row.inventory.started_at)} · {row.inventory.os}
          </span>
        )}
        {row.policy && <span className="mono-meta" title="update policy">policy {row.policy}</span>}
        <StateChip row={row} />
        <OutcomeChip o={row.last_outcome} />
        <span style={{ flex: 1 }} />
        {canAct && !draining && !updating && row.available && (
          <>
            <button className="btn sm" onClick={() => void onAct("update", () => api.requestUpdate("quiet", target))} title="drain: refuse new sessions, wait until every session is idle, then update and revive">
              update
            </button>
            <button className="btn ghost sm" onClick={() => setConfirm(true)} title="interrupts turns in flight">
              update now
            </button>
          </>
        )}
        {canAct && draining && (
          <>
            <button className="btn ghost sm" onClick={() => setConfirm(true)} title="stop waiting; restart through whatever is running">
              now
            </button>
            <button className="btn ghost sm" onClick={() => void onAct("cancel", () => api.cancelUpdate(target))}>cancel</button>
          </>
        )}
        {canAct && !updating && (
          <button className="btn ghost sm" onClick={() => void onAct("check", () => api.checkUpdate(target))} title="ask this node to check the release channel now">
            check
          </button>
        )}
        {canAct && <button className="btn ghost sm" onClick={() => setLogs(true)}>logs</button>}
        {row.self && (
          <button className="btn ghost sm" onClick={onTogglePolicy} aria-expanded={showPolicy}>
            policy {showPolicy ? "▴" : "▾"}
          </button>
        )}
      </div>
      {confirm && (
        <ConfirmNow
          row={row}
          busy={busyOn(agents, row)}
          onClose={() => setConfirm(false)}
          onConfirm={() => {
            setConfirm(false);
            void onAct("update now", () => api.requestUpdate("now", target));
          }}
        />
      )}
      {logs && <LogsDrawer node={row.node} self={row.self} onClose={() => setLogs(false)} />}
    </>
  );
}

function RolloutLine({ r, onStop }: { r: Rollout; onStop: () => void }) {
  const status = r.finished
    ? r.failed
      ? `stopped at ${r.failed[0]}: ${r.failed[1]}`
      : r.stopped
        ? "stopped"
        : "done"
    : r.current
      ? `updating ${r.current}…`
      : "starting";
  return (
    <div className="mesh-row">
      <span className="chip mono" style={{ color: r.failed ? "var(--sig-gate)" : r.finished ? "var(--live)" : "var(--sig-normal)" }}>
        rollout → v{r.target}
      </span>
      <span className="mono-meta">
        {r.order.map((n) => (r.done.includes(n) ? `✓ ${n}` : n === r.current ? `▶ ${n}` : n)).join(" · ")}
      </span>
      <span className="mono-meta">{status}</span>
      <span style={{ flex: 1 }} />
      {!r.finished && (
        <button className="btn ghost sm" onClick={onStop}>stop after this node</button>
      )}
    </div>
  );
}

/** Mesh → list: one row per node with version, harness, state, actions. */
export function ServicingPanel() {
  const fs = useFleetServicing(5000);
  const { agents } = useAppData();
  const [err, setErr] = useState<string | null>(null);
  const [showPolicy, setShowPolicy] = useState(false);
  const [showNotes, setShowNotes] = useState(false);
  async function act(label: string, f: () => Promise<unknown>) {
    setErr(null);
    try {
      await f();
      await fs.refresh();
    } catch (e) {
      setErr(`${label}: ${e instanceof Error ? e.message : "failed"}`);
    }
  }
  const self = fs.self;
  const meshed = (fs.mesh?.peers ?? []).length > 0;
  const summary = self
    ? self.available && !self.skipped
      ? `v${self.available.version} available · ${fs.behind.length} of ${fs.rows.length} node${fs.rows.length === 1 ? "" : "s"} behind`
      : self.withdrawn
        ? `running v${self.current}, newer than the latest published (v${self.latest}) — withdrawn?`
        : self.last_check?.ok === false
          ? `last check failed: ${self.last_check.error}`
          : self.last_check
            ? `up to date (v${self.current}) · checked ${relTime(self.last_check.at)} ago`
            : `v${self.current} · not checked yet`
    : "…";
  return (
    <div className="strip mesh-panel" id="nodes">
      <div className="mesh-head">
        <span className="label">Nodes</span>
        <span className="mono">{summary}</span>
        <span style={{ flex: 1 }} />
        {self?.available && !self.skipped && (
          <>
            <button className="btn ghost sm" onClick={() => setShowNotes((v) => !v)} aria-expanded={showNotes}>
              notes {showNotes ? "▴" : "▾"}
            </button>
            {meshed && !(fs.rollout && !fs.rollout.finished) && (
              <button className="btn primary sm" onClick={() => void act("update fleet", () => api.updateFleet("quiet"))} title="one node at a time, each when quiet, this node last">
                update fleet
              </button>
            )}
            <button
              className="btn ghost sm"
              onClick={() => void act("snooze", () => api.setUpdatePolicy({ ...self.policy, skip: self.available!.version }, meshed ? "*" : undefined))}
              title="ignore this version (every node)"
            >
              snooze v{self.available.version}
            </button>
          </>
        )}
      </div>
      <ErrorBar error={err ?? fs.error} />
      {showNotes && self?.available?.notes && <pre className="svc-notes">{self.available.notes}</pre>}
      {fs.rollout && <RolloutLine r={fs.rollout} onStop={() => void act("stop rollout", api.stopFleet)} />}
      <div className="mesh-section" style={{ borderTop: "none", paddingTop: 0 }}>
        {fs.rows.map((r) => (
          <NodeRowView
            key={r.node}
            row={r}
            agents={agents}
            onAct={act}
            showPolicy={r.self && showPolicy}
            onTogglePolicy={() => setShowPolicy((v) => !v)}
          />
        ))}
        {showPolicy && self && (
          <PolicyEditor
            key={JSON.stringify(self.policy)}
            initial={self.policy}
            nodeName={fs.mesh?.node ?? "this node"}
            meshed={meshed}
            onSaved={() => void fs.refresh()}
          />
        )}
      </div>
    </div>
  );
}

/** Now → Needs you: one card while anything about updates needs a human. */
export function UpdateCard() {
  const fs = useFleetServicing(10000);
  const nav = useNavigate();
  const [err, setErr] = useState<string | null>(null);
  const [showNotes, setShowNotes] = useState(false);
  const self = fs.self;
  if (!self) return null;
  const now = Date.now() / 1000;
  const busyNodes = fs.rows.filter((r) => r.state && r.state !== "ready");
  const overdue = fs.rows.filter((r) => r.detail?.startsWith("overdue"));
  const failed = fs.rows.filter(
    (r) => r.last_outcome && !r.last_outcome.ok && r.last_outcome.finished_at && now - r.last_outcome.finished_at < 86400,
  );
  const rollout = fs.rollout && (!fs.rollout.finished || (fs.rollout.finished_at && now - fs.rollout.finished_at < 3600)) ? fs.rollout : null;
  const available = self.available && !self.skipped ? self.available : null;
  if (!available && busyNodes.length === 0 && failed.length === 0 && !rollout && !self.withdrawn) return null;
  const meshed = (fs.mesh?.peers ?? []).length > 0;
  async function act(label: string, f: () => Promise<unknown>) {
    setErr(null);
    try {
      await f();
      await fs.refresh();
    } catch (e) {
      setErr(`${label}: ${e instanceof Error ? e.message : "failed"}`);
    }
  }
  const tone = overdue.length || failed.length ? "var(--sig-gate)" : "var(--sig-normal)";
  return (
    <div className="need-card need-cue" style={{ flexWrap: "wrap" }}>
      <span className="chip mono" style={{ color: tone }}>
        {failed.length ? "update failed" : overdue.length ? "update overdue" : busyNodes.length ? "updating" : self.withdrawn ? "withdrawn release" : "update"}
      </span>
      {available ? (
        <span className="mono">
          v{available.version} available
          {fs.rows.length > 1 ? ` · ${fs.behind.length} of ${fs.rows.length} nodes behind` : ""}
          {available.published_at ? <span className="mono-meta"> · published {relTime(available.published_at)} ago</span> : null}
        </span>
      ) : self.withdrawn ? (
        <span className="mono">
          this node runs v{self.current}, newer than the latest published v{self.latest}
          <span className="mono-meta"> · aspen update --version v{self.latest} to go back</span>
        </span>
      ) : null}
      {busyNodes.map((r) => (
        <span key={r.node} className="mono-meta">
          {r.node}: {r.state}
          {r.detail ? ` — ${r.detail}` : ""}
        </span>
      ))}
      {failed.map((r) => (
        <span key={`f:${r.node}`} className="mono-meta" style={{ color: "var(--sig-gate)" }}>
          {r.node}: {r.last_outcome?.rolled_back ? "rolled back" : "failed"} {r.last_outcome?.from} → {r.last_outcome?.to}
          {r.last_outcome?.error ? ` — ${r.last_outcome.error}` : ""}
        </span>
      ))}
      {rollout && (
        <span className="mono-meta">
          rollout → v{rollout.target}: {rollout.order.map((n) => (rollout.done.includes(n) ? `✓ ${n}` : n === rollout.current ? `▶ ${n}` : n)).join(" · ")}
          {rollout.finished ? (rollout.failed ? ` — stopped at ${rollout.failed[0]}: ${rollout.failed[1]}` : rollout.stopped ? " — stopped" : " — done") : ""}
        </span>
      )}
      <span style={{ flex: 1 }} />
      {available && showNotes && <pre className="svc-notes" style={{ flexBasis: "100%" }}>{available.notes ?? "(no notes)"}</pre>}
      {available && (
        <button className="btn ghost sm" onClick={() => setShowNotes((v) => !v)}>{showNotes ? "hide notes" : "notes"}</button>
      )}
      {available && self.state.state === "ready" && !(rollout && !rollout.finished) && (
        meshed ? (
          <button className="btn sm" onClick={() => void act("update fleet", () => api.updateFleet("quiet"))} title="one node at a time, each when quiet, this node last">
            update fleet
          </button>
        ) : (
          <button className="btn sm" onClick={() => void act("update", () => api.requestUpdate("quiet"))} title="wait until every session is idle, then update and revive">
            update
          </button>
        )
      )}
      {rollout && !rollout.finished && (
        <button className="btn ghost sm" onClick={() => void act("stop rollout", api.stopFleet)}>stop rollout</button>
      )}
      {available && (
        <button
          className="btn ghost sm"
          onClick={() => void act("snooze", () => api.setUpdatePolicy({ ...self.policy, skip: available.version }, meshed ? "*" : undefined))}
          title="ignore this version"
        >
          snooze
        </button>
      )}
      <button className="btn ghost sm" onClick={() => nav("/mesh?view=list#nodes")}>nodes</button>
      {err && <span className="mono-meta" style={{ color: "var(--sig-gate)", flexBasis: "100%" }}>{err}</span>}
    </div>
  );
}
