// Plugins (PROPOSALS-2026-09 §7, reshaped by PROPOSALS-2026-10-P.md): the
// library you picked, the marketplaces you browse to pick from, every
// node's sync state, and session templates. Activation by scope (mesh,
// node, repo, session) is unchanged: from a plugin, where it is active.

import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useNavigate } from "react-router-dom";
import { api, type MarketSource, type PluginRegistry, type PluginRule, type CatalogPlugin, type MeshInfo, type Template, type TemplateSpec, type Board, type NodePluginStatus } from "../api";
import { useAppData } from "../App";
import { ErrorBar, relTime } from "../components";
import { libraryIndex, pluginKey, PluginFinder, ProvidesChips, searchPlugins, titleOf, whereItLives } from "../pluginLib";
import "./plugins.css";

function ruleId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

type Tab = "library" | "browse" | "marketplaces" | "templates";
const TABS: { id: Tab; label: string }[] = [
  { id: "library", label: "Library" },
  { id: "browse", label: "Browse" },
  { id: "marketplaces", label: "Marketplaces" },
  { id: "templates", label: "Templates" },
];
const WHY: Record<string, string> = {
  picked: "added",
  rule: "a rule names it",
  marketplace: "whole marketplace",
};

export default function Plugins() {
  const { agents } = useAppData();
  const [params, setParams] = useSearchParams();
  const [reg, setReg] = useState<PluginRegistry | null>(null);
  const [mesh, setMesh] = useState<MeshInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(params.get("plugin")); // "plugin@market"
  const tab: Tab = (TABS.find((t) => t.id === params.get("tab"))?.id ?? (params.get("plugin") ? "library" : "library")) as Tab;
  const setTab = (t: Tab) => {
    const next = new URLSearchParams(params);
    next.set("tab", t);
    next.delete("plugin");
    setParams(next, { replace: true });
  };

  async function load() {
    try {
      const [r, m] = await Promise.all([api.plugins(), api.mesh().catch(() => null)]);
      setReg(r);
      setMesh(m);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "failed to load");
    }
  }
  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 15000);
    return () => window.clearInterval(t);
  }, []);

  async function setRule(r: Omit<PluginRule, "updated_at" | "deleted">) {
    try {
      await api.putPluginRule({ ...r, updated_at: 0, deleted: false });
      await load();
    } catch (e) {
      setErr(`rule: ${e instanceof Error ? e.message : "failed"}`);
    }
  }
  async function dropRule(id: string) {
    try {
      await api.deletePluginRule(id);
      await load();
    } catch (e) {
      setErr(`rule: ${e instanceof Error ? e.message : "failed"}`);
    }
  }
  async function shelve(p: CatalogPlugin, add: boolean) {
    try {
      if (add) await api.addToLibrary(p.marketplace, p.name);
      else await api.removeFromLibrary(p.marketplace, p.name);
      setNote(add ? `${p.name} is in your library` : `${p.name} left your library`);
      await load();
    } catch (e) {
      setErr(`library: ${e instanceof Error ? e.message : "failed"}`);
    }
  }

  const nodes = useMemo(() => {
    const set = new Set<string>();
    if (mesh?.node) set.add(mesh.node);
    for (const p of mesh?.peers ?? []) set.add(p.node);
    for (const a of agents) set.add(a.node);
    return Array.from(set).sort();
  }, [mesh, agents]);
  const repos = useMemo(() => {
    const set = new Set<string>();
    for (const a of agents) set.add(a.channel);
    return Array.from(set).sort();
  }, [agents]);
  const sessions = useMemo(
    () => agents.filter((a) => !a.moved_to).map((a) => ({ name: a.name, bare: a.remote ? a.name.replace(/@[^@]+$/, "") : a.name, node: a.node })).sort((x, y) => x.name.localeCompare(y.name)),
    [agents],
  );

  const catalog = reg?.catalog.plugins ?? [];
  const lib = useMemo(() => libraryIndex(reg), [reg]);
  const rulesFor = (p: CatalogPlugin) => (reg?.rules ?? []).filter((r) => r.marketplace === p.marketplace && r.plugin === p.name);
  const opened = catalog.find((p) => pluginKey(p.marketplace, p.name) === open);
  const counts: Record<Tab, string> = {
    library: `${lib.size}`,
    browse: `${catalog.length}`,
    marketplaces: `${reg?.marketplaces.length ?? 0}`,
    templates: "",
  };

  const matrix = opened && (
    <Matrix
      plugin={opened}
      rules={rulesFor(opened)}
      nodes={nodes}
      repos={repos}
      sessions={sessions}
      onSet={setRule}
      onDrop={dropRule}
      onClose={() => setOpen(null)}
    />
  );

  return (
    <div className="stage-body plugins-page">
      <h1>Plugins</h1>
      <p className="dim">
        Your <b>library</b> is the plugins you keep at hand; the <b>marketplaces</b> are where you find them. Turn a
        plugin on for the mesh, a node, a repo or one session; each node caches what its sessions need and starts them
        with <code>--plugin-dir</code>. A running session keeps the version it started with.
      </p>
      <div className="pl-tabs" role="tablist">
        {TABS.map((t) => (
          <button key={t.id} role="tab" aria-selected={tab === t.id} className={`pl-tab${tab === t.id ? " active" : ""}`} onClick={() => setTab(t.id)}>
            {t.label}
            {counts[t.id] && <span className="mono-meta"> {counts[t.id]}</span>}
          </button>
        ))}
      </div>
      <ErrorBar error={err} />
      {note && <div className="mono-meta" style={{ margin: "4px 0 8px" }}>{note}</div>}

      {tab === "library" && (
        <LibraryTab
          reg={reg}
          lib={lib}
          catalog={catalog}
          rulesFor={rulesFor}
          open={open}
          setOpen={setOpen}
          matrix={matrix}
          onRemove={(p) => void shelve(p, false)}
          onBrowse={() => setTab("browse")}
        />
      )}
      {tab === "browse" && (
        <BrowseTab
          catalog={catalog}
          lib={lib}
          marketplaces={(reg?.marketplaces ?? []).map((m) => m.name)}
          open={open}
          setOpen={setOpen}
          matrix={matrix}
          onShelve={shelve}
          onInspected={load}
          setErr={setErr}
        />
      )}
      {tab === "marketplaces" && <MarketplacesTab reg={reg} lib={lib} reload={load} setErr={setErr} setNote={setNote} />}
      {tab === "templates" && <TemplatesSection catalog={catalog} lib={lib} />}
    </div>
  );
}

/** The shelf: what is at hand, where it is on, and why it is here. */
function LibraryTab({
  reg,
  lib,
  catalog,
  rulesFor,
  open,
  setOpen,
  matrix,
  onRemove,
  onBrowse,
}: {
  reg: PluginRegistry | null;
  lib: Map<string, string>;
  catalog: CatalogPlugin[];
  rulesFor: (p: CatalogPlugin) => PluginRule[];
  open: string | null;
  setOpen: (k: string | null) => void;
  matrix: React.ReactNode;
  onRemove: (p: CatalogPlugin) => void;
  onBrowse: () => void;
}) {
  const [q, setQ] = useState("");
  const rows = catalog
    .filter((p) => lib.has(pluginKey(p.marketplace, p.name)))
    .filter((p) => !q || `${p.name} ${p.display_name ?? ""} ${p.description ?? ""} ${p.marketplace} ${p.author ?? ""}`.toLowerCase().includes(q.toLowerCase()))
    .sort((a, b) => a.name.localeCompare(b.name));
  if (!reg) return <div className="dim">loading…</div>;
  return (
    <>
      {lib.size > 12 && <input className="plg-search" placeholder="filter your library" value={q} onChange={(e) => setQ(e.target.value)} />}
      <div className="plg-table-wrap">
        <table className="plg-table">
          <thead>
            <tr>
              <th>plugin</th>
              <th>marketplace</th>
              <th>version</th>
              <th>active for</th>
              <th>in library</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => {
              const rs = rulesFor(p);
              const on = rs.filter((r) => r.enabled && !r.deleted);
              const key = pluginKey(p.marketplace, p.name);
              const why = lib.get(key) ?? "";
              return (
                <tr key={key} className={open === key ? "open" : ""}>
                  <td>
                    <span className="mono">{p.name}</span>
                    {p.description && <div className="mono-meta plg-desc" title={p.description}>{p.description}</div>}
                    <ProvidesChips p={p.provides} />
                  </td>
                  <td className="mono-meta">{p.marketplace}</td>
                  <td className="mono-meta" title={p.cached.length ? `cached here: ${p.cached.join(", ")}` : "not cached on this node"}>
                    {p.current}
                    {p.cached.length ? "" : " · not cached"}
                  </td>
                  <td className="mono-meta">
                    {on.length === 0 ? "nowhere" : on.map((r) => (r.scope_kind === "mesh" ? "mesh" : `${r.scope_kind}:${r.scope}`)).join(" · ")}
                    {rs.some((r) => !r.enabled && !r.deleted) && <span title="some scopes turn it off"> · exceptions</span>}
                  </td>
                  <td className="mono-meta">{WHY[why] ?? why}</td>
                  <td className="pl-actions">
                    <button className="btn sm" onClick={() => setOpen(open === key ? null : key)}>{open === key ? "close" : "activate…"}</button>
                    {why === "picked" && (
                      <button className="btn ghost sm" onClick={() => onRemove(p)} title="take it off the shelf; it stays in its marketplace">remove</button>
                    )}
                  </td>
                </tr>
              );
            })}
            {rows.length === 0 && (
              <tr>
                <td colSpan={6} className="dim">
                  {catalog.length === 0 ? (
                    "no plugins yet — add a marketplace"
                  ) : lib.size === 0 ? (
                    <>
                      your library is empty —{" "}
                      <button className="linklike" onClick={onBrowse}>browse the marketplaces</button> and add what you want at hand
                    </>
                  ) : (
                    "nothing in your library matches"
                  )}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {lib.size > 0 && (
        <p className="mono-meta dim" style={{ marginTop: 8 }}>
          A plugin a rule names stays here while the rule does; a marketplace of up to 24 plugins is here whole unless set
          otherwise on the Marketplaces tab.
        </p>
      )}
      {matrix}
    </>
  );
}

const PAGE = 40;

/** The store: search, category and publisher chips, cards. */
function BrowseTab({
  catalog,
  lib,
  marketplaces,
  open,
  setOpen,
  matrix,
  onShelve,
  onInspected,
  setErr,
}: {
  catalog: CatalogPlugin[];
  lib: Map<string, string>;
  marketplaces: string[];
  open: string | null;
  setOpen: (k: string | null) => void;
  matrix: React.ReactNode;
  onShelve: (p: CatalogPlugin, add: boolean) => Promise<void>;
  onInspected: () => Promise<void>;
  setErr: (e: string | null) => void;
}) {
  const [q, setQ] = useState("");
  const [cat, setCat] = useState<string | null>(null);
  const [market, setMarket] = useState<string>("");
  const [author, setAuthor] = useState<string>("");
  const [shelfFirst, setShelfFirst] = useState(false);
  const [shown, setShown] = useState(PAGE);
  const [inspecting, setInspecting] = useState<string | null>(null);
  useEffect(() => setShown(PAGE), [q, cat, market, author, shelfFirst]);

  const scoped = catalog.filter((p) => (!market || p.marketplace === market) && (!author || p.author === author));
  const categories = useMemo(() => {
    const m = new Map<string, number>();
    for (const p of scoped) {
      const c = p.category || "uncategorized";
      m.set(c, (m.get(c) ?? 0) + 1);
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  }, [scoped]);
  const publishers = useMemo(() => {
    const m = new Map<string, number>();
    for (const p of catalog) if (p.author) m.set(p.author, (m.get(p.author) ?? 0) + 1);
    return [...m.entries()].filter(([, n]) => n >= 3).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  }, [catalog]);
  const inCat = scoped.filter((p) => !cat || (p.category || "uncategorized") === cat);
  let list = q.trim() ? searchPlugins(inCat, q) : inCat.slice().sort((a, b) => titleOf(a).localeCompare(titleOf(b)));
  if (shelfFirst) {
    const on = (p: CatalogPlugin) => (lib.has(pluginKey(p.marketplace, p.name)) ? 0 : 1);
    list = list.slice().sort((a, b) => on(a) - on(b));
  }

  async function inspect(p: CatalogPlugin) {
    const k = pluginKey(p.marketplace, p.name);
    setInspecting(k);
    setErr(null);
    try {
      await api.pluginsInspect(p.marketplace, p.name);
      await onInspected();
    } catch (e) {
      setErr(`look inside ${p.name}: ${e instanceof Error ? e.message : "failed"}`);
    } finally {
      setInspecting(null);
    }
  }

  return (
    <>
      <div className="pl-browse-bar">
        <input className="plg-search" type="search" placeholder="search name, description, publisher, category" value={q} onChange={(e) => setQ(e.target.value)} />
        {marketplaces.length > 1 && (
          <select value={market} onChange={(e) => setMarket(e.target.value)} title="marketplace">
            <option value="">every marketplace</option>
            {marketplaces.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        )}
        {publishers.length > 0 && (
          <select value={author} onChange={(e) => setAuthor(e.target.value)} title="publisher, as the marketplace states it">
            <option value="">any publisher</option>
            {publishers.map(([a, n]) => (
              <option key={a} value={a}>{a} ({n})</option>
            ))}
          </select>
        )}
        <label className="mono-meta pl-check">
          <input type="checkbox" checked={shelfFirst} onChange={(e) => setShelfFirst(e.target.checked)} /> library first
        </label>
      </div>
      <div className="pl-chips" role="group" aria-label="category">
        <button className={`chip mono pl-chip${cat === null ? " on" : ""}`} onClick={() => setCat(null)}>all {scoped.length}</button>
        {categories.map(([c, n]) => (
          <button key={c} className={`chip mono pl-chip${cat === c ? " on" : ""}`} onClick={() => setCat(cat === c ? null : c)}>
            {c} {n}
          </button>
        ))}
      </div>
      {matrix}
      <div className="mono-meta dim" style={{ margin: "6px 0" }}>
        {list.length} {list.length === 1 ? "plugin" : "plugins"}
        {q.trim() ? ` matching “${q.trim()}”` : ""}
      </div>
      <div className="pl-cards">
        {list.slice(0, shown).map((p) => {
          const k = pluginKey(p.marketplace, p.name);
          const why = lib.get(k);
          return (
            <div className={`pl-card${why ? " shelved" : ""}${open === k ? " open" : ""}`} key={k}>
              <div className="pl-card-head">
                <span className="pl-card-title">{titleOf(p)}</span>
                {why && <span className="chip mono pl-inlib" title={`in your library: ${WHY[why] ?? why}`}>in library</span>}
              </div>
              <div className="mono-meta">
                {p.name}@{p.marketplace}
                {p.author ? ` · ${p.author}` : ""}
                {p.category ? ` · ${p.category}` : ""}
              </div>
              {p.description && <div className="pl-card-desc" title={p.description}>{p.description}</div>}
              <div className="mono-meta pl-card-where" title="where its code lives">
                {whereItLives(p)}
                {p.homepage && (
                  <>
                    {" · "}
                    <a href={p.homepage} target="_blank" rel="noreferrer noopener">homepage</a>
                  </>
                )}
              </div>
              <div className="pl-card-brings">
                <ProvidesChips p={p.provides} unknown="contents known once fetched" />
              </div>
              <div className="pl-card-actions">
                {!why && <button className="btn sm primary" onClick={() => void onShelve(p, true)}>add to library</button>}
                {why === "picked" && <button className="btn ghost sm" onClick={() => void onShelve(p, false)}>remove from library</button>}
                <button className="btn sm" onClick={() => setOpen(open === k ? null : k)} title="where it is on: mesh, nodes, repos, sessions (adds it to your library)">
                  {open === k ? "close" : "activate…"}
                </button>
                {!p.provides && (
                  <button className="btn ghost sm" disabled={inspecting === k} onClick={() => void inspect(p)} title="fetch it into this node's cache without turning it on, and count what it brings">
                    {inspecting === k ? "fetching…" : "look inside"}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {list.length > shown && (
        <div style={{ margin: "10px 0" }}>
          <button className="btn sm" onClick={() => setShown((n) => n + PAGE)}>show {Math.min(PAGE, list.length - shown)} more</button>
          <span className="mono-meta dim"> of {list.length - shown} left</span>
        </div>
      )}
      {catalog.length === 0 && <div className="dim">no plugins yet — add a marketplace on the Marketplaces tab</div>}
    </>
  );
}

/** Marketplaces: each one's library setting and counts, adding one (which
 *  asks how much of it to shelve), and every node's sync state (L-7). */
function MarketplacesTab({
  reg,
  lib,
  reload,
  setErr,
  setNote,
}: {
  reg: PluginRegistry | null;
  lib: Map<string, string>;
  reload: () => Promise<void>;
  setErr: (e: string | null) => void;
  setNote: (n: string | null) => void;
}) {
  const [mName, setMName] = useState("");
  const [mKind, setMKind] = useState<"github" | "git" | "directory">("github");
  const [mValue, setMValue] = useState("");
  const [removeAsk, setRemoveAsk] = useState<string | null>(null);
  const [ask, setAsk] = useState<string | null>(null); // a just-added marketplace awaiting "store or library?"
  const [status, setStatus] = useState<NodePluginStatus[] | null>(null);
  const [syncing, setSyncing] = useState<string | null>(null); // "mesh" | node
  const [results, setResults] = useState<{ node: string; ok: boolean; error?: string; errors?: Record<string, string> | null }[] | null>(null);
  const catalog = reg?.catalog.plugins ?? [];

  async function loadStatus() {
    try {
      const r = await api.pluginsStatus();
      setStatus(r.nodes);
    } catch {
      setStatus([]);
    }
  }
  useEffect(() => {
    void loadStatus();
    const t = window.setInterval(() => void loadStatus(), 30000);
    return () => window.clearInterval(t);
  }, []);

  async function syncMesh(node?: string) {
    setSyncing(node ?? "mesh");
    setResults(null);
    setErr(null);
    try {
      const r = await api.pluginsSyncMesh(node);
      setResults(r.nodes);
      await Promise.all([reload(), loadStatus()]);
    } catch (e) {
      setErr(`sync: ${e instanceof Error ? e.message : "failed"}`);
    } finally {
      setSyncing(null);
    }
  }
  async function addMarketplace() {
    const name = mName.trim();
    const v = mValue.trim();
    if (!name || !v) return;
    const source: MarketSource =
      mKind === "github" ? { source: "github", repo: v } : mKind === "git" ? { source: "git", url: v } : { source: "directory", path: v };
    try {
      await api.putMarketplace(name, source);
      setMName("");
      setMValue("");
      setAsk(name);
      setNote(`added ${name}; syncing…`);
      window.setTimeout(() => void reload(), 3000);
      window.setTimeout(() => void reload(), 10000);
      await reload();
    } catch (e) {
      setErr(`add: ${e instanceof Error ? e.message : "failed"}`);
    }
  }
  async function setLibrary(name: string, library: "all" | "picked" | null) {
    try {
      await api.setMarketplaceLibrary(name, library);
      await reload();
    } catch (e) {
      setErr(`library: ${e instanceof Error ? e.message : "failed"}`);
    }
  }

  const asked = ask ? reg?.marketplaces.find((m) => m.name === ask) : null;
  const askCount = ask ? catalog.filter((p) => p.marketplace === ask).length : 0;
  const nodeStates = status ?? [];

  return (
    <>
      {asked && askCount > 0 && (
        <div className="strip pl-ask">
          <span>
            <b>{asked.name}</b> offers {askCount} {askCount === 1 ? "plugin" : "plugins"}. How much of it do you want at hand?
          </span>
          <span className="pl-ask-actions">
            <button className={`btn sm${askCount > 24 ? " primary" : ""}`} onClick={() => { void setLibrary(asked.name, "picked"); setAsk(null); }}>
              keep it as a store — browse and pick
            </button>
            <button className={`btn sm${askCount <= 24 ? " primary" : ""}`} onClick={() => { void setLibrary(asked.name, "all"); setAsk(null); }}>
              put all {askCount} in the library
            </button>
          </span>
        </div>
      )}
      <div className="mk-list">
        {(reg?.marketplaces ?? []).map((m) => {
          const at = reg?.catalog.synced_at?.[m.name];
          const e = reg?.catalog.errors?.[m.name];
          const count = catalog.filter((p) => p.marketplace === m.name).length;
          const inLib = catalog.filter((p) => p.marketplace === m.name && lib.has(pluginKey(p.marketplace, p.name))).length;
          const whole = (reg?.whole_library ?? []).includes(m.name);
          return (
            <div className="mk-row" key={m.name}>
              <span className="mono mk-name">{m.name}</span>
              <span className="mono-meta">{describeSource(m.source)}</span>
              <span className="mono-meta">
                {count} offered · {inLib} in library{at ? ` · synced here ${relTime(at)} ago` : " · not synced here yet"}
              </span>
              {e && <span className="error-text mono-meta" title={e}>sync error here: {e}</span>}
              <span style={{ flex: 1 }} />
              <label className="mono-meta pl-libsel" title="how much of this marketplace is in your library">
                <select value={m.library ?? ""} onChange={(ev) => void setLibrary(m.name, (ev.target.value || null) as "all" | "picked" | null)}>
                  <option value="">by size ({count === 0 ? "decided once it offers plugins" : whole ? "all — 24 or fewer" : "picked — over 24"})</option>
                  <option value="all">all in library</option>
                  <option value="picked">browse and pick</option>
                </select>
              </label>
              {removeAsk === m.name ? (
                <span className="inline-confirm">
                  <span className="mono-meta">remove {m.name} and its rules?</span>
                  <button className="btn sm" onClick={async () => { await api.deleteMarketplace(m.name).catch(() => {}); setRemoveAsk(null); await reload(); }}>yes</button>
                  <button className="btn sm" onClick={() => setRemoveAsk(null)}>no</button>
                </span>
              ) : (
                <button className="btn ghost sm" onClick={() => setRemoveAsk(m.name)}>remove</button>
              )}
            </div>
          );
        })}
        {(reg?.marketplaces ?? []).length === 0 && <div className="dim">no marketplaces yet — add one below</div>}
      </div>
      <div className="mk-add">
        <input placeholder="name (e.g. official)" value={mName} onChange={(e) => setMName(e.target.value)} />
        <select value={mKind} onChange={(e) => setMKind(e.target.value as typeof mKind)}>
          <option value="github">GitHub owner/repo</option>
          <option value="git">git URL</option>
          <option value="directory">directory on this node</option>
        </select>
        <input
          className="mono"
          style={{ minWidth: 0, flex: "1 1 260px" }}
          placeholder={mKind === "github" ? "anthropics/claude-plugins-official" : mKind === "git" ? "https://…/marketplace.git" : "/path/to/marketplace"}
          value={mValue}
          onChange={(e) => setMValue(e.target.value)}
        />
        <button className="btn primary sm" disabled={!mName.trim() || !mValue.trim()} onClick={() => void addMarketplace()}>add marketplace</button>
      </div>
      {mKind === "directory" && <div className="mono-meta dim" style={{ marginTop: 4 }}>A directory exists only on this node; other nodes need the marketplace as a git repository.</div>}

      <h2>On each node</h2>
      <p className="dim" style={{ marginTop: 0 }}>
        Every node keeps its own copy of each marketplace and caches the plugins its sessions use. <b>Sync the mesh</b>{" "}
        sends this node's marketplaces and rules to every node, then each one fetches and reports back.
      </p>
      <div className="mk-add" style={{ marginTop: 0 }}>
        <button className="btn sm primary" disabled={!!syncing} onClick={() => void syncMesh()}>{syncing === "mesh" ? "syncing every node…" : "sync the mesh"}</button>
        <button className="btn ghost sm" onClick={() => void loadStatus()}>refresh</button>
      </div>
      {results && (
        <div className="pl-results mono-meta">
          {results.map((r) => {
            const errs = Object.entries(r.errors ?? {});
            return (
              <div key={r.node} className={r.ok && !errs.length ? "" : "error-text"}>
                {r.node}: {r.ok ? (errs.length ? errs.map(([k, v]) => `${k}: ${v}`).join(" · ") : "synced") : `did not sync — ${r.error}`}
              </div>
            );
          })}
        </div>
      )}
      {status === null && <div className="dim">asking every node…</div>}
      <div className="pl-nodes">
        {nodeStates.map((n) => {
          const name = n.status?.node ?? n.node ?? "this node";
          const known = new Map((n.status?.marketplaces ?? []).map((m) => [m.name, m]));
          return (
            <div className="pl-node" key={name}>
              <div className="pl-node-head">
                <span className="mono mk-name">{name}</span>
                <span className="mono-meta">{n.ok ? (n.status?.version ? `v${n.status.version}` : n.legacy ? "older version" : "") : `unreachable — ${n.error ?? "no answer"}`}</span>
                <span style={{ flex: 1 }} />
                {n.ok && name && (
                  <button className="btn ghost sm" disabled={!!syncing} onClick={() => void syncMesh(name)}>{syncing === name ? "syncing…" : "sync this node"}</button>
                )}
              </div>
              {n.ok &&
                (reg?.marketplaces ?? []).map((m) => {
                  const s = known.get(m.name);
                  let state: { cls: string; text: string };
                  if (!s) state = { cls: "error-text", text: "does not know this marketplace — sync this node to send it" };
                  else if (s.error) state = { cls: "error-text", text: s.error };
                  else if (s.checkout === false) state = { cls: "error-text", text: "no copy on this node yet" };
                  else if (s.synced_at) state = { cls: "", text: `${s.plugins} plugins · synced ${relTime(s.synced_at)} ago` };
                  else state = { cls: "dim", text: "not synced yet" };
                  const perr = s?.plugin_errors ?? [];
                  return (
                    <div className="pl-node-row" key={m.name}>
                      <span className="mono">{m.name}</span>
                      <span className={`mono-meta ${state.cls}`} title={state.text}>{state.text}</span>
                      {perr.length > 0 && (
                        <span className="mono-meta error-text" title={perr.map((e) => `${e.plugin}: ${e.error}`).join("\n")}>
                          {perr.length} plugin{perr.length === 1 ? "" : "s"} failed to cache
                        </span>
                      )}
                    </div>
                  );
                })}
              {n.ok &&
                (n.status?.marketplaces ?? [])
                  .filter((s) => !(reg?.marketplaces ?? []).some((m) => m.name === s.name))
                  .map((s) => (
                    <div className="pl-node-row" key={`x-${s.name}`}>
                      <span className="mono">{s.name}</span>
                      <span className="mono-meta dim">known there, not here</span>
                    </div>
                  ))}
            </div>
          );
        })}
      </div>
    </>
  );
}

function describeSource(s: MarketSource): string {
  switch (s.source) {
    case "github":
      return `github · ${s.repo}`;
    case "git":
      return s.url;
    case "directory":
      return `directory · ${s.path}`;
  }
}

/** Where a plugin is active: one row per scope with enable / disable /
 *  unset. The most specific rule wins at spawn. */
function Matrix({
  plugin,
  rules,
  nodes,
  repos,
  sessions,
  onSet,
  onDrop,
  onClose,
}: {
  plugin: CatalogPlugin;
  rules: PluginRule[];
  nodes: string[];
  repos: string[];
  sessions: { name: string; bare: string; node: string }[];
  onSet: (r: Omit<PluginRule, "updated_at" | "deleted">) => Promise<void>;
  onDrop: (id: string) => Promise<void>;
  onClose: () => void;
}) {
  const [pin, setPin] = useState("");
  const find = (kind: string, scope: string) => rules.find((r) => r.scope_kind === kind && r.scope === scope);
  const row = (kind: string, scope: string, label: string) => {
    const r = find(kind, scope);
    const state = r ? (r.enabled ? "on" : "off") : "unset";
    return (
      <div className="mx-row" key={`${kind}:${scope}`}>
        <span className="mono-meta mx-kind">{kind}</span>
        <span className="mono mx-scope">{label}</span>
        <span className="seg">
          <button
            className={state === "on" ? "on" : ""}
            onClick={() => void onSet({ id: r?.id ?? ruleId(), marketplace: plugin.marketplace, plugin: plugin.name, scope_kind: kind, scope, enabled: true, pin: pin.trim() || r?.pin || null })}
            title="enabled here (and everything inside, unless a more specific rule says otherwise)"
          >
            on
          </button>
          <button
            className={state === "off" ? "on" : ""}
            onClick={() => void onSet({ id: r?.id ?? ruleId(), marketplace: plugin.marketplace, plugin: plugin.name, scope_kind: kind, scope, enabled: false, pin: null })}
            title="explicitly disabled here — overrides an enclosing rule"
          >
            off
          </button>
          <button className={state === "unset" ? "on" : ""} disabled={!r} onClick={() => r && void onDrop(r.id)} title="no rule at this scope">
            unset
          </button>
        </span>
        {r?.pin && <span className="mono-meta">pinned {r.pin}</span>}
      </div>
    );
  };
  return (
    <div className="matrix">
      <div className="trust-head">
        <span className="label">
          {plugin.name}@{plugin.marketplace} — where it is active
        </span>
        <span style={{ flex: 1 }} />
        <label className="mono-meta" style={{ display: "inline-flex", gap: 6, alignItems: "center" }}>
          pin new rules to
          <select value={pin} onChange={(e) => setPin(e.target.value)}>
            <option value="">newest cached</option>
            {plugin.cached.map((v) => (
              <option key={v} value={v}>{v}</option>
            ))}
          </select>
        </label>
        <button className="btn ghost sm" onClick={onClose}>close</button>
      </div>
      <p className="dim" style={{ margin: "4px 0 8px" }}>
        The most specific rule decides at spawn: a session-level <b>off</b> beats a mesh-level <b>on</b>. Repos are
        matched by git origin or by name across nodes.
      </p>
      {row("mesh", "", "every session on every node")}
      <div className="mx-group label">nodes</div>
      {nodes.map((n) => row("node", n, n))}
      <div className="mx-group label">repos</div>
      {repos.map((r) => row("repo", r, `#${r}`))}
      <div className="mx-group label">sessions</div>
      {sessions.map((s) => row("session", s.bare, `@${s.name}`))}
      {rules.filter((r) => !["mesh", "node", "repo", "session"].includes(r.scope_kind) || (r.scope_kind === "node" && !nodes.includes(r.scope)) || (r.scope_kind === "repo" && !repos.includes(r.scope)) || (r.scope_kind === "session" && !sessions.some((s) => s.bare === r.scope))).map((r) => (
        <div className="mx-row" key={r.id}>
          <span className="mono-meta mx-kind">{r.scope_kind}</span>
          <span className="mono mx-scope">{r.scope || "(mesh)"}</span>
          <span className="mono-meta">{r.enabled ? "on" : "off"} · not currently on the fleet</span>
          <button className="btn ghost sm" onClick={() => void onDrop(r.id)}>unset</button>
        </div>
      ))}
    </div>
  );
}


/** Session templates (PLUGINS.md §templates): named recipes, synced
 *  mesh-wide, spawned from Now's panel, the palette, or the CLI. */
function TemplatesSection({ catalog, lib }: { catalog: CatalogPlugin[]; lib: Map<string, string> }) {
  const nav = useNavigate();
  const [templates, setTemplates] = useState<Template[]>([]);
  const [boards, setBoards] = useState<Board[]>([]);
  const [editing, setEditing] = useState<Template | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [removeAsk, setRemoveAsk] = useState<string | null>(null);
  async function load() {
    try {
      const [t, b] = await Promise.all([api.templates(), api.boards().catch(() => [] as Board[])]);
      setTemplates(t);
      setBoards(b.filter((x) => !x.query));
    } catch (e) {
      setErr(e instanceof Error ? e.message : "failed to load templates");
    }
  }
  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 15000);
    return () => window.clearInterval(t);
  }, []);
  const blank = (): Template => ({ id: `t-${Date.now().toString(36)}`, name: "", spec: { permission: "ask", plugins: [] }, updated_at: 0 });
  return (
    <>
      <h2>Session templates</h2>
      <p className="dim">
        A named recipe — repo, model, permission mode, charter, extra args, plugins, a board to land on — started in one click from Now's new-session panel, the palette, or <code>aspen session new --template</code>. Synced across the mesh like boards.
      </p>
      <ErrorBar error={err} />
      <div className="mk-list">
        {templates.map((t) => (
          <div className="mk-row" key={t.id}>
            <span className="mono mk-name">{t.name}</span>
            <span className="mono-meta">
              {t.spec.repo ? `#${t.spec.repo}` : "any repo"}
              {t.spec.harness ? ` · ${t.spec.harness}` : ""}
              {t.spec.model ? ` · ${t.spec.model}` : ""}
              {t.spec.permission === "skip" ? " · skip permissions" : ""}
              {t.spec.plugins?.length ? ` · ${t.spec.plugins.map((p) => p.plugin).join(", ")}` : ""}
              {t.spec.board ? ` · board ${boards.find((b) => b.id === t.spec.board?.id)?.name ?? t.spec.board.id}` : ""}
            </span>
            <span style={{ flex: 1 }} />
            <button className="btn primary sm" onClick={() => nav(`/?new=${encodeURIComponent(t.id)}`)} title="open the new-session panel with this template filled in">start…</button>
            <button className="btn sm" onClick={() => setEditing(t)}>edit</button>
            {removeAsk === t.id ? (
              <span className="inline-confirm">
                <span className="mono-meta">remove {t.name}?</span>
                <button className="btn sm" onClick={async () => { await api.deleteTemplate(t.id).catch(() => {}); setRemoveAsk(null); await load(); }}>yes</button>
                <button className="btn sm" onClick={() => setRemoveAsk(null)}>no</button>
              </span>
            ) : (
              <button className="btn ghost sm" onClick={() => setRemoveAsk(t.id)}>remove</button>
            )}
          </div>
        ))}
        {templates.length === 0 && <div className="dim">no templates yet</div>}
      </div>
      {editing ? (
        <TemplateEditor
          t={editing}
          catalog={catalog}
          lib={lib}
          boards={boards}
          onClose={() => setEditing(null)}
          onSaved={async () => {
            setEditing(null);
            await load();
          }}
        />
      ) : (
        <div className="mk-add">
          <button className="btn sm" onClick={() => setEditing(blank())}>new template</button>
        </div>
      )}
    </>
  );
}

function TemplateEditor({ t, catalog, lib, boards, onClose, onSaved }: { t: Template; catalog: CatalogPlugin[]; lib: Map<string, string>; boards: Board[]; onClose: () => void; onSaved: () => void | Promise<void> }) {
  const [name, setName] = useState(t.name);
  const [spec, setSpec] = useState<TemplateSpec>({ ...t.spec, plugins: t.spec.plugins ?? [] });
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const set = (patch: Partial<TemplateSpec>) => setSpec((s) => ({ ...s, ...patch }));
  const has = (p: CatalogPlugin) => (spec.plugins ?? []).some((x) => x.marketplace === p.marketplace && x.plugin === p.name);
  const toggle = (p: CatalogPlugin) =>
    set({ plugins: has(p) ? (spec.plugins ?? []).filter((x) => !(x.marketplace === p.marketplace && x.plugin === p.name)) : [...(spec.plugins ?? []), { marketplace: p.marketplace, plugin: p.name }] });
  async function save() {
    if (!name.trim()) {
      setErr("a name is required");
      return;
    }
    setBusy(true);
    setErr(null);
    try {
      const clean: TemplateSpec = { ...spec };
      for (const k of Object.keys(clean) as (keyof TemplateSpec)[]) {
        const v = clean[k];
        if (v === "" || v === null || v === undefined) delete clean[k];
      }
      await api.putTemplate(t.id, name.trim(), clean);
      await onSaved();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "save failed");
    } finally {
      setBusy(false);
    }
  }
  return (
    <form
      className="strip tpl-editor"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span className="label">{t.name ? `Edit ${t.name}` : "New template"}</span>
        <span style={{ flex: 1 }} />
        <button type="button" className="btn ghost sm" onClick={onClose}>close</button>
      </div>
      <ErrorBar error={err} />
      <div className="grid cols">
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Template name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. reviewer" />
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Default agent name (optional)</span>
          <input value={spec.name ?? ""} onChange={(e) => set({ name: e.target.value })} placeholder="e.g. review" spellCheck={false} />
        </label>
      </div>
      <div className="grid cols">
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Repo (handle, basename, origin URL, or path; blank = ask)</span>
          <input className="mono" value={spec.repo ?? ""} onChange={(e) => set({ repo: e.target.value })} placeholder="hub" spellCheck={false} />
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Model (optional)</span>
          <input value={spec.model ?? ""} onChange={(e) => set({ model: e.target.value })} placeholder="default" spellCheck={false} />
        </label>
      </div>
      <label style={{ display: "grid", gap: 4 }}>
        <span className="label">Charter (optional)</span>
        <textarea rows={3} value={spec.charter ?? ""} onChange={(e) => set({ charter: e.target.value })} placeholder="what sessions from this template are here to do" />
      </label>
      <div className="grid cols">
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Runtime</span>
          <select value={spec.harness ?? ""} onChange={(e) => set({ harness: (e.target.value || undefined) as TemplateSpec["harness"] })}>
            <option value="">repo default</option>
            <option value="claude">claude</option>
            <option value="codex">codex</option>
          </select>
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Extra runtime args (optional)</span>
          <input className="mono" value={spec.extra_args ?? ""} onChange={(e) => set({ extra_args: e.target.value })} placeholder="--chrome" spellCheck={false} />
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Permissions</span>
          <select value={spec.permission ?? "ask"} onChange={(e) => set({ permission: e.target.value as "ask" | "skip" })}>
            <option value="ask">ask (route prompts to the console)</option>
            <option value="skip">skip (no prompts: bypassPermissions / full access)</option>
          </select>
        </label>
      </div>
      <div style={{ display: "grid", gap: 4 }}>
        <span className="label">Plugins (session-scope rules are written for each new session)</span>
        <div className="tpl-plugins">
          {catalog.length === 0 && <span className="dim">no plugins yet</span>}
          {catalog
            .filter((p) => has(p) || lib.has(pluginKey(p.marketplace, p.name)))
            .map((p) => (
              <label key={`${p.marketplace}/${p.name}`} className={`chip mono kind-toggle${has(p) ? "" : " off"}`}>
                <input type="checkbox" checked={has(p)} onChange={() => toggle(p)} />
                {p.name}
              </label>
            ))}
        </div>
        {catalog.length > 0 && (
          <PluginFinder catalog={catalog} library={lib} limit={6} action={(p) => (has(p) ? null : { label: "add", run: () => toggle(p) })} />
        )}
      </div>
      <div className="grid cols">
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Board (optional)</span>
          <select value={spec.board?.id ?? ""} onChange={(e) => set({ board: e.target.value ? { id: e.target.value, mode: spec.board?.mode ?? "fill" } : null })}>
            <option value="">— none —</option>
            {boards.map((b) => (
              <option key={b.id} value={b.id}>{b.name}</option>
            ))}
          </select>
        </label>
        <label style={{ display: "grid", gap: 4 }}>
          <span className="label">Placement</span>
          <select value={spec.board?.mode ?? "fill"} onChange={(e) => set({ board: spec.board ? { ...spec.board, mode: e.target.value as "fill" | "right" | "down" } : null })} disabled={!spec.board}>
            <option value="fill">first empty pane, else split right</option>
            <option value="right">split right</option>
            <option value="down">split down</option>
          </select>
        </label>
      </div>
      <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
        <button type="button" className="btn ghost" onClick={onClose} disabled={busy}>cancel</button>
        <button type="submit" className="btn primary" disabled={busy}>{busy ? "saving…" : "save template"}</button>
      </div>
    </form>
  );
}
