// The top of a session (docs/PROPOSALS-2026-09-G.md): one bar shared by
// the session page and a board pane — identity and presence on the
// left, readouts and three verbs on the right, and one ⋯ menu that is
// the controls. Presence is one glyph; the bar has three verb slots and
// two readout slots, and the next feature goes in the menu.

import { useEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";

export type BarPresence = "busy" | "live" | "reconnecting" | "exited" | "readonly";

const GLYPH: Record<BarPresence, { glyph: string; title: string; cls: string }> = {
  busy: { glyph: "◐", title: "busy — a turn is running", cls: "p-busy" },
  live: { glyph: "●", title: "live — idle, connected", cls: "p-live" },
  reconnecting: { glyph: "◌", title: "reconnecting to the node", cls: "p-reconnecting" },
  exited: { glyph: "○", title: "not running", cls: "p-exited" },
  readonly: { glyph: "◇", title: "read-only view", cls: "p-readonly" },
};

/** One presence glyph, one place (§2): from the agent's liveness, the
 *  turn state, the event socket and the view mode. */
export function barPresence(a: { live?: boolean; turn_state?: string | null } | undefined, ws: string, exited: boolean, readOnly: boolean): BarPresence {
  if (readOnly) return "readonly";
  if (exited || (a && !a.live)) return "exited";
  if (ws !== "open") return "reconnecting";
  return a?.turn_state === "busy" ? "busy" : "live";
}

export function PresenceGlyph({ presence }: { presence: BarPresence }) {
  const g = GLYPH[presence];
  return (
    <span className={`presence-glyph ${g.cls}`} title={g.title} aria-label={g.title} role="img">
      {g.glyph}
    </span>
  );
}

/** An icon verb on the bar: a glyph, a tooltip, a 36 px hit area. */
export function BarVerb({ glyph, title, onClick, disabled, danger, className }: { glyph: string; title: string; onClick: () => void; disabled?: boolean; danger?: boolean; className?: string }) {
  return (
    <button type="button" className={`bar-verb${danger ? " danger" : ""}${className ? ` ${className}` : ""}`} title={title} aria-label={title} onClick={onClick} disabled={disabled}>
      {glyph}
    </button>
  );
}

/** The ⋯ menu. Always mounted (its rows own state — the plugins, MCP
 *  and activity panels — and the status line can open those by signal);
 *  hidden when closed. Closes on Esc, on a click outside it and outside
 *  any panel it opened, and when a row says so. Arrow keys walk the
 *  rows. Under 720 px it is a bottom sheet (session.css). */
export function SessionMenu({ open, onClose, anchor, children }: { open: boolean; onClose: () => void; anchor: { top: number; left: number; right: number } | null; children: ReactNode }) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        const items = Array.from(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]:not([disabled]), select, button.charter-toggle") ?? []);
        if (items.length === 0) return;
        const i = items.indexOf(document.activeElement as HTMLElement);
        const next = e.key === "ArrowDown" ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
        items[next]?.focus();
        e.preventDefault();
      }
    };
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t) return;
      if (ref.current?.contains(t)) return;
      // A panel a row opened (plugins, MCP, activity, artifacts, board),
      // or the ⋯ button itself — its click toggles; closing here first
      // would reopen it.
      if (t.closest(".artifacts-menu, .bar-menu-btn")) return;
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown, true);
    // First row takes focus so the keyboard works at once.
    window.setTimeout(() => ref.current?.querySelector<HTMLElement>("[role=menuitem], select")?.focus(), 0);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown, true);
    };
  }, [open, onClose]);
  const style: React.CSSProperties = anchor ? { top: anchor.top, right: anchor.right, left: "auto" } : {};
  return createPortal(
    <div ref={ref} className="session-menu" role="menu" hidden={!open} style={style} onClick={(e) => e.stopPropagation()}>
      {children}
    </div>,
    document.body,
  );
}

export function MenuGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="menu-group">
      <span className="menu-label">{label}</span>
      {children}
    </div>
  );
}

/** A plain action row. */
export function MenuRow({ label, hint, onClick, disabled, badge, live }: { label: string; hint?: string; onClick: () => void; disabled?: boolean; badge?: string | number | null; live?: boolean }) {
  return (
    <button type="button" role="menuitem" className={`menu-row${live ? " live" : ""}`} onClick={onClick} disabled={disabled} title={hint}>
      <span className="menu-row-label">{label}</span>
      {badge !== undefined && badge !== null && badge !== "" && badge !== 0 && <span className="menu-badge mono">{badge}</span>}
    </button>
  );
}

/** A row that carries a control (a select, a segmented switch). */
export function MenuField({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <div className="menu-row menu-field" title={hint}>
      <span className="menu-row-label">{label}</span>
      <span className="menu-field-ctl">{children}</span>
    </div>
  );
}

/** Where a row's panel opens. A row in the ⋯ menu puts its panel beside
 *  the menu (to its left, level with the row) so the menu stays open and
 *  the operator can go down the list — plugins, then MCP, then activity
 *  — without reopening it; the panel stacks above the menu. When there
 *  is no room beside (a phone), it opens over the row instead. A row on
 *  the bar itself opens under it, as before. */
export function panelAnchor(el: HTMLElement | null, width: number): { top: number; left: number } {
  const r = el?.getBoundingClientRect();
  if (!r || r.width === 0) return { top: 52, left: Math.max(8, window.innerWidth - width - 16) };
  const menu = el?.closest(".session-menu")?.getBoundingClientRect();
  if (menu) {
    const beside = menu.left - width - 8;
    const top = Math.max(8, Math.min(r.top, window.innerHeight - 80));
    if (beside >= 8) return { top, left: beside };
    return { top: Math.max(8, r.top - 8), left: 8 };
  }
  return { top: r.bottom + 4, left: Math.max(8, Math.min(r.left, window.innerWidth - width - 8)) };
}

/** A panel opened from the bar or the menu (plugins, MCP, activity,
 *  artifacts, boards, usage): fixed at `at`, a real popover — it stays
 *  while the pointer wanders, closes on Esc, on a click outside it (a
 *  click in the ⋯ menu or another panel does not count, so the operator
 *  can go down the list), or on its × (PROPOSALS-2026-09-G.md §3). */
export function PanelFrame({ at, width, className, title, extra, onClose, children, style }: { at: { top: number; left: number }; width: number; className?: string; title?: string; extra?: ReactNode; onClose: () => void; children: ReactNode; style?: React.CSSProperties }) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t || ref.current?.contains(t)) return;
      if (t.closest(".session-menu, .artifacts-menu, .bar-menu-btn")) return;
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown, true);
    };
  }, [onClose]);
  return createPortal(
    <div ref={ref} className={`artifacts-menu panel-frame${className ? ` ${className}` : ""}`} style={{ position: "fixed", top: at.top, left: at.left, right: "auto", width, ...style }} role="dialog" aria-label={title}>
      {title !== undefined && (
        <div className="row panel-head">
          <span className="label">{title}</span>
          <span style={{ flex: 1 }} />
          {extra}
          <button type="button" className="status-note-x" onClick={onClose} title="close (esc)" aria-label="close">×</button>
        </div>
      )}
      {title === undefined && (
        <button type="button" className="status-note-x panel-x" onClick={onClose} title="close (esc)" aria-label="close">×</button>
      )}
      {children}
    </div>,
    document.body,
  );
}
