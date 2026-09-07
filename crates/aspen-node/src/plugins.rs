//! Plugins: a mesh-managed library, activated by scope
//! (PROPOSALS-2026-09 §7; reference docs/PLUGINS.md).
//!
//! The **registry** — marketplaces and activation rules — lives in the
//! store and rides the mesh (roster digest, last writer wins, tombstones).
//! Everything else is per node and derived from it: the marketplace
//! checkout under `<data>/plugins/marketplaces/<name>/`, the parsed
//! catalog, and the version cache `<data>/plugins/cache/<market>/<plugin>/
//! <version>/`. A session is spawned with `--plugin-dir <cached path>` for
//! every plugin whose rules enclose it; it records what it started with,
//! so a newer cached version can be offered as a restart rather than
//! pulled out from under it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::node::NodeInner;

// --------------------------------------------------------------- registry

/// How a marketplace is reached. Mirrors the harness's known shapes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum MarketSource {
    /// `owner/repo` on GitHub.
    Github { repo: String },
    /// Any git URL.
    Git { url: String },
    /// A directory on this node (a local marketplace; not synced by us).
    Directory { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source: MarketSource,
    pub added_at: f64,
    pub updated_at: f64,
    #[serde(default)]
    pub deleted: bool,
}

/// One activation: `plugin@marketplace` enabled (or explicitly disabled)
/// for a scope. The most specific enclosing rule for a plugin decides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub marketplace: String,
    pub plugin: String,
    /// mesh | node | repo | session
    pub scope_kind: String,
    /// "" for mesh; node name; repo identity (origin URL or basename);
    /// agent name (bare@repo, node-less).
    pub scope: String,
    pub enabled: bool,
    /// Pin to a cached version; None = newest cached.
    #[serde(default)]
    pub pin: Option<String>,
    pub updated_at: f64,
    #[serde(default)]
    pub deleted: bool,
}

pub fn specificity(kind: &str) -> u8 {
    match kind {
        "session" => 4,
        "repo" => 3,
        "node" => 2,
        _ => 1,
    }
}

// ---------------------------------------------------------------- catalog

/// One plugin as a marketplace lists it, plus what we hold of it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogPlugin {
    pub marketplace: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    /// The marketplace's declared version, if any.
    pub version: Option<String>,
    /// The version we would materialize now: declared, else the
    /// marketplace checkout's short commit.
    pub current: String,
    pub source: Value,
    /// Cached versions on this node, newest first.
    #[serde(default)]
    pub cached: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Catalog {
    pub synced_at: HashMap<String, f64>,
    pub errors: HashMap<String, String>,
    pub plugins: Vec<CatalogPlugin>,
}

pub fn plugins_root(data_dir: &Path) -> PathBuf {
    data_dir.join("plugins")
}
fn checkouts_root(data_dir: &Path) -> PathBuf {
    plugins_root(data_dir).join("marketplaces")
}
pub fn cache_root(data_dir: &Path) -> PathBuf {
    plugins_root(data_dir).join("cache")
}
fn catalog_path(data_dir: &Path) -> PathBuf {
    plugins_root(data_dir).join("catalog.json")
}

pub fn load_catalog(data_dir: &Path) -> Catalog {
    std::fs::read(catalog_path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_catalog(data_dir: &Path, c: &Catalog) -> Result<()> {
    std::fs::create_dir_all(plugins_root(data_dir))?;
    std::fs::write(catalog_path(data_dir), serde_json::to_vec_pretty(c)?)?;
    Ok(())
}

fn git(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut c = crate::gitstate::quiet_command("git");
    if let Some(d) = cwd {
        c.arg("-C").arg(d);
    }
    let out = c.args(args).output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Bring the checkout up to date; returns its path and short commit.
fn checkout(data_dir: &Path, m: &Marketplace) -> Result<(PathBuf, String)> {
    match &m.source {
        MarketSource::Directory { path } => {
            let p = PathBuf::from(path);
            if !p.is_dir() {
                return Err(anyhow!("marketplace directory missing: {path}"));
            }
            let sha = git(&["rev-parse", "--short=12", "HEAD"], Some(&p))
                .unwrap_or_else(|_| "local".into());
            Ok((p, sha))
        }
        MarketSource::Github { .. } | MarketSource::Git { .. } => {
            let url = match &m.source {
                MarketSource::Github { repo } => format!("https://github.com/{repo}.git"),
                MarketSource::Git { url } => url.clone(),
                _ => unreachable!(),
            };
            let dir = checkouts_root(data_dir).join(safe(&m.name));
            if dir.join(".git").is_dir() {
                // Keep whatever branch it is on; a failed pull keeps the
                // old checkout usable.
                if let Err(e) = git(&["pull", "--ff-only", "--quiet"], Some(&dir)) {
                    tracing::warn!(marketplace = %m.name, error = %e, "marketplace pull failed; using the last checkout");
                }
            } else {
                std::fs::create_dir_all(checkouts_root(data_dir))?;
                git(
                    &[
                        "clone",
                        "--depth",
                        "1",
                        "--quiet",
                        &url,
                        &dir.to_string_lossy(),
                    ],
                    None,
                )?;
            }
            let sha = git(&["rev-parse", "--short=12", "HEAD"], Some(&dir))?;
            Ok((dir, sha))
        }
    }
}

fn read_manifest(dir: &Path) -> Result<Value> {
    let p = dir.join(".claude-plugin").join("marketplace.json");
    let b = std::fs::read(&p).map_err(|e| anyhow!("{}: {e}", p.display()))?;
    Ok(serde_json::from_slice(&b)?)
}

fn cached_versions(data_dir: &Path, market: &str, plugin: &str) -> Vec<String> {
    let dir = cache_root(data_dir).join(safe(market)).join(safe(plugin));
    let mut v: Vec<(f64, String)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| {
                    let t = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    (t, e.file_name().to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    v.into_iter().map(|x| x.1).collect()
}

/// Copy a directory tree (plugins are small).
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let d = dst.join(&name);
        if p.is_dir() {
            copy_tree(&p, &d)?;
        } else {
            std::fs::copy(&p, &d)?;
        }
    }
    Ok(())
}

/// Put `plugin`'s current version on disk; returns the cache path.
/// Relative sources copy out of the checkout; git sources shallow-clone
/// at their ref into a temp dir and copy the subdir.
fn materialize(data_dir: &Path, checkout_dir: &Path, cp: &CatalogPlugin) -> Result<PathBuf> {
    let dest = cache_root(data_dir)
        .join(safe(&cp.marketplace))
        .join(safe(&cp.name))
        .join(safe(&cp.current));
    if dest.join(".claude-plugin").join("plugin.json").is_file()
        || dest.join("plugin.json").is_file()
    {
        return Ok(dest);
    }
    let tmp = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    match &cp.source {
        Value::String(rel) => {
            let src = checkout_dir.join(rel.trim_start_matches("./"));
            if !src.is_dir() {
                return Err(anyhow!(
                    "plugin source missing in checkout: {}",
                    src.display()
                ));
            }
            copy_tree(&src, &tmp)?;
        }
        Value::Object(o) => {
            let kind = o.get("source").and_then(|s| s.as_str()).unwrap_or("");
            let url = match kind {
                "github" => o
                    .get("repo")
                    .and_then(|r| r.as_str())
                    .map(|r| format!("https://github.com/{r}.git")),
                "git-subdir" | "url" | "git" => {
                    o.get("url").and_then(|u| u.as_str()).map(str::to_owned)
                }
                _ => None,
            }
            .ok_or_else(|| anyhow!("unsupported plugin source: {kind}"))?;
            let clone = tmp.with_extension("clone");
            let _ = std::fs::remove_dir_all(&clone);
            std::fs::create_dir_all(clone.parent().unwrap())?;
            let mut args = vec!["clone", "--depth", "1", "--quiet"];
            let r#ref = o.get("ref").and_then(|r| r.as_str()).map(str::to_owned);
            if let Some(r) = &r#ref {
                args.push("--branch");
                args.push(r);
            }
            args.push(&url);
            let cl = clone.to_string_lossy().into_owned();
            args.push(&cl);
            git(&args, None)?;
            let sub = o
                .get("path")
                .and_then(|p| p.as_str())
                .map(|p| p.trim_start_matches("./"))
                .unwrap_or("");
            let src = if sub.is_empty() {
                clone.clone()
            } else {
                clone.join(sub)
            };
            copy_tree(&src, &tmp)?;
            let _ = std::fs::remove_dir_all(&clone);
        }
        _ => return Err(anyhow!("bad plugin source")),
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// Sync one marketplace: checkout, catalog, and materialize every plugin
/// a rule activates. Blocking (git); call from a blocking task.
pub fn sync_marketplace(
    data_dir: &Path,
    m: &Marketplace,
    rules: &[Rule],
    catalog: &mut Catalog,
) -> Result<usize> {
    let (dir, sha) = checkout(data_dir, m)?;
    let manifest = read_manifest(&dir)?;
    let mut n = 0usize;
    let mut fresh: Vec<CatalogPlugin> = Vec::new();
    for p in manifest
        .get("plugins")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let Some(name) = p.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let version = p.get("version").and_then(|v| v.as_str()).map(str::to_owned);
        let mut cp = CatalogPlugin {
            marketplace: m.name.clone(),
            name: name.to_owned(),
            description: p
                .get("description")
                .and_then(|d| d.as_str())
                .map(str::to_owned),
            category: p
                .get("category")
                .and_then(|d| d.as_str())
                .map(str::to_owned),
            current: version.clone().unwrap_or_else(|| sha.clone()),
            version,
            source: p.get("source").cloned().unwrap_or(Value::Null),
            cached: Vec::new(),
        };
        let wanted = rules
            .iter()
            .any(|r| !r.deleted && r.enabled && r.marketplace == m.name && r.plugin == name);
        if wanted {
            match materialize(data_dir, &dir, &cp) {
                Ok(_) => n += 1,
                Err(e) => {
                    catalog
                        .errors
                        .insert(format!("{}@{}", name, m.name), format!("{e:#}"));
                }
            }
        }
        cp.cached = cached_versions(data_dir, &m.name, name);
        fresh.push(cp);
    }
    catalog.plugins.retain(|c| c.marketplace != m.name);
    catalog.plugins.extend(fresh);
    catalog
        .synced_at
        .insert(m.name.clone(), crate::store::now_epoch());
    catalog.errors.retain(|k, _| {
        !k.ends_with(&format!("@{}", m.name)) || catalog.plugins.iter().any(|_| false)
    });
    Ok(n)
}

/// Sync every (non-deleted) marketplace; errors are recorded per
/// marketplace in the catalog, never fatal.
pub fn sync_all(
    data_dir: &Path,
    markets: &[Marketplace],
    rules: &[Rule],
    only: Option<&str>,
) -> Catalog {
    let mut catalog = load_catalog(data_dir);
    for m in markets.iter().filter(|m| !m.deleted) {
        if only.is_some_and(|o| o != m.name) {
            continue;
        }
        match sync_marketplace(data_dir, m, rules, &mut catalog) {
            Ok(n) => {
                catalog.errors.remove(&m.name);
                tracing::info!(marketplace = %m.name, materialized = n, "marketplace synced");
            }
            Err(e) => {
                tracing::warn!(marketplace = %m.name, error = %e, "marketplace sync failed");
                catalog.errors.insert(m.name.clone(), format!("{e:#}"));
            }
        }
    }
    let _ = save_catalog(data_dir, &catalog);
    catalog
}

// --------------------------------------------------------- effective set

/// A repo's identity for `repo` rules: its origin URL when it has one,
/// else its basename. Rules may name either.
pub fn repo_identities(repo: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(o) = crate::migrate::git_origin(repo) {
        v.push(o);
    }
    if let Some(b) = repo.file_name().map(|n| n.to_string_lossy().into_owned()) {
        v.push(b);
    }
    v.push(repo.to_string_lossy().into_owned());
    v
}

/// A plugin the session runs with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePlugin {
    pub marketplace: String,
    pub plugin: String,
    pub version: String,
    pub path: String,
    /// Which rule scope decided it.
    pub via: String,
}

/// Which plugins enclose (node, repo, agent): the most specific rule per
/// plugin decides; enabled ones are returned with the scope that won.
pub fn effective(
    rules: &[Rule],
    node: &str,
    repo: &Path,
    agent: &str,
) -> Vec<(String, String, String, Option<String>)> {
    let ids = repo_identities(repo);
    // (specificity, enabled, scope kind, pin) per (marketplace, plugin)
    type Best = (u8, bool, String, Option<String>);
    let mut best: HashMap<(String, String), Best> = HashMap::new();
    for r in rules.iter().filter(|r| !r.deleted) {
        let encloses = match r.scope_kind.as_str() {
            "mesh" => true,
            "node" => r.scope == node,
            "repo" => ids.iter().any(|i| i == &r.scope),
            "session" => r.scope == agent,
            _ => false,
        };
        if !encloses {
            continue;
        }
        let sp = specificity(&r.scope_kind);
        let key = (r.marketplace.clone(), r.plugin.clone());
        let entry = best.entry(key).or_insert((0, false, String::new(), None));
        if sp > entry.0 || (sp == entry.0 && r.enabled) {
            *entry = (sp, r.enabled, r.scope_kind.clone(), r.pin.clone());
        }
    }
    let mut out: Vec<(String, String, String, Option<String>)> = best
        .into_iter()
        .filter(|(_, v)| v.1)
        .map(|((m, p), v)| (m, p, v.2, v.3))
        .collect();
    out.sort();
    out
}

/// Resolve the effective set to cached paths for `--plugin-dir`.
/// Missing versions are reported, not fatal: the session starts without
/// them and the catalog error says why.
pub fn resolve(
    data_dir: &Path,
    rules: &[Rule],
    node: &str,
    repo: &Path,
    agent: &str,
) -> (Vec<ActivePlugin>, Vec<String>) {
    let catalog = load_catalog(data_dir);
    let mut active = Vec::new();
    let mut missing = Vec::new();
    for (market, plugin, via, pin) in effective(rules, node, repo, agent) {
        let versions = cached_versions(data_dir, &market, &plugin);
        let want = match &pin {
            Some(p) => versions.iter().find(|v| *v == p).cloned(),
            None => {
                // Prefer the catalog's current if cached, else newest cached.
                let cur = catalog
                    .plugins
                    .iter()
                    .find(|c| c.marketplace == market && c.name == plugin)
                    .map(|c| c.current.clone());
                cur.filter(|c| versions.contains(c))
                    .or_else(|| versions.first().cloned())
            }
        };
        match want {
            Some(v) => {
                let path = cache_root(data_dir)
                    .join(safe(&market))
                    .join(safe(&plugin))
                    .join(safe(&v));
                active.push(ActivePlugin {
                    marketplace: market,
                    plugin,
                    version: v,
                    path: path.to_string_lossy().into_owned(),
                    via,
                });
            }
            None => missing.push(format!(
                "{plugin}@{market}{}",
                pin.map(|p| format!(" (pinned {p})")).unwrap_or_default()
            )),
        }
    }
    (active, missing)
}

/// For a running session's recorded set: which plugins have a newer
/// version on disk than the one it started with.
pub fn updates_for(data_dir: &Path, running: &[ActivePlugin]) -> Vec<Value> {
    let catalog = load_catalog(data_dir);
    running
        .iter()
        .filter_map(|a| {
            let cur = catalog
                .plugins
                .iter()
                .find(|c| c.marketplace == a.marketplace && c.name == a.plugin)
                .map(|c| c.current.clone())?;
            let cached = cached_versions(data_dir, &a.marketplace, &a.plugin);
            if cur != a.version && cached.contains(&cur) {
                Some(json!({ "plugin": a.plugin, "marketplace": a.marketplace, "running": a.version, "available": cur }))
            } else {
                None
            }
        })
        .collect()
}

// ------------------------------------------------------------ node glue

/// The registry as the console and peers see it.
pub fn registry_json(inner: &Arc<NodeInner>) -> Value {
    let markets = inner.store.marketplaces(false).unwrap_or_default();
    let rules = inner.store.plugin_rules(false).unwrap_or_default();
    let catalog = inner
        .data_dir
        .as_deref()
        .map(load_catalog)
        .unwrap_or_default();
    json!({
        "marketplaces": markets,
        "rules": rules,
        "catalog": catalog,
        "node": inner.mesh().map(|m| m.identity.node.clone()),
    })
}

/// Sync in the background (git is blocking) and log the outcome.
pub fn spawn_sync(inner: Arc<NodeInner>, only: Option<String>) -> tokio::task::JoinHandle<Catalog> {
    tokio::task::spawn_blocking(move || {
        let Some(dd) = inner.data_dir.clone() else {
            return Catalog::default();
        };
        let markets = inner.store.marketplaces(false).unwrap_or_default();
        let rules = inner.store.plugin_rules(false).unwrap_or_default();
        sync_all(&dd, &markets, &rules, only.as_deref())
    })
}

/// The sync timer: at start, then every `sync_minutes`.
pub fn spawn_sync_timer(inner: Arc<NodeInner>) {
    tokio::spawn(async move {
        let mins = inner
            .data_dir
            .as_deref()
            .map(crate::settings::load)
            .and_then(|s| s.plugins.sync_minutes)
            .unwrap_or(60)
            .max(1);
        loop {
            let _ = spawn_sync(inner.clone(), None).await;
            tokio::time::sleep(std::time::Duration::from_secs(mins * 60)).await;
        }
    });
}
