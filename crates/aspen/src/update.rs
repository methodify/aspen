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
//! Servicing (docs/SERVICING.md §5): the previous binary is kept in a
//! rollback slot, a restarted daemon is health-checked and rolled back if
//! it doesn't come up on the new version, and every run leaves
//! `update-outcome.json` for the next daemon to report. The daemon launches
//! this same command (`--unattended`) when its policy or an operator says
//! so; there is one code path.
//!
//! Environment: see aspen_node::release (`ASPEN_RELEASE_REPO`,
//! `ASPEN_GITHUB_API`, `GITHUB_TOKEN`/`GH_TOKEN`).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::Digest as _;

use aspen_node::release;
use aspen_node::servicing::{write_outcome, Outcome};

pub struct Opts<'a> {
    pub version: Option<&'a str>,
    pub force: bool,
    pub restart: bool,
    /// Launched by the daemon: no interactive expectations; always writes
    /// the outcome file with `trigger`.
    pub unattended: bool,
    pub trigger: &'a str,
    /// Swap the rollback slot back in (and restart if a daemon runs).
    pub rollback: bool,
    /// Only report what the channel has; change nothing.
    pub check: bool,
}

pub fn run(data_dir: &Path, o: Opts<'_>) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    // Resolved before any replacement: once the binary is renamed over,
    // /proc/self/exe reads as "… (deleted)" on Linux and can't be re-run.
    let exe = std::env::current_exe().context("locating current executable")?;

    if o.unattended {
        // Our output lands in aspen.log; stamp it so the run is findable.
        println!(
            "[aspen] unattended update ({}) starting — running v{current}",
            o.trigger
        );
    }
    if o.rollback {
        return rollback(data_dir, &exe, current, o.trigger);
    }

    let ch = release::channel();
    let info = release::fetch(&ch, o.version)
        .with_context(|| format!("fetching release info from {}", ch.repo))?;
    let remote_version = info.version.as_str();
    let tag = info.tag.as_str();

    if o.check {
        println!(
            "running  aspen {current}\nlatest   {tag}{}",
            info.published_at
                .map(|t| format!(" (published {})", crate::relative(t)))
                .unwrap_or_default()
        );
        if release::is_newer(remote_version, current) {
            println!("update available: aspen update --restart");
        } else if release::is_newer(current, remote_version) {
            println!("note: this version is newer than the latest published release (withdrawn?)");
        } else {
            println!("up to date.");
        }
        if let Some(n) = &info.notes {
            println!("\n{n}");
        }
        return Ok(());
    }

    if remote_version == current && !o.force {
        println!("aspen {current} is already the latest ({tag}). Use --force to reinstall.");
        // Nothing to replace, but --restart still bounces the daemon.
        if o.restart {
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
    if !o.restart && crate::read_daemon_state(data_dir).is_some() {
        bail!(
            "a daemon is running and holds aspen.exe open — Windows can't replace it in place. \
             Re-run as `aspen update --restart`, or `aspen down` first."
        );
    }

    let started_at = aspen_node::store::now_epoch();
    let outcome = |ok: bool, rolled_back: bool, error: Option<String>| Outcome {
        from: current.to_owned(),
        to: remote_version.to_owned(),
        ok,
        rolled_back,
        error,
        trigger: o.trigger.to_owned(),
        started_at,
        finished_at: aspen_node::store::now_epoch(),
        recorded: false,
    };

    let asset_name = format!(
        "aspen-{}{}",
        env!("ASPEN_TARGET"),
        std::env::consts::EXE_SUFFIX
    );
    let find = |name: &str| {
        info.asset_urls
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, u)| u.clone())
    };
    let agent = ureq::AgentBuilder::new()
        .redirects(0) // asset downloads: follow the 302 ourselves, without auth
        .timeout(std::time::Duration::from_secs(120))
        .build();
    let fetched: Result<Vec<u8>> = (|| {
        let bin_url =
            find(&asset_name).with_context(|| format!("release {tag} has no asset {asset_name}"))?;
        let sums_url = find("SHA256SUMS").context("release has no SHA256SUMS")?;
        println!("downloading {asset_name} {tag} …");
        let sums = String::from_utf8(download(&agent, &sums_url, ch.token.as_deref())?)
            .context("SHA256SUMS is not UTF-8")?;
        let expected = sums
            .lines()
            .filter_map(|l| l.split_once("  "))
            .find(|(_, name)| name.trim() == asset_name)
            .map(|(hash, _)| hash.trim().to_owned())
            .with_context(|| format!("SHA256SUMS has no entry for {asset_name}"))?;
        let bytes = download(&agent, &bin_url, ch.token.as_deref())?;
        let actual = hex::encode(sha2::Sha256::digest(&bytes));
        if actual != expected {
            bail!("checksum mismatch for {asset_name}: expected {expected}, got {actual}");
        }
        Ok(bytes)
    })();
    let bytes = match fetched {
        Ok(b) => b,
        Err(e) => {
            // Nothing touched yet; the daemon (if it launched us) sees this
            // in the outcome file and returns to ready.
            let _ = write_outcome(data_dir, &outcome(false, false, Some(format!("{e:#}"))));
            return Err(e);
        }
    };

    // Stop the daemon BEFORE replacing: on Windows it holds the .exe open,
    // and stopping now also lets the resume ledger land before the new
    // binary reads it. Capture how to relaunch it.
    let restart_params = if o.restart {
        match stop_daemon(data_dir) {
            Ok(p) => p,
            Err(e) => {
                let _ = write_outcome(data_dir, &outcome(false, false, Some(format!("{e:#}"))));
                return Err(e);
            }
        }
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
        let _ = write_outcome(data_dir, &outcome(false, false, Some(format!("{e:#}"))));
        return Err(e);
    }
    println!(
        "updated: aspen {current} → {remote_version} ({})",
        exe.display()
    );

    if !o.restart {
        let _ = write_outcome(data_dir, &outcome(true, false, None));
        if crate::read_daemon_state(data_dir).is_some() {
            println!("daemon still running on the old binary — restart with: aspen update --restart (or aspen down && aspen up -d)");
        }
        return Ok(());
    }
    let Some(params) = restart_params else {
        let _ = write_outcome(data_dir, &outcome(true, false, None));
        println!("no running daemon was found to restart.");
        return Ok(());
    };

    // Start the new daemon and make sure it is really the new one.
    match start_daemon(&exe, data_dir, &params).and_then(|()| health_check(data_dir, remote_version)) {
        Ok(()) => {
            let _ = write_outcome(data_dir, &outcome(true, false, None));
            Ok(())
        }
        Err(e) => {
            eprintln!("new daemon failed its health check: {e:#}");
            eprintln!("rolling back to aspen {current} …");
            let rb = restore_slot(&exe).and_then(|()| {
                let _ = stop_daemon(data_dir);
                start_daemon(&exe, data_dir, &params)
            });
            match rb {
                Ok(()) => {
                    let _ = write_outcome(
                        data_dir,
                        &outcome(false, true, Some(format!("health check failed: {e:#}"))),
                    );
                    bail!("update to {remote_version} rolled back: {e:#}");
                }
                Err(rbe) => {
                    let _ = write_outcome(
                        data_dir,
                        &outcome(
                            false,
                            false,
                            Some(format!("health check failed ({e:#}); rollback also failed: {rbe:#}")),
                        ),
                    );
                    bail!("update failed ({e:#}) and rollback failed ({rbe:#}) — start the node by hand: aspen up -d");
                }
            }
        }
    }
}

/// The rollback slot: where the previous binary is kept.
fn slot_path(exe: &Path) -> PathBuf {
    if cfg!(windows) {
        exe.with_extension("exe.old")
    } else {
        exe.with_extension("prev")
    }
}

/// `aspen update --rollback`: put the slot back, restarting a running
/// daemon onto it.
fn rollback(data_dir: &Path, exe: &Path, current: &str, trigger: &str) -> Result<()> {
    let slot = slot_path(exe);
    if !slot.exists() {
        bail!("no previous binary to roll back to ({})", slot.display());
    }
    let started_at = aspen_node::store::now_epoch();
    let params = stop_daemon(data_dir)?;
    restore_slot(exe)?;
    println!("rolled back: {} restored from {}", exe.display(), slot.display());
    let ver = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let to = ver
        .split_whitespace()
        .nth(1)
        .unwrap_or("previous")
        .to_owned();
    let _ = write_outcome(
        data_dir,
        &Outcome {
            from: current.to_owned(),
            to,
            ok: true,
            rolled_back: true,
            error: None,
            trigger: trigger.to_owned(),
            started_at,
            finished_at: aspen_node::store::now_epoch(),
            recorded: false,
        },
    );
    if let Some(p) = params {
        start_daemon(exe, data_dir, &p)?;
    }
    Ok(())
}

/// Swap the slot back over the binary. The current (bad) binary takes the
/// slot's place, so a rollback is itself reversible with --rollback.
fn restore_slot(exe: &Path) -> Result<()> {
    let slot = slot_path(exe);
    if !slot.exists() {
        bail!("rollback slot {} is missing", slot.display());
    }
    let tmp = exe.with_extension(format!("swap-{}", std::process::id()));
    std::fs::rename(exe, &tmp).with_context(|| format!("moving {} aside", exe.display()))?;
    if let Err(e) = std::fs::rename(&slot, exe) {
        let _ = std::fs::rename(&tmp, exe);
        return Err(e).with_context(|| format!("restoring {}", slot.display()));
    }
    let _ = std::fs::rename(&tmp, &slot);
    Ok(())
}

/// Wait for the restarted daemon to answer /api/node with the expected
/// version. `start_daemon` already waited for daemon.json; this confirms
/// the process serving it is the new binary and is actually serving.
fn health_check(data_dir: &Path, expect_version: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut last = String::from("daemon.json not written");
    loop {
        if let Some(state) = crate::read_daemon_state(data_dir) {
            if let Some(listen) = state["listen"].as_str() {
                let mut req = ureq::get(&format!("http://{listen}/api/node"))
                    .timeout(std::time::Duration::from_secs(3));
                let loopback = listen
                    .parse::<std::net::SocketAddr>()
                    .map(|a| a.ip().is_loopback())
                    .unwrap_or(true);
                if !loopback {
                    if let Ok(tok) = std::fs::read_to_string(data_dir.join("api-token")) {
                        req = req.set("X-Aspen-Token", tok.trim());
                    }
                }
                match req.call() {
                    Ok(resp) => {
                        let v: serde_json::Value = resp.into_json().unwrap_or_default();
                        let got = v["version"].as_str().unwrap_or("?");
                        if got == expect_version {
                            println!("health: daemon answers on http://{listen} as v{got}");
                            return Ok(());
                        }
                        last = format!("daemon answers as v{got}, expected v{expect_version}");
                    }
                    Err(e) => last = format!("http://{listen}/api/node: {e}"),
                }
            }
        }
        if std::time::Instant::now() > deadline {
            bail!("{last}");
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
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
        let resp = req.call().map_err(release::describe)?;
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

/// Atomically swap the running binary for `bytes`, keeping the previous one
/// in the rollback slot.
///
/// Unix: copy the current binary to the slot, write a temp file next to the
/// target (same filesystem), mark it executable, rename over — the running
/// process keeps its old inode.
/// Windows: the running exe's file can't be overwritten but CAN be renamed,
/// so move it to the slot (`.exe.old`) and move the new one into place.
fn replace_binary(exe: &Path, bytes: &[u8]) -> Result<()> {
    let dir = exe.parent().context("executable has no parent directory")?;
    let staged = dir.join(format!(".aspen-update-{}", std::process::id()));
    std::fs::write(&staged, bytes)?;
    let slot = slot_path(exe);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        let _ = std::fs::remove_file(&slot);
        std::fs::copy(exe, &slot).with_context(|| format!("keeping a copy in {}", slot.display()))?;
        if let Err(e) = std::fs::rename(&staged, exe) {
            let _ = std::fs::remove_file(&staged);
            return Err(e.into());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::remove_file(&slot);
        std::fs::rename(exe, &slot)?;
        if let Err(e) = std::fs::rename(&staged, exe) {
            // Roll back so the install isn't left with no binary at all.
            let _ = std::fs::rename(&slot, exe);
            let _ = std::fs::remove_file(&staged);
            return Err(e.into());
        }
    }
    Ok(())
}

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
    if let Err(e) = crate::stop_detached(data_dir) {
        // A stale state file (daemon crashed earlier) is not a reason to
        // abort the update: stop_detached clears it; we still know how the
        // daemon was configured and will start it fresh after the swap.
        if data_dir.join("daemon.json").exists() {
            return Err(e);
        }
        println!("(daemon was not actually running — stale state cleared; will start fresh after update)");
        return Ok(Some(params));
    }
    // Clean shutdown removes daemon.json last; wait for it so the agents'
    // live marks are final before the new daemon reads them.
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
    let mut cmd = aspen_node::gitstate::quiet_command(&exe.to_string_lossy());
    cmd.arg("--data-dir")
        .arg(data_dir)
        .args(["up", "-d", "--listen", &params.listen])
        // When the daemon launched us it passed its own ASPEN_DETACHED=1
        // along; with it set, `up -d` would run the daemon in the
        // foreground inside this child instead of detaching.
        .env_remove("ASPEN_DETACHED");
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
    println!("daemon restarted; previous sessions are being revived.");
    Ok(())
}
