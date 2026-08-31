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

#[derive(Parser)]
#[command(name = "aspen", version, about = "Aspen node daemon")]
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

/// Find a built console without configuration: ui/dist relative to the
/// executable's dev tree, or ~/.aspen/ui.
fn default_ui_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::current_dir().ok().map(|d| d.join("ui/dist")),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.ancestors().nth(3).map(|r| r.join("ui/dist"))),
        Some(default_data_dir().join("ui")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|d| d.join("index.html").is_file())
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
        /// Listen address for the API and console.
        #[arg(long, default_value = "127.0.0.1:7420")]
        listen: std::net::SocketAddr,
        /// Directory with the built SPA (defaults to ui/dist next to the
        /// binary's source tree, if present).
        #[arg(long)]
        ui: Option<PathBuf>,
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
    /// Mesh membership: identity, certification, peers.
    Mesh {
        #[command(subcommand)]
        command: MeshCommand,
    },
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
    /// Where the root key lives: turn an enroll blob into a cert blob.
    Certify { blob: String },
    /// On the new node: install the cert blob, joining the mesh.
    Join { blob: String },
    /// Print this node's cert blob (give it to peers via `peers add`).
    Export,
    /// Register a peer's cert blob, optionally with a dial URL.
    PeersAdd {
        blob: String,
        /// ws://host:port/api/federation/ws — omit if the peer dials us.
        #[arg(long)]
        url: Option<String>,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aspen=info,aspen_claude=info,aspen_node=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Up { listen, ui } => {
            let node = Node::open(&cli.data_dir)?;
            let ui_dir = ui.or_else(default_ui_dir);
            match &ui_dir {
                Some(d) => eprintln!("[aspen] serving console from {}", d.display()),
                None => eprintln!("[aspen] no ui/dist found — API only"),
            }
            api::serve(node, listen, ui_dir, &cli.data_dir).await
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

fn mesh_command(data_dir: &std::path::Path, cmd: MeshCommand) -> Result<()> {
    use aspen_node::mesh::{MeshConfig, MeshFiles};
    use aspen_wire::identity::{self, JoinRequest, MeshRoot, NodeCert, NodeIdentity};

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
            if files.load_mesh()?.is_some() {
                anyhow::bail!("this node already belongs to a mesh (see `aspen mesh status`)");
            }
            let node_name = node.unwrap_or_else(default_node);
            let root = MeshRoot::create(&mesh);
            let mut id = NodeIdentity::create(&node_name);
            let cert = root.certify(&id.join_request())?;
            id.install_cert(cert)?;
            files.save_root(&root)?;
            files.save_identity(&id)?;
            files.save_mesh(&MeshConfig {
                mesh: mesh.clone(),
                root_public: root.root_public.clone(),
                peers: vec![],
                relay: None,
            })?;
            println!("mesh '{mesh}' created; this node is '{node_name}'.");
            println!("ROOT KEY at {} — it IS the mesh. Back it up; never copy it to nodes that don't certify.", data_dir.join("root.key").display());
            println!(
                "\nThis node's cert blob (for `aspen mesh peers-add` on other nodes):\n{}",
                identity::to_blob(
                    "cert",
                    files.load_identity()?.unwrap().cert.as_ref().unwrap()
                )?
            );
            Ok(())
        }
        MeshCommand::Enroll { node } => {
            let node_name = node.unwrap_or_else(default_node);
            let id = match files.load_identity()? {
                Some(existing) if existing.cert.is_some() => {
                    anyhow::bail!("this node already has a certified identity")
                }
                Some(existing) => existing,
                None => {
                    let id = NodeIdentity::create(&node_name);
                    files.save_identity(&id)?;
                    id
                }
            };
            println!("enroll blob for '{}' — run `aspen mesh certify <blob>` where the root key lives:\n{}",
                id.node, identity::to_blob("enroll", &id.join_request())?);
            Ok(())
        }
        MeshCommand::Certify { blob } => {
            let root = files.load_root()?.ok_or_else(|| {
                anyhow::anyhow!("no root key here — run this on the mesh's root node")
            })?;
            let req: JoinRequest = identity::from_blob("enroll", &blob)?;
            let cert = root.certify(&req)?;
            files.add_peer(cert.clone(), None).ok(); // register them here too
            println!(
                "cert blob for '{}' — run `aspen mesh join <blob>` on that node:\n{}",
                cert.node,
                identity::to_blob("cert", &cert)?
            );
            println!("\n(peer '{}' was also added to THIS node's mesh.json — set its URL with `aspen mesh peers-add` if you dial it)", cert.node);
            Ok(())
        }
        MeshCommand::Join { blob } => {
            let cert: NodeCert = identity::from_blob("cert", &blob)?;
            let mut id = files.load_identity()?.ok_or_else(|| {
                anyhow::anyhow!("no identity here — run `aspen mesh enroll` first")
            })?;
            id.install_cert(cert.clone())?;
            files.save_identity(&id)?;
            if files.load_mesh()?.is_none() {
                // First join: trust the root key this cert carries (verified
                // against itself at install; the operator carried the blob).
                files.save_mesh(&MeshConfig {
                    mesh: cert.mesh.clone(),
                    root_public: cert.root_public.clone(),
                    peers: vec![],
                    relay: None,
                })?;
            }
            println!("joined mesh '{}' as node '{}'.", cert.mesh, cert.node);
            println!(
                "\nThis node's cert blob (for `aspen mesh peers-add` on other nodes):\n{}",
                identity::to_blob("cert", &cert)?
            );
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
            let cert: NodeCert = identity::from_blob("cert", &blob)?;
            files.add_peer(cert.clone(), url.clone())?;
            println!(
                "peer '{}' registered{}",
                cert.node,
                url.map(|u| format!(" (dialing {u})")).unwrap_or_default()
            );
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
            let mut mesh = files
                .load_mesh()?
                .ok_or_else(|| anyhow::anyhow!("this node has not joined a mesh"))?;
            mesh.relay = url.clone();
            files.save_mesh(&mesh)?;
            match url {
                Some(u) => println!("relay set: {u} (restart `aspen up` to connect)"),
                None => println!("relay cleared"),
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
                                .unwrap_or_else(|| " — inbound only".into())
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
