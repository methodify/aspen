//! Repo git state for the fleet view: branch, dirty count, ahead/behind.
//! Refreshed in the background for every registered repo (git status can
//! be slow on big trees; the API only ever reads the cache).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, serde::Serialize)]
pub struct GitState {
    pub branch: Option<String>,
    pub dirty: u32,
    pub ahead: u32,
    pub behind: u32,
    pub checked_at: f64,
}

static CACHE: OnceLock<Mutex<HashMap<PathBuf, GitState>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<PathBuf, GitState>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn get(path: &Path) -> Option<GitState> {
    cache().lock().unwrap().get(path).cloned()
}

/// Blocking probe. `git status --porcelain=v2 --branch`.
fn probe(path: &Path) -> Option<GitState> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(path)
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut st = GitState {
        branch: None,
        dirty: 0,
        ahead: 0,
        behind: 0,
        checked_at: crate::store::now_epoch(),
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            st.branch = Some(rest.trim().to_owned()).filter(|b| b != "(detached)");
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for tok in rest.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    st.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    st.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            st.dirty += 1;
        }
    }
    Some(st)
}

/// Keep the cache fresh for every registered repo, every `every` seconds.
pub fn spawn_refresher(inner: Arc<crate::node::NodeInner>, every: u64) {
    tokio::spawn(async move {
        loop {
            let repos: Vec<PathBuf> = inner
                .store
                .repos()
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.path)
                .collect();
            let results = tokio::task::spawn_blocking(move || {
                repos
                    .into_iter()
                    .filter_map(|p| probe(&p).map(|s| (p, s)))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            {
                let mut c = cache().lock().unwrap();
                for (p, s) in results {
                    c.insert(p, s);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(every)).await;
        }
    });
}
