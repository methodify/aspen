// Plugins (PROPOSALS-2026-09 §7): the mesh's marketplaces, the catalog,
// and activation by scope — mesh, node, repo, session. From a plugin,
// see and edit where it is active; from a scope, its roster.

import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { api, type MarketSource, type PluginRegistry, type PluginRule, type CatalogPlugin, type MeshInfo } from "../api";
import { useAppData } from "../App";
import { ErrorBar, relTime } from "../components";
import "./plugins.css";

function ruleId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

export default function Plugins() {
  const { agents } = useAppData();
  const [params] = useSearchParams();
  const [reg, setReg] = useState<PluginRegistry | null>(null);
  const [mesh, setMesh] = useState<MeshInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [open, setOpen] = useState<string | null>(params.get("plugin")); // "plugin@market"
  const [syncing, setSyncing] = useState(false);
  // add-marketplace form
  const [mName, setMName] = useState("");
  const [mKind, setMKind] = useState<"github" | "git" | "directory">("github");
  const [mValue, setMValue] = useState("");
  const [removeAsk, setRemoveAsk] = useState<string | null>(null);

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

  async function sync(marketplace?: string) {
    setSyncing(true);
    setNote(null);
    try {
      const c = await api.pluginsSync(marketplace);
      const errs = Object.entries(c.errors ?? {});
      setNote(errs.length ? `synced with ${errs.length} error(s): ${errs.map(([k, v]) => `${k}: ${v}`).join(" · ")}` : "synced");
      await load();
    } catch (e) {
      setErr(`sync: ${e instanceof Error ? e.message : "failed"}`);
    } finally {
      setSyncing(false);
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
      setNote(`added ${name}; syncing…`);
      window.setTimeout(() => void load(), 3000);
      await load();
    } catch (e) {
      setErr(`add: ${e instanceof Error ? e.message : "failed"}`);
    }
  }
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
  const filtered = catalog.filter((p) => !q || `${p.name} ${p.description ?? ""} ${p.marketplace}`.toLowerCase().includes(q.toLowerCase()));
  const rulesFor = (p: CatalogPlugin) => (reg?.rules ?? []).filter((r) => r.marketplace === p.marketplace && r.plugin === p.name);
  const opened = catalog.find((p) => `${p.name}@${p.marketplace}` === open);

  return (
    <div className="stage-body plugins-page">
      <h1>Plugins</h1>
      <p className="dim">
        A library for the whole mesh: marketplaces registered once, plugins activated for the mesh, a node, a repo,
        or one session. Each node caches the versions its sessions need and starts them with <code>--plugin-dir</code>.
        A running session keeps the version it started with; a newer one is offered as a restart.
      </p>
      <ErrorBar error={err} />
      {note && <div className="mono-meta" style={{ margin: "4px 0 8px" }}>{note}</div>}

      <h2>Marketplaces</h2>
      <div className="mk-list">
        {(reg?.marketplaces ?? []).map((m) => {
          const at = reg?.catalog.synced_at?.[m.name];
          const e = reg?.catalog.errors?.[m.name];
          const count = catalog.filter((p) => p.marketplace === m.name).length;
          return (
            <div className="mk-row" key={m.name}>
              <span className="mono mk-name">{m.name}</span>
              <span className="mono-meta">{describeSource(m.source)}</span>
              <span className="mono-meta">{count} plugins{at ? ` · synced ${relTime(at)} ago` : " · not synced yet"}</span>
              {e && <span className="error-text mono-meta" title={e}>sync error</span>}
              <span style={{ flex: 1 }} />
              <button className="btn sm" disabled={syncing} onClick={() => void sync(m.name)}>sync now</button>
              {removeAsk === m.name ? (
                <span className="inline-confirm">
                  <span className="mono-meta">remove {m.name} and its rules?</span>
                  <button className="btn sm" onClick={async () => { await api.deleteMarketplace(m.name).catch(() => {}); setRemoveAsk(null); await load(); }}>yes</button>
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
          style={{ minWidth: 320 }}
          placeholder={mKind === "github" ? "anthropics/claude-plugins-official" : mKind === "git" ? "https://…/marketplace.git" : "/path/to/marketplace"}
          value={mValue}
          onChange={(e) => setMValue(e.target.value)}
        />
        <button className="btn primary sm" disabled={!mName.trim() || !mValue.trim()} onClick={() => void addMarketplace()}>add marketplace</button>
        <button className="btn sm" disabled={syncing} onClick={() => void sync()}>{syncing ? "syncing…" : "sync all"}</button>
      </div>

      <h2>Catalog</h2>
      <input className="plg-search" placeholder="filter plugins by name, description, marketplace" value={q} onChange={(e) => setQ(e.target.value)} />
      <table className="plg-table">
        <thead>
          <tr>
            <th>plugin</th>
            <th>marketplace</th>
            <th>current</th>
            <th>cached</th>
            <th>active for</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {filtered.map((p) => {
            const rs = rulesFor(p);
            const on = rs.filter((r) => r.enabled);
            const key = `${p.name}@${p.marketplace}`;
            return (
              <tr key={key} className={open === key ? "open" : ""}>
                <td>
                  <span className="mono">{p.name}</span>
                  {p.description && <div className="mono-meta plg-desc" title={p.description}>{p.description}</div>}
                </td>
                <td className="mono-meta">{p.marketplace}</td>
                <td className="mono">{p.current}</td>
                <td className="mono-meta">{p.cached.length ? p.cached.join(", ") : "—"}</td>
                <td className="mono-meta">
                  {on.length === 0 ? "nowhere" : on.map((r) => (r.scope_kind === "mesh" ? "mesh" : `${r.scope_kind}:${r.scope}`)).join(" · ")}
                  {rs.some((r) => !r.enabled) && <span title="some scopes disable it"> · exceptions</span>}
                </td>
                <td>
                  <button className="btn sm" onClick={() => setOpen(open === key ? null : key)}>{open === key ? "close" : "activate…"}</button>
                </td>
              </tr>
            );
          })}
          {filtered.length === 0 && (
            <tr>
              <td colSpan={6} className="dim">{catalog.length === 0 ? "no plugins yet — add and sync a marketplace" : "no plugins match"}</td>
            </tr>
          )}
        </tbody>
      </table>

      {opened && (
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
      )}
    </div>
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
