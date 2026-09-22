// The branch card: the one card every entry to the branch verb opens (the
// ⎇ button, the ⋯ row, `/branch`, `/fork`, `/branch <name>`). Two
// choices, spelled out, and a button that says what will happen — nothing
// acts unseen (PROPOSALS-2026-09-J.md §4.2). Move (the name follows the
// branch, this point is kept as an earlier transcript) is the default;
// "start a new agent" is the split.
import { useEffect, useRef, useState } from "react";

export type BranchChoice = "move" | "new";

/** First free `<bare>-N` among the repo's names, N from 2. */
export function suggestBranchName(bare: string, taken: string[]): string {
  const set = new Set(taken.map((t) => t.toLowerCase()));
  for (let n = 2; n < 1000; n++) {
    const cand = `${bare}-${n}`;
    if (!set.has(cand.toLowerCase())) return cand;
  }
  return `${bare}-${Date.now() % 10000}`;
}

export function BranchCard({
  bare,
  taken,
  initialChoice,
  initialName,
  initialLabel,
  busy,
  onSubmit,
  onCancel,
}: {
  /** The agent's bare name (no @repo). */
  bare: string;
  /** Bare names already in this repo (rejected inline). */
  taken: string[];
  initialChoice: BranchChoice;
  initialName?: string;
  initialLabel?: string;
  busy: boolean;
  onSubmit: (choice: BranchChoice, opts: { name: string; label: string }) => void;
  onCancel: () => void;
}) {
  const [choice, setChoice] = useState<BranchChoice>(initialChoice);
  const [name, setName] = useState(initialName ?? suggestBranchName(bare, taken));
  const [label, setLabel] = useState(initialLabel ?? "");
  const nameRef = useRef<HTMLInputElement | null>(null);
  const labelRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    // The focused field is the one the choice reads; the prefilled name is
    // selected so typing replaces it and Enter accepts it.
    if (choice === "new") {
      nameRef.current?.focus();
      nameRef.current?.select();
    } else {
      labelRef.current?.focus();
    }
  }, [choice]);
  const trimmed = name.trim();
  const valid = /^[A-Za-z0-9_-]+$/.test(trimmed);
  const isTaken = taken.some((t) => t.toLowerCase() === trimmed.toLowerCase()) || trimmed.toLowerCase() === bare.toLowerCase();
  const nameProblem = choice !== "new" ? null : !trimmed ? "a name is needed" : !valid ? "letters, digits, - and _" : isTaken ? `@${trimmed} already exists in this repo` : null;
  const canGo = !busy && nameProblem === null;
  function go() {
    if (!canGo) return;
    onSubmit(choice, { name: trimmed, label: label.trim() });
  }
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      go();
    }
    if (e.key === "Escape") onCancel();
  };
  return (
    <div className="branch-card" role="dialog" aria-label={`branch @${bare} here`} onKeyDown={onKey}>
      <div className="branch-card-title">Branch @{bare} here</div>
      <label className={`branch-choice${choice === "move" ? " on" : ""}`}>
        <input type="radio" name="branch-choice" checked={choice === "move"} onChange={() => setChoice("move")} />
        <span className="branch-choice-body">
          <span className="branch-choice-head">Move @{bare} onto the branch</span>
          <span className="branch-choice-hint">This transcript stays as an earlier point of @{bare}. Restarts @{bare}; a running turn is interrupted.</span>
          <span className="branch-choice-field">
            <span className="mono-meta">label</span>
            <input ref={labelRef} className="mono" value={label} onChange={(e) => setLabel(e.target.value)} placeholder="optional" onFocus={() => setChoice("move")} aria-label="label for the earlier point" />
          </span>
        </span>
      </label>
      <label className={`branch-choice${choice === "new" ? " on" : ""}`}>
        <input type="radio" name="branch-choice" checked={choice === "new"} onChange={() => setChoice("new")} />
        <span className="branch-choice-body">
          <span className="branch-choice-head">Start a new agent on the branch</span>
          <span className="branch-choice-hint">@{bare} keeps this transcript and keeps running.</span>
          <span className="branch-choice-field">
            <span className="mono-meta">@</span>
            <input ref={nameRef} className="mono" value={name} onChange={(e) => setName(e.target.value)} onFocus={() => setChoice("new")} aria-label="name of the new agent" />
            {nameProblem && choice === "new" && <span className="branch-problem">{nameProblem}</span>}
          </span>
        </span>
      </label>
      <div className="branch-card-actions">
        <button type="button" className="btn primary sm" disabled={!canGo} onClick={go}>
          {busy ? "branching…" : choice === "new" ? `Start @${trimmed || "…"}` : `Move @${bare}`}
        </button>
        <button type="button" className="btn ghost sm" onClick={onCancel}>Cancel</button>
      </div>
    </div>
  );
}
