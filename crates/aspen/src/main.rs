//! aspen — the Aspen node daemon and CLI.
//!
//! P0: the node layer (session manager, bus store, delivery engine) driven
//! by dev harness commands. The daemon proper (API/WS, SPA) lands next.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use aspen_core::SessionEvent;
use aspen_node::{Node, SpawnOpts};

mod api;
mod meshops;
mod status;
mod update;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ASPEN_GIT_SHA"),
    ", built ",
    env!("ASPEN_BUILD_DATE"),
    ")"
);

#[derive(Parser)]
#[command(name = "aspen", version, long_version = LONG_VERSION, about = "Aspen node daemon")]
struct Cli {
    /// Node data directory (bus store, config).
    #[arg(long, global = true, default_value_os_t = default_data_dir())]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

fn default_data_dir() -> PathBuf {
    dirs_home().join(".aspen")
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Subcommand)]
enum Command {
    /// Run the node: API + web console on localhost.
    Up {
        /// Listen address for the API and console. Default 127.0.0.1:7420;
        /// headless nodes default to an ephemeral port (port 0 = let the OS
        /// pick — the actual address lands in daemon.json for status/CLI).
        #[arg(long)]
        listen: Option<std::net::SocketAddr>,
        /// Directory with the built SPA (defaults to ui/dist next to the
        /// binary's source tree, if present).
        #[arg(long)]
        ui: Option<PathBuf>,
        /// Run detached in the background; logs to <data-dir>/aspen.log.
        #[arg(short = 'd', long)]
        detach: bool,
        /// Skip auto-reviving the sessions that were live at last shutdown.
        #[arg(long)]
        no_resume: bool,
        /// API only — don't serve the console (e.g. a second node on the
        /// same machine where one console is enough).
        #[arg(long)]
        headless: bool,
        /// Force the console on, overriding a headless default from config.
        #[arg(long, conflicts_with = "headless")]
        no_headless: bool,
    },
    /// Stop a detached node started with `up -d`.
    Down,
    /// Restart the daemon in the same mode and port it's running in.
    Restart,
    /// Show node status: daemon, sessions, mesh.
    Status,
    /// Get or set daemon start defaults (headless, listen), harness args,
    /// and the update policy.
    Config {
        /// Setting name: headless | listen | topology | claude-args |
        /// update | update-window | update-soak | update-skip | update-check.
        /// Omit to list all.
        key: Option<String>,
        /// New value. Omit to show the current value; "-" clears it.
        value: Option<String>,
    },
    /// Update aspen in place from the latest release (or a given version).
    Update {
        /// Install a specific version (tag), e.g. v0.2.0. Default: latest.
        #[arg(long)]
        version: Option<String>,
        /// Reinstall even if the version matches.
        #[arg(long)]
        force: bool,
        /// Restart a running daemon on the new binary after updating
        /// (sessions are revived automatically on startup). The new daemon
        /// is health-checked; if it doesn't come up, the previous binary is
        /// put back.
        #[arg(long)]
        restart: bool,
        /// Only report what the release channel has; change nothing.
        #[arg(long, conflicts_with_all = ["force", "restart", "rollback"])]
        check: bool,
        /// Put the previous binary back (the rollback slot kept by the last
        /// update) and restart a running daemon onto it.
        #[arg(long, conflicts_with_all = ["force", "version"])]
        rollback: bool,
        /// Launched by the daemon (policy or an operator's request); no
        /// behavior change beyond labeling the outcome.
        #[arg(long, hide = true)]
        unattended: bool,
        /// Who asked (policy | operator | peer:<node>), for the outcome.
        #[arg(long, hide = true, default_value = "cli")]
        trigger: String,
    },
    /// Claude Code hook relay (installed by `aspen hooks install`): reads the
    /// hook's JSON on stdin and nudges the local daemon. Always exits 0.
    #[command(hide = true)]
    Hook,
    /// Claude Code hooks that let Aspen notice sessions started, resumed, or
    /// forked outside it (SessionStart/SessionEnd → the local daemon).
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    /// Tail the daemon log (<data-dir>/aspen.log).
    Logs {
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// Developer harness commands against a live agent runtime.
    Dev {
        #[command(subcommand)]
        command: DevCommand,
    },
    /// Bus inspection.
    Bus {
        #[command(subcommand)]
        command: BusCommand,
    },
    /// Repo registry operations.
    Repos {
        #[command(subcommand)]
        command: ReposCommand,
    },
    /// Mesh membership: identity, certification, peers.
    Mesh {
        #[command(subcommand)]
        command: MeshCommand,
    },
}

#[derive(Subcommand)]
enum HooksCommand {
    /// Add the SessionStart/SessionEnd hooks to ~/.claude/settings.json.
    Install,
    /// Remove them.
    Uninstall,
    /// Show whether they are installed.
    Status,
}

#[derive(Subcommand)]
enum ReposCommand {
    /// Find repos from Claude Code's session store (~/.claude/projects)
    /// and add them to this node's registry.
    Discover,
}

#[derive(Subcommand)]
enum MeshCommand {
    /// Create a new mesh here: root key + this node's certified identity.
    Init {
        /// Mesh name.
        #[arg(long)]
        mesh: String,
        /// This node's name (defaults to hostname).
        #[arg(long)]
        node: Option<String>,
    },
    /// On a new node: generate identity, print the enroll blob for the
    /// root holder.
    Enroll {
        #[arg(long)]
        node: Option<String>,
    },
    /// Where the root key lives: turn an enroll blob into a join bundle
    /// (the cert plus this node's cert/URL/relay so `join` wires the link).
    Certify {
        blob: String,
        /// How the joining node reaches THIS node, e.g.
        /// ws://host:7420/api/federation/ws. Omit if this node will dial
        /// the joiner instead (then add it with `peers-add --url` here).
        #[arg(long)]
        url: Option<String>,
    },
    /// On the new node: install a join bundle (or bare cert blob).
    Join { blob: String },
    /// Tell the running daemon to re-read mesh files (done automatically
    /// after every mesh command; use this if the daemon was down then).
    Reload,
    /// Execute the mesh changes the console queued (mesh-pending.json),
    /// reviewing each one. This is the shell-side half of the ceremony:
    /// the console authors, a shell with the keys applies.
    Apply {
        /// Apply everything without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Print this node's cert blob (give it to peers via `peers add`).
    Export,
    /// Register a peer's cert blob, optionally with a dial URL.
    PeersAdd {
        blob: String,
        /// ws://host:port/api/federation/ws — omit if the peer dials us.
        #[arg(long)]
        url: Option<String>,
    },
    /// Forget a peer (stop dialing/accepting it here). Its cert is not
    /// revoked — remove it on every node that lists it.
    PeersRemove { node: String },
    /// Leave the mesh: drop membership and this node's cert; keep the
    /// keypair for a future enroll.
    Leave {
        /// This node holds the root key: delete it too, ending the mesh.
        #[arg(long)]
        discard_root: bool,
    },
    /// Print this mesh's ROOT PUBLIC key (safe to give a relay).
    RootPubkey,
    /// Set (or clear) the rendezvous relay URL this node dials.
    Relay {
        /// wss://host/relay — omit to clear.
        url: Option<String>,
    },
    /// Show mesh membership as configured on disk.
    Status,
}

#[derive(Subcommand)]
enum DevCommand {
    /// Spawn one agent, send one prompt, stream the turn, shut down.
    Oneshot {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        allow_all: bool,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        resume: Option<String>,
        /// Agent name on the bus.
        #[arg(long, default_value = "dev")]
        name: String,
    },
    /// Interactive chat with a spawned agent (terminal REPL).
    Chat {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        allow_all: bool,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        resume: Option<String>,
        #[arg(long, default_value = "dev")]
        name: String,
    },
    /// Two agents in one repo talking over the bus — the P0 proof.
    Duo {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        model: Option<String>,
        /// Seconds to wait for the round trip before giving up.
        #[arg(long, default_value_t = 240)]
        timeout: u64,
    },
}

#[derive(Subcommand)]
enum BusCommand {
    /// The lookback: chronology, delivery state, ingestion acks.
    Log {
        #[arg(short = 'n', long, default_value_t = 30)]
        lines: i64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Die quietly when a pipe closes (`aspen status | head`), like other
    // unix CLIs, instead of panicking on the failed println.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aspen=info,aspen_claude=info,aspen_node=info".into()),
        )
        .with_writer(std::io::stderr)
        // Detached daemons log to a file: no color codes there.
        .with_ansi(std::env::var_os("ASPEN_DETACHED").is_none())
        .init();

    let _ = aspen_node::federation::VERSION.set((
        env!("CARGO_PKG_VERSION").to_owned(),
        env!("ASPEN_GIT_SHA").to_owned(),
    ));
    let cli = Cli::parse();
    match cli.command {
        Command::Up {
            listen,
            ui,
            detach,
            no_resume,
            headless,
            no_headless,
        } => {
            let cfg = aspen_node::settings::load(&cli.data_dir).daemon;
            // headless: explicit flags win, else the configured default.
            let headless = if headless {
                true
            } else if no_headless {
                false
            } else {
                cfg.headless.unwrap_or(false)
            };
            // listen: --listen wins, else config, else the built-in default.
            // Headless nodes take an ephemeral port (the fixed port exists
            // for the console; WSL2 forwards Linux listeners onto Windows
            // localhost, so fixed ports collide across the sides).
            let listen = listen
                .or_else(|| cfg.listen.as_deref().and_then(|s| s.parse().ok()))
                .unwrap_or_else(|| {
                    let port = if headless { 0 } else { 7420 };
                    std::net::SocketAddr::from(([127, 0, 0, 1], port))
                });
            // Detached start: re-exec self in a new session, redirect output
            // to a log file, record the pid, and return.
            if detach && std::env::var_os("ASPEN_DETACHED").is_none() {
                return spawn_detached(&cli.data_dir, listen, ui.as_deref(), no_resume, headless);
            }
            let node = Node::open(&cli.data_dir)?;
            if let Some(d) = &ui {
                eprintln!("[aspen] serving console from {}", d.display());
            }
            if !no_resume {
                auto_revive(&cli.data_dir, node.clone());
            }
            // Servicing: report the last updater's outcome, learn the
            // harness version, start the release check + drain loops.
            aspen_node::servicing::on_start(&node.inner);
            aspen_node::servicing::spawn_loops(node.inner.clone());
            // Adoption: notice forks/resumes of our agents' sessions made
            // outside Aspen (docs/SERVICING.md-adjacent; see adoption.rs).
            aspen_node::adoption::spawn_scanner(node.inner.clone(), 15);
            // daemon.json is written by serve() once the port is bound and
            // removed here only if it is still ours — a failed bind must not
            // clobber a healthy daemon's state file.
            let result = api::serve(node, listen, ui, headless, &cli.data_dir).await;
            let ours = read_daemon_state(&cli.data_dir)
                .and_then(|s| s.get("pid").and_then(|p| p.as_u64()))
                == Some(std::process::id() as u64);
            if ours {
                remove_daemon_state(&cli.data_dir);
            }
            result
        }
        Command::Down => stop_detached(&cli.data_dir),
        Command::Restart => restart_daemon(&cli.data_dir),
        Command::Config { key, value } => config_command(&cli.data_dir, key, value),
        Command::Status => status::run(&cli.data_dir),
        Command::Repos { command } => match command {
            ReposCommand::Discover => {
                let node = Node::open(&cli.data_dir)?;
                let found = node.discover_repos()?;
                if found.is_empty() {
                    println!("no repos found under ~/.claude/projects");
                }
                for (path, sessions, added) in found {
                    println!(
                        "{} {}  ({} session{})",
                        if added { "added " } else { "known " },
                        path.display(),
                        sessions,
                        if sessions == 1 { "" } else { "s" },
                    );
                }
                Ok(())
            }
        },
        Command::Update {
            version,
            force,
            restart,
            check,
            rollback,
            unattended,
            trigger,
        } => update::run(
            &cli.data_dir,
            update::Opts {
                version: version.as_deref(),
                force,
                restart,
                unattended,
                trigger: &trigger,
                rollback,
                check,
            },
        ),
        Command::Hook => hook_relay(&cli.data_dir),
        Command::Hooks { command } => hooks_command(&cli.data_dir, command),
        Command::Logs { lines } => {
            for l in aspen_node::servicing::tail_log(&cli.data_dir, lines) {
                println!("{l}");
            }
            Ok(())
        }
        Command::Dev { command } => match command {
            DevCommand::Oneshot {
                repo,
                prompt,
                allow_all,
                model,
                resume,
                name,
            } => {
                let node = Node::open(&cli.data_dir)?;
                dev_oneshot(&node, name, repo, prompt, allow_all, model, resume).await
            }
            DevCommand::Chat {
                repo,
                allow_all,
                model,
                resume,
                name,
            } => {
                let node = Node::open(&cli.data_dir)?;
                dev_chat(&node, name, repo, allow_all, model, resume).await
            }
            DevCommand::Duo {
                repo,
                model,
                timeout,
            } => {
                let node = Node::open(&cli.data_dir)?;
                dev_duo(&node, repo, model, timeout).await
            }
        },
        Command::Bus { command } => match command {
            BusCommand::Log { lines } => bus_log(&cli.data_dir, lines),
        },
        Command::Mesh { command } => mesh_command(&cli.data_dir, command),
    }
}

fn pidfile(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("aspen.pid")
}

fn state_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("daemon.json")
}

/// How the daemon was started — enough for `update --restart` to relaunch
/// it identically. The legacy aspen.pid is kept for older tooling.
pub(crate) fn write_daemon_state(
    data_dir: &std::path::Path,
    listen: std::net::SocketAddr,
    requested: std::net::SocketAddr,
    ui: Option<&std::path::Path>,
    headless: bool,
) {
    std::fs::create_dir_all(data_dir).ok();
    let state = serde_json::json!({
        "pid": std::process::id(),
        "listen": listen.to_string(),
        // What was asked for (may be port 0 = ephemeral) — restart re-uses
        // this, not the specific port the OS happened to hand out.
        "requested": requested.to_string(),
        "ui": ui.map(|p| p.to_string_lossy()),
        "headless": headless,
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    let _ = std::fs::write(state_path(data_dir), state.to_string());
    let _ = std::fs::write(pidfile(data_dir), std::process::id().to_string());
}

pub(crate) fn remove_daemon_state(data_dir: &std::path::Path) {
    let _ = std::fs::remove_file(state_path(data_dir));
    let _ = std::fs::remove_file(pidfile(data_dir));
}

pub(crate) fn read_daemon_state(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(state_path(data_dir)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Revive whatever was live when the daemon last went away — cleanly
/// (`aspen down`, `update --restart`) or not (crash, power). The agents
/// table's `live` mark is maintained continuously, so no shutdown-time
/// ledger is needed. Sessions the operator stopped, or that exited on
/// their own, are not marked and stay down.
fn auto_revive(data_dir: &std::path::Path, node: Node) {
    // Legacy ledger from pre-0.1.7 daemons: honor it once, then retire it.
    let legacy = data_dir.join("resume.json");
    let mut names: Vec<String> = std::fs::read_to_string(&legacy)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&legacy);
    for n in node.inner.store.agents_marked_live().unwrap_or_default() {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    if names.is_empty() {
        return;
    }
    tokio::spawn(async move {
        // A beat for the listener to come up before sessions start talking.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        for name in names {
            match node.revive_agent(&name, true).await {
                Ok(_) => eprintln!("[aspen] revived @{name}"),
                Err(e) => eprintln!("[aspen] could not revive @{name}: {e:#}"),
            }
        }
    });
}

/// Re-exec self detached (new session, output to a log file), record the
/// child pid, and return. The child sees ASPEN_DETACHED and runs normally.
/// Restart the running daemon in the same mode and (requested) port. Reads
/// daemon.json, stops, waits for the port to free, and starts detached with
/// the same binary and parameters.
fn restart_daemon(data_dir: &std::path::Path) -> Result<()> {
    let state = read_daemon_state(data_dir)
        .ok_or_else(|| anyhow::anyhow!("no running daemon found (nothing to restart)"))?;
    // The requested address (may be port 0 = ephemeral), not the specific
    // port the OS handed out.
    let listen: std::net::SocketAddr = state["requested"]
        .as_str()
        .or_else(|| state["listen"].as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| std::net::SocketAddr::from(([127, 0, 0, 1], 7420)));
    let ui = state["ui"].as_str().map(PathBuf::from);
    let headless = state["headless"].as_bool().unwrap_or(false);

    println!("stopping daemon …");
    stop_detached(data_dir)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while data_dir.join("daemon.json").exists() {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("daemon did not shut down within 30s; start it with `aspen up -d`");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    spawn_detached(data_dir, listen, ui.as_deref(), false, headless)
}

/// Get or set daemon start defaults (settings.json). Keys: headless, listen,
/// claude-args.
fn config_command(
    data_dir: &std::path::Path,
    key: Option<String>,
    value: Option<String>,
) -> Result<()> {
    use aspen_node::settings;
    let mut s = settings::load(data_dir);

    let show_all = || {
        println!(
            "headless    {}",
            s.daemon
                .headless
                .map(|b| b.to_string())
                .unwrap_or_else(|| "(unset → false)".into())
        );
        println!(
            "listen      {}",
            s.daemon
                .listen
                .clone()
                .unwrap_or_else(|| "(unset → 127.0.0.1:7420, or ephemeral if headless)".into())
        );
        println!(
            "topology    {}",
            s.topology
                .clone()
                .unwrap_or_else(|| "(unset → open)".into())
        );
        println!(
            "claude-args {}",
            match s.harness.get("claude").map(|h| h.args.as_str()) {
                Some(a) if !a.is_empty() => a.to_owned(),
                _ => "(none)".into(),
            }
        );
        let u = &s.update;
        println!(
            "update      {}   (notify: check + badge, never apply; auto: apply when quiet)",
            u.mode.clone().unwrap_or_else(|| "(unset → notify)".into())
        );
        println!(
            "update-window {}   (HH:MM-HH:MM local; auto only fires inside it)",
            u.window.clone().unwrap_or_else(|| "(none)".into())
        );
        println!(
            "update-soak {}   (minimum release age before auto applies, e.g. 24h)",
            u.soak.clone().unwrap_or_else(|| "(none)".into())
        );
        println!(
            "update-skip {}   (a version to ignore)",
            u.skip.clone().unwrap_or_else(|| "(none)".into())
        );
        println!(
            "update-check {}   (false = never contact the release channel)",
            u.check
                .map(|b| b.to_string())
                .unwrap_or_else(|| "(unset → true)".into())
        );
    };

    let Some(key) = key else {
        show_all();
        return Ok(());
    };

    // No value → show just this key by listing all (simple + consistent).
    let Some(value) = value else {
        show_all();
        return Ok(());
    };
    let clear = value == "-";

    match key.as_str() {
        "headless" => {
            s.daemon.headless = if clear {
                None
            } else {
                Some(
                    value
                        .parse::<bool>()
                        .map_err(|_| anyhow::anyhow!("headless takes true or false"))?,
                )
            };
        }
        "listen" => {
            s.daemon.listen = if clear {
                None
            } else {
                // Validate now so a typo fails here, not at next `up`.
                value
                    .parse::<std::net::SocketAddr>()
                    .map_err(|e| anyhow::anyhow!("listen must be host:port ({e})"))?;
                Some(value.clone())
            };
        }
        "topology" => {
            s.topology = if clear {
                None
            } else if value == "open" || value == "closed" {
                Some(value.clone())
            } else {
                anyhow::bail!("topology takes open or closed");
            };
        }
        "claude-args" => {
            settings::split_args(if clear { "" } else { &value }, None)?;
            s.harness.entry("claude".into()).or_default().args =
                if clear { String::new() } else { value.clone() };
        }
        "update" | "update-mode" => {
            s.update.mode = if clear { None } else { Some(value.clone()) };
        }
        "update-window" => {
            s.update.window = if clear { None } else { Some(value.clone()) };
        }
        "update-soak" => {
            s.update.soak = if clear { None } else { Some(value.clone()) };
        }
        "update-skip" => {
            s.update.skip = if clear {
                None
            } else {
                Some(value.trim_start_matches('v').to_owned())
            };
        }
        "update-check" => {
            s.update.check = if clear {
                None
            } else {
                Some(
                    value
                        .parse::<bool>()
                        .map_err(|_| anyhow::anyhow!("update-check takes true or false"))?,
                )
            };
        }
        other => {
            anyhow::bail!(
                "unknown setting '{other}' (headless | listen | topology | claude-args | update | update-window | update-soak | update-skip | update-check)"
            )
        }
    }
    s.update.validate()?;
    settings::save(data_dir, &s)?;
    println!("set {key} = {}", if clear { "(cleared)" } else { &value });
    Ok(())
}

/// `aspen hook`: forward Claude Code's hook payload to the daemon. Quiet and
/// fast — this runs on the session's start path. Aspen's own sessions are
/// skipped (their environment carries our entrypoint).
fn hook_relay(data_dir: &std::path::Path) -> Result<()> {
    if std::env::var("CLAUDE_CODE_ENTRYPOINT").as_deref() == Ok("aspen") {
        return Ok(());
    }
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let body: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::json!({}));
    let Some(state) = read_daemon_state(data_dir) else {
        return Ok(());
    };
    let Some(listen) = state["listen"].as_str() else {
        return Ok(());
    };
    let mut req = ureq::post(&format!("http://{listen}/api/hooks/session"))
        .timeout(std::time::Duration::from_secs(2));
    let loopback = listen
        .parse::<std::net::SocketAddr>()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(true);
    if !loopback {
        if let Ok(tok) = std::fs::read_to_string(data_dir.join("api-token")) {
            req = req.set("X-Aspen-Token", tok.trim());
        }
    }
    let _ = req.send_json(body);
    Ok(())
}

fn claude_settings_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".claude"))
        .join("settings.json")
}

/// The hook command line: this binary, this data dir, `hook`.
fn hook_command_line(data_dir: &std::path::Path) -> Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!(
        "\"{}\" --data-dir \"{}\" hook",
        exe.display(),
        data_dir.display()
    ))
}

fn is_our_hook(entry: &serde_json::Value) -> bool {
    entry["hooks"].as_array().is_some_and(|hs| {
        hs.iter().any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.contains("aspen") && c.trim_end().ends_with(" hook"))
        })
    })
}

fn hooks_command(data_dir: &std::path::Path, cmd: HooksCommand) -> Result<()> {
    let path = claude_settings_path();
    let mut settings: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !settings.is_object() {
        anyhow::bail!("{} is not a JSON object", path.display());
    }
    let events = ["SessionStart", "SessionEnd"];
    match cmd {
        HooksCommand::Status => {
            for ev in events {
                let installed = settings["hooks"][ev]
                    .as_array()
                    .is_some_and(|a| a.iter().any(is_our_hook));
                println!(
                    "{ev:<13} {}",
                    if installed {
                        "installed"
                    } else {
                        "not installed"
                    }
                );
            }
            println!("settings: {}", path.display());
            Ok(())
        }
        HooksCommand::Install => {
            let line = hook_command_line(data_dir)?;
            let hooks = settings
                .as_object_mut()
                .unwrap()
                .entry("hooks")
                .or_insert_with(|| serde_json::json!({}));
            if !hooks.is_object() {
                anyhow::bail!("settings.hooks is not an object");
            }
            for ev in events {
                let arr = hooks
                    .as_object_mut()
                    .unwrap()
                    .entry(ev)
                    .or_insert_with(|| serde_json::json!([]));
                let Some(list) = arr.as_array_mut() else {
                    anyhow::bail!("settings.hooks.{ev} is not an array");
                };
                list.retain(|e| !is_our_hook(e));
                list.push(serde_json::json!({
                    "hooks": [{ "type": "command", "command": line, "timeout": 5 }]
                }));
            }
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
            println!(
                "installed SessionStart + SessionEnd hooks in {}",
                path.display()
            );
            println!("  {line}");
            println!("Sessions started, resumed, or forked outside Aspen now reach the daemon within a second (new claude processes only).");
            Ok(())
        }
        HooksCommand::Uninstall => {
            let mut removed = 0;
            if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                for ev in events {
                    if let Some(list) = hooks.get_mut(ev).and_then(|a| a.as_array_mut()) {
                        let before = list.len();
                        list.retain(|e| !is_our_hook(e));
                        removed += before - list.len();
                        if list.is_empty() {
                            hooks.remove(ev);
                        }
                    }
                }
                if hooks.is_empty() {
                    settings.as_object_mut().unwrap().remove("hooks");
                }
            }
            std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
            println!(
                "removed {removed} hook entr{} from {}",
                if removed == 1 { "y" } else { "ies" },
                path.display()
            );
            Ok(())
        }
    }
}

fn spawn_detached(
    data_dir: &std::path::Path,
    listen: std::net::SocketAddr,
    ui: Option<&std::path::Path>,
    no_resume: bool,
    headless: bool,
) -> Result<()> {
    use std::process::{Command, Stdio};

    std::fs::create_dir_all(data_dir).ok();
    let log_path = data_dir.join("aspen.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_err = log.try_clone()?;

    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--data-dir")
        .arg(data_dir)
        .arg("up")
        .arg("--listen")
        .arg(listen.to_string());
    if let Some(ui) = ui {
        cmd.arg("--ui").arg(ui);
    }
    if no_resume {
        cmd.arg("--no-resume");
    }
    // Pin the resolved headless decision so the re-exec'd child doesn't
    // re-derive it from config (which could flip a --no-headless back on).
    cmd.arg(if headless {
        "--headless"
    } else {
        "--no-headless"
    });
    cmd.env("ASPEN_DETACHED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    // Detach from the controlling terminal so closing it doesn't SIGHUP us.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    let mut child = cmd.spawn()?;

    // Don't declare success until the child proves it: daemon.json is only
    // written after the listener binds, so wait for it to carry the child's
    // pid — and surface the log if the child dies instead (e.g. the port is
    // already taken, which on WSL2 includes ports the *other* side of the
    // machine forwards).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if read_daemon_state(data_dir).and_then(|s| s.get("pid").and_then(|p| p.as_u64()))
            == Some(child.id() as u64)
        {
            break;
        }
        if let Some(status) = child.try_wait()? {
            eprintln!("aspen failed to start (exit {status}). Last log lines:");
            print_log_tail(&log_path, 6);
            anyhow::bail!(
                "daemon did not start — if the port is in use, pick another with `aspen up -d --listen 127.0.0.1:<port>`"
            );
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!(
                "daemon (pid {}) has not confirmed startup after 15s — check {}",
                child.id(),
                log_path.display()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    std::fs::write(pidfile(data_dir), child.id().to_string())?;
    // Port 0 means the OS chose — read the actual address the child bound.
    let actual = read_daemon_state(data_dir)
        .and_then(|s| s.get("listen").and_then(|l| l.as_str().map(str::to_owned)))
        .unwrap_or_else(|| listen.to_string());
    println!(
        "aspen started detached (pid {}) on http://{actual}\n  logs: {}\n  stop: aspen --data-dir {} down",
        child.id(),
        log_path.display(),
        data_dir.display(),
    );
    Ok(())
}

fn print_log_tail(log_path: &std::path::Path, n: usize) {
    if let Ok(text) = std::fs::read_to_string(log_path) {
        let lines: Vec<&str> = text.lines().collect();
        for line in lines.iter().rev().take(n).rev() {
            eprintln!("  | {line}");
        }
    }
}

/// Is a process with this pid alive? Unix: kill(0). Windows: tasklist.
pub(crate) fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(false)
    }
}

/// Stop the daemon. Graceful first — POST /api/shutdown runs the same
/// ladder as SIGTERM on every platform (Windows has no SIGTERM, and a
/// detached process has no window for taskkill to close). Falls back to a
/// forced kill only when the API doesn't answer. State is cleared only once
/// the process is confirmed gone; a daemon we failed to stop stays visible.
pub(crate) fn stop_detached(data_dir: &std::path::Path) -> Result<()> {
    let path = pidfile(data_dir);
    let state = read_daemon_state(data_dir);
    let pid: u32 = state
        .as_ref()
        .and_then(|s| s.get("pid").and_then(|p| p.as_u64()))
        .map(|p| p as u32)
        .or_else(|| {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
        })
        .ok_or_else(|| anyhow::anyhow!("no running detached node found ({})", path.display()))?;

    if !process_alive(pid) {
        remove_daemon_state(data_dir);
        anyhow::bail!("process {pid} not running (cleared stale state)");
    }

    // 1. Graceful, over the API.
    let listen = state
        .as_ref()
        .and_then(|s| s["listen"].as_str().map(str::to_owned));
    let mut graceful = false;
    if let Some(listen) = &listen {
        let mut req = ureq::post(&format!("http://{listen}/api/shutdown"))
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
        graceful = req.send_json(serde_json::json!({})).is_ok();
    }

    // 2. Fallback: signal (unix = still graceful) / forced kill (windows).
    if !graceful {
        #[cfg(unix)]
        {
            if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
                anyhow::bail!("could not signal process {pid}");
            }
        }
        #[cfg(windows)]
        {
            // /T takes the session processes down too (they revive via
            // their live marks on the next `up`).
            let ok = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                anyhow::bail!(
                    "could not stop process {pid} (try Task Manager / `taskkill /F /T /PID {pid}`)"
                );
            }
        }
    }

    // 3. Wait for it to actually go. On a clean exit the daemon removes its
    // own state file; after a forced kill we clean up for it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while process_alive(pid) {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("process {pid} still running after 30s");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    if !graceful {
        remove_daemon_state(data_dir);
    }
    println!(
        "aspen node (pid {pid}) stopped{}",
        if graceful { "" } else { " (forced)" }
    );
    Ok(())
}

/// After a mesh mutation, ask a running daemon to apply it live. Returns
/// whether a daemon was reached. Never fatal: the files are the source of
/// truth and the next `aspen up` reads them anyway.
fn notify_daemon_reload(data_dir: &std::path::Path) -> bool {
    let Some(state) = read_daemon_state(data_dir) else {
        return false;
    };
    let Some(listen) = state["listen"].as_str() else {
        return false;
    };
    let mut req = ureq::post(&format!("http://{listen}/api/mesh/reload"))
        .timeout(std::time::Duration::from_secs(5));
    let loopback = listen
        .parse::<std::net::SocketAddr>()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(true);
    if !loopback {
        if let Ok(tok) = std::fs::read_to_string(data_dir.join("api-token")) {
            req = req.set("X-Aspen-Token", tok.trim());
        }
    }
    match req.send_json(serde_json::json!({})) {
        Ok(resp) => {
            let v: serde_json::Value = resp.into_json().unwrap_or_default();
            println!(
                "daemon: {}",
                v["summary"].as_str().unwrap_or("mesh reloaded")
            );
            true
        }
        Err(e) => {
            println!("(daemon not reloaded: {e} — it reads mesh files at next start, or run `aspen mesh reload`)");
            false
        }
    }
}

pub(crate) fn relative(epoch: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(epoch);
    let s = (now - epoch).max(0.0) as u64;
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else {
        format!("{}h ago", s / 3600)
    }
}

fn mesh_command(data_dir: &std::path::Path, cmd: MeshCommand) -> Result<()> {
    use aspen_node::mesh::MeshFiles;
    use aspen_wire::identity;

    let files = MeshFiles::new(data_dir);
    let default_node = || {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                s.trim()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "node".into())
    };

    match cmd {
        MeshCommand::Init { mesh, node } => {
            let node_name = node.unwrap_or_else(default_node);
            let d = meshops::init(&files, &mesh, &node_name)?;
            println!("{}.", d.summary);
            println!("The root key IS the mesh: never copy it to nodes that don't certify.");
            println!(
                "\nThis node's cert blob (for `aspen mesh peers-add` on other nodes; re-print anytime with `aspen mesh export`):\n{}",
                d.artifact.unwrap_or_default()
            );
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::PeersRemove { node } => {
            let d = meshops::peers_remove(&files, &node)?;
            println!("{}", d.summary);
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::Leave { discard_root } => {
            let d = meshops::leave(&files, discard_root)?;
            println!("{}", d.summary);
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::Enroll { node } => {
            let node_name = node.unwrap_or_else(default_node);
            let d = meshops::enroll(&files, &node_name)?;
            println!("{}:\n{}", d.summary, d.artifact.unwrap_or_default());
            Ok(())
        }
        MeshCommand::Certify { blob, url } => {
            let d = meshops::certify(&files, &blob, url.as_deref())?;
            println!("{}\n\n{}", d.summary, d.artifact.unwrap_or_default());
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::Join { blob } => {
            let d = meshops::join(&files, &blob)?;
            println!("{}", d.summary);
            println!(
                "(this node's cert blob is saved — print it any time with `aspen mesh export`)"
            );
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::Reload => {
            if !notify_daemon_reload(data_dir) {
                println!("no running daemon to reload (it reads mesh files at startup).");
            }
            Ok(())
        }
        MeshCommand::Export => {
            let id = files
                .load_identity()?
                .ok_or_else(|| anyhow::anyhow!("no identity on this node"))?;
            let cert = id
                .cert
                .ok_or_else(|| anyhow::anyhow!("this node is not certified yet"))?;
            println!("{}", identity::to_blob("cert", &cert)?);
            Ok(())
        }
        MeshCommand::PeersAdd { blob, url } => {
            let d = meshops::peers_add(&files, &blob, url.as_deref())?;
            println!("{}", d.summary);
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::RootPubkey => {
            let mesh = files
                .load_mesh()?
                .ok_or_else(|| anyhow::anyhow!("this node has not joined a mesh"))?;
            println!("{}", aspen_wire::b64::encode(&mesh.root_public));
            Ok(())
        }
        MeshCommand::Relay { url } => {
            let d = meshops::relay(&files, url.as_deref())?;
            println!("{}", d.summary);
            notify_daemon_reload(data_dir);
            Ok(())
        }
        MeshCommand::Apply { yes } => {
            use aspen_node::pending;
            let q = pending::load(data_dir);
            if q.proposals.is_empty() {
                println!(
                    "nothing queued (the console queues mesh changes here; see the Mesh page)."
                );
                return Ok(());
            }
            println!(
                "{} queued mesh change(s) from the console:",
                q.proposals.len()
            );
            let mut any = false;
            for p in &q.proposals {
                let g = |k: &str| p.args.get(k).and_then(|v| v.as_str()).map(str::to_owned);
                let describe = match p.kind.as_str() {
                    "enroll" => format!(
                        "enroll this node as '{}'",
                        g("node").unwrap_or_else(&default_node)
                    ),
                    "certify" => format!(
                        "certify the enroll blob ({}…){}",
                        g("blob")
                            .map(|b| b.chars().take(24).collect::<String>())
                            .unwrap_or_default(),
                        g("url")
                            .map(|u| format!(" with dial URL {u}"))
                            .unwrap_or_default()
                    ),
                    "join" => format!(
                        "join with the bundle ({}…)",
                        g("blob")
                            .map(|b| b.chars().take(24).collect::<String>())
                            .unwrap_or_default()
                    ),
                    "peers_add" => format!(
                        "register a peer{}",
                        g("url")
                            .map(|u| format!(" dialing {u}"))
                            .unwrap_or_default()
                    ),
                    "relay" => format!(
                        "set relay to {}",
                        g("url").unwrap_or_else(|| "(cleared)".into())
                    ),
                    "init" => format!(
                        "CREATE mesh '{}' here as node '{}' (mints the root key on this machine)",
                        g("mesh").unwrap_or_default(),
                        g("node").unwrap_or_else(&default_node)
                    ),
                    "peers_remove" => format!("forget peer '{}'", g("node").unwrap_or_default()),
                    "leave" => format!(
                        "LEAVE the mesh{}",
                        if p.args.get("discard_root").and_then(|b| b.as_bool()) == Some(true) {
                            " and DISCARD THE ROOT KEY (ends the mesh)"
                        } else {
                            ""
                        }
                    ),
                    other => format!("unknown kind {other}"),
                };
                println!(
                    "\n  [{}] {} — queued {}",
                    p.id,
                    describe,
                    relative(p.created_at)
                );
                if !yes {
                    print!("  apply? [y/N] ");
                    std::io::stdout().flush().ok();
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).ok();
                    if !matches!(line.trim(), "y" | "Y" | "yes") {
                        println!(
                            "  skipped (still queued; withdraw it in the console or apply later)"
                        );
                        continue;
                    }
                }
                let result = match p.kind.as_str() {
                    "enroll" => meshops::enroll(&files, &g("node").unwrap_or_else(&default_node)),
                    "certify" => meshops::certify(
                        &files,
                        &g("blob").unwrap_or_default(),
                        g("url").as_deref(),
                    ),
                    "join" => meshops::join(&files, &g("blob").unwrap_or_default()),
                    "peers_add" => meshops::peers_add(
                        &files,
                        &g("blob").unwrap_or_default(),
                        g("url").as_deref(),
                    ),
                    "relay" => meshops::relay(&files, g("url").as_deref()),
                    "init" => meshops::init(
                        &files,
                        &g("mesh").unwrap_or_default(),
                        &g("node").unwrap_or_else(&default_node),
                    ),
                    "peers_remove" => meshops::peers_remove(&files, &g("node").unwrap_or_default()),
                    "leave" => meshops::leave(
                        &files,
                        p.args.get("discard_root").and_then(|b| b.as_bool()) == Some(true),
                    ),
                    other => Err(anyhow::anyhow!("unknown proposal kind {other}")),
                };
                match result {
                    Ok(d) => {
                        println!("  ✓ {}", d.summary);
                        if let Some(a) = &d.artifact {
                            println!("  artifact (also shown in the console):\n  {a}");
                        }
                        pending::settle(data_dir, &p.id, true, d.summary, d.artifact)?;
                        any = true;
                    }
                    Err(e) => {
                        println!("  ✗ {e:#}");
                        pending::settle(data_dir, &p.id, false, format!("{e:#}"), None)?;
                    }
                }
            }
            if any {
                notify_daemon_reload(data_dir);
            }
            Ok(())
        }
        MeshCommand::Status => {
            match (files.load_identity()?, files.load_mesh()?) {
                (Some(id), Some(mesh)) => {
                    println!(
                        "mesh '{}' — this node: '{}' ({})",
                        mesh.mesh,
                        id.node,
                        if id.cert.is_some() {
                            "certified"
                        } else {
                            "NOT certified"
                        }
                    );
                    if files.load_root()?.is_some() {
                        println!("root key: PRESENT on this node");
                    }
                    if let Some(u) = &mesh.relay {
                        println!("relay: {u}");
                    }
                    let peers = files.verified_peers()?;
                    if peers.is_empty() {
                        println!("peers: none");
                    }
                    for p in peers {
                        println!(
                            "  peer '{}'{}",
                            p.cert.node,
                            p.url
                                .map(|u| format!(" — dials {u}"))
                                .unwrap_or_else(|| " — inbound only (this peer dials us)".into())
                        );
                    }
                }
                (Some(id), None) => println!(
                    "identity '{}' exists but no mesh joined (enrolled, awaiting join?)",
                    id.node
                ),
                _ => println!("no mesh membership on this node (see `aspen mesh init` / `enroll`)"),
            }
            Ok(())
        }
    }
}

fn bus_log(data_dir: &std::path::Path, lines: i64) -> Result<()> {
    let store = aspen_node::BusStore::open(&data_dir.join("bus.db"))?;
    let rows = store.log(lines)?;
    if rows.is_empty() {
        println!("(no messages)");
        return Ok(());
    }
    for m in rows {
        let state = if m.ingested_at.is_some() {
            format!("ingested via {}", m.delivered_via.as_deref().unwrap_or("?"))
        } else if m.delivered_at.is_some() {
            format!(
                "delivered via {}",
                m.delivered_via.as_deref().unwrap_or("?")
            )
        } else {
            "pending".into()
        };
        let mark = match m.urgency.as_str() {
            "gating" => "!",
            "notice" => "~",
            _ => " ",
        };
        println!(
            "{:>4} {mark} @{} → {}  [{state}]{}{}",
            m.id,
            m.sender,
            m.to_display,
            m.thread
                .as_deref()
                .map(|t| format!("  (t:{t})"))
                .unwrap_or_default(),
            m.record_ref
                .as_deref()
                .map(|r| format!("  rec:{r}"))
                .unwrap_or_default(),
        );
        let snippet: String = m.body.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet: String = snippet.chars().take(100).collect();
        println!("       {snippet}");
    }
    Ok(())
}

async fn dev_oneshot(
    node: &Node,
    name: String,
    repo: PathBuf,
    prompt: String,
    allow_all: bool,
    model: Option<String>,
    resume: Option<String>,
) -> Result<()> {
    let opts = SpawnOpts {
        allow_all,
        model,
        resume,
        ..Default::default()
    };
    eprintln!("[aspen] spawning @{name}…");
    node.spawn_agent(&name, repo, opts).await?;
    let mut events = node.subscribe(&name).expect("just spawned");
    node.send_operator_message(&name, prompt).await?;

    while let Ok(ev) = events.recv().await {
        if render_event(&name, &ev, false) {
            break;
        }
    }
    node.shutdown_agent(&name).await.ok();
    drain_until_exit(&name, node).await;
    eprintln!("[aspen] session closed");
    Ok(())
}

async fn dev_chat(
    node: &Node,
    name: String,
    repo: PathBuf,
    allow_all: bool,
    model: Option<String>,
    resume: Option<String>,
) -> Result<()> {
    let opts = SpawnOpts {
        allow_all,
        model,
        resume,
        ..Default::default()
    };
    eprintln!("[aspen] spawning @{name}… (/quit to exit, /int to interrupt)");
    node.spawn_agent(&name, repo, opts).await?;
    let mut events = node.subscribe(&name).expect("just spawned");
    eprintln!("[aspen] ready.");

    {
        let name = name.clone();
        tokio::spawn(async move {
            while let Ok(ev) = events.recv().await {
                render_event(&name, &ev, false);
                if matches!(ev, SessionEvent::Exited { .. }) {
                    break;
                }
            }
        });
    }

    let stdin = std::io::stdin();
    loop {
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end().to_owned();
        match line.as_str() {
            "" => continue,
            "/quit" => break,
            "/int" => {
                node.interrupt(&name).await.ok();
            }
            _ => {
                node.send_operator_message(&name, line).await?;
            }
        }
    }
    node.shutdown_agent(&name).await.ok();
    Ok(())
}

/// Two agents, one repo, a full round trip over the bus:
/// operator → @ping → bus → @pong → bus → @ping.
async fn dev_duo(node: &Node, repo: PathBuf, model: Option<String>, timeout_s: u64) -> Result<()> {
    let ping_opts = SpawnOpts {
        model: model.clone(),
        charter: Some(
            "You are demonstrating the aspen bus. When the operator kicks you off: send @pong \
             one short question you are genuinely curious about, via bus_send (urgency normal), \
             then END YOUR TURN — the reply cannot arrive while you hold the turn. When a \
             [aspen bus] message from @pong arrives, thank them briefly via bus_send, then say \
             ROUNDTRIP-COMPLETE followed by a one-line summary of the exchange."
                .into(),
        ),
        ..Default::default()
    };
    let pong_opts = SpawnOpts {
        model,
        charter: Some(
            "When an [aspen bus] message from a peer arrives, answer it briefly and helpfully \
             via bus_send back to the sender (urgency normal), then end your turn."
                .into(),
        ),
        ..Default::default()
    };

    eprintln!("[aspen] spawning @ping and @pong…");
    node.spawn_agent("ping", repo.clone(), ping_opts).await?;
    node.spawn_agent("pong", repo, pong_opts).await?;
    let mut ping_ev = node.subscribe("ping").expect("spawned");
    let mut pong_ev = node.subscribe("pong").expect("spawned");
    eprintln!("[aspen] both up; kicking off @ping\n");

    node.send_operator_message(
        "ping",
        "Kick off the demonstration now: message @pong per your charter, then end your turn."
            .into(),
    )
    .await?;

    // Completion is judged from the STORE, never from prose: the round trip
    // is done when a reply from @pong has actually been delivered into
    // @ping's session and @ping has finished the turn that received it.
    // (A text sentinel fired falsely on first run — the model *mentioned*
    // the sentinel while planning to say it later.)
    let store = node.inner.store.clone();
    let round_trip_done = move || -> bool {
        store.log(100).is_ok_and(|rows| {
            rows.iter()
                .any(|m| m.sender == "pong" && m.recipient == "ping" && m.delivered_at.is_some())
        })
    };
    let watch = async {
        loop {
            let ev = tokio::select! {
                ev = ping_ev.recv() => ev.map(|e| ("ping", e)),
                ev = pong_ev.recv() => ev.map(|e| ("pong", e)),
            };
            let Ok((who, ev)) = ev else { continue };
            render_event(who, &ev, true);
            if who == "ping" && matches!(ev, SessionEvent::TurnEnded { .. }) && round_trip_done() {
                break;
            }
        }
    };
    let timed_out = tokio::time::timeout(std::time::Duration::from_secs(timeout_s), watch)
        .await
        .is_err();

    eprintln!("\n[aspen] shutting both down…");
    node.shutdown_agent("ping").await.ok();
    node.shutdown_agent("pong").await.ok();
    drain_until_exit("ping", node).await;
    drain_until_exit("pong", node).await;

    println!("\n=== bus trail ===");
    for m in node.inner.store.log(20)? {
        let state = if m.ingested_at.is_some() {
            "ingested"
        } else if m.delivered_at.is_some() {
            "delivered"
        } else {
            "pending"
        };
        println!(
            "#{} @{} → {} ({}) [{}] {:?}",
            m.id, m.sender, m.to_display, m.urgency, state, m.body
        );
    }
    if timed_out {
        anyhow::bail!("duo round trip did not complete within {timeout_s}s");
    }
    println!("\n[aspen] duo round trip COMPLETE");
    Ok(())
}

async fn drain_until_exit(name: &str, node: &Node) {
    if let Some(mut rx) = node.subscribe(name) {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            while let Ok(ev) = rx.recv().await {
                if matches!(ev, SessionEvent::Exited { .. }) {
                    break;
                }
            }
        })
        .await;
    }
}

/// Print one event for terminal consumption. Returns true on turn end.
fn render_event(name: &str, ev: &SessionEvent, prefix: bool) -> bool {
    let mut out = std::io::stdout().lock();
    let tag = if prefix {
        format!("{name:>5} | ")
    } else {
        String::new()
    };
    match ev {
        SessionEvent::TextDelta { text, thinking } => {
            if !thinking {
                if prefix {
                    // In multiplexed mode, buffer-free line tagging: print
                    // deltas raw; turn boundaries re-anchor the columns.
                    let _ = write!(out, "{text}");
                } else {
                    let _ = write!(out, "{text}");
                }
                let _ = out.flush();
            }
        }
        SessionEvent::ToolUse { tool_name, .. } => {
            let _ = writeln!(out, "\n{tag}[tool] {tool_name}");
        }
        SessionEvent::PermissionSettled {
            tool_name, allowed, ..
        } => {
            let _ = writeln!(
                out,
                "{tag}[perm] {tool_name}: {}",
                if *allowed { "allowed" } else { "denied" }
            );
        }
        SessionEvent::RuntimeInit {
            session_id, model, ..
        } => {
            let _ = writeln!(
                out,
                "{tag}[init] {session_id} ({})",
                model.as_deref().unwrap_or("?")
            );
        }
        SessionEvent::TurnEnded {
            subtype,
            total_cost_usd,
            ..
        } => {
            let _ = writeln!(
                out,
                "\n{tag}[turn end] {subtype} (session total ${:.4})",
                total_cost_usd.unwrap_or(0.0)
            );
            return true;
        }
        SessionEvent::Exited { code } => {
            let _ = writeln!(out, "{tag}[exit] {code:?}");
        }
        _ => {}
    }
    false
}
