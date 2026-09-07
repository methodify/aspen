// Usage (docs/USAGE.md, PROPOSALS-B §4): what every session consumed and
// cost, across the estate. Tokens come from each transcript (subagents
// folded in); money is the harness's own figure — the session total it
// wrote to the transcript, and the per-turn deltas Aspen observed in the
// chosen window. Group by node, repo, or model.

import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, type UsageRow } from "../api";
import { usePoll } from "../hooks";
import { useHotkeys } from "../hotkeys";
import { ErrorBar } from "../components";
import "./usage.css";

type Range = "today" | "week" | "all";
type Group = "session" | "node" | "repo" | "model";

function rangeFrom(r: Range): number {
  const now = Date.now() / 1000;
  if (r === "today") {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    return d.getTime() / 1000;
  }
  if (r === "week") return now - 7 * 86400;
  return 0;
}

export function fmtTokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)}k`;
  return String(n);
}
export function fmtUsd(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  return n >= 100 ? `$${n.toFixed(0)}` : n >= 1 ? `$${n.toFixed(2)}` : `$${n.toFixed(3)}`;
}

interface Agg {
  key: string;
  rows: UsageRow[];
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
  calls: number;
  turns: number;
  cost: number;
  costKnown: boolean;
  windowCost: number;
  windowTurns: number;
  subagents: number;
  models: Set<string>;
}

function aggregate(rows: UsageRow[], group: Group): Agg[] {
  const m = new Map<string, Agg>();
  const bump = (key: string, r: UsageRow, mu: { input: number; output: number; cache_read: number; cache_create: number; calls: number; cost_usd: number | null }, whole: boolean) => {
    const a =
      m.get(key) ??
      ({ key, rows: [], input: 0, output: 0, cacheRead: 0, cacheCreate: 0, calls: 0, turns: 0, cost: 0, costKnown: false, windowCost: 0, windowTurns: 0, subagents: 0, models: new Set() } as Agg);
    a.rows.push(r);
    a.input += mu.input;
    a.output += mu.output;
    a.cacheRead += mu.cache_read;
    a.cacheCreate += mu.cache_create;
    a.calls += mu.calls;
    if (whole) {
      a.turns += r.usage.turns;
      a.subagents += r.usage.subagents;
      a.windowCost += r.window.cost_usd;
      a.windowTurns += r.window.turns;
      if (r.usage.cost_usd !== null) {
        a.cost += r.usage.cost_usd;
        a.costKnown = true;
      }
      for (const [k, mu] of Object.entries(r.usage.models)) if (!k.startsWith("<") || mu.output > 0) a.models.add(k);
    } else if (mu.cost_usd !== null) {
      a.cost += mu.cost_usd;
      a.costKnown = true;
    }
    m.set(key, a);
  };
  for (const r of rows) {
    if (group === "model") {
      for (const [model, mu] of Object.entries(r.usage.models)) if (!model.startsWith("<") || mu.output > 0) bump(model, r, mu, false);
    } else {
      const key = group === "session" ? r.agent : group === "node" ? (r.node ?? "local") : `${r.channel}`;
      bump(key, r, r.usage.total, true);
    }
  }
  return [...m.values()].sort((a, b) => b.cost - a.cost || b.output - a.output);
}

export default function Usage() {
  const [range, setRange] = useState<Range>("today");
  const [group, setGroup] = useState<Group>("session");
  const from = rangeFrom(range);
  const poll = usePoll<UsageRow[]>(() => api.usage(from), 10000);
  const rows = poll.data ?? [];
  const aggs = useMemo(() => aggregate(rows, group), [rows, group]);
  const totals = useMemo(() => aggregate(rows, "node").reduce(
    (t, a) => ({ cost: t.cost + a.cost, window: t.window + a.windowCost, output: t.output + a.output, input: t.input + a.input + a.cacheRead + a.cacheCreate }),
    { cost: 0, window: 0, output: 0, input: 0 },
  ), [rows]);

  useHotkeys("usage", [
    { key: "1", description: "by session", handler: () => setGroup("session") },
    { key: "2", description: "by node", handler: () => setGroup("node") },
    { key: "3", description: "by repo", handler: () => setGroup("repo") },
    { key: "4", description: "by model", handler: () => setGroup("model") },
    { key: "t", description: "today / week / all", handler: () => setRange((r) => (r === "today" ? "week" : r === "week" ? "all" : "today")) },
  ]);

  return (
    <>
      <div className="stage-head">
        <span className="t-display">Usage</span>
        <span className="mono-meta">
          {rows.length} sessions · {fmtUsd(totals.cost)} lifetime · {fmtUsd(totals.window)} {range === "all" ? "observed" : range === "today" ? "today" : "this week"} · {fmtTokens(totals.input)} in · {fmtTokens(totals.output)} out
        </span>
        <span style={{ flex: 1 }} />
        <span className="seg">
          {(["today", "week", "all"] as Range[]).map((r) => (
            <button key={r} className={range === r ? "on" : ""} onClick={() => setRange(r)}>
              {r}
            </button>
          ))}
        </span>
        <span className="seg">
          {(["session", "node", "repo", "model"] as Group[]).map((g) => (
            <button key={g} className={group === g ? "on" : ""} onClick={() => setGroup(g)}>
              {g}
            </button>
          ))}
        </span>
      </div>
      <div className="stage-body usage-page">
        <ErrorBar error={poll.error} />
        <p className="dim usage-note">
          Lifetime figures are the harness's own: tokens from each transcript (subagents folded in) and the session total it prices itself. The window column is the spend Aspen observed turn by turn in the chosen range, on every node that was up.
        </p>
        <div className="usage-table-wrap">
          <table className="usage-table">
            <thead>
              <tr>
                <th>{group}</th>
                {group !== "model" && <th className="num">sessions</th>}
                <th className="num">turns</th>
                <th className="num">calls</th>
                <th className="num">in</th>
                <th className="num">cache read</th>
                <th className="num">cache write</th>
                <th className="num">out</th>
                {group !== "model" && <th>models</th>}
                <th className="num">lifetime</th>
                <th className="num">{range === "all" ? "observed" : range}</th>
              </tr>
            </thead>
            <tbody>
              {aggs.map((a) => (
                <tr key={a.key}>
                  <td className="mono key-cell">
                    {group === "session" ? (
                      <Link to={`/session/${encodeURIComponent(a.key)}`}>
                        @{a.key}
                        {a.rows[0]?.live && <span className="live-dot" title="live" />}
                      </Link>
                    ) : group === "repo" ? (
                      `#${a.key}`
                    ) : (
                      a.key
                    )}
                    {group === "session" && a.rows[0]?.title && <span className="dim usage-title"> {a.rows[0].title}</span>}
                  </td>
                  {group !== "model" && <td className="num mono">{a.rows.length}</td>}
                  <td className="num mono">{a.turns || "—"}</td>
                  <td className="num mono">{a.calls}</td>
                  <td className="num mono">{fmtTokens(a.input)}</td>
                  <td className="num mono">{fmtTokens(a.cacheRead)}</td>
                  <td className="num mono">{fmtTokens(a.cacheCreate)}</td>
                  <td className="num mono">{fmtTokens(a.output)}</td>
                  {group !== "model" && (
                    <td className="mono-meta models-cell">
                      {[...a.models]
                        .map((m) => m.replace(/^claude-/, ""))
                        .join(", ")}
                      {a.subagents > 0 && <span className="dim"> · {a.subagents} subagent{a.subagents === 1 ? "" : "s"}</span>}
                    </td>
                  )}
                  <td className="num mono cost">{a.costKnown ? fmtUsd(a.cost) : "—"}</td>
                  <td className="num mono cost">{group === "model" ? "" : fmtUsd(a.windowCost)}</td>
                </tr>
              ))}
              {aggs.length === 0 && (
                <tr>
                  <td colSpan={12} className="dim">
                    {poll.data === null ? "loading…" : "no sessions with transcripts"}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </>
  );
}
