// Global command palette — ⌘K / Ctrl+K anywhere in the console. One input,
// one ranked list. Navigation, open-session, open-channel, and three
// argument-taking verbs (msg / interrupt / start) parsed straight from the
// query. Switchboard voice throughout; keyboard-first, focus-trapped.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router-dom";
import { api, ApiError } from "./api";
import type { Channel, Repo } from "./api";
import { useAppData } from "./App";
import { presenceOf } from "./components";
import type { Presence } from "./components";
import "./palette.css";

interface Item {
  key: string;
  section: string;
  node: ReactNode;
  run: () => void | Promise<void>;
}

const NAV_TARGETS: { label: string; to: string }[] = [
  { label: "Now", to: "/" },
  { label: "Flow", to: "/flow" },
  { label: "Mesh", to: "/mesh" },
  { label: "Mesh · list", to: "/mesh?view=list" },
  { label: "History", to: "/history" },
];

const presenceColor: Record<Presence, string> = {
  busy: "var(--live)",
  idle: "var(--idle)",
  off: "var(--offline)",
};

/**
 * Case-insensitive match score: substring beats subsequence, earlier and
 * shorter beats later and longer. Negative = no match.
 */
function score(query: string, target: string): number {
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  if (!q) return 0;
  const idx = t.indexOf(q);
  if (idx >= 0) return 1000 - idx - t.length * 0.01;
  let ti = 0;
  for (const ch of q) {
    ti = t.indexOf(ch, ti);
    if (ti < 0) return -1;
    ti += 1;
  }
  return 100 - t.length * 0.1;
}

function Dot({ presence }: { presence: Presence }) {
  return (
    <span
      aria-hidden
      className="pal-dot"
      style={{ background: presenceColor[presence] }}
    />
  );
}

export default function Palette() {
  const nav = useNavigate();
  const { agents, refreshAgents } = useAppData();

  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [repos, setRepos] = useState<Repo[]>([]);
  const [status, setStatus] = useState<{ text: string; err: boolean } | null>(null);
  const [pending, setPending] = useState(false);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const prevFocus = useRef<HTMLElement | null>(null);
  const closeTimer = useRef<number | null>(null);

  const close = useCallback(() => setOpen(false), []);

  // ⌘K / Ctrl+K toggle + Escape close, everywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setOpen((o) => !o);
      } else if (e.key === "Escape" && open) {
        e.preventDefault();
        close();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, close]);

  // Open: remember focus, reset, fetch sources. Close: restore focus.
  useEffect(() => {
    if (open) {
      prevFocus.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
      setQ("");
      setSel(0);
      setStatus(null);
      setPending(false);
      inputRef.current?.focus();
      void api.channels().then(setChannels).catch(() => setChannels([]));
      void api.repos().then(setRepos).catch(() => setRepos([]));
    } else {
      if (closeTimer.current !== null) {
        window.clearTimeout(closeTimer.current);
        closeTimer.current = null;
      }
      prevFocus.current?.focus();
      prevFocus.current = null;
    }
  }, [open]);

  useEffect(() => {
    return () => {
      if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    };
  }, []);

  const goto = useCallback(
    (path: string) => {
      setOpen(false);
      nav(path);
    },
    [nav],
  );

  const fill = useCallback((prefix: string) => {
    setQ(prefix);
    setSel(0);
    setStatus(null);
    inputRef.current?.focus();
  }, []);

  const doSend = useCallback(async (to: string, body: string) => {
    setPending(true);
    setStatus(null);
    try {
      const res = await api.busSend({ to, body, urgency: "normal" });
      const notes = (res.notes ?? []).join(" · ");
      setStatus({ text: `sent to ${to}${notes ? ` — ${notes}` : ""}`, err: false });
      closeTimer.current = window.setTimeout(() => setOpen(false), 1400);
    } catch (e) {
      setStatus({ text: e instanceof Error ? e.message : String(e), err: true });
    } finally {
      setPending(false);
    }
  }, []);

  const doInterrupt = useCallback(async (name: string) => {
    setPending(true);
    setStatus(null);
    try {
      await api.interrupt(name);
      setStatus({ text: `interrupted @${name}`, err: false });
      closeTimer.current = window.setTimeout(() => setOpen(false), 1000);
    } catch (e) {
      setStatus({ text: e instanceof Error ? e.message : String(e), err: true });
    } finally {
      setPending(false);
    }
  }, []);

  const doStart = useCallback(
    async (name: string, repo: string) => {
      setPending(true);
      setStatus({ text: `starting @${name} in ${repo}…`, err: false });
      try {
        await api.startAgent({ name, repo });
        void refreshAgents();
        setOpen(false);
        nav(`/session/${encodeURIComponent(name)}`);
      } catch (e) {
        // The Sessions/Library pages own the full trust review flow —
        // point there instead of dead-ending on the raw 428.
        const untrusted = e instanceof ApiError && e.status === 428;
        setStatus({
          text: untrusted
            ? "untrusted repository — start it from Now (n) or Mesh (m) to review what it auto-runs"
            : e instanceof Error
              ? e.message
              : String(e),
          err: true,
        });
      } finally {
        setPending(false);
      }
    },
    [nav, refreshAgents],
  );

  const items = useMemo<Item[]>(() => {
    const t = q.trim();
    const out: Item[] = [];

    const sessionRow = (name: string, presence: Presence, title?: string | null) => (
      <>
        <Dot presence={presence} />
        <span>open</span>
        <span className="pal-mono">@{name}</span>
        {title && <span className="pal-sub">{title}</span>}
      </>
    );

    // ── msg @agent <text> | msg #channel <text> ────────────────────────
    if (/^msg(\s|$)/i.test(t)) {
      const full = /^msg\s+([@#])(\S+)\s+(\S[\s\S]*)$/i.exec(t);
      if (full) {
        const to = `${full[1]}${full[2]}`;
        const body = full[3];
        out.push({
          key: `send:${to}`,
          section: "Send",
          node: (
            <>
              <span>send to</span>
              <span className="pal-mono">{to}</span>
              <span className="pal-sub">“{body.length > 60 ? `${body.slice(0, 60)}…` : body}”</span>
            </>
          ),
          run: () => doSend(to, body),
        });
        return out;
      }
      const partial = /^msg\s*([@#]?)(\S*)$/i.exec(t);
      const sigil = partial?.[1] ?? "";
      const frag = (partial?.[2] ?? "").toLowerCase();
      if (sigil !== "#") {
        for (const a of agents) {
          if (frag && !a.name.toLowerCase().includes(frag)) continue;
          out.push({
            key: `msg-to:@${a.name}`,
            section: "Message",
            node: (
              <>
                <Dot presence={presenceOf(a.live, a.turn_state)} />
                <span className="pal-mono">msg @{a.name}</span>
                <span className="pal-sub">then type the text</span>
              </>
            ),
            run: () => fill(`msg @${a.name} `),
          });
        }
      }
      if (sigil !== "@") {
        for (const c of channels) {
          if (frag && !c.name.toLowerCase().includes(frag)) continue;
          out.push({
            key: `msg-to:#${c.name}`,
            section: "Message",
            node: (
              <>
                <span className="pal-mono">msg #{c.name}</span>
                <span className="pal-sub">then type the text</span>
              </>
            ),
            run: () => fill(`msg #${c.name} `),
          });
        }
      }
      if (out.length === 0) {
        out.push({
          key: "msg-hint",
          section: "Message",
          node: (
            <>
              <span className="pal-mono">msg @agent | #channel &lt;text&gt;</span>
              <span className="pal-sub">no matching target</span>
            </>
          ),
          run: () => fill("msg "),
        });
      }
      return out;
    }

    // ── interrupt @agent ───────────────────────────────────────────────
    if (/^int(errupt)?(\s|$)/i.test(t)) {
      const partial = /^\S+\s*@?(\S*)$/.exec(t);
      const frag = (partial?.[1] ?? "").toLowerCase();
      const busy = agents.filter((a) => a.live && a.turn_state === "busy");
      for (const a of busy) {
        if (frag && !a.name.toLowerCase().includes(frag)) continue;
        out.push({
          key: `int:@${a.name}`,
          section: "Interrupt",
          node: (
            <>
              <Dot presence="busy" />
              <span>interrupt</span>
              <span className="pal-mono">@{a.name}</span>
              <span className="pal-sub">abort the in-flight turn</span>
            </>
          ),
          run: () => doInterrupt(a.name),
        });
      }
      if (out.length === 0) {
        out.push({
          key: "int-none",
          section: "Interrupt",
          node: <span className="pal-sub">no busy agents{frag ? ` matching “${frag}”` : ""}</span>,
          run: () => {},
        });
      }
      return out;
    }

    // ── start <name> in <repo-substring> ───────────────────────────────
    if (/^start(\s|$)/i.test(t)) {
      const full = /^start\s+@?(\S+)\s+in\s+(\S.*)$/i.exec(t);
      if (full) {
        const name = full[1];
        const frag = full[2].trim().toLowerCase();
        const matches = repos.filter((r) => r.path.toLowerCase().includes(frag));
        for (const r of matches) {
          out.push({
            key: `start:${name}:${r.path}`,
            section: "Start",
            node: (
              <>
                <span>start</span>
                <span className="pal-mono">@{name}</span>
                <span>in</span>
                <span className="pal-mono">{r.path}</span>
              </>
            ),
            run: () => doStart(name, r.path),
          });
        }
        if (matches.length === 0) {
          out.push({
            key: "start-none",
            section: "Start",
            node: <span className="pal-sub">no remembered repo matches “{frag}”</span>,
            run: () => {},
          });
        }
      } else {
        out.push({
          key: "start-hint",
          section: "Start",
          node: (
            <>
              <span className="pal-mono">start &lt;name&gt; in &lt;repo-substring&gt;</span>
              <span className="pal-sub">spawn a session in a remembered repo</span>
            </>
          ),
          run: () => fill("start "),
        });
      }
      return out;
    }

    // ── default: rank navigation + sessions + channels ─────────────────
    const ranked: { s: number; order: number; item: Item }[] = [];
    let order = 0;

    for (const n of NAV_TARGETS) {
      const s = score(t, n.label);
      if (s < 0) continue;
      ranked.push({
        s,
        order: order++,
        item: {
          key: `nav:${n.to}`,
          section: "Navigate",
          node: (
            <>
              <span>{n.label}</span>
              <span className="pal-sub">{n.to}</span>
            </>
          ),
          run: () => goto(n.to),
        },
      });
    }

    {
      const s = Math.max(score(t, "check for updates"), score(t, "update aspen"));
      if (s >= 0) {
        ranked.push({
          s,
          order: order++,
          item: {
            key: "act:check-updates",
            section: "Actions",
            node: (
              <>
                <span>Check for updates</span>
                <span className="pal-sub">every node asks the release channel now</span>
              </>
            ),
            run: async () => {
              await api.checkUpdatesAll();
              goto("/mesh?view=list#nodes");
            },
          },
        });
      }
    }

    for (const a of agents) {
      const s = Math.max(score(t, `open @${a.name}`), score(t, a.title ?? ""));
      if (s < 0) continue;
      const target = a.remote ? `${a.name}@${a.node}` : a.name;
      ranked.push({
        s,
        order: order++,
        item: {
          key: `sess:${target}`,
          section: "Sessions",
          node: sessionRow(a.name, presenceOf(a.live, a.turn_state), a.title),
          run: () => goto(`/session/${encodeURIComponent(target)}`),
        },
      });
    }

    for (const c of channels) {
      const s = score(t, `open #${c.name}`);
      if (s < 0) continue;
      ranked.push({
        s,
        order: order++,
        item: {
          key: `chan:${c.name}`,
          section: "Channels",
          node: (
            <>
              <span>open</span>
              <span className="pal-mono">#{c.name}</span>
              {c.topic && <span className="pal-sub">{c.topic}</span>}
            </>
          ),
          run: () => goto(`/flow/${encodeURIComponent(c.name)}`),
        },
      });
    }

    if (t) ranked.sort((a, b) => b.s - a.s || a.order - b.order);
    for (const r of ranked.slice(0, 24)) out.push(r.item);

    // Hint rows for the argument verbs — shown when the input is empty.
    if (!t) {
      const hints: { prefix: string; usage: string; sub: string }[] = [
        { prefix: "msg ", usage: "msg @agent | #channel <text>", sub: "send on the bus (normal class)" },
        { prefix: "interrupt ", usage: "interrupt @agent", sub: "abort a busy agent's turn" },
        { prefix: "start ", usage: "start <name> in <repo-substring>", sub: "spawn a session" },
      ];
      for (const h of hints) {
        out.push({
          key: `hint:${h.prefix}`,
          section: "Commands",
          node: (
            <>
              <span className="pal-mono">{h.usage}</span>
              <span className="pal-sub">{h.sub}</span>
            </>
          ),
          run: () => fill(h.prefix),
        });
      }
    }

    return out;
  }, [q, agents, channels, repos, doSend, doInterrupt, doStart, fill, goto]);

  const selIdx = items.length === 0 ? 0 : Math.min(sel, items.length - 1);

  // Keep the selected row in view.
  useEffect(() => {
    const el = listRef.current?.querySelector(".pal-row.sel");
    el?.scrollIntoView({ block: "nearest" });
  }, [selIdx, q]);

  if (!open) return null;

  const onInputKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (items.length > 0) setSel((selIdx + 1) % items.length);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (items.length > 0) setSel((selIdx - 1 + items.length) % items.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!pending) {
        const it = items[selIdx];
        if (it) void it.run();
      }
    } else if (e.key === "Tab") {
      // Focus stays in the input — the palette is a single-field modal.
      e.preventDefault();
    }
  };

  return (
    <div
      className="pal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="pal-panel" role="dialog" aria-modal="true" aria-label="command palette">
        <div className="pal-input-row">
          <span className="pal-glyph" aria-hidden>⌘</span>
          <input
            ref={inputRef}
            className="pal-input"
            value={q}
            placeholder="type to jump, or: msg · interrupt · start"
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => {
              setQ(e.target.value);
              setSel(0);
              setStatus(null);
            }}
            onKeyDown={onInputKey}
            onBlur={() => {
              // Trap focus: anything inside the palette bounces back.
              if (open) inputRef.current?.focus();
            }}
            aria-label="command"
          />
          <span className="pal-esc micro">esc</span>
        </div>

        <div className="pal-list" ref={listRef}>
          {items.length === 0 && <div className="pal-empty mono-meta">no matches</div>}
          {items.map((it, i) => (
            <div key={it.key}>
              {(i === 0 || items[i - 1].section !== it.section) && (
                <div className="label pal-section">{it.section}</div>
              )}
              <div
                className={`pal-row${i === selIdx ? " sel" : ""}`}
                onClick={() => {
                  if (!pending) void it.run();
                }}
                onMouseEnter={() => setSel(i)}
              >
                {it.node}
              </div>
            </div>
          ))}
        </div>

        {status && (
          <div className={`pal-status mono-meta${status.err ? " err" : ""}`} role="status">
            {status.text}
          </div>
        )}
      </div>
    </div>
  );
}
