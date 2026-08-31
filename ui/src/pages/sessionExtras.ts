// Pure helpers for the Session view's interactive extras: question cards
// (AskUserQuestion §7.6), always-allow suggestion summaries, slash-command
// autocomplete, model normalization, context-usage summarization, render
// modes, and status-event notes. No React, no I/O — unit-testable.

import type { RuntimeInfo } from "../api";

function asRecord(v: unknown): Record<string, unknown> | null {
  return v !== null && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
}

// ---------------------------------------------------------------------------
// AskUserQuestion (§7.6)

export interface QuestionOption {
  label: string;
  description: string | null;
}

export interface QuestionSpec {
  question: string;
  header: string | null;
  multiSelect: boolean;
  options: QuestionOption[];
}

/**
 * Parse an AskUserQuestion-shaped tool input into question specs.
 * Returns null when the input carries no usable `questions` array — the
 * caller should then fall back to a plain permission card.
 */
export function parseQuestions(input: unknown): QuestionSpec[] | null {
  const r = asRecord(input);
  const raw = r?.["questions"];
  if (!Array.isArray(raw)) return null;
  const out: QuestionSpec[] = [];
  for (const entry of raw) {
    const q = asRecord(entry);
    if (!q || typeof q["question"] !== "string" || !q["question"]) continue;
    const options: QuestionOption[] = [];
    const rawOpts = q["options"];
    if (Array.isArray(rawOpts)) {
      for (const o of rawOpts) {
        const or = asRecord(o);
        if (!or || typeof or["label"] !== "string" || !or["label"]) continue;
        options.push({
          label: or["label"],
          description: typeof or["description"] === "string" ? or["description"] : null,
        });
      }
    }
    out.push({
      question: q["question"],
      header: typeof q["header"] === "string" && q["header"] ? q["header"] : null,
      multiSelect: q["multiSelect"] === true,
      options,
    });
  }
  return out.length > 0 ? out : null;
}

/**
 * Build the §7.6 `updated_input` for an allow: echo the original `questions`
 * verbatim, key `answers` by question text (label for single-select, label
 * array for multiSelect; unanswered questions omitted), and attach the
 * optional free-text `response`.
 */
export function buildQuestionUpdatedInput(
  originalInput: unknown,
  questions: QuestionSpec[],
  picks: string[][],
  response: string,
): Record<string, unknown> {
  const r = asRecord(originalInput);
  const answers: Record<string, string | string[]> = {};
  questions.forEach((q, i) => {
    const picked = picks[i] ?? [];
    if (picked.length === 0) return;
    answers[q.question] = q.multiSelect ? picked : picked[0]!;
  });
  const out: Record<string, unknown> = {
    questions: r?.["questions"] ?? [],
    answers,
  };
  const trimmed = response.trim();
  if (trimmed) out["response"] = trimmed;
  return out;
}

// ---------------------------------------------------------------------------
// Always-allow suggestion summaries (§7.4 PermissionUpdate[])

/**
 * Human-readable summary of the CLI's suggested permission grant, when the
 * shape is recognizable; null otherwise (caller shows a generic caption).
 */
export function summarizeSuggestions(suggestions: unknown): string | null {
  if (!Array.isArray(suggestions) || suggestions.length === 0) return null;
  const parts: string[] = [];
  for (const entry of suggestions) {
    const r = asRecord(entry);
    if (!r) continue;
    const type = r["type"];
    if (type === "setMode" && typeof r["mode"] === "string") {
      parts.push(`mode → ${r["mode"]}`);
      continue;
    }
    if (type === "addDirectories" && Array.isArray(r["directories"])) {
      const dirs = r["directories"].filter((d): d is string => typeof d === "string");
      if (dirs.length) parts.push(`add dirs ${dirs.join(", ")}`);
      continue;
    }
    if ((type === "addRules" || type === "replaceRules") && Array.isArray(r["rules"])) {
      const rules: string[] = [];
      for (const rule of r["rules"]) {
        const rr = asRecord(rule);
        if (!rr || typeof rr["toolName"] !== "string") continue;
        rules.push(
          typeof rr["ruleContent"] === "string" && rr["ruleContent"]
            ? `${rr["toolName"]}(${rr["ruleContent"]})`
            : rr["toolName"],
        );
      }
      if (rules.length) {
        const dest = typeof r["destination"] === "string" ? ` · ${r["destination"]}` : "";
        parts.push(`allow ${rules.join(", ")}${dest}`);
      }
      continue;
    }
  }
  return parts.length > 0 ? parts.join("; ") : null;
}

/** True when the suggestions payload can back an "always allow" button. */
export function hasSuggestions(suggestions: unknown): boolean {
  return Array.isArray(suggestions) && suggestions.length > 0;
}

// ---------------------------------------------------------------------------
// Slash-command autocomplete

export interface SlashCommand {
  name: string;
  description: string | null;
  argumentHint: string | null;
}

export function slashCommandsOf(runtime: RuntimeInfo | null): SlashCommand[] {
  const raw = runtime?.handshake?.commands;
  if (!Array.isArray(raw)) return [];
  const out: SlashCommand[] = [];
  for (const c of raw) {
    const r = asRecord(c);
    if (!r || typeof r["name"] !== "string" || !r["name"]) continue;
    out.push({
      name: r["name"],
      description: typeof r["description"] === "string" ? r["description"] : null,
      argumentHint: typeof r["argumentHint"] === "string" ? r["argumentHint"] : null,
    });
  }
  return out.sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * The partial command being typed, or null when the draft is not in the
 * "typing a slash command name" state (must be `/name-ish` with no spaces).
 */
export function slashPartialOf(draft: string): string | null {
  const m = /^\/([A-Za-z0-9:_-]*)$/.exec(draft);
  return m ? m[1]! : null;
}

export function filterSlashCommands(
  commands: SlashCommand[],
  partial: string,
  limit = 8,
): SlashCommand[] {
  const p = partial.toLowerCase();
  return commands.filter((c) => c.name.toLowerCase().startsWith(p)).slice(0, limit);
}

// ---------------------------------------------------------------------------
// Model list normalization (handshake.models items: strings or objects)

export interface ModelOption {
  id: string;
  label: string;
}

export function normalizeModels(models: unknown): ModelOption[] {
  if (!Array.isArray(models)) return [];
  const out: ModelOption[] = [];
  for (const m of models) {
    if (typeof m === "string" && m) {
      out.push({ id: m, label: m });
      continue;
    }
    const r = asRecord(m);
    if (!r) continue;
    const id = [r["id"], r["name"], r["model"]].find(
      (v): v is string => typeof v === "string" && v !== "",
    );
    if (!id) continue;
    const label = [r["displayName"], r["display_name"], r["name"]].find(
      (v): v is string => typeof v === "string" && v !== "",
    );
    out.push({ id, label: label ?? id });
  }
  // Dedupe by id, keeping first occurrence.
  const seen = new Set<string>();
  return out.filter((o) => (seen.has(o.id) ? false : (seen.add(o.id), true)));
}

// ---------------------------------------------------------------------------
// Context-usage summarization (GET /api/agents/{name}/context)

export interface ContextCategory {
  name: string;
  tokens: number;
}

export interface ContextSummary {
  /** 0–100, or null when not derivable. */
  percent: number | null;
  usedTokens: number | null;
  maxTokens: number | null;
  autoCompactThreshold: number | null;
  /** Sorted descending by tokens. */
  categories: ContextCategory[];
}

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** Defensive summary of the context-usage payload; null when nothing usable. */
export function summarizeContext(payload: unknown): ContextSummary | null {
  const r = asRecord(payload);
  if (!r) return null;

  const categories: ContextCategory[] = [];
  const rawCats = r["categories"];
  if (Array.isArray(rawCats)) {
    for (const c of rawCats) {
      const cr = asRecord(c);
      if (!cr) continue;
      const name = [cr["name"], cr["label"], cr["id"], cr["category"]].find(
        (v): v is string => typeof v === "string" && v !== "",
      );
      const tokens =
        num(cr["tokens"]) ?? num(cr["tokenCount"]) ?? num(cr["used"]) ?? num(cr["count"]);
      if (name && tokens !== null) categories.push({ name, tokens });
    }
  } else {
    const cr = asRecord(rawCats);
    if (cr) {
      for (const [name, v] of Object.entries(cr)) {
        const tokens = num(v) ?? num(asRecord(v)?.["tokens"]);
        if (tokens !== null) categories.push({ name, tokens });
      }
    }
  }
  categories.sort((a, b) => b.tokens - a.tokens);

  const maxTokens = num(r["maxTokens"]) ?? num(r["max_tokens"]);
  const catSum = categories.reduce((s, c) => s + c.tokens, 0);
  const usedTokens =
    num(r["usedTokens"]) ??
    num(r["used_tokens"]) ??
    num(r["totalTokens"]) ??
    num(r["total_tokens"]) ??
    (categories.length > 0 ? catSum : null);

  let percent: number | null = null;
  const explicit = num(r["percentage"]) ?? num(r["percent"]) ?? num(r["percentUsed"]);
  if (explicit !== null) {
    percent = explicit <= 1 ? explicit * 100 : explicit;
  } else if (usedTokens !== null && maxTokens !== null && maxTokens > 0) {
    percent = (usedTokens / maxTokens) * 100;
  }
  if (percent !== null) percent = Math.max(0, Math.min(100, percent));

  const autoCompactThreshold =
    num(r["autoCompactThreshold"]) ?? num(r["auto_compact_threshold"]);

  if (percent === null && categories.length === 0) return null;
  return { percent, usedTokens, maxTokens, autoCompactThreshold, categories };
}

export function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

// ---------------------------------------------------------------------------
// Render modes

export type RenderMode = "chat" | "console" | "source";

const RENDER_MODE_KEY = "aspen.session.renderMode";

export function loadRenderMode(): RenderMode {
  try {
    const v = localStorage.getItem(RENDER_MODE_KEY);
    if (v === "console" || v === "source" || v === "chat") return v;
  } catch {
    /* private mode */
  }
  return "chat";
}

export function storeRenderMode(mode: RenderMode): void {
  try {
    localStorage.setItem(RENDER_MODE_KEY, mode);
  } catch {
    /* private mode */
  }
}

// ---------------------------------------------------------------------------
// Status-event notes (raw `status` frames) + tool-use names

/**
 * A transient status-line note from a raw `status` WS frame:
 * compacting / api_retry — null when the frame carries neither.
 */
export function statusNoteOf(raw: unknown): string | null {
  const r = asRecord(raw);
  if (!r) return null;
  if (r["status"] === "compacting" || r["subtype"] === "compacting") {
    return "compacting context…";
  }
  if (r["subtype"] === "api_retry" || r["status"] === "api_retry") {
    const attempt = num(r["attempt"]) ?? num(r["retry"]) ?? num(r["retry_count"]);
    const max =
      num(r["max_attempts"]) ?? num(r["maxAttempts"]) ?? num(r["max_retries"]);
    if (attempt !== null && max !== null) return `API retry ${attempt}/${max}…`;
    if (attempt !== null) return `API retry ${attempt}…`;
    return "API retry…";
  }
  return null;
}

/** Permission-mode echo in a `status` frame (§7.1), when present. */
export function statusModeOf(raw: unknown): string | null {
  const r = asRecord(raw);
  if (!r) return null;
  const m = r["permissionMode"] ?? r["permission_mode"] ?? r["mode"];
  return typeof m === "string" && m ? m : null;
}

/** Best-effort tool name from a tool_use event (top-level or raw envelope). */
export function toolUseNameOf(ev: {
  tool_name?: string;
  name?: string;
  raw?: unknown;
}): string | null {
  if (typeof ev.tool_name === "string" && ev.tool_name) return ev.tool_name;
  if (typeof ev.name === "string" && ev.name) return ev.name;
  const r = asRecord(ev.raw);
  if (r) {
    const n = r["name"] ?? r["tool_name"];
    if (typeof n === "string" && n) return n;
    const msg = asRecord(r["message"]);
    const content = msg?.["content"] ?? r["content"];
    if (Array.isArray(content)) {
      for (const block of content) {
        const b = asRecord(block);
        if (b && b["type"] === "tool_use" && typeof b["name"] === "string" && b["name"]) {
          return b["name"];
        }
      }
    }
  }
  return null;
}
