//! Read-only import of mcc's per-repo session register (.mcc/sessions):
//! session names and configured CLI args. Aspen never writes this file —
//! it only borrows the names and args at discovery time so sessions started
//! under mcc keep their identity here.
//!
//! Format, one entry per line:
//!   <name>=<session-uuid>
//!   <name>:args=<cli args>

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct MccSession {
    pub name: String,
    /// Args as configured in mcc, minus permission flags (aspen's own
    /// permission model covers those).
    pub args: Option<String>,
    /// mcc had --dangerously-skip-permissions configured for this session.
    pub skip_permissions: bool,
}

/// Session-id → mcc registration for a repo. Empty map when no register.
pub fn read(repo: &Path) -> HashMap<String, MccSession> {
    let Ok(text) = std::fs::read_to_string(repo.join(".mcc/sessions")) else {
        return HashMap::new();
    };
    let mut by_name: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(name) = key.strip_suffix(":args") {
            by_name.entry(name.to_owned()).or_default().1 = Some(value.to_owned());
        } else if uuid::Uuid::parse_str(value).is_ok() {
            by_name.entry(key.to_owned()).or_default().0 = Some(value.to_owned());
        }
    }
    let mut out = HashMap::new();
    for (name, (sid, args)) in by_name {
        let Some(sid) = sid else { continue };
        let (args, skip) = match args {
            Some(a) => {
                let kept: Vec<&str> = a
                    .split_whitespace()
                    .filter(|t| *t != "--dangerously-skip-permissions")
                    .collect();
                let skip = kept.len() != a.split_whitespace().count();
                (Some(kept.join(" ")).filter(|s| !s.is_empty()), skip)
            }
            None => (None, false),
        };
        out.insert(
            sid,
            MccSession {
                name,
                args,
                skip_permissions: skip,
            },
        );
    }
    out
}
