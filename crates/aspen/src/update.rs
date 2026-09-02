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
        // Nothing to replace, but --restart still bounces the daemon.
        if restart {
            match stop_daemon(data_dir)? {
                Some(params) => start_daemon(&exe, data_dir, &params)?,
                None => println!("no running daemon to restart."),
            }
        } else if crate::read_daemon_state(data_dir).is_some() {
            println!("(daemon is running on this same version.)");
        }
        return Ok(());
    }

    // On Windows a running process holds its .exe open without share-delete,
    // so the file can't be replaced while the daemon is up. --restart stops
    // it first (below); a plain update can't, so say so rather than fail with
    // a bare OS error. (Unix replaces via inode swap, daemon or not.)
    #[cfg(windows)]
    if !restart && crate::read_daemon_state(data_dir).is_some() {
        bail!(
            "a daemon is running and holds aspen.exe open — Windows can't replace it in place. \
             Re-run as `aspen update --restart`, or `aspen down` first."
        );
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

    // Stop the daemon BEFORE replacing: on Windows it holds the .exe open,
    // and stopping now also lets the resume ledger land before the new
    // binary reads it. Capture how to relaunch it.
    let restart_params = if restart {
        stop_daemon(data_dir)?
    } else {
        None
    };

    if let Err(e) = replace_binary(&exe, &bytes)
        .with_context(|| format!("installing new binary over {}", exe.display()))
    {
        // We may have just stopped a healthy daemon; bring it back on the
        // old binary (replace_binary rolls back, so exe is intact) so a
        // failed update doesn't leave the node down.
        if let Some(params) = &restart_params {
            eprintln!("update failed after stopping the daemon — restarting the previous binary…");
            let _ = start_daemon(&exe, data_dir, params);
        }
        return Err(e);
    }
    println!(
        "updated: aspen {current} → {remote_version} ({})",
        exe.display()
    );

    if restart {
        match restart_params {
            Some(params) => start_daemon(&exe, data_dir, &params)?,
            None => println!("no running daemon was found to restart."),
        }
    } else if crate::read_daemon_state(data_dir).is_some() {
        println!("daemon still running on the old binary — restart with: aspen update --restart (or aspen down && aspen up -d)");
    }
    Ok(())
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
/// How to relaunch the daemon after an update — captured from daemon.json
/// before it's stopped.
struct RestartParams {
    listen: String,
    ui: Option<String>,
    headless: bool,
}

/// Stop a running daemon and wait until it has fully released the port and
/// state file. Returns how to relaunch it, or None if none was running.
fn stop_daemon(data_dir: &Path) -> Result<Option<RestartParams>> {
    let Some(state) = crate::read_daemon_state(data_dir) else {
        return Ok(None);
    };
    // Prefer the *requested* address: an ephemeral node restarts on port 0
    // (a fresh OS-assigned port), not on the one it happened to hold.
    let listen = state["requested"]
        .as_str()
        .or(state["listen"].as_str())
        .unwrap_or("127.0.0.1:7420")
        .to_owned();
    let params = RestartParams {
        listen: listen.clone(),
        ui: state["ui"].as_str().map(str::to_owned),
        headless: state["headless"].as_bool().unwrap_or(false),
    };

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
    // state file goes away; wait until the port actually rebinds. (Port 0 is
    // ephemeral — binding it always succeeds, so the loop just falls through.)
    if let Ok(addr) = listen.parse::<std::net::SocketAddr>() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::net::TcpListener::bind(addr).is_err() {
            if std::time::Instant::now() > deadline {
                bail!("port {listen} still busy after shutdown; start manually with `aspen up -d`");
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    Ok(Some(params))
}

/// Launch the daemon detached with the captured parameters.
fn start_daemon(exe: &Path, data_dir: &Path, params: &RestartParams) -> Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--data-dir")
        .arg(data_dir)
        .args(["up", "-d", "--listen", &params.listen]);
    if let Some(ui) = &params.ui {
        cmd.args(["--ui", ui]);
    }
    if params.headless {
        cmd.arg("--headless");
    }
    let status = cmd.status().context("starting new daemon")?;
    if !status.success() {
        bail!("`aspen up -d` exited with {status}");
    }
    println!("daemon restarted on the new binary; previous sessions are being revived.");
    Ok(())
}
