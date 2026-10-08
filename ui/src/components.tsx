// Shared design-system components for the Switchboard console. Every surface
// composes these so the identity stays coherent: the bus capsule, the
// engraved class badge, the carrier meter, class-coded message rows, and the
// class selector all speak the same two color systems (priority + presence).

import { useSyncExternalStore, type ReactNode } from "react";
import { serverNow, type Urgency, type TurnState } from "./api";

export type Presence = "busy" | "idle" | "off";

export function presenceOf(live: boolean, turn: TurnState | null): Presence {
  if (!live) return "off";
  return turn === "busy" ? "busy" : "idle";
}

/** A capsule on the bus rail — priority legible before a word is read. */
export function Capsule({ urgency }: { urgency: Urgency }) {
  const cls = urgency === "gating" ? "gate" : urgency === "notice" ? "notice" : "normal";
  return <span className={`capsule ${cls}`} aria-hidden />;
}

/** The engraved delivery-class plate: GATE / NORM / NOTE. */
export function ClassBadge({ urgency }: { urgency: Urgency }) {
  const cls = urgency === "gating" ? "gate" : urgency === "notice" ? "notice" : "normal";
  const label = urgency === "gating" ? "gate" : urgency === "notice" ? "note" : "norm";
  return <span className={`class-badge ${cls}`}>{label}</span>;
}

/** Presence as a live carrier meter, never a static dot. */
export function Meter({ presence }: { presence: Presence }) {
  const cls = presence === "busy" ? "meter busy" : presence === "off" ? "meter off" : "meter";
  return (
    <span className={cls} role="img" aria-label={`${presence} agent`}>
      <i /><i /><i /><i />
    </span>
  );
}

/** Up to this many bars; a bigger board's members share bars in proportion. */
const BOARD_BARS = 6;

/** A board's meter (BOARDS.md §10): one bar per member, busy first — a
 *  busy member's bar dances, a live idle one stands full, a stopped one
 *  lies flat — so a board reads at a glance as "two working, one idle,
 *  three down". Past six members the bars are shared in proportion, never
 *  dropping a state that is present. No members: the flat line of the
 *  plain meter. */
export function BoardMeter({ bars, label }: { bars: Presence[]; label?: string }) {
  if (bars.length === 0) return <Meter presence="off" />;
  let shown = bars;
  if (bars.length > BOARD_BARS) {
    const count = (p: Presence) => bars.filter((b) => b === p).length;
    const share = (n: number) => (n ? Math.max(1, Math.round((n * BOARD_BARS) / bars.length)) : 0);
    let b = share(count("busy"));
    let i = share(count("idle"));
    let o = share(count("off"));
    // Trim the biggest share until it fits (each present state keeps ≥ 1).
    while (b + i + o > BOARD_BARS) {
      if (o >= i && o >= b && o > 1) o--;
      else if (i >= b && i > 1) i--;
      else if (b > 1) b--;
      else break;
    }
    shown = [...Array<Presence>(b).fill("busy"), ...Array<Presence>(i).fill("idle"), ...Array<Presence>(o).fill("off")];
  }
  return (
    <span className="board-meter" role="img" aria-label={label ?? `${bars.length} sessions`}>
      {shown.map((p, k) => (
        <i key={k} className={p} />
      ))}
    </span>
  );
}

/** The three-way outbound class selector used by every composer. */
export function ClassSelect({
  value,
  onChange,
}: {
  value: Urgency;
  onChange: (u: Urgency) => void;
}) {
  const opts: { u: Urgency; cls: string; label: string }[] = [
    { u: "normal", cls: "normal", label: "norm" },
    { u: "gating", cls: "gate", label: "gate" },
    { u: "notice", cls: "notice", label: "note" },
  ];
  return (
    <div className="class-select" role="group" aria-label="delivery class">
      {opts.map((o) => (
        <button
          key={o.u}
          type="button"
          className={o.cls}
          aria-pressed={value === o.u}
          onClick={() => onChange(o.u)}
          title={
            o.u === "gating"
              ? "gating — interrupt the recipient's turn now"
              : o.u === "notice"
                ? "notice — ambient, never interrupts or wakes"
                : "normal — arrives at a turn boundary, or wakes an idle agent"
          }
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A message rendered as a row docked off the bus, class-coded. */
export function MessageRow({
  urgency,
  sender,
  meta,
  children,
  actions,
}: {
  urgency: Urgency;
  sender: string;
  meta?: ReactNode;
  children: ReactNode;
  actions?: ReactNode;
}) {
  if (urgency === "notice") {
    return (
      <div className="notice-line">
        <Capsule urgency={urgency} /> {sender}: {children}
      </div>
    );
  }
  const cls = urgency === "gating" ? "gate" : "normal";
  return (
    <div className={`msg ${cls}`}>
      <div className="tab" />
      <div className="msg-inner">
        <div className="msg-head">
          <ClassBadge urgency={urgency} />
          <span className="sender mono">@{sender}</span>
          <span style={{ flex: 1 }} />
          {meta}
        </div>
        <div className="msg-body">{children}</div>
        {actions && <div style={{ marginTop: 8, display: "flex", gap: 8, flexWrap: "wrap" }}>{actions}</div>}
      </div>
    </div>
  );
}

export function ErrorBar({ error }: { error: string | null }) {
  if (!error) return null;
  return <div className="error-bar">{error}</div>;
}

export function Empty({ mark, children }: { mark?: string; children: ReactNode }) {
  return (
    <div className="empty">
      {mark && <div className="empty-mark">{mark}</div>}
      {children}
    </div>
  );
}

/** Relative "42s / 6m / 3h / 2d" from an epoch-seconds timestamp. */
export function relTime(epochSeconds: number): string {
  const s = Math.max(0, serverNow() - epochSeconds);
  if (s < 60) return `${Math.floor(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** Dark/light theme with an explicit toggle over prefers-color-scheme. */
// One theme for the whole console: a module-level value every useTheme()
// shares. Local state per caller let the palette and the More sheet
// disagree after a toggle in either (the 2026-10 quality pass).
const themeListeners = new Set<() => void>();
let themeValue: string = (() => {
  try {
    return localStorage.getItem("aspen.theme") || "system";
  } catch {
    return "system";
  }
})();
function applyTheme(t: string) {
  if (typeof document === "undefined") return; // tests, no DOM
  const root = document.documentElement;
  if (t === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", t);
  try {
    localStorage.setItem("aspen.theme", t);
  } catch {
    /* private mode */
  }
}
applyTheme(themeValue);
function setThemeValue(t: string) {
  themeValue = t;
  applyTheme(t);
  for (const l of themeListeners) l();
}

export function useTheme(): [string, () => void] {
  const theme = useSyncExternalStore(
    (cb) => {
      themeListeners.add(cb);
      return () => themeListeners.delete(cb);
    },
    () => themeValue,
  );
  const toggle = () => setThemeValue(theme === "dark" ? "light" : theme === "light" ? "system" : "dark");
  return [theme, toggle];
}
