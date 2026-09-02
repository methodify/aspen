//! Node settings: user-set defaults kept in <data-dir>/settings.json.
//!
//! Today that is per-harness default CLI args (only "claude" exists). Args
//! are a literal string, whitespace-split with quoting; templating waits
//! until a real shape for replacement params emerges.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Per-harness defaults, keyed by harness name ("claude").
    #[serde(default)]
    pub harness: BTreeMap<String, HarnessSettings>,
    /// How `aspen up` should start when flags don't say otherwise.
    #[serde(default)]
    pub daemon: DaemonDefaults,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DaemonDefaults {
    /// Start headless by default (no console). None = unset (→ false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,
    /// Default listen address, e.g. "127.0.0.1:7420". None = unset (→ the
    /// built-in default, which depends on headless).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HarnessSettings {
    /// Extra CLI args appended to every session of this harness,
    /// e.g. "--chrome". Applied before any per-session args.
    #[serde(default)]
    pub args: String,
}

pub fn load(data_dir: &Path) -> Settings {
    std::fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(data_dir: &Path, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(
        data_dir.join("settings.json"),
        serde_json::to_string_pretty(settings)?,
    )?;
    Ok(())
}

/// Flags the protocol client owns; user args must not fight it for them.
const RESERVED: &[&str] = &[
    "--print",
    "-p",
    "--output-format",
    "--input-format",
    "--include-partial-messages",
    "--replay-user-messages",
    "--session-id",
    "--resume",
    "-r",
    "--fork-session",
    "--permission-prompt-tool",
    "--permission-mode",
    "--model",
    "--dangerously-skip-permissions",
    "--verbose",
];

/// Split an args string (defaults + per-session), rejecting reserved flags.
pub fn split_args(defaults: &str, session: Option<&str>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for chunk in [Some(defaults), session].into_iter().flatten() {
        out.extend(
            shell_words::split(chunk)
                .map_err(|e| anyhow::anyhow!("bad quoting in harness args {chunk:?}: {e}"))?,
        );
    }
    for arg in &out {
        let bare = arg.split('=').next().unwrap_or(arg);
        if RESERVED.contains(&bare) {
            anyhow::bail!(
                "harness arg {arg:?} conflicts with a protocol-owned flag ({bare}); \
                 aspen manages that one itself"
            );
        }
    }
    Ok(out)
}
