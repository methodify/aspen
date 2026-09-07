// Per-tool renderers for tool cards (PROPOSALS-2026-09 §2). A tool's input
// is shown the way its user would read it — a command, a path and range,
// a diff — not as raw JSON; the summary line carries a one-word result
// hint the way the claude TUI's collapsed line does.

import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import type { ToolCardItem } from "./transcript";
import { viewHref } from "./pathLinks";

/** A path in a tool card links to the viewer for this agent. */
function PathLink({ path, agent }: { path: string; agent?: string }) {
  if (!agent || !path) return <>{path}</>;
  return (
    <Link className="tool-path" to={viewHref(agent, path)} title="open in the viewer">
      {path}
    </Link>
  );
}

function rec(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}
function str(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}
function lineCount(s: string): number {
  if (!s) return 0;
  return s.split("\n").length - (s.endsWith("\n") ? 1 : 0);
}

/** One-word-ish outcome for the collapsed summary line, once done. */
export function resultHint(item: ToolCardItem): string {
  if (!item.done) return "";
  if (item.isError) return "error";
  const input = rec(item.input);
  const result = item.result ?? "";
  switch (item.name) {
    case "Bash": {
      const n = lineCount(result);
      return n === 0 ? "no output" : `${n} line${n === 1 ? "" : "s"}`;
    }
    case "Write": {
      const content = str(input?.["content"]) ?? "";
      const n = lineCount(content);
      return n ? `wrote ${n} line${n === 1 ? "" : "s"}` : "wrote";
    }
    case "Edit":
      return input?.["replace_all"] ? "edited all" : "edited";
    case "Read": {
      const n = lineCount(result);
      return n ? `${n} line${n === 1 ? "" : "s"}` : "read";
    }
    case "Grep":
    case "Glob": {
      const n = lineCount(result);
      return /no (matches|files) found/i.test(result) ? "no matches" : n ? `${n} match${n === 1 ? "" : "es"}` : "done";
    }
    default:
      return item.result === null ? "" : "done";
  }
}

/** Unified-ish diff of old→new for an Edit: whole blocks, prefixed. */
function DiffView({ oldText, newText }: { oldText: string; newText: string }) {
  return (
    <pre className="tool-diff">
      {oldText.split("\n").map((l, i) => (
        <span key={`o${i}`} className="diff-del">{`- ${l}\n`}</span>
      ))}
      {newText.split("\n").map((l, i) => (
        <span key={`n${i}`} className="diff-add">{`+ ${l}\n`}</span>
      ))}
    </pre>
  );
}

function Collapsible({ text, limit = 40, className }: { text: string; limit?: number; className?: string }) {
  const lines = text.split("\n");
  if (lines.length <= limit) return <pre className={className}>{text}</pre>;
  return (
    <details className="tool-more">
      <summary className="mono-meta">{`${lines.length} lines — first ${limit} shown`}</summary>
      <pre className={className}>{text}</pre>
    </details>
  );
}

/** The card body: what the tool was asked, then what came back. */
export function ToolBody({ item, agent }: { item: ToolCardItem; agent?: string }): ReactNode {
  const input = rec(item.input);
  const result = item.result;
  const resultBlock =
    result !== null ? (
      <>
        <div className="tool-section-label">{item.isError ? "error" : "result"}</div>
        <Collapsible text={result} limit={60} className={item.isError ? "tool-err" : undefined} />
      </>
    ) : item.done ? (
      <div className="dim tool-section-label">no result recorded (turn ended)</div>
    ) : null;

  switch (item.name) {
    case "Bash": {
      const cmd = str(input?.["command"]) ?? "";
      const desc = str(input?.["description"]);
      return (
        <>
          {desc && <div className="mono-meta">{desc}</div>}
          <pre className="tool-cmd">{`$ ${cmd}`}</pre>
          {resultBlock}
        </>
      );
    }
    case "Read": {
      const path = str(input?.["file_path"]) ?? "";
      const off = input?.["offset"];
      const lim = input?.["limit"];
      return (
        <>
          <div className="mono">
            <PathLink path={path} agent={agent} />
            {typeof off === "number" || typeof lim === "number" ? (
              <span className="mono-meta">{` · from line ${typeof off === "number" ? off : 1}${typeof lim === "number" ? `, ${lim} lines` : ""}`}</span>
            ) : null}
          </div>
          {resultBlock}
        </>
      );
    }
    case "Edit": {
      const path = str(input?.["file_path"]) ?? "";
      return (
        <>
          <div className="mono">
            <PathLink path={path} agent={agent} />
            {input?.["replace_all"] ? <span className="mono-meta"> · replace all</span> : null}
          </div>
          <DiffView oldText={str(input?.["old_string"]) ?? ""} newText={str(input?.["new_string"]) ?? ""} />
          {resultBlock}
        </>
      );
    }
    case "Write": {
      const path = str(input?.["file_path"]) ?? "";
      const content = str(input?.["content"]) ?? "";
      return (
        <>
          <div className="mono">
            <PathLink path={path} agent={agent} />
            <span className="mono-meta">{` · ${lineCount(content)} lines`}</span>
          </div>
          <Collapsible text={content} />
          {resultBlock}
        </>
      );
    }
    case "Grep":
    case "Glob": {
      const pattern = str(input?.["pattern"]) ?? "";
      const path = str(input?.["path"]);
      return (
        <>
          <div className="mono">
            <span className="tool-pattern">{pattern}</span>
            {path && <span className="mono-meta">{` in ${path}`}</span>}
          </div>
          {resultBlock}
        </>
      );
    }
    default:
      return (
        <>
          <div className="tool-section-label">input</div>
          <Collapsible text={JSON.stringify(item.input, null, 2)} />
          {resultBlock}
        </>
      );
  }
}
