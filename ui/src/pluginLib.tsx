/* The plugin library and the marketplaces it is picked from
 * (PROPOSALS-2026-10-P.md): who is in the library and why, a ranked
 * search over the whole store, what a plugin brings, and the "find a
 * plugin" field every plugin surface shares. */
import { useMemo, useState } from "react";
import type { CatalogPlugin, LibraryMember, PluginProvides, PluginRegistry } from "./api";
import "./pluginLib.css";

export const pluginKey = (marketplace: string, plugin: string) => `${plugin}@${marketplace}`;

/** The library as a lookup: key → why it is there. A node before v0.49
 *  sends no members; then everything a rule names counts, plus every
 *  plugin of a marketplace of at most 24 (the same line the node draws). */
export function libraryIndex(reg: PluginRegistry | null): Map<string, LibraryMember["why"]> {
  const m = new Map<string, LibraryMember["why"]>();
  if (!reg) return m;
  if (reg.library_members) {
    for (const l of reg.library_members) m.set(pluginKey(l.marketplace, l.plugin), l.why);
    return m;
  }
  const counts = new Map<string, number>();
  for (const c of reg.catalog.plugins) counts.set(c.marketplace, (counts.get(c.marketplace) ?? 0) + 1);
  for (const c of reg.catalog.plugins) if ((counts.get(c.marketplace) ?? 0) <= 24) m.set(pluginKey(c.marketplace, c.name), "marketplace");
  for (const r of reg.rules) if (!r.deleted) m.set(pluginKey(r.marketplace, r.plugin), "rule");
  return m;
}

export const titleOf = (p: CatalogPlugin) => p.display_name || p.name;

/** Rank the store for a query: name starts with it, then name contains,
 *  then display name, publisher or category, then description. Every
 *  word must match somewhere. */
export function searchPlugins(catalog: CatalogPlugin[], q: string, limit = Infinity): CatalogPlugin[] {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const scored: { p: CatalogPlugin; s: number }[] = [];
  for (const p of catalog) {
    const name = p.name.toLowerCase();
    const title = (p.display_name ?? "").toLowerCase();
    const meta = `${p.author ?? ""} ${p.category ?? ""} ${p.marketplace}`.toLowerCase();
    const desc = (p.description ?? "").toLowerCase();
    let s = 0;
    let all = true;
    for (const w of words) {
      if (name.startsWith(w)) s += 40;
      else if (name.includes(w)) s += 25;
      else if (title.includes(w)) s += 20;
      else if (meta.includes(w)) s += 12;
      else if (desc.includes(w)) s += 5;
      else {
        all = false;
        break;
      }
    }
    if (all) scored.push({ p, s });
  }
  scored.sort((a, b) => b.s - a.s || a.p.name.localeCompare(b.p.name));
  return scored.slice(0, limit).map((x) => x.p);
}

/** "3 skills · 2 commands · 1 MCP server · hooks" */
export function providesParts(p: PluginProvides | null | undefined): string[] {
  if (!p) return [];
  const n = (k: number, one: string, many: string) => (k ? [`${k} ${k === 1 ? one : many}`] : []);
  return [
    ...n(p.skills, "skill", "skills"),
    ...n(p.commands, "command", "commands"),
    ...n(p.agents, "subagent", "subagents"),
    ...n(p.mcp, "MCP server", "MCP servers"),
    ...n(p.lsp, "LSP server", "LSP servers"),
    ...(p.hooks ? ["hooks"] : []),
  ];
}

/** Sum what a set of plugins brings, for a session's header. */
export function sumProvides(list: (PluginProvides | null | undefined)[]): PluginProvides {
  const t: PluginProvides = { skills: 0, commands: 0, agents: 0, hooks: false, mcp: 0, lsp: 0 };
  for (const p of list) {
    if (!p) continue;
    t.skills += p.skills;
    t.commands += p.commands;
    t.agents += p.agents;
    t.mcp += p.mcp;
    t.lsp += p.lsp;
    t.hooks ||= p.hooks;
  }
  return t;
}

/** Chips for what a plugin brings; hooks and MCP servers are marked, since
 *  hooks run on every tool call and MCP servers are processes. */
export function ProvidesChips({ p, unknown }: { p: PluginProvides | null | undefined; unknown?: string }) {
  const parts = providesParts(p);
  if (!p) return unknown ? <span className="mono-meta dim">{unknown}</span> : null;
  if (!parts.length) return <span className="mono-meta dim">nothing found to load</span>;
  return (
    <span className="pl-provides">
      {parts.map((x) => (
        <span key={x} className={`chip mono${/MCP|hooks/.test(x) ? " pl-weighty" : ""}`}>{x}</span>
      ))}
    </span>
  );
}

/** Where a plugin's code lives: the marketplace's own repo, or an outside
 *  one (host and path). */
export function whereItLives(p: CatalogPlugin): string {
  const s = p.source;
  if (typeof s === "string") return "in the marketplace's repo";
  if (s && typeof s === "object") {
    const o = s as Record<string, unknown>;
    const repo = typeof o.repo === "string" ? `github.com/${o.repo}` : null;
    const url = typeof o.url === "string" ? o.url.replace(/^https?:\/\//, "").replace(/\.git$/, "") : null;
    const path = typeof o.path === "string" && o.path ? ` · ${o.path.replace(/^\.\//, "")}` : "";
    if (repo || url) return `${repo ?? url}${path}`;
  }
  return "source not stated";
}

/** "find a plugin…": a search over the whole store with an action per
 *  result. Used where the library is listed (session menu, template
 *  editor), so the store is reachable without ever being listed whole. */
export function PluginFinder({
  catalog,
  library,
  action,
  limit = 8,
  placeholder = "find a plugin in the marketplaces…",
}: {
  catalog: CatalogPlugin[];
  library: Map<string, string>;
  action: (p: CatalogPlugin) => { label: string; run: () => void | Promise<void>; disabled?: boolean } | null;
  limit?: number;
  placeholder?: string;
}) {
  const [q, setQ] = useState("");
  const hits = useMemo(() => searchPlugins(catalog, q, limit), [catalog, q, limit]);
  return (
    <div className="pl-finder">
      <input type="search" className="pl-finder-input" placeholder={placeholder} value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
      {q.trim() && hits.length === 0 && <div className="mono-meta dim pl-finder-empty">nothing in the marketplaces matches</div>}
      {hits.map((p) => {
        const a = action(p);
        const k = pluginKey(p.marketplace, p.name);
        return (
          <div className="pl-finder-row" key={k}>
            <span className="pl-finder-body">
              <span className="mono">{titleOf(p)}<span className="mono-meta"> @{p.marketplace}{library.has(k) ? " · in library" : ""}</span></span>
              {p.description && <span className="mono-meta pl-finder-desc" title={p.description}>{p.description}</span>}
            </span>
            {a && (
              <button className="btn sm" disabled={a.disabled} onClick={() => void a.run()}>
                {a.label}
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
