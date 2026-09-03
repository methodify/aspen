// The hotkey registry: one place every binding lives, so the help dialog
// can always tell the truth. Bindings are SCOPED — "global" plus one scope
// per surface (derived from the route) — and the help dialog ("?") shows
// exactly what is in scope right now, straight from the registry.

import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useLocation, useNavigate } from "react-router-dom";
import "./hotkeys.css";

export interface HotkeyDef {
  /** Display form shown in the help dialog, e.g. "c", "i", "⌘K", "?". */
  key: string;
  description: string;
  /** Custom matcher; defaults to a plain `e.key === key` (case-insensitive). */
  match?: (e: KeyboardEvent) => boolean;
  /** Omit for display-only entries (handled elsewhere, e.g. the palette). */
  handler?: () => void;
  /** Extra enablement beyond scope (e.g. only while the session is busy). */
  when?: () => boolean;
  /** Fire even when an input/textarea has focus (rare; default false). */
  inInputs?: boolean;
}

interface RegistryEntry {
  scope: string;
  defs: HotkeyDef[];
}

interface HotkeysCtx {
  register: (id: symbol, scope: string, defs: HotkeyDef[]) => void;
  unregister: (id: symbol) => void;
  activeScope: string;
}

const Ctx = createContext<HotkeysCtx | null>(null);

/** Route → surface scope. Keep in sync with the App's routes. */
export function scopeForPath(pathname: string): string {
  if (pathname === "/") return "now";
  if (pathname.startsWith("/flow")) return "flow";
  if (pathname.startsWith("/session/")) return "session";
  if (pathname.startsWith("/mesh")) return "mesh";
  return "global";
}

const SCOPE_LABELS: Record<string, string> = {
  global: "Everywhere",
  now: "Now",
  flow: "Flow",
  session: "Session",
  mesh: "Mesh",
};

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

/**
 * Register bindings for a scope. Defs are refreshed every render so
 * handlers close over current state; everything unregisters on unmount.
 */
export function useHotkeys(scope: string, defs: HotkeyDef[]): void {
  const ctx = useContext(Ctx);
  const idRef = useRef<symbol | null>(null);
  if (idRef.current === null) idRef.current = Symbol(scope);
  const id = idRef.current;

  useEffect(() => {
    ctx?.register(id, scope, defs);
  });
  useEffect(() => {
    return () => ctx?.unregister(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}

export function HotkeysProvider({ children }: { children: ReactNode }) {
  const location = useLocation();
  const activeScope = scopeForPath(location.pathname);
  const entries = useRef<Map<symbol, RegistryEntry>>(new Map());
  const scopeRef = useRef(activeScope);
  scopeRef.current = activeScope;
  const [helpOpen, setHelpOpen] = useState(false);
  const helpOpenRef = useRef(false);
  helpOpenRef.current = helpOpen;

  const ctx: HotkeysCtx = {
    register: (id, scope, defs) => {
      entries.current.set(id, { scope, defs });
    },
    unregister: (id) => {
      entries.current.delete(id);
    },
    activeScope,
  };

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.defaultPrevented) return;
      // The help dialog itself: "?" opens, Escape closes.
      const isHelpKey = e.key === "?" || (e.key === "/" && e.shiftKey);
      if (helpOpenRef.current) {
        if (e.key === "Escape" || isHelpKey) {
          e.preventDefault();
          setHelpOpen(false);
        }
        return;
      }
      const editable = isEditable(e.target);
      if (!editable && isHelpKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        setHelpOpen(true);
        return;
      }
      for (const { scope, defs } of entries.current.values()) {
        if (scope !== "global" && scope !== scopeRef.current) continue;
        for (const d of defs) {
          if (!d.handler) continue;
          if (editable && !d.inInputs) continue;
          const hit = d.match
            ? d.match(e)
            : e.key.toLowerCase() === d.key.toLowerCase() &&
              !e.ctrlKey &&
              !e.metaKey &&
              !e.altKey;
          if (!hit) continue;
          if (d.when && !d.when()) continue;
          e.preventDefault();
          d.handler();
          return;
        }
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <Ctx.Provider value={ctx}>
      {children}
      {helpOpen && (
        <HelpDialog
          activeScope={activeScope}
          entries={[...entries.current.values()]}
          onClose={() => setHelpOpen(false)}
        />
      )}
    </Ctx.Provider>
  );
}

function HelpDialog({
  activeScope,
  entries,
  onClose,
}: {
  activeScope: string;
  entries: RegistryEntry[];
  onClose: () => void;
}) {
  // Everything in scope right now, grouped: global first, then this view.
  const groups: { scope: string; defs: HotkeyDef[] }[] = [];
  for (const scope of ["global", activeScope]) {
    const defs = entries
      .filter((en) => en.scope === scope)
      .flatMap((en) => en.defs)
      .filter((d) => !d.when || d.when());
    if (defs.length > 0 && !groups.some((g) => g.scope === scope)) {
      groups.push({ scope, defs });
    }
  }
  return (
    <div className="hk-backdrop" onClick={onClose} role="presentation">
      <div
        className="hk-panel"
        role="dialog"
        aria-label="keyboard shortcuts"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="hk-head">
          <span className="label">Keyboard</span>
          <span style={{ flex: 1 }} />
          <button className="btn ghost sm" onClick={onClose}>
            esc
          </button>
        </div>
        {groups.map((g) => (
          <div key={g.scope} className="hk-group">
            <div className="hk-scope label">
              {g.scope === "global"
                ? SCOPE_LABELS.global
                : `This view · ${SCOPE_LABELS[g.scope] ?? g.scope}`}
            </div>
            {g.defs.map((d, i) => (
              <div key={`${d.key}-${i}`} className="hk-row">
                <kbd>{d.key}</kbd>
                <span>{d.description}</span>
              </div>
            ))}
          </div>
        ))}
        <div className="hk-foot mono-meta">
          Shortcuts pause while a text field has focus.
        </div>
      </div>
    </div>
  );
}

/** Global bindings: surface navigation (the letters already shown in the
 * nav), the palette (display-only — the palette owns its own listener),
 * and this dialog. Mount once inside the provider + router. */
export function GlobalHotkeys() {
  const nav = useNavigate();
  useHotkeys("global", [
    { key: "n", description: "go to Now", handler: () => nav("/") },
    { key: "f", description: "go to Flow", handler: () => nav("/flow") },
    { key: "m", description: "go to Mesh", handler: () => nav("/mesh") },
    { key: "⌘K", description: "command palette" },
    { key: "?", description: "this help" },
  ]);
  return null;
}
