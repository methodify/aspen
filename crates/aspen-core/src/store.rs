//! The session store seam (PROPOSALS-HARNESSES.md §3.4): where a harness
//! keeps sessions on disk and how Aspen reads them — enumeration,
//! rehydration, lineage, files to carry, usage, activity, the project's
//! directories. Claude's is a JSONL transcript per session under
//! `~/.claude/projects/<slug>/`; Codex's is a rollout per thread under
//! `$CODEX_HOME/sessions/`. Everything above the seam asks this trait.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::Harness;

/// One session found on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub title: Option<String>,
    /// Which program wrote it (Claude: the `entrypoint` stamp; Codex: the
    /// `originator`).
    pub entrypoint: Option<String>,
    pub modified_epoch: f64,
    pub user_messages: usize,
    #[serde(default)]
    pub harness: Harness,
}

/// Where a session came from and who writes it now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionOrigin {
    /// (parent session id, last copied message ref) when forked.
    pub forked_from: Option<(String, Option<String>)>,
    /// The first conversation line's ref (shared with the parent if a fork).
    pub first_ref: Option<String>,
    /// The writer of the newest line.
    pub last_entrypoint: Option<String>,
    /// The newest line's timestamp (epoch seconds).
    pub last_ts: Option<f64>,
}

/// A harness's per-project directories, for migration sentinels, the
/// artifact viewer's jail, memory convergence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectDirs {
    /// The per-project directory (Claude: `<home>/projects/<slug>`).
    pub project_dir: Option<PathBuf>,
    /// The harness's per-project memory directory, when it has one.
    pub memory_dir: Option<PathBuf>,
    /// Roots the artifact viewer may serve from besides the repo.
    pub artifact_roots: Vec<PathBuf>,
    /// The encoded project name used in paths (for migration sentinels).
    pub encoded: Option<String>,
    /// The harness home (for migration sentinels).
    pub home: Option<PathBuf>,
}

pub trait SessionStore: Send + Sync {
    fn harness(&self) -> Harness;
    /// Does a session with this id exist on disk for the repo?
    fn exists(&self, repo: &Path, session_id: &str) -> bool;
    /// When the session's main file was last written, epoch seconds.
    fn modified(&self, repo: &Path, session_id: &str) -> Option<f64>;
    fn enumerate(&self, repo: &Path) -> Result<Vec<SessionInfo>>;
    /// The whole conversation as rehydrated items (the console's HistoryItem shape).
    fn rehydrate(&self, repo: &Path, session_id: &str) -> Result<Vec<Value>>;
    /// Items after the user message with this ref, and whether it was found.
    fn rehydrate_after(
        &self,
        repo: &Path,
        session_id: &str,
        after: &str,
    ) -> Result<(Vec<Value>, bool)>;
    /// Rehydrate any transcript file of this harness's format (a replica,
    /// a subagent's).
    fn rehydrate_file(&self, path: &Path) -> Result<Vec<Value>>;
    fn origin(&self, repo: &Path, session_id: &str) -> Option<SessionOrigin>;
    /// Every file that makes up the session, as `(rel, path)` — for
    /// replication and migration. `rel` is the path under the project dir.
    fn files(&self, repo: &Path, session_id: &str) -> Vec<(String, PathBuf)>;
    /// The session's main file's path (may not exist yet).
    fn main_path(&self, repo: &Path, session_id: &str) -> PathBuf;
    fn usage(&self, repo: &Path, session_id: &str) -> Value;
    /// The activity ledger (docs/ACTIVITY.md), settled before `since`.
    fn activities(&self, repo: &Path, session_id: &str, since: Option<f64>) -> Vec<Value>;
    fn activity_counts(&self, repo: &Path, session_id: Option<&str>, since: Option<f64>) -> Value;
    /// A subagent's transcript, rehydrated.
    fn subagent(&self, repo: &Path, session_id: &str, agent_id: &str) -> Result<Vec<Value>>;
    fn project_dirs(&self, repo: &Path) -> ProjectDirs;
    /// Repos this harness has sessions for on this machine (discovery).
    fn discover_repos(&self) -> Vec<(PathBuf, usize)>;
}
