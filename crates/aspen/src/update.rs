//! `aspen update` — in-place self-update from GitHub Releases.
//!
//! Release layout (produced by .github/workflows/release.yml): each tag
//! carries raw binaries named `aspen-<target-triple>[.exe]` plus a single
//! `SHA256SUMS` file covering all of them. While the repo is private the
//! release assets are only reachable through the asset API endpoint with
//! `Accept: application/octet-stream` and a token — browser_download_url
//! 404s — so we always go through the API endpoint; it works for public
//! repos too.
//!
//! Environment:
//! - `ASPEN_RELEASE_REPO`  owner/repo override (default methodify/aspen)
//! - `ASPEN_GITHUB_API`    API base override (default https://api.github.com;
//!   lets tests point at a local fake server)
//! - `GITHUB_TOKEN` / `GH_TOKEN`  auth, required while the repo is private

use std::io::Read as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use sha2::Digest as _;

const DEFAULT_REPO: &str = "methodify/aspen";

pub fn run(data_dir: &Path, version: Option<&str>, force: bool, restart: bool) -> Result<()> {
    let repo = std::env::var("ASPEN_RELEASE_REPO").unwrap_or_else(|_| DEFAULT_REPO.into());
    let api = std::env::var("ASPEN_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into());
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok();

    let agent = ureq::AgentBuilder::new()
        .redirects(0) // asset downloads: follow the 302 ourselves, without auth
        .timeout(std::time::Duration::from_secs(120))
        .build();

    let release_url = match version {
        Some(tag) => {
            let tag =
                if tag.starts_with('v') || tag.chars().next().is_none_or(|c| !c.is_ascii_digit()) {
                    tag.to_owned()
                } else {
                    format!("v{tag}")
                };
            format!("{api}/repos/{repo}/releases/tags/{tag}")
        }
        None => format!("{api}/repos/{repo}/releases/latest"),
    };
    let release: serde_json::Value = get_json(&agent, &release_url, token.as_deref())
        .with_context(|| format!("fetching release info from {repo}"))?;
    let tag = release["tag_name"].as_str().unwrap_or("").to_owned();
    let remote_version = tag.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");

    // Resolved before any replacement: once the binary is renamed over,
    // /proc/self/exe reads as "… (deleted)" on Linux and can't be re-run.
    let exe = std::env::current_exe().context("locating current executable")?;

    if remote_version == current && !force {
        println!("aspen {current} is already the latest ({tag}). Use --force to reinstall.");
        return maybe_restart(data_dir, restart, false, &exe);
    }

    let asset_name = format!(
        "aspen-{}{}",
        env!("ASPEN_TARGET"),
        std::env::consts::EXE_SUFFIX
    );
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let find = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| a["url"].as_str().map(str::to_owned))
    };
    let bin_url =
        find(&asset_name).with_context(|| format!("release {tag} has no asset {asset_name}"))?;
    let sums_url = find("SHA256SUMS").context("release has no SHA256SUMS")?;

    println!("downloading {asset_name} {tag} …");
    let sums = String::from_utf8(download(&agent, &sums_url, token.as_deref())?)
        .context("SHA256SUMS is not UTF-8")?;
    let expected = sums
        .lines()
        .filter_map(|l| l.split_once("  "))
        .find(|(_, name)| name.trim() == asset_name)
        .map(|(hash, _)| hash.trim().to_owned())
        .with_context(|| format!("SHA256SUMS has no entry for {asset_name}"))?;

    let bytes = download(&agent, &bin_url, token.as_deref())?;
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if actual != expected {
        bail!("checksum mismatch for {asset_name}: expected {expected}, got {actual}");
    }

    replace_binary(&exe, &bytes)
        .with_context(|| format!("installing new binary over {}", exe.display()))?;
    println!(
        "updated: aspen {current} → {remote_version} ({})",
        exe.display()
    );

    maybe_restart(data_dir, restart, true, &exe)
}

fn get_json(agent: &ureq::Agent, url: &str, token: Option<&str>) -> Result<serde_json::Value> {
    let mut req = agent
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "aspen-update");
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().map_err(describe)?;
    Ok(resp.into_json()?)
}

/// GET an asset endpoint as a raw stream. GitHub answers with a 302 to
/// short-lived storage that rejects requests carrying an Authorization
/// header, so redirects are followed manually with auth stripped.
fn download(agent: &ureq::Agent, url: &str, token: Option<&str>) -> Result<Vec<u8>> {
    let mut url = url.to_owned();
    let mut with_auth = true;
    for _ in 0..5 {
        let mut req = agent
            .get(&url)
            .set("Accept", "application/octet-stream")
            .set("User-Agent", "aspen-update");
        if with_auth {
            if let Some(t) = token {
                req = req.set("Authorization", &format!("Bearer {t}"));
            }
        }
        let resp = req.call().map_err(describe)?;
        if (300..400).contains(&resp.status()) {
            url = resp
                .header("location")
                .context("redirect without Location header")?
                .to_owned();
            with_auth = false;
            continue;
        }
        let mut body = Vec::new();
        resp.into_reader()
            .take(512 * 1024 * 1024)
            .read_to_end(&mut body)?;
        return Ok(body);
    }
    bail!("too many redirects downloading {url}");
}

fn describe(e: ureq::Error) -> anyhow::Error {
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

/// Atomically swap the running binary for `bytes`.
///
/// Unix: write a temp file next to the target (same filesystem), mark it
/// executable, rename over — the running process keeps its old inode.
/// Windows: the running exe's file can't be overwritten but CAN be renamed,
/// so move it aside to `.exe.old` (cleaned up on next start) and move the
/// new one into place.
fn replace_binary(exe: &Path, bytes: &[u8]) -> Result<()> {
    let dir = exe.parent().context("executable has no parent directory")?;
    let staged = dir.join(format!(".aspen-update-{}", std::process::id()));
    std::fs::write(&staged, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&staged, exe)?;
    }
    #[cfg(not(unix))]
    {
        let old = exe.with_extension("exe.old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)?;
        if let Err(e) = std::fs::rename(&staged, exe) {
            // Roll back so the install isn't left with no binary at all.
            let _ = std::fs::rename(&old, exe);
            return Err(e.into());
        }
    }
    Ok(())
}

/// After a successful update (or a no-op with --restart), bounce the daemon
/// onto the (possibly new) binary. Uses daemon.json to stop the old process,
/// waits for its clean shutdown (which writes the resume ledger), then
/// starts the new binary detached — which auto-revives the sessions.
fn maybe_restart(data_dir: &Path, restart: bool, updated: bool, exe: &Path) -> Result<()> {
    if !restart {
        if updated && crate::read_daemon_state(data_dir).is_some() {
            println!("daemon still running on the old binary — restart with: aspen update --restart (or aspen down && aspen up -d)");
        }
        return Ok(());
    }
    let Some(state) = crate::read_daemon_state(data_dir) else {
        println!("no running daemon found — nothing to restart.");
        return Ok(());
    };
    // Prefer the *requested* address: an ephemeral node restarts on port 0
    // (a fresh OS-assigned port), not on the one it happened to hold.
    let listen = state["requested"]
        .as_str()
        .or(state["listen"].as_str())
        .unwrap_or("127.0.0.1:7420")
        .to_owned();
    let ui = state["ui"].as_str().map(str::to_owned);
    let headless = state["headless"].as_bool().unwrap_or(false);

    println!("stopping daemon …");
    crate::stop_detached(data_dir)?;
    // Clean shutdown removes daemon.json last; wait for it so the resume
    // ledger is fully written before the new daemon reads it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while data_dir.join("daemon.json").exists() {
        if std::time::Instant::now() > deadline {
            bail!("daemon did not shut down within 30s; start it manually with `aspen up -d`");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // The listener may still be draining connections for a moment after the
    // state file goes away; wait until the port actually rebinds.
    if let Ok(addr) = listen.parse::<std::net::SocketAddr>() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::net::TcpListener::bind(addr).is_err() {
            if std::time::Instant::now() > deadline {
                bail!("port {listen} still busy after shutdown; start manually with `aspen up -d`");
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--data-dir")
        .arg(data_dir)
        .args(["up", "-d", "--listen", &listen]);
    if let Some(ui) = ui {
        cmd.args(["--ui", &ui]);
    }
    if headless {
        cmd.arg("--headless");
    }
    let status = cmd.status().context("starting new daemon")?;
    if !status.success() {
        bail!("`aspen up -d` exited with {status}");
    }
    println!("daemon restarted on the new binary; previous sessions are being revived.");
    Ok(())
}
