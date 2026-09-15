// The session menu's registry (docs/PROPOSALS-2026-09-G.md §2): every
// entry in the ⋯ menu of the session in view is also a command in the
// palette, from the same spec. A new session feature ships as one of
// these and never touches the bar. One session registers at a time —
// the page, or the focused pane of a board.

import { useSyncExternalStore } from "react";

export type CommandGroup = "verb" | "setup" | "inspect" | "move";

export interface SessionCommand {
  id: string;
  group: CommandGroup;
  label: string;
  /** A short hint shown beside the label in the palette. */
  hint?: string;
  run: () => void;
}

interface Registration {
  agent: string;
  commands: SessionCommand[];
}

let current: Registration | null = null;
const listeners = new Set<() => void>();

export function setSessionCommands(agent: string, commands: SessionCommand[]): void {
  current = { agent, commands };
  for (const l of listeners) l();
}
export function clearSessionCommands(agent: string): void {
  if (current?.agent !== agent) return;
  current = null;
  for (const l of listeners) l();
}
function subscribe(l: () => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}
export function useSessionCommands(): Registration | null {
  return useSyncExternalStore(subscribe, () => current);
}
