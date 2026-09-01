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

/** Start a session, routing untrusted repos through the review dialog.
 * Resolves the started Agent, or null when the operator declines. */
export type TrustedStart = (req: StartAgentRequest) => Promise<Agent | null>;

export function useTrustedStart(): {
  start: (req: StartAgentRequest) => Promise<Agent | null>;
  dialog: ReactNode;
} {
  const [pending, setPending] = useState<PendingReview | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const resolver = useRef<((a: Agent | null) => void) | null>(null);

  async function start(req: StartAgentRequest): Promise<Agent | null> {
    try {
      return await api.startAgent(req);
    } catch (e) {
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
    setConfirming(false);
    setErr(null);
    r?.(agent);
  }

  async function confirm() {
    if (!pending) return;
    setConfirming(true);
    setErr(null);
    try {
      const agent = await api.startAgent({ ...pending.req, acknowledge_trust: true });
      settle(agent);
    } catch (e) {
      setConfirming(false);
      setErr(e instanceof Error ? e.message : "failed to start");
    }
  }

  const dialog: ReactNode = pending ? (
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
