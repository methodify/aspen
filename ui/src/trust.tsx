// The trust gate's review flow (reference §7.7). Headless sessions never
// show the workspace-trust dialog, so the console owns it: before the first
// session in a repo that would auto-run anything, show exactly what will
// execute — hooks, MCP servers, skills — and record consent once.
//
// Usage: const trust = useTrustedStart();
//        const agent = await trust.start(req);   // null = operator declined
//        … render {trust.dialog} once in the page.

import { useRef, useState, type ReactNode } from "react";
import {
  api,
  ApiError,
  type Agent,
  type RepoAutorun,
  type StartAgentRequest,
} from "./api";
import "./trust.css";

interface PendingReview {
  req: StartAgentRequest;
  autorun: RepoAutorun | null;
}

/** The session to resume is being written by a process the node doesn't
 *  manage (a terminal, another node). Nothing happens until the operator
 *  chooses: fork (history kept, new id) or resume in place anyway. */
interface PendingLive {
  req: StartAgentRequest;
  session: string;
  writtenAgo: number;
}

/** Start a session, routing untrusted repos through the review dialog.
 * Resolves the started Agent, or null when the operator declines. */
export type TrustedStart = (req: StartAgentRequest) => Promise<Agent | null>;

/** A plain spawn, or a template spawn when the request names one; the
 *  latter answers with the new address, shaped enough like an Agent for
 *  the callers (they navigate by name). */
async function startRequest(req: StartAgentRequest): Promise<Agent> {
  if (!req.template) return api.startAgent(req);
  const r = await api.templateSpawn(req.template, {
    name: req.name || undefined,
    repo: req.repo || undefined,
    charter: req.charter,
    model: req.model,
    extra_args: req.extra_args,
    skip_permissions: req.skip_permissions,
    title: req.title,
    acknowledge_trust: req.acknowledge_trust,
  });
  if (r.board?.id) {
    try {
      const { placeOnBoardId } = await import("./boardOps");
      await placeOnBoardId(r.board.id, r.name, r.board.mode ?? "fill");
    } catch {
      // the session started; the board placement is best effort
    }
  }
  return { name: r.name, bare: r.bare, node: r.node, repo: null, channel: "", session_id: "", charter: null, live: true, turn_state: "idle", pending: 0 } as Agent;
}

export function useTrustedStart(): {
  start: (req: StartAgentRequest) => Promise<Agent | null>;
  dialog: ReactNode;
} {
  const [pending, setPending] = useState<PendingReview | null>(null);
  const [live, setLive] = useState<PendingLive | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const resolver = useRef<((a: Agent | null) => void) | null>(null);

  async function start(req: StartAgentRequest): Promise<Agent | null> {
    try {
      return await startRequest(req);
    } catch (e) {
      if (e instanceof ApiError && e.status === 409 && e.body && e.body["live_elsewhere"]) {
        const le = e.body["live_elsewhere"] as { session?: string; written_ago_secs?: number };
        setErr(null);
        setLive({ req, session: le.session ?? req.resume ?? "", writtenAgo: le.written_ago_secs ?? 0 });
        return new Promise<Agent | null>((resolve) => {
          resolver.current = resolve;
        });
      }
      if (!(e instanceof ApiError) || e.status !== 428) throw e;
      // Untrusted repo: fetch what it would auto-run and put it to the
      // operator. The promise settles when they decide.
      let autorun: RepoAutorun | null = null;
      try {
        autorun = await api.repoAutorun(req.repo);
      } catch {
        // review still shown, just without the inventory
      }
      setErr(null);
      setPending({ req, autorun });
      return new Promise<Agent | null>((resolve) => {
        resolver.current = resolve;
      });
    }
  }

  function settle(agent: Agent | null) {
    const r = resolver.current;
    resolver.current = null;
    setPending(null);
    setLive(null);
    setConfirming(false);
    setErr(null);
    r?.(agent);
  }

  async function chooseLive(choice: "fork" | "in_place") {
    if (!live) return;
    setConfirming(true);
    setErr(null);
    try {
      // The retry can still hit the trust gate; route it through start()
      // so that dialog follows, then settle with whatever it yields.
      const req = { ...live.req, resume_choice: choice };
      setLive(null);
      let agent: Agent | null;
      try {
        agent = await startRequest(req);
      } catch (e) {
        if (e instanceof ApiError && e.status === 428) {
          setConfirming(false);
          agent = await start(req);
        } else {
          throw e;
        }
      }
      settle(agent);
    } catch (e) {
      setConfirming(false);
      setErr(e instanceof Error ? e.message : "failed to start");
      setLive((cur) => cur ?? live);
    }
  }

  async function confirm() {
    if (!pending) return;
    setConfirming(true);
    setErr(null);
    try {
      const agent = await startRequest({ ...pending.req, acknowledge_trust: true });
      settle(agent);
    } catch (e) {
      setConfirming(false);
      setErr(e instanceof Error ? e.message : "failed to start");
    }
  }

  const liveDialog: ReactNode = live ? (
    <LiveDialog
      session={live.session}
      writtenAgo={live.writtenAgo}
      err={err}
      busy={confirming}
      onCancel={() => settle(null)}
      onChoose={(c) => void chooseLive(c)}
    />
  ) : null;

  const dialog: ReactNode = live ? liveDialog : pending ? (
    <div className="trust-backdrop" onClick={() => settle(null)} role="presentation">
      <div
        className="trust-panel"
        role="dialog"
        aria-label="repository trust review"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="trust-head">
          <span className="label">Trust this repository?</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={() => settle(null)}>
            esc
          </button>
        </div>
        <div className="mono trust-path">{pending.req.repo}</div>
        <p className="trust-lede">
          Sessions here run without the terminal's trust prompt. This is
          everything the repository will execute automatically, before it runs:
        </p>
        {pending.autorun ? (
          <div className="trust-lists">
            <AutorunSection
              title="Hooks"
              note="commands run at session events"
              items={pending.autorun.hooks}
            />
            <AutorunSection
              title="MCP servers"
              note="processes/connections started for the session"
              items={pending.autorun.mcp_servers}
            />
            <AutorunSection
              title="Skills"
              note="loaded into the session's context"
              items={pending.autorun.skills}
            />
            <AutorunSection title="Plugins" items={pending.autorun.plugins} />
          </div>
        ) : (
          <div className="mono-meta">could not read the repository's autorun surface</div>
        )}
        {err && <div className="error-bar">{err}</div>}
        <div className="trust-actions">
          <button className="btn ghost" onClick={() => settle(null)} disabled={confirming}>
            don't start
          </button>
          <button className="btn primary" onClick={() => void confirm()} disabled={confirming}>
            {confirming ? "starting…" : "trust and start"}
          </button>
        </div>
        <div className="mono-meta trust-foot">
          Trust is remembered for this repository on this node (revoke from Library).
        </div>
      </div>
    </div>
  ) : null;

  return { start, dialog };
}

function AutorunSection({
  title,
  note,
  items,
}: {
  title: string;
  note?: string;
  items: string[];
}) {
  if (items.length === 0) return null;
  return (
    <div className="trust-section">
      <div className="trust-section-head">
        <span className="label">{title}</span>
        {note && <span className="mono-meta">{note}</span>}
      </div>
      {items.map((it, i) => (
        <div key={i} className="mono trust-item">
          {it}
        </div>
      ))}
    </div>
  );
}

// ---- the live-elsewhere gate, reusable ------------------------------------

/** The dialog behind the one-writer-per-transcript rule (DESIGN §14b,
 *  2026-09-06): nothing has started; the operator picks fork, resume in
 *  place, or cancel. */
export function LiveDialog({
  session,
  writtenAgo,
  err,
  busy,
  onCancel,
  onChoose,
}: {
  session: string;
  writtenAgo: number;
  err: string | null;
  busy: boolean;
  onCancel: () => void;
  onChoose: (choice: "fork" | "in_place") => void;
}) {
  return (
    <div className="trust-backdrop" onClick={onCancel} role="presentation">
      <div className="trust-panel" role="dialog" aria-label="session is live elsewhere" onClick={(e) => e.stopPropagation()}>
        <div className="trust-head">
          <span className="label">This session is being written elsewhere</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onCancel}>esc</button>
        </div>
        <div className="mono trust-path">session {session.slice(0, 8)} · written {writtenAgo}s ago</div>
        <p className="trust-lede">
          Another process — a terminal, the desktop app, or another node — wrote to this transcript moments ago. Resuming it
          here would put two processes on one transcript, which corrupts the history over time. Nothing has been started.
        </p>
        <div className="trust-lists">
          <div>
            <b>Fork</b> <span className="mono-meta">— keep the whole history, continue under a new session id here; the other process keeps the original. Lineage is recorded.</span>
          </div>
          <div style={{ marginTop: 6 }}>
            <b>Resume anyway</b> <span className="mono-meta">— use the same session id; only if you know the other process is gone.</span>
          </div>
        </div>
        {err && <div className="error-bar">{err}</div>}
        <div className="trust-actions">
          <button className="btn ghost" onClick={onCancel} disabled={busy}>cancel</button>
          <button className="btn ghost" onClick={() => onChoose("in_place")} disabled={busy}>resume anyway</button>
          <button className="btn primary" onClick={() => onChoose("fork")} disabled={busy}>
            {busy ? "starting…" : "fork and start"}
          </button>
        </div>
      </div>
    </div>
  );
}

interface PendingGate {
  session: string;
  writtenAgo: number;
  run: (choice: "fork" | "in_place") => Promise<unknown>;
}

/** Any call that can hit the live-elsewhere 409 — revive, most of all —
 *  goes through `guard`: it runs the call, and if the node refuses under
 *  the gate, shows the dialog and re-runs it with the operator's choice.
 *  Resolves null when they cancel; other errors propagate unchanged.
 *
 *  Usage: const gate = useLiveGate();
 *         const agent = await gate.guard((c) => api.revive(name, c));
 *         … render {gate.dialog} once in the page. */
export function useLiveGate(): {
  guard: <T>(call: (choice?: "fork" | "in_place") => Promise<T>) => Promise<T | null>;
  dialog: ReactNode;
} {
  const [pending, setPending] = useState<PendingGate | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const resolver = useRef<((v: unknown) => void) | null>(null);

  async function guard<T>(call: (choice?: "fork" | "in_place") => Promise<T>): Promise<T | null> {
    try {
      return await call();
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 409 && e.body && e.body["live_elsewhere"])) throw e;
      const le = e.body["live_elsewhere"] as { session?: string; written_ago_secs?: number };
      setErr(null);
      setBusy(false);
      setPending({ session: le.session ?? "", writtenAgo: le.written_ago_secs ?? 0, run: call });
      return new Promise<T | null>((resolve) => {
        resolver.current = resolve as (v: unknown) => void;
      });
    }
  }

  function settle(v: unknown) {
    const r = resolver.current;
    resolver.current = null;
    setPending(null);
    setBusy(false);
    setErr(null);
    r?.(v);
  }

  async function choose(choice: "fork" | "in_place") {
    if (!pending) return;
    setBusy(true);
    setErr(null);
    try {
      settle(await pending.run(choice));
    } catch (e) {
      setBusy(false);
      setErr(e instanceof Error ? e.message : "failed");
    }
  }

  const dialog: ReactNode = pending ? (
    <LiveDialog
      session={pending.session}
      writtenAgo={pending.writtenAgo}
      err={err}
      busy={busy}
      onCancel={() => settle(null)}
      onChoose={(c) => void choose(c)}
    />
  ) : null;

  return { guard, dialog };
}
