// "Since you last looked" (PROPOSALS-2026-09-C.md §1): the console remembers,
// per session and per browser, where the operator's eyes were, and on
// return sums up what happened after that point from the transcript
// already in hand — turns, tool uses, files, prompts, the agent's latest
// word. No model call. The harness's own one-line recap (Claude's /recap)
// is a button here, and on return after a while away, automatic.

import { useEffect, useState } from "react";
import { api } from "../api";
import type { TranscriptItem } from "../transcript";

export interface SeenMarker {
  /** uuid of the last user line seen, or null when none had one. */
  head: string | null;
  /** Items after that line (or from the start) that were seen. */
  after: number;
  /** When, epoch ms. */
  at: number;
}

export interface CatchUp {
  /** Index of the first unseen item, or items.length when nothing is new. */
  firstUnseen: number;
  unseen: number;
  turns: number;
  tools: number;
  toolNames: [string, number][];
  files: number;
  prompts: number;
  /** First line of the agent's latest message among the unseen items. */
  lastWord: string | null;
  /** How long ago the marker was written, seconds. */
  awaySecs: number;
}

/** Away this long and the bar shows even with nothing new — and the recap is asked. */
export const AWAY_SECS = 5 * 60;

const key = (name: string) => `aspen.seen.${name}`;

export function readMarker(name: string): SeenMarker | null {
  try {
    const raw = localStorage.getItem(key(name));
    if (!raw) return null;
    const m = JSON.parse(raw) as SeenMarker;
    return typeof m.after === "number" && typeof m.at === "number" ? m : null;
  } catch {
    return null;
  }
}

/** Where the operator's eyes are now: the last user line and what follows it. */
export function markerOf(items: TranscriptItem[]): SeenMarker {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i]!;
    if (it.kind === "user" && it.uuid) return { head: it.uuid, after: items.length - 1 - i, at: Date.now() };
  }
  return { head: null, after: items.length, at: Date.now() };
}

export function writeMarker(name: string, items: TranscriptItem[]) {
  try {
    localStorage.setItem(key(name), JSON.stringify(markerOf(items)));
  } catch {
    /* storage may be unavailable */
  }
}

function filePathOf(input: unknown): string | null {
  if (!input || typeof input !== "object") return null;
  const o = input as Record<string, unknown>;
  for (const k of ["file_path", "path", "notebook_path"]) {
    if (typeof o[k] === "string") return o[k] as string;
  }
  return null;
}

/** What happened after the marker. Null when the marker's line is gone
 *  (a fresh session, a compaction): there is nothing honest to say. */
export function computeCatchUp(items: TranscriptItem[], m: SeenMarker): CatchUp | null {
  let start: number;
  if (m.head) {
    const idx = items.findIndex((it) => it.kind === "user" && it.uuid === m.head);
    if (idx < 0) return null;
    start = Math.min(items.length, idx + 1 + m.after);
  } else {
    start = Math.min(items.length, m.after);
  }
  const unseen = items.slice(start);
  const names = new Map<string, number>();
  const files = new Set<string>();
  let turnEnds = 0;
  let asks = 0;
  let tools = 0;
  let prompts = 0;
  let lastWord: string | null = null;
  for (const it of unseen) {
    switch (it.kind) {
      // Rehydrated history has no turn-end items; the operator's lines
      // are the turns then.
      case "turn_end":
        turnEnds++;
        break;
      case "user":
        asks++;
        break;
      case "tool": {
        tools++;
        names.set(it.name, (names.get(it.name) ?? 0) + 1);
        const f = filePathOf(it.input);
        if (f) files.add(f);
        break;
      }
      case "permission":
        prompts++;
        break;
      case "assistant": {
        const line = it.text.split("\n").find((l) => l.trim().length > 0)?.trim();
        if (line) lastWord = line.length > 200 ? `${line.slice(0, 200)}…` : line;
        break;
      }
      default:
        break;
    }
  }
  return {
    firstUnseen: start,
    unseen: unseen.length,
    turns: Math.max(turnEnds, asks),
    tools,
    toolNames: [...names.entries()].sort((a, b) => b[1] - a[1]).slice(0, 4),
    files: files.size,
    prompts,
    lastWord,
    awaySecs: Math.max(0, (Date.now() - m.at) / 1000),
  };
}

export function fmtAway(secs: number): string {
  if (secs < 90) return "a minute";
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.round((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}

const AUTO_KEY = "aspen.recapOnReturn";
export function recapOnReturn(): boolean {
  try {
    return localStorage.getItem(AUTO_KEY) !== "off";
  } catch {
    return true;
  }
}

export function CatchUpBar({
  agent,
  cu,
  canRecap,
  idle,
  onJump,
  onDismiss,
}: {
  agent: string;
  cu: CatchUp;
  /** The harness has a recap of its own (Claude); Codex shows the digest only. */
  canRecap: boolean;
  idle: boolean;
  onJump: () => void;
  onDismiss: () => void;
}) {
  const [recap, setRecap] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [auto, setAuto] = useState(recapOnReturn);
  const ask = async () => {
    setBusy(true);
    setErr(null);
    try {
      const r = await api.recap(agent);
      setRecap(r.text);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "no recap");
    } finally {
      setBusy(false);
    }
  };
  // On return after a while away, with the session idle: ask once.
  useEffect(() => {
    if (!canRecap || !auto || !idle || recap !== null || busy || cu.awaySecs < AWAY_SECS) return;
    void ask();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canRecap, auto, idle]);
  const parts: string[] = [];
  if (cu.turns) parts.push(`${cu.turns} turn${cu.turns === 1 ? "" : "s"}`);
  if (cu.tools) parts.push(`${cu.tools} tool use${cu.tools === 1 ? "" : "s"}${cu.toolNames.length ? ` (${cu.toolNames.map(([n, c]) => `${n} ×${c}`).join(", ")})` : ""}`);
  if (cu.files) parts.push(`${cu.files} file${cu.files === 1 ? "" : "s"}`);
  if (cu.prompts) parts.push(`${cu.prompts} prompt${cu.prompts === 1 ? "" : "s"}`);
  return (
    <div className="catchup" role="status">
      <div className="catchup-row">
        <span className="catchup-title mono">since you last looked</span>
        <span className="mono-meta" title="how long ago this browser last had the session in view">{fmtAway(cu.awaySecs)} ago</span>
        <span className="catchup-stats">
          {cu.unseen === 0 ? <span className="dim">nothing new</span> : parts.length ? parts.join(" · ") : `${cu.unseen} item${cu.unseen === 1 ? "" : "s"}`}
        </span>
        <span style={{ flex: 1 }} />
        {cu.unseen > 0 && (
          <button className="btn ghost sm" onClick={onJump} title="scroll to the first thing you have not seen">
            jump ↓
          </button>
        )}
        {canRecap ? (
          <button className="btn ghost sm" disabled={busy || !idle} onClick={() => void ask()} title={idle ? "ask the harness for a one-line recap of the whole session (nothing enters the conversation)" : "the session is mid-turn; a recap waits for idle"}>
            {busy ? "asking…" : recap !== null ? "recap again" : "recap"}
          </button>
        ) : (
          <span className="mono-meta" title="this harness has no recap of its own; the digest is what there is">no recap on this harness</span>
        )}
        <button className="status-note-x" onClick={onDismiss} title="mark everything as seen" aria-label="dismiss">×</button>
      </div>
      {cu.lastWord && cu.unseen > 0 && (
        <div className="catchup-row">
          <span className="mono-meta">latest</span>
          <span className="catchup-word">{cu.lastWord}</span>
        </div>
      )}
      {(recap !== null || err) && (
        <div className="catchup-row">
          <span className="mono-meta">recap</span>
          {err ? <span className="error-text mono-meta">{err}</span> : <span className="catchup-recap">{recap}</span>}
          {canRecap && (
            <label className="mono-meta catchup-auto" title="ask for a recap automatically when you come back after five minutes or more">
              <input
                type="checkbox"
                checked={auto}
                onChange={(e) => {
                  setAuto(e.target.checked);
                  try {
                    localStorage.setItem(AUTO_KEY, e.target.checked ? "on" : "off");
                  } catch {
                    /* ignore */
                  }
                }}
              />
              on return
            </label>
          )}
        </div>
      )}
    </div>
  );
}
