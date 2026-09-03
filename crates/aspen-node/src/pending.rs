//! The mesh proposal queue: the console *authors* mesh changes here, a
//! human with a shell *executes* them with `aspen mesh apply`.
//!
//! This is the line the API's authority stops at. Trust operations (init,
//! certify, join, peers-add, relay) need the root key's host — a shell —
//! not a bearer token; so the daemon only ever writes proposals to
//! <data-dir>/mesh-pending.json, and `apply` (which runs in a shell) reads,
//! reviews, executes, and records the results (public artifacts like the
//! enroll blob or the join bundle) for the console to show.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    /// enroll | certify | join | peers_add | relay
    pub kind: String,
    /// Kind-specific arguments (node, blob, url, …).
    #[serde(default)]
    pub args: serde_json::Value,
    pub created_at: f64,
    /// Who authored it (console, cli).
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub kind: String,
    pub ok: bool,
    /// Human summary, or the error.
    pub message: String,
    /// A public artifact produced by the step (enroll blob, join bundle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    pub applied_at: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    #[serde(default)]
    pub outcomes: Vec<Outcome>,
}

fn path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("mesh-pending.json")
}

pub fn load(data_dir: &Path) -> Queue {
    std::fs::read_to_string(path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(data_dir: &Path, q: &Queue) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(path(data_dir), serde_json::to_string_pretty(q)?)?;
    Ok(())
}

pub fn propose(
    data_dir: &Path,
    kind: &str,
    args: serde_json::Value,
    source: &str,
) -> Result<Proposal> {
    let mut q = load(data_dir);
    let p = Proposal {
        id: uuid::Uuid::new_v4().to_string()[..8].to_owned(),
        kind: kind.to_owned(),
        args,
        created_at: crate::store::now_epoch(),
        source: source.to_owned(),
    };
    q.proposals.push(p.clone());
    save(data_dir, &q)?;
    Ok(p)
}

pub fn withdraw(data_dir: &Path, id: &str) -> Result<bool> {
    let mut q = load(data_dir);
    let before = q.proposals.len();
    q.proposals.retain(|p| p.id != id);
    save(data_dir, &q)?;
    Ok(q.proposals.len() != before)
}

/// Record an outcome and drop the proposal. Keeps the last 20 outcomes.
pub fn settle(
    data_dir: &Path,
    id: &str,
    ok: bool,
    message: String,
    artifact: Option<String>,
) -> Result<()> {
    let mut q = load(data_dir);
    let kind = q
        .proposals
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.kind.clone())
        .unwrap_or_default();
    q.proposals.retain(|p| p.id != id);
    q.outcomes.push(Outcome {
        id: id.to_owned(),
        kind,
        ok,
        message,
        artifact,
        applied_at: crate::store::now_epoch(),
    });
    if q.outcomes.len() > 20 {
        let drop = q.outcomes.len() - 20;
        q.outcomes.drain(0..drop);
    }
    save(data_dir, &q)
}

pub fn clear_outcomes(data_dir: &Path) -> Result<()> {
    let mut q = load(data_dir);
    q.outcomes.clear();
    save(data_dir, &q)
}
