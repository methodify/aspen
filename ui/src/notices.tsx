// Notices (docs/NOTIFICATIONS.md, PROPOSALS-B §3): the node tells us when a
// turn ends, a session asks a question or needs a permission, a background
// activity settles, a session exits, or an agent writes to the operator.
// This module polls `/api/notices` with per-node cursors, keeps the unseen
// count in the tab title, shows a toast stack, and — when the operator has
// turned it on — raises a browser Notification while the page is hidden.
// Every preference is this browser's, in localStorage.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "react-router-dom";
import { api, serverNow, type Notice, type NoticeKind } from "./api";
import { pushCurrent, pushSubscribe, pushSupported, pushUnsubscribe } from "./pwa";
import { loadIdentity } from "./tunnel";

export const NOTICE_KINDS: { kind: NoticeKind; label: string; hint: string }[] = [
  { kind: "question", label: "questions", hint: "a session asked you something" },
  { kind: "permission", label: "permissions", hint: "a session needs a tool approved" },
  { kind: "turn_ended", label: "turn ended", hint: "a session finished a turn" },
  { kind: "activity_settled", label: "activity settled", hint: "a background task, subagent or workflow finished" },
  { kind: "exited", label: "exits", hint: "a session died or was killed" },
  { kind: "inbox", label: "operator mail", hint: "an agent wrote to @operator on the bus" },
];

interface Prefs {
  browser: boolean;
  toasts: boolean;
  kinds: Record<string, boolean>;
  mutedNodes: Record<string, boolean>;
}

const PREFS_KEY = "aspen.notices.prefs";
const CURSORS_KEY = "aspen.notices.cursors";
const DEFAULT_PREFS: Prefs = {
  browser: false,
  toasts: true,
  kinds: { question: true, permission: true, turn_ended: true, activity_settled: true, exited: true, inbox: true },
  mutedNodes: {},
};

function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return DEFAULT_PREFS;
    const p = JSON.parse(raw) as Partial<Prefs>;
    return { ...DEFAULT_PREFS, ...p, kinds: { ...DEFAULT_PREFS.kinds, ...(p.kinds ?? {}) }, mutedNodes: p.mutedNodes ?? {} };
  } catch {
    return DEFAULT_PREFS;
  }
}
function loadCursors(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(CURSORS_KEY) ?? "{}") as Record<string, number>;
  } catch {
    return {};
  }
}

export interface NoticesApi {
  /** Notices received this page-life, newest first (capped). */
  recent: Notice[];
  unseen: number;
  markSeen: () => void;
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  /** Ask the browser for Notification permission; resolves to the result. */
  requestBrowser: () => Promise<NotificationPermission>;
  dismissToast: (key: string) => void;
  toasts: Notice[];
}

const Ctx = createContext<NoticesApi | null>(null);
export function useNotices(): NoticesApi {
  const c = useContext(Ctx);
  if (!c) throw new Error("useNotices outside NoticesProvider");
  return c;
}

function noticeKey(n: Notice): string {
  return `${n.node ?? "local"}:${n.id}`;
}

export function NoticesProvider({ children }: { children: ReactNode }) {
  const [prefs, setPrefsRaw] = useState<Prefs>(loadPrefs);
  const [recent, setRecent] = useState<Notice[]>([]);
  const [unseen, setUnseen] = useState(0);
  const [toasts, setToasts] = useState<Notice[]>([]);
  const cursors = useRef<Record<string, number>>(loadCursors());
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  const baseTitle = useRef(document.title);

  const setPrefs = useCallback((p: Prefs) => {
    setPrefsRaw(p);
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify(p));
    } catch {
      // storage unavailable
    }
  }, []);

  const requestBrowser = useCallback(async () => {
    if (!("Notification" in window)) return "denied" as NotificationPermission;
    if (Notification.permission === "granted") return "granted" as NotificationPermission;
    return Notification.requestPermission();
  }, []);

  const dismissToast = useCallback((key: string) => {
    setToasts((t) => t.filter((n) => noticeKey(n) !== key));
  }, []);

  useEffect(() => {
    let live = true;
    let timer = 0;
    const poll = async () => {
      try {
        const since = Object.entries(cursors.current)
          .map(([n, id]) => `${n}=${id}`)
          .join(",");
        const res = await api.notices(since);
        if (!live) return;
        const first = Object.keys(cursors.current).length === 0;
        cursors.current = { ...cursors.current, ...res.cursors };
        try {
          localStorage.setItem(CURSORS_KEY, JSON.stringify(cursors.current));
        } catch {
          // storage unavailable
        }
        // The first poll of a fresh browser only learns where "now" is.
        if (first) return;
        const p = prefsRef.current;
        const fresh = res.notices.filter((n) => p.kinds[n.kind] !== false && !p.mutedNodes[n.node ?? ""]);
        if (fresh.length === 0) return;
        setRecent((r) => [...fresh.slice().reverse(), ...r].slice(0, 200));
        setUnseen((u) => u + fresh.length);
        if (p.toasts) setToasts((t) => [...t, ...fresh].slice(-5));
        if (p.browser && "Notification" in window && Notification.permission === "granted" && document.visibilityState !== "visible") {
          for (const n of fresh.slice(-3)) {
            try {
              const note = new Notification(n.title, { body: n.body ?? undefined, tag: noticeKey(n) });
              note.onclick = () => {
                window.focus();
                if (n.link) window.location.hash = "";
                if (n.link) window.history.pushState({}, "", n.link);
                window.dispatchEvent(new PopStateEvent("popstate"));
                note.close();
              };
            } catch {
              // notification blocked
            }
          }
        }
      } catch {
        // node unreachable; try again next tick
      } finally {
        if (live) timer = window.setTimeout(() => void poll(), 3000);
      }
    };
    void poll();
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, []);

  // Toasts age out on their own after 12 s.
  useEffect(() => {
    if (toasts.length === 0) return;
    const t = window.setTimeout(() => setToasts((x) => x.slice(1)), 12000);
    return () => window.clearTimeout(t);
  }, [toasts]);

  // The tab title carries the unseen count.
  useEffect(() => {
    document.title = unseen > 0 ? `(${unseen}) ${baseTitle.current}` : baseTitle.current;
  }, [unseen]);

  const markSeen = useCallback(() => setUnseen(0), []);

  const value = useMemo<NoticesApi>(
    () => ({ recent, unseen, markSeen, prefs, setPrefs, requestBrowser, dismissToast, toasts }),
    [recent, unseen, markSeen, prefs, setPrefs, requestBrowser, dismissToast, toasts],
  );
  return (
    <Ctx.Provider value={value}>
      {children}
      <ToastStack />
    </Ctx.Provider>
  );
}

function kindGlyph(kind: string): string {
  switch (kind) {
    case "question":
      return "?";
    case "permission":
      return "!";
    case "turn_ended":
      return "✓";
    case "activity_settled":
      return "◆";
    case "mcp_failed":
      return "✕";
    case "mcp_recovered":
      return "↻";
    case "exited":
      return "×";
    case "inbox":
      return "✉";
    default:
      return "•";
  }
}

function ToastStack() {
  const { toasts, dismissToast } = useNotices();
  const nav = useNavigate();
  if (toasts.length === 0) return null;
  return createPortal(
    <div className="toast-stack" role="status" aria-live="polite">
      {toasts.map((n) => {
        const key = noticeKey(n);
        return (
          <div
            className={`toast toast-${n.kind}`}
            key={key}
            onClick={() => {
              dismissToast(key);
              if (n.link) nav(n.link);
            }}
          >
            <span className={`toast-glyph mono k-${n.kind}`}>{kindGlyph(n.kind)}</span>
            <span className="toast-body">
              <span className="toast-title">{n.title}</span>
              {n.body && <span className="toast-text">{n.body}</span>}
              <span className="mono-meta">{n.node ?? ""}</span>
            </span>
            <button
              className="toast-x"
              aria-label="dismiss"
              onClick={(e) => {
                e.stopPropagation();
                dismissToast(key);
              }}
            >
              ×
            </button>
          </div>
        );
      })}
    </div>,
    document.body,
  );
}

/** The bell: unseen count, the recent list, and this browser's toggles. */
export function NoticesBell() {
  const { recent, unseen, markSeen, prefs, setPrefs, requestBrowser } = useNotices();
  const [open, setOpen] = useState(false);
  const [at, setAt] = useState({ top: 0, left: 0 });
  const [hook, setHook] = useState<{ webhook: string; command: string; kinds: string } | null>(null);
  const [hookNote, setHookNote] = useState<string | null>(null);
  const nav = useNavigate();
  const nodes = useMemo(() => [...new Set(recent.map((n) => n.node ?? "local"))], [recent]);

  useEffect(() => {
    if (!open) return;
    markSeen();
    api
      .settings()
      .then((s) => setHook({ webhook: s.notify?.webhook ?? "", command: s.notify?.command ?? "", kinds: (s.notify?.kinds ?? []).join(", ") }))
      .catch(() => setHook({ webhook: "", command: "", kinds: "" }));
  }, [open, markSeen]);

  async function saveHook() {
    if (!hook) return;
    const kinds = hook.kinds
      .split(",")
      .map((k) => k.trim())
      .filter(Boolean);
    try {
      await api.saveSettings({ notify: { webhook: hook.webhook.trim() || undefined, command: hook.command.trim() || undefined, kinds: kinds.length ? kinds : undefined } });
      setHookNote("saved on this node");
    } catch (e) {
      setHookNote(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <span className="artifacts-wrap">
      <button
        className={`btn ghost sm bell${unseen > 0 ? " has-unseen" : ""}`}
        title="notifications"
        aria-label="notifications"
        onClick={(e) => {
          const r = (e.currentTarget as HTMLButtonElement).getBoundingClientRect();
          setAt({ top: r.bottom + 4, left: Math.max(8, Math.min(r.left - 380, window.innerWidth - 480)) });
          setOpen((o) => !o);
        }}
      >
        ◔{unseen > 0 && <span className="bell-count mono">{unseen}</span>}
      </button>
      {open &&
        createPortal(
          <div className="artifacts-menu notices-menu" style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width: 470 }} role="menu" onMouseLeave={() => setOpen(false)}>
            <div className="row notices-head">
              <span className="label">Notifications</span>
              <span className="spacer" />
              <label className="mono-meta">
                <input type="checkbox" checked={prefs.toasts} onChange={(e) => setPrefs({ ...prefs, toasts: e.target.checked })} /> toasts
              </label>
              <label className="mono-meta" title="raise a browser notification while this tab is hidden">
                <input
                  type="checkbox"
                  checked={prefs.browser}
                  onChange={async (e) => {
                    if (e.target.checked) {
                      const r = await requestBrowser();
                      setPrefs({ ...prefs, browser: r === "granted" });
                    } else setPrefs({ ...prefs, browser: false });
                  }}
                />{" "}
                browser{"Notification" in window && Notification.permission === "denied" ? " (blocked)" : ""}
              </label>
            </div>
            <div className="row notices-kinds">
              {NOTICE_KINDS.map((k) => (
                <label key={k.kind} className={`chip kind-toggle${prefs.kinds[k.kind] === false ? " off" : ""}`} title={k.hint}>
                  <input type="checkbox" checked={prefs.kinds[k.kind] !== false} onChange={(e) => setPrefs({ ...prefs, kinds: { ...prefs.kinds, [k.kind]: e.target.checked } })} />
                  {k.label}
                </label>
              ))}
            </div>
            <PushRow />
            {nodes.length > 0 && (
              <div className="row notices-kinds">
                <span className="mono-meta">nodes:</span>
                {nodes.map((n) => (
                  <label key={n} className={`chip kind-toggle${prefs.mutedNodes[n] ? " off" : ""}`}>
                    <input type="checkbox" checked={!prefs.mutedNodes[n]} onChange={(e) => setPrefs({ ...prefs, mutedNodes: { ...prefs.mutedNodes, [n]: !e.target.checked } })} />
                    {n}
                  </label>
                ))}
              </div>
            )}
            <div className="notices-list">
              {recent.length === 0 ? (
                <div className="row dim">nothing yet this session — notices arrive as sessions finish turns, ask, or exit</div>
              ) : (
                recent.slice(0, 40).map((n) => (
                  <div
                    className="row notice-row"
                    key={noticeKey(n)}
                    onClick={() => {
                      setOpen(false);
                      if (n.link) nav(n.link);
                    }}
                  >
                    <span className={`toast-glyph mono k-${n.kind}`}>{kindGlyph(n.kind)}</span>
                    <span className="toast-body">
                      <span className="toast-title">{n.title}</span>
                      {n.body && <span className="toast-text">{n.body}</span>}
                    </span>
                    <span className="mono-meta">{relTime(n.ts)}</span>
                  </div>
                ))
              )}
            </div>
            {hook && (
              <form
                className="notices-hook"
                onSubmit={(e) => {
                  e.preventDefault();
                  void saveHook();
                }}
              >
                <span className="label">Outbound hook (this node)</span>
                <input className="mono" placeholder="webhook URL — receives each notice as JSON by POST" value={hook.webhook} onChange={(e) => setHook({ ...hook, webhook: e.target.value })} spellCheck={false} />
                <input className="mono" placeholder="command — given the notice JSON on stdin (e.g. an ntfy curl)" value={hook.command} onChange={(e) => setHook({ ...hook, command: e.target.value })} spellCheck={false} />
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <input className="mono" style={{ flex: 1 }} placeholder="kinds (default: question, permission, exited; * for all)" value={hook.kinds} onChange={(e) => setHook({ ...hook, kinds: e.target.value })} spellCheck={false} />
                  <button className="btn sm" type="submit">
                    save
                  </button>
                </div>
                {hookNote && <span className="mono-meta">{hookNote}</span>}
              </form>
            )}
          </div>,
          document.body,
        )}
    </span>
  );
}

function relTime(epochSeconds: number): string {
  const s = Math.max(0, Math.round(serverNow() - epochSeconds));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
}

/** This browser's name to the node it subscribes on: the console
 *  identity when there is one, else an id kept in this browser. */
function consoleName(): string {
  const id = loadIdentity();
  if (id?.node) return id.node;
  try {
    let v = localStorage.getItem("aspen.console.id");
    if (!v) {
      v = `browser-${Math.random().toString(36).slice(2, 8)}`;
      localStorage.setItem("aspen.console.id", v);
    }
    return v;
  } catch {
    return "browser";
  }
}

const PUSH_KINDS_KEY = "aspen.push.kinds";
const PUSH_DEFAULT = ["question", "permission"];
function loadPushKinds(): string[] {
  try {
    const raw = localStorage.getItem(PUSH_KINDS_KEY);
    return raw ? (JSON.parse(raw) as string[]) : PUSH_DEFAULT;
  } catch {
    return PUSH_DEFAULT;
  }
}

/** Web Push (CONSOLE_APP.md §5): notices as OS notifications with the
 *  console closed, from the node this console talks to. Off by default;
 *  needs-you kinds by default. */
function PushRow() {
  const [sub, setSub] = useState<PushSubscription | null>(null);
  const [kinds, setKinds] = useState<string[]>(loadPushKinds);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    pushCurrent().then(setSub).catch(() => setSub(null));
  }, []);
  if (!pushSupported()) {
    const ios = /iPhone|iPad|iPod/.test(navigator.userAgent);
    return (
      <div className="row mono-meta" title="Web Push needs a service worker, a push manager and the Notification API">
        {ios
          ? "push to this device: on iPhone and iPad, first add Aspen to the Home Screen (Share → Add to Home Screen) and open it from there — Safari allows Web Push only for installed apps"
          : "push to this device: this browser has no Web Push"}
      </div>
    );
  }
  const saveKinds = async (next: string[]) => {
    setKinds(next);
    try {
      localStorage.setItem(PUSH_KINDS_KEY, JSON.stringify(next));
    } catch {
      /* ignore */
    }
    if (sub) {
      try {
        await api.pushSubscribe(sub.toJSON(), consoleName(), next);
      } catch (e) {
        setNote(e instanceof Error ? e.message : "could not update");
      }
    }
  };
  const toggle = async () => {
    setBusy(true);
    setNote(null);
    try {
      if (sub) {
        await pushUnsubscribe();
        setSub(null);
        setNote("this device will not be pushed");
      } else {
        const s = await pushSubscribe(consoleName(), kinds);
        setSub(s);
        setNote("subscribed on this node");
      }
    } catch (e) {
      setNote(e instanceof Error ? e.message : "push failed");
    } finally {
      setBusy(false);
    }
  };
  const test = async () => {
    if (!sub) return;
    setBusy(true);
    setNote(null);
    try {
      await api.pushTest(sub.endpoint);
      setNote("sent — the push service accepted it");
    } catch (e) {
      setNote(e instanceof Error ? e.message : "test failed");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="row notices-kinds" style={{ alignItems: "center", flexWrap: "wrap", gap: 6 }}>
      <label className="mono-meta" title="OS notifications with the console closed, sent by the node this console talks to; blocked sites need the browser's site settings">
        <input type="checkbox" checked={!!sub} disabled={busy} onChange={() => void toggle()} /> push to this device
      </label>
      {sub && (
        <>
          {NOTICE_KINDS.map((k) => (
            <label key={k.kind} className={`chip kind-toggle${kinds.includes(k.kind) ? "" : " off"}`} title={`push ${k.hint}`}>
              <input
                type="checkbox"
                checked={kinds.includes(k.kind)}
                onChange={(e) => void saveKinds(e.target.checked ? [...kinds, k.kind] : kinds.filter((x) => x !== k.kind))}
              />
              {k.label}
            </label>
          ))}
          <button className="btn ghost sm" disabled={busy} onClick={() => void test()} title="send a test notification to this device">test</button>
        </>
      )}
      {note && <span className="mono-meta">{note}</span>}
    </div>
  );
}
