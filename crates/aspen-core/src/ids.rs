use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A session's identity. For runtimes that support pre-assigned ids (Claude's
/// `--session-id`) this is also the runtime's own session id, which makes
/// transcript correlation deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The short, human-facing name an agent goes by on the bus (`arch`, `impl`).
/// Distinct from the session id: names are stable across restarts and
/// resumes; ids are per-runtime-session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentName(pub String);

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// A repo registered with a node — the unit of automatic agent community.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepoId(pub String);

/// A machine in the mesh.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);
