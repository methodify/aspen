//! Auto-start of the daemon at login, as the user (PROPOSALS-2026-09-C.md
//! §3). Never a system service: the node runs the operator's repos with
//! the operator's harness credentials, so it runs under the operator's
//! own login and nothing else.
//!
//! | platform | mechanism |
//! |---|---|
//! | Linux, WSL with systemd | a `systemd --user` unit; the daemon runs in the foreground under it, restarted on failure |
//! | macOS | a LaunchAgent; `KeepAlive` on failure only |
//! | Windows | a Scheduled Task at logon, limited rights, running `aspen up -d` — no supervisor |
//!
//! Where a supervisor exists the daemon knows it (`ASPEN_SUPERVISOR` in
//! its environment, echoed into daemon.json), and `aspen down`, `aspen
//! restart` and the updater's restart go through the supervisor instead of
//! signalling and re-spawning — otherwise the supervisor and the CLI would
//! fight over who runs the daemon.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use aspen_node::gitstate::quiet_command;
use serde::{Deserialize, Serialize};

/// What is installed, as recorded at enable time (`autostart.json` in the
/// data dir) — the cheap answer for `/api/node`; the CLI's `status`
/// asks the platform and reconciles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installed {
    pub kind: String,
    pub unit: String,
    pub path: Option<PathBuf>,
    pub at: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// The platform can do this (systemd --user reachable, launchd, or
    /// Windows Task Scheduler).
    pub supported: bool,
    /// "systemd" | "launchd" | "schtasks".
    pub kind: Option<String>,
    pub enabled: bool,
    /// The unit / label / task name.
    pub unit: Option<String>,
    /// Path of the unit file or plist (none for a scheduled task).
    pub path: Option<PathBuf>,
    /// The running daemon was started by the supervisor.
    pub supervised: bool,
    /// Why not, or what to know (e.g. the linger hint).
    pub note: Option<String>,
}

fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join("autostart.json")
}

pub fn installed(data_dir: &Path) -> Option<Installed> {
    let s = std::fs::read_to_string(marker_path(data_dir)).ok()?;
    serde_json::from_str(&s).ok()
}

/// What supervises the running daemon, per daemon.json.
pub fn supervisor_of(data_dir: &Path) -> Option<String> {
    crate::read_daemon_state(data_dir)?
        .get("supervisor")
        .and_then(|s| s.as_str())
        .map(str::to_owned)
}

/// The unit's name: `aspen` for the default data dir, else a suffix from
/// the dir so two nodes on one login do not collide.
pub fn unit_name(data_dir: &Path) -> String {
    let canon = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let default = std::fs::canonicalize(crate::default_data_dir())
        .unwrap_or_else(|_| crate::default_data_dir());
    if canon == default {
        return "aspen".to_owned();
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut h);
    format!("aspen-{:08x}", h.finish() as u32)
}

/// The cheap status: the marker plus the running daemon's own word.
pub fn status_quick(data_dir: &Path) -> Status {
    let inst = installed(data_dir);
    let supervised = supervisor_of(data_dir).is_some();
    let kind = platform_kind();
    Status {
        supported: kind.is_some(),
        kind: inst
            .as_ref()
            .map(|i| i.kind.clone())
            .or_else(|| kind.map(str::to_owned)),
        enabled: inst.is_some(),
        unit: inst.as_ref().map(|i| i.unit.clone()),
        path: inst.as_ref().and_then(|i| i.path.clone()),
        supervised,
        note: if kind.is_none() {
            Some(unsupported_reason())
        } else {
            None
        },
    }
}

/// The real status: ask the platform, and fix the marker if it lies.
pub fn status(data_dir: &Path) -> Status {
    let mut st = status_quick(data_dir);
    let Some(kind) = platform_kind() else {
        return st;
    };
    let unit = unit_name(data_dir);
    let really = match kind {
        "systemd" => systemd::is_enabled(&unit),
        "launchd" => launchd::is_loaded(&unit),
        "schtasks" => schtasks::exists(&unit),
        _ => false,
    };
    if really != st.enabled {
        if really {
            let _ = write_marker(data_dir, kind, &unit, unit_path(kind, data_dir, &unit));
        } else {
            let _ = std::fs::remove_file(marker_path(data_dir));
        }
        st = status_quick(data_dir);
        st.enabled = really;
    }
    if kind == "systemd" && st.enabled && !systemd::linger_enabled() {
        st.note = Some("without lingering the unit starts at your first login and stops at your last logout; for a box that should run headless: loginctl enable-linger".into());
    }
    st
}

fn write_marker(data_dir: &Path, kind: &str, unit: &str, path: Option<PathBuf>) -> Result<()> {
    let m = Installed {
        kind: kind.to_owned(),
        unit: unit.to_owned(),
        path,
        at: aspen_node::store::now_epoch(),
    };
    std::fs::create_dir_all(data_dir).ok();
    std::fs::write(marker_path(data_dir), serde_json::to_string_pretty(&m)?)?;
    Ok(())
}

fn unit_path(kind: &str, _data_dir: &Path, unit: &str) -> Option<PathBuf> {
    match kind {
        "systemd" => Some(systemd::unit_file(unit)),
        "launchd" => Some(launchd::plist_file(unit)),
        _ => None,
    }
}

fn platform_kind() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("launchd")
    } else if cfg!(windows) {
        Some("schtasks")
    } else if cfg!(target_os = "linux") && systemd::user_manager_reachable() {
        Some("systemd")
    } else {
        None
    }
}

fn unsupported_reason() -> String {
    if cfg!(target_os = "linux") {
        "no systemd user manager here (on WSL: set systemd=true under [boot] in /etc/wsl.conf and restart the distro)".into()
    } else {
        "no supported auto-start mechanism on this platform".into()
    }
}

/// How the daemon should be started: what the running one was asked for,
/// else the configured defaults (which `up` applies on its own).
struct StartSpec {
    listen: Option<String>,
    headless: bool,
    ui: Option<String>,
}

fn start_spec(data_dir: &Path) -> StartSpec {
    match crate::read_daemon_state(data_dir) {
        Some(st) => StartSpec {
            listen: st["requested"]
                .as_str()
                .or(st["listen"].as_str())
                .map(str::to_owned),
            headless: st["headless"].as_bool().unwrap_or(false),
            ui: st["ui"].as_str().map(str::to_owned),
        },
        None => {
            let cfg = aspen_node::settings::load(data_dir).daemon;
            StartSpec {
                listen: cfg.listen.clone(),
                headless: cfg.headless.unwrap_or(false),
                ui: None,
            }
        }
    }
}

fn up_args(data_dir: &Path, spec: &StartSpec, detach: bool) -> Vec<String> {
    let mut a = vec![
        "--data-dir".to_owned(),
        data_dir.to_string_lossy().into_owned(),
        "up".to_owned(),
    ];
    if detach {
        a.push("-d".into());
    }
    if let Some(l) = &spec.listen {
        a.push("--listen".into());
        a.push(l.clone());
    }
    if spec.headless {
        a.push("--headless".into());
    }
    if let Some(u) = &spec.ui {
        a.push("--ui".into());
        a.push(u.clone());
    }
    a
}

/// Install and start. A daemon already running by hand is stopped and
/// brought back under the supervisor (its sessions are revived by the
/// new process, as after any restart); on Windows there is no supervisor
/// and the running daemon is left alone.
pub fn enable(data_dir: &Path, adopt: bool) -> Result<Status> {
    let kind = platform_kind().ok_or_else(|| anyhow!("{}", unsupported_reason()))?;
    let unit = unit_name(data_dir);
    let exe = std::env::current_exe().context("locating this binary")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let spec = start_spec(data_dir);
    let running_by_hand =
        crate::read_daemon_state(data_dir).is_some() && supervisor_of(data_dir).is_none();
    match kind {
        "systemd" => {
            let path = systemd::install(&unit, &exe, data_dir, &up_args(data_dir, &spec, false))?;
            write_marker(data_dir, kind, &unit, Some(path))?;
            if adopt {
                if running_by_hand {
                    println!("stopping the daemon that was started by hand …");
                    crate::stop_detached(data_dir)?;
                    wait_gone(data_dir, 60)?;
                }
                if crate::read_daemon_state(data_dir).is_none() {
                    systemd::start(&unit)?;
                    wait_up(data_dir, 30)?;
                }
            }
        }
        "launchd" => {
            let path = launchd::install(&unit, &exe, data_dir, &up_args(data_dir, &spec, false))?;
            write_marker(data_dir, kind, &unit, Some(path.clone()))?;
            if adopt {
                if running_by_hand {
                    println!("stopping the daemon that was started by hand …");
                    crate::stop_detached(data_dir)?;
                    wait_gone(data_dir, 60)?;
                }
                launchd::bootstrap(&unit, &path)?;
                if crate::read_daemon_state(data_dir).is_none() {
                    launchd::start(&unit)?;
                    wait_up(data_dir, 30)?;
                }
            }
        }
        "schtasks" => {
            schtasks::install(&unit, &exe, &up_args(data_dir, &spec, true))?;
            write_marker(data_dir, kind, &unit, None)?;
            if adopt && crate::read_daemon_state(data_dir).is_none() {
                schtasks::run(&unit)?;
                wait_up(data_dir, 30)?;
            }
        }
        _ => unreachable!(),
    }
    Ok(status(data_dir))
}

/// Remove the unit. Stops nothing: a running daemon keeps running (under
/// systemd/launchd until it exits on its own; the next login starts
/// nothing).
pub fn disable(data_dir: &Path) -> Result<Status> {
    let unit = unit_name(data_dir);
    let kind = installed(data_dir)
        .map(|i| i.kind)
        .or_else(|| platform_kind().map(str::to_owned));
    match kind.as_deref() {
        Some("systemd") => systemd::uninstall(&unit)?,
        Some("launchd") => launchd::uninstall(&unit)?,
        Some("schtasks") => schtasks::uninstall(&unit)?,
        _ => {}
    }
    let _ = std::fs::remove_file(marker_path(data_dir));
    Ok(status(data_dir))
}

// ------------------------------------------------- supervisor-aware control

/// Stop a supervised daemon through its supervisor and wait for it to be
/// gone. `None` when the daemon is not supervised (the caller falls back
/// to the signal ladder).
pub fn supervised_stop(data_dir: &Path) -> Option<Result<()>> {
    let sup = supervisor_of(data_dir)?;
    let unit = unit_name(data_dir);
    Some(match sup.as_str() {
        "systemd" => systemd::stop(&unit).and_then(|()| wait_gone(data_dir, 90)),
        "launchd" => launchd::stop(&unit).and_then(|()| wait_gone(data_dir, 90)),
        other => Err(anyhow!("unknown supervisor {other}")),
    })
}

/// Start the daemon through the supervisor named by the marker (the
/// updater's and `restart`'s path once the old daemon is gone).
pub fn supervised_start(data_dir: &Path, kind: &str) -> Result<()> {
    let unit = unit_name(data_dir);
    match kind {
        "systemd" => systemd::start(&unit)?,
        "launchd" => launchd::start(&unit)?,
        other => bail!("unknown supervisor {other}"),
    }
    wait_up(data_dir, 30)
}

fn wait_gone(data_dir: &Path, secs: u64) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while data_dir.join("daemon.json").exists() {
        if std::time::Instant::now() > deadline {
            bail!("the daemon did not stop within {secs}s");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(())
}

fn wait_up(data_dir: &Path, secs: u64) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(st) = crate::read_daemon_state(data_dir) {
            if st
                .get("pid")
                .and_then(|p| p.as_u64())
                .is_some_and(|p| crate::process_alive(p as u32))
            {
                return Ok(());
            }
        }
        if std::time::Instant::now() > deadline {
            bail!(
                "the daemon did not come up within {secs}s — see {}",
                data_dir.join("aspen.log").display()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn run_ok(mut cmd: std::process::Command, what: &str) -> Result<()> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).into_owned()
        } else {
            err.into_owned()
        };
        bail!("{what} failed: {}", err.trim());
    }
    Ok(())
}

fn systemd_quote(s: &str) -> String {
    if s.chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_owned()
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ------------------------------------------------------------------ systemd

mod systemd {
    use super::*;

    pub fn user_manager_reachable() -> bool {
        quiet_command("systemctl")
            .args(["--user", "is-system-running"])
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                // "running", "degraded" — anything but a failure to reach the manager.
                !s.trim().is_empty()
                    && !String::from_utf8_lossy(&o.stderr).contains("Failed to connect")
            })
            .unwrap_or(false)
    }

    pub fn unit_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| crate::dirs_home().join(".config"));
        base.join("systemd").join("user")
    }

    pub fn unit_file(unit: &str) -> PathBuf {
        unit_dir().join(format!("{unit}.service"))
    }

    pub fn install(unit: &str, exe: &Path, data_dir: &Path, args: &[String]) -> Result<PathBuf> {
        let dir = unit_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = unit_file(unit);
        let exec: Vec<String> = std::iter::once(exe.to_string_lossy().into_owned())
            .chain(args.iter().cloned())
            .map(|a| systemd_quote(&a))
            .collect();
        // The user manager's PATH is not the login shell's: the harness
        // binaries (claude, codex) usually live under ~/.local/bin or a
        // version manager. Carry today's PATH along.
        let path_env = std::env::var("PATH").unwrap_or_default();
        let body = format!(
            "# Written by `aspen autostart enable` — edit and `systemctl --user daemon-reload`, or re-run enable.\n\
             [Unit]\n\
             Description=Aspen node ({})\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={}\n\
             Restart=on-failure\n\
             RestartSec=3\n\
             TimeoutStopSec=90\n\
             # Only the daemon is signalled on stop: it shuts its sessions down\n\
             # itself, and an updater it launched must outlive it.\n\
             KillMode=process\n\
             # The same log as `aspen up -d` writes, so `aspen logs` and the console's log view keep working.\n\
             StandardOutput=append:{}\n\
             StandardError=inherit\n\
             Environment=ASPEN_SUPERVISOR=systemd\n\
             Environment={}\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            data_dir.display(),
            exec.join(" "),
            data_dir.join("aspen.log").display(),
            systemd_quote(&format!("PATH={path_env}")),
        );
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        run_ok(cmd(&["daemon-reload"]), "systemctl --user daemon-reload")?;
        run_ok(
            cmd(&["enable", &format!("{unit}.service")]),
            "systemctl --user enable",
        )?;
        Ok(path)
    }

    pub fn uninstall(unit: &str) -> Result<()> {
        let svc = format!("{unit}.service");
        let _ = cmd(&["disable", &svc]).output();
        let path = unit_file(unit);
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
        let _ = cmd(&["daemon-reload"]).output();
        Ok(())
    }

    pub fn is_enabled(unit: &str) -> bool {
        cmd(&["is-enabled", &format!("{unit}.service")])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
            .unwrap_or(false)
    }

    pub fn linger_enabled() -> bool {
        let user = std::env::var("USER").unwrap_or_default();
        if user.is_empty() {
            return true;
        }
        quiet_command("loginctl")
            .args(["show-user", &user, "-p", "Linger", "--value"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
            .unwrap_or(true)
    }

    pub fn start(unit: &str) -> Result<()> {
        run_ok(
            cmd(&["start", &format!("{unit}.service")]),
            "systemctl --user start",
        )
    }

    pub fn stop(unit: &str) -> Result<()> {
        run_ok(
            cmd(&["stop", &format!("{unit}.service")]),
            "systemctl --user stop",
        )
    }

    fn cmd(args: &[&str]) -> std::process::Command {
        let mut c = quiet_command("systemctl");
        c.arg("--user").args(args);
        c
    }
}

// ------------------------------------------------------------------ launchd

mod launchd {
    use super::*;

    pub fn label(unit: &str) -> String {
        format!("dev.aspen.{unit}")
    }

    pub fn plist_file(unit: &str) -> PathBuf {
        crate::dirs_home()
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", label(unit)))
    }

    fn domain() -> String {
        #[cfg(unix)]
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 0;
        format!("gui/{uid}")
    }

    pub fn install(unit: &str, exe: &Path, data_dir: &Path, args: &[String]) -> Result<PathBuf> {
        let path = plist_file(unit);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        let mut argv = vec![exe.to_string_lossy().into_owned()];
        argv.extend(args.iter().cloned());
        let argv_xml: String = argv
            .iter()
            .map(|a| format!("      <string>{}</string>\n", xml_escape(a)))
            .collect();
        let log = data_dir.join("aspen.log");
        let path_env = std::env::var("PATH").unwrap_or_default();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \x20 <key>Label</key><string>{label}</string>\n\
             \x20 <key>ProgramArguments</key>\n\
             \x20 <array>\n{argv_xml}\x20 </array>\n\
             \x20 <key>RunAtLoad</key><true/>\n\
             \x20 <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
             \x20 <key>AbandonProcessGroup</key><true/>\n\
             \x20 <key>StandardOutPath</key><string>{log}</string>\n\
             \x20 <key>StandardErrorPath</key><string>{log}</string>\n\
             \x20 <key>EnvironmentVariables</key>\n\
             \x20 <dict>\n\
             \x20   <key>ASPEN_SUPERVISOR</key><string>launchd</string>\n\
             \x20   <key>PATH</key><string>{path}</string>\n\
             \x20 </dict>\n\
             </dict>\n\
             </plist>\n",
            label = label(unit),
            argv_xml = argv_xml,
            log = xml_escape(&log.to_string_lossy()),
            path = xml_escape(&path_env),
        );
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    pub fn bootstrap(unit: &str, plist: &Path) -> Result<()> {
        // Re-bootstrapping an already loaded job fails; boot it out first.
        let _ = quiet_command("launchctl")
            .args(["bootout", &format!("{}/{}", domain(), label(unit))])
            .output();
        let mut c = quiet_command("launchctl");
        c.args(["bootstrap", &domain()]).arg(plist);
        run_ok(c, "launchctl bootstrap")
    }

    pub fn uninstall(unit: &str) -> Result<()> {
        // Disable, not bootout: the running daemon is left alone.
        let _ = quiet_command("launchctl")
            .args(["disable", &format!("{}/{}", domain(), label(unit))])
            .output();
        let path = plist_file(unit);
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
        Ok(())
    }

    pub fn is_loaded(unit: &str) -> bool {
        plist_file(unit).exists()
            && quiet_command("launchctl")
                .args(["print", &format!("{}/{}", domain(), label(unit))])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }

    pub fn start(unit: &str) -> Result<()> {
        let mut c = quiet_command("launchctl");
        c.args(["kickstart", &format!("{}/{}", domain(), label(unit))]);
        run_ok(c, "launchctl kickstart")
    }

    pub fn stop(unit: &str) -> Result<()> {
        // A clean exit is not restarted (KeepAlive.SuccessfulExit=false).
        let mut c = quiet_command("launchctl");
        c.args(["kill", "TERM", &format!("{}/{}", domain(), label(unit))]);
        run_ok(c, "launchctl kill")
    }
}

// ----------------------------------------------------------------- schtasks

mod schtasks {
    use super::*;

    pub fn task_name(unit: &str) -> String {
        if unit == "aspen" {
            "Aspen".to_owned()
        } else {
            format!("Aspen ({unit})")
        }
    }

    fn ps(script: &str) -> std::process::Command {
        let mut c = quiet_command("powershell.exe");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        c
    }

    fn ps_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }

    pub fn install(unit: &str, exe: &Path, args: &[String]) -> Result<()> {
        // conhost --headless: a console program started by the scheduler
        // in the interactive session would otherwise flash a window.
        let argument = std::iter::once(exe.to_string_lossy().into_owned())
            .chain(args.iter().cloned())
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            "$a = New-ScheduledTaskAction -Execute 'conhost.exe' -Argument {arg}; \
             $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
             $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -MultipleInstances IgnoreNew; \
             Register-ScheduledTask -TaskName {name} -Action $a -Trigger $t -Settings $s -RunLevel Limited -Force | Out-Null",
            arg = ps_quote(&format!("--headless {argument}")),
            name = ps_quote(&task_name(unit)),
        );
        run_ok(ps(&script), "Register-ScheduledTask")
    }

    pub fn uninstall(unit: &str) -> Result<()> {
        let script = format!(
            "Unregister-ScheduledTask -TaskName {} -Confirm:$false -ErrorAction SilentlyContinue",
            ps_quote(&task_name(unit))
        );
        run_ok(ps(&script), "Unregister-ScheduledTask")
    }

    pub fn exists(unit: &str) -> bool {
        let script = format!("if (Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue) {{ 'yes' }} else {{ 'no' }}", ps_quote(&task_name(unit)));
        ps(&script)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
            .unwrap_or(false)
    }

    pub fn run(unit: &str) -> Result<()> {
        let script = format!(
            "Start-ScheduledTask -TaskName {}",
            ps_quote(&task_name(unit))
        );
        run_ok(ps(&script), "Start-ScheduledTask")
    }
}
