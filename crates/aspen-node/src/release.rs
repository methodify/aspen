//! The release channel: what version is published, and is it newer than
//! this binary. Shared by the daemon's periodic check (servicing.rs) and
//! `aspen update` (which additionally downloads and verifies assets).
//!
//! Environment (same as `aspen update`):
//! - `ASPEN_RELEASE_REPO`  owner/repo override (default methodify/aspen)
//! - `ASPEN_GITHUB_API`    API base override (lets tests use a fake server)
//! - `GITHUB_TOKEN` / `GH_TOKEN`  auth, required while the repo is private

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_REPO: &str = "methodify/aspen";

/// Where and how to reach the release channel, from the environment.
pub struct Channel {
    pub repo: String,
    pub api: String,
    pub token: Option<String>,
}

pub fn channel() -> Channel {
    Channel {
        repo: std::env::var("ASPEN_RELEASE_REPO").unwrap_or_else(|_| DEFAULT_REPO.into()),
        api: std::env::var("ASPEN_GITHUB_API")
            .unwrap_or_else(|_| "https://api.github.com".into()),
        token: std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .ok()
            .filter(|t| !t.trim().is_empty()),
    }
}

/// A published release, as much of it as servicing needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseInfo {
    /// Bare version (`0.4.0`).
    pub version: String,
    /// The tag (`v0.4.0`).
    pub tag: String,
    /// Epoch seconds, from `published_at`.
    pub published_at: Option<f64>,
    /// The release body (notes), trimmed to a sane size.
    pub notes: Option<String>,
    /// Asset names present — lets a node know whether its target is built.
    #[serde(default)]
    pub assets: Vec<String>,
    /// The raw release JSON's asset list (name → API url), for `update`.
    #[serde(skip)]
    pub asset_urls: Vec<(String, String)>,
}

/// Fetch release metadata: latest, or a specific tag (`v0.4.0` / `0.4.0`).
pub fn fetch(ch: &Channel, version: Option<&str>) -> Result<ReleaseInfo> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let url = match version {
        Some(tag) => {
            let tag = if tag.starts_with('v') || tag.chars().next().is_none_or(|c| !c.is_ascii_digit()) {
                tag.to_owned()
            } else {
                format!("v{tag}")
            };
            format!("{}/repos/{}/releases/tags/{tag}", ch.api, ch.repo)
        }
        None => format!("{}/repos/{}/releases/latest", ch.api, ch.repo),
    };
    let mut req = agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "aspen-update");
    if let Some(t) = &ch.token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().map_err(describe)?;
    let v: serde_json::Value = resp
        .into_json()
        .with_context(|| format!("parsing release JSON from {url}"))?;
    parse(&v)
}

pub fn parse(v: &serde_json::Value) -> Result<ReleaseInfo> {
    let tag = v["tag_name"]
        .as_str()
        .filter(|t| !t.is_empty())
        .context("release has no tag_name")?
        .to_owned();
    let version = tag.trim_start_matches('v').to_owned();
    let published_at = v["published_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp() as f64);
    let notes = v["body"].as_str().map(|b| {
        let b = b.trim();
        if b.chars().count() > 4000 {
            format!("{}…", b.chars().take(4000).collect::<String>())
        } else {
            b.to_owned()
        }
    });
    let assets_raw = v["assets"].as_array().cloned().unwrap_or_default();
    let assets: Vec<String> = assets_raw
        .iter()
        .filter_map(|a| a["name"].as_str().map(str::to_owned))
        .collect();
    let asset_urls = assets_raw
        .iter()
        .filter_map(|a| Some((a["name"].as_str()?.to_owned(), a["url"].as_str()?.to_owned())))
        .collect();
    Ok(ReleaseInfo {
        version,
        tag,
        published_at,
        notes: notes.filter(|n| !n.is_empty()),
        assets,
        asset_urls,
    })
}

pub fn describe(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let url = resp.get_url().to_owned();
            let body = resp.into_string().unwrap_or_default();
            let hint = if code == 404 {
                " (private repo? set GITHUB_TOKEN)"
            } else if code == 401 || code == 403 {
                " (check GITHUB_TOKEN)"
            } else {
                ""
            };
            anyhow::anyhow!(
                "HTTP {code} from {url}{hint}: {}",
                body.chars().take(200).collect::<String>()
            )
        }
        other => anyhow::anyhow!(other),
    }
}

/// Numeric semver triple (extra segments ignored; non-numeric → 0).
pub fn parse_version(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Is `a` strictly newer than `b`?
pub fn is_newer(a: &str, b: &str) -> bool {
    parse_version(a) > parse_version(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.4.0", "0.3.1"));
        assert!(is_newer("v0.4.0", "0.3.11"));
        assert!(!is_newer("0.3.1", "0.3.1"));
        assert!(!is_newer("0.3.1", "0.4.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert_eq!(parse_version("v1.2.3-rc1"), (1, 2, 3));
    }

    #[test]
    fn parses_release_json() {
        let v = serde_json::json!({
            "tag_name": "v0.4.0",
            "published_at": "2026-09-03T12:00:00Z",
            "body": "  notes here  ",
            "assets": [
                { "name": "aspen-x86_64-unknown-linux-gnu", "url": "https://api/x" },
                { "name": "SHA256SUMS", "url": "https://api/s" }
            ]
        });
        let r = parse(&v).unwrap();
        assert_eq!(r.version, "0.4.0");
        assert_eq!(r.tag, "v0.4.0");
        assert_eq!(r.notes.as_deref(), Some("notes here"));
        assert!(r.published_at.is_some());
        assert_eq!(r.assets.len(), 2);
        assert_eq!(r.asset_urls[1].0, "SHA256SUMS");
    }
}
