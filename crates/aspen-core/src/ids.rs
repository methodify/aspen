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
