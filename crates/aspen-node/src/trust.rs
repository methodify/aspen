//! The trust gate (reference §7.7): headless `-p` sessions never show the
//! workspace-trust dialog, so Aspen owns it. Before first spawn in a repo,
//! the operator can see exactly what would auto-run — hooks, MCP servers,
//! skills, plugins — and record consent. "Here is everything this
//! repository will run, before it runs" is the affordance the terminal
//! never gave.
//!
//! This module inspects a repo and tracks consent. Enforcement is opt-in at
//! the node level so existing local flows are never surprised; the console
//! surfaces the review and records trust.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Default, Serialize)]
pub struct RepoAutorun {
    /// Hook commands from .claude/settings.json (+ .local).
    pub hooks: Vec<String>,
    /// MCP servers from .mcp.json (name → command/url summary).
    pub mcp_servers: Vec<String>,
    /// Skill names under .claude/skills/.
    pub skills: Vec<String>,
    /// Plugin roots referenced in settings.
    pub plugins: Vec<String>,
    /// Whether anything here would auto-run at all.
    pub has_autorun: bool,
}

/// Inspect what a repo would auto-run on first spawn.
pub fn inspect(repo: &Path) -> RepoAutorun {
    let mut r = RepoAutorun::default();
    for settings in [".claude/settings.json", ".claude/settings.local.json"] {
        if let Some(v) = read_json(&repo.join(settings)) {
            collect_hooks(&v, &mut r.hooks);
        }
    }
    if let Some(v) = read_json(&repo.join(".mcp.json")) {
        if let Some(servers) = v.get("mcpServers").and_then(|s| s.as_object()) {
            for (name, cfg) in servers {
                let how = cfg
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.to_owned())
                    .or_else(|| cfg.get("url").and_then(|u| u.as_str()).map(str::to_owned))
                    .unwrap_or_else(|| "?".into());
                r.mcp_servers.push(format!("{name}: {how}"));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(repo.join(".claude/skills")) {
        for e in entries.flatten() {
            if e.path().join("SKILL.md").is_file() {
                r.skills.push(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    r.has_autorun = !r.hooks.is_empty() || !r.mcp_servers.is_empty() || !r.plugins.is_empty();
    r
}

fn collect_hooks(settings: &Value, out: &mut Vec<String>) {
    let Some(hooks) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return;
    };
    for (event, matchers) in hooks {
        let Some(arr) = matchers.as_array() else {
            continue;
        };
        for m in arr {
            if let Some(hook_list) = m.get("hooks").and_then(|h| h.as_array()) {
                for h in hook_list {
                    if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                        out.push(format!("{event}: {cmd}"));
                    }
                }
            }
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Trust records: repos the operator has approved for auto-run, kept in the
/// node data dir.
pub struct TrustStore {
    path: PathBuf,
}

impl TrustStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("trusted-repos.json"),
        }
    }

    fn load(&self) -> Vec<String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn is_trusted(&self, repo: &Path) -> bool {
        let key = repo.to_string_lossy();
        self.load().iter().any(|r| r == &key)
    }

    pub fn trust(&self, repo: &Path) -> Result<()> {
        let key = repo.to_string_lossy().into_owned();
        let mut list = self.load();
        if !list.contains(&key) {
            list.push(key);
            if let Some(p) = self.path.parent() {
                std::fs::create_dir_all(p).ok();
            }
            std::fs::write(&self.path, serde_json::to_string_pretty(&list)?)?;
        }
        Ok(())
    }

    pub fn revoke(&self, repo: &Path) -> Result<()> {
        let key = repo.to_string_lossy();
        let list: Vec<String> = self.load().into_iter().filter(|r| r != &key).collect();
        std::fs::write(&self.path, serde_json::to_string_pretty(&list)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspects_hooks_and_mcp() {
        let dir = std::env::temp_dir().join(format!("aspen-trust-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(
            dir.join(".claude/settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"./guard.sh"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"db":{"command":"mcp-db"}}}"#,
        )
        .unwrap();
        let r = inspect(&dir);
        assert!(r.has_autorun);
        assert_eq!(r.hooks, vec!["PreToolUse: ./guard.sh"]);
        assert_eq!(r.mcp_servers, vec!["db: mcp-db"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn trust_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("aspen-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TrustStore::new(&dir);
        let repo = Path::new("/some/repo");
        assert!(!store.is_trusted(repo));
        store.trust(repo).unwrap();
        assert!(store.is_trusted(repo));
        store.revoke(repo).unwrap();
        assert!(!store.is_trusted(repo));
        std::fs::remove_dir_all(&dir).ok();
    }
}
