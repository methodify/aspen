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
    /// "open" (default): sends outside declared topology deliver with a
    /// note. "closed": they are refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<String>,
    /// Self-update policy (docs/SERVICING.md §2).
    #[serde(default)]
    pub update: UpdateSettings,
}

/// The update policy: three knobs plus a snooze. Every field optional so an
/// unset policy is the safe default (check, notify, never apply).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// "notify" (default) | "auto".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// "HH:MM-HH:MM" in node-local time; auto only fires inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Minimum release age before auto applies it ("24h", "90m").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soak: Option<String>,
    /// A version to ignore (badge quiet, auto skips it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
    /// false = never check the release channel. None → true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<bool>,
}

impl UpdateSettings {
    pub fn auto(&self) -> bool {
        self.mode.as_deref() == Some("auto")
    }
    pub fn checks(&self) -> bool {
        self.check.unwrap_or(true)
    }
    pub fn soak_secs(&self) -> Option<u64> {
        self.soak.as_deref().and_then(|s| parse_duration(s).ok())
    }
    /// Reject bad values now so `aspen config` / PUT /api/settings fail
    /// loudly instead of a policy silently never firing.
    pub fn validate(&self) -> Result<()> {
        if let Some(m) = &self.mode {
            if m != "notify" && m != "auto" {
                anyhow::bail!("update mode takes notify or auto (got {m:?})");
            }
        }
        if let Some(w) = &self.window {
            parse_window(w)?;
        }
        if let Some(s) = &self.soak {
            parse_duration(s)?;
        }
        Ok(())
    }
    /// Is the local clock inside the window (or is there no window)?
    pub fn in_window_now(&self) -> bool {
        let Some(w) = &self.window else { return true };
        let Ok((from, to)) = parse_window(w) else {
            return true;
        };
        let now = chrono::Local::now();
        use chrono::Timelike as _;
        let m = now.hour() * 60 + now.minute();
        if from <= to {
            m >= from && m < to
        } else {
            // wraps midnight: 22:00-06:00
            m >= from || m < to
        }
    }
}

/// "24h" / "90m" / "3600s" / "2d" → seconds.
pub fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("bad duration {s:?} (use 24h, 90m, 30s, 2d)"))?;
    let mult = match unit.trim() {
        "s" => 1,
        "m" | "min" => 60,
        "h" | "" => 3600,
        "d" => 86400,
        other => anyhow::bail!("bad duration unit {other:?} in {s:?} (s, m, h, d)"),
    };
    Ok(n * mult)
}

/// "02:00-06:00" → (minutes from midnight, minutes from midnight).
pub fn parse_window(s: &str) -> Result<(u32, u32)> {
    let bad = || anyhow::anyhow!("bad window {s:?} (use HH:MM-HH:MM, e.g. 02:00-06:00)");
    let (a, b) = s.trim().split_once('-').ok_or_else(bad)?;
    let hm = |t: &str| -> Result<u32> {
        let (h, m) = t.trim().split_once(':').ok_or_else(bad)?;
        let h: u32 = h.parse().map_err(|_| bad())?;
        let m: u32 = m.parse().map_err(|_| bad())?;
        if h > 23 || m > 59 {
            return Err(bad());
        }
        Ok(h * 60 + m)
    };
    let (from, to) = (hm(a)?, hm(b)?);
    if from == to {
        anyhow::bail!("window {s:?} is empty");
    }
    Ok((from, to))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("24h").unwrap(), 86400);
        assert_eq!(parse_duration("90m").unwrap(), 5400);
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("2d").unwrap(), 172800);
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("5w").is_err());
    }

    #[test]
    fn windows() {
        assert_eq!(parse_window("02:00-06:00").unwrap(), (120, 360));
        assert_eq!(parse_window("22:00-06:00").unwrap(), (1320, 360));
        assert!(parse_window("2-6").is_err());
        assert!(parse_window("25:00-06:00").is_err());
        assert!(parse_window("03:00-03:00").is_err());
    }

    #[test]
    fn validate_policy() {
        let mut p = UpdateSettings::default();
        assert!(p.validate().is_ok());
        assert!(!p.auto());
        assert!(p.checks());
        p.mode = Some("auto".into());
        p.window = Some("01:00-05:00".into());
        p.soak = Some("24h".into());
        assert!(p.validate().is_ok());
        assert_eq!(p.soak_secs(), Some(86400));
        p.mode = Some("yolo".into());
        assert!(p.validate().is_err());
    }
}
