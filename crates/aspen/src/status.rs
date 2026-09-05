//! `aspen status` — one readout of the whole node: binary, daemon, sessions,
//! mesh. Disk state is always reported; live detail (roster, link health)
//! comes from the daemon's API when it answers.

use std::path::Path;

use anyhow::Result;

pub fn run(data_dir: &Path) -> Result<()> {
    println!("aspen {}", crate::LONG_VERSION);
    println!("data:   {}", data_dir.display());
    println!();

    let state = crate::read_daemon_state(data_dir);
    let api = match &state {
        None => {
            println!("daemon: not running");
            None
        }
        Some(s) => {
            let pid = s.get("pid").and_then(|p| p.as_u64()).unwrap_or(0);
            let listen = s
                .get("listen")
                .and_then(|l| l.as_str())
                .unwrap_or("127.0.0.1:7420")
                .to_owned();
            let alive = pid_alive(pid);
            let base = format!("http://{}", crate::dial_addr(&listen));
            match query(&base, "/api/node", data_dir, &listen) {
                Ok(node) => {
                    let up = s
                        .get("started_at")
                        .and_then(|t| t.as_u64())
                        .map(|t| {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(t);
                            format!(", up {}", human_duration(now.saturating_sub(t)))
                        })
                        .unwrap_or_default();
                    let ver = node["version"].as_str().unwrap_or("?");
                    let sha = node["sha"].as_str().unwrap_or("?");
                    let headless = if s.get("headless").and_then(|h| h.as_bool()) == Some(true) {
                        " — headless"
                    } else {
                        ""
                    };
                    println!(
                        "daemon: running (pid {pid}) — {} — v{ver} ({sha}){up}{headless}",
                        crate::console_url(data_dir, &listen)
                    );
                    if !listen
                        .parse::<std::net::SocketAddr>()
                        .map(|a| a.ip().is_loopback())
                        .unwrap_or(true)
                    {
                        println!("        listening beyond loopback: other machines can dial this node; the console needs the token above");
                    }
                    if ver != env!("CARGO_PKG_VERSION") {
                        println!(
                            "        note: daemon runs v{ver}, this binary is v{} — `aspen update --restart` or `aspen down && aspen up -d` to switch",
                            env!("CARGO_PKG_VERSION")
                        );
                    }
                    Some((base, listen))
                }
                Err(e) => {
                    if alive == Some(false) {
                        println!(
                            "daemon: STALE state file — pid {pid} is not running (remove {})",
                            data_dir.join("daemon.json").display()
                        );
                    } else {
                        println!("daemon: pid {pid} on {base}, but the API is not answering: {e}");
                    }
                    None
                }
            }
        }
    };

    // Sessions: live roster when the daemon answers, otherwise the ledger.
    if let Some((base, listen)) = &api {
        match query(base, "/api/agents", data_dir, listen) {
            Ok(serde_json::Value::Array(agents)) => {
                let live = agents
                    .iter()
                    .filter(|a| a["live"].as_bool() == Some(true))
                    .count();
                println!("sessions: {live} live / {} registered", agents.len());
                for a in &agents {
                    let name = a["name"].as_str().unwrap_or("?");
                    let node = a["node"]
                        .as_str()
                        .map(|n| format!("@{n}"))
                        .unwrap_or_default();
                    let channel = a["channel"].as_str().unwrap_or("?");
                    let state = if a["live"].as_bool() == Some(true) {
                        let ts = a["turn_state"].as_str().unwrap_or("?");
                        match a["last_tool"].as_str() {
                            Some(tool) if ts != "idle" => format!("{ts} ({tool})"),
                            _ => ts.to_owned(),
                        }
                    } else {
                        "stopped".into()
                    };
                    println!("  @{name}{node}  #{channel}  {state}");
                }
            }
            Ok(_) => {}
            Err(e) => println!("sessions: could not read roster: {e}"),
        }
    }
    let ledger = data_dir.join("resume.json");
    if let Ok(text) = std::fs::read_to_string(&ledger) {
        let names: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
        if !names.is_empty() {
            println!(
                "resume: {} session(s) pending revive on next `aspen up`: {}",
                names.len(),
                names
                    .iter()
                    .map(|n| format!("@{n}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
    println!();

    // Servicing: what the daemon knows about releases and its own state.
    if let Some((base, listen)) = &api {
        if let Ok(u) = query(base, "/api/update", data_dir, listen) {
            let policy = &u["policy"];
            let mode = policy["mode"].as_str().unwrap_or("notify");
            let mut pol = format!("policy {mode}");
            if let Some(w) = policy["window"].as_str() {
                pol.push_str(&format!(" · window {w}"));
            }
            if let Some(sk) = policy["soak"].as_str() {
                pol.push_str(&format!(" · soak {sk}"));
            }
            if let Some(v) = u["available"]["version"].as_str() {
                let skipped = u["skipped"].as_bool() == Some(true);
                println!(
                    "update: v{v} available{} — {pol}",
                    if skipped { " (snoozed)" } else { "" }
                );
            } else if u["withdrawn"].as_bool() == Some(true) {
                println!(
                    "update: running v{} is newer than the latest published (v{}) — withdrawn? — {pol}",
                    u["current"].as_str().unwrap_or("?"),
                    u["latest"].as_str().unwrap_or("?")
                );
            } else {
                match u["last_check"]["ok"].as_bool() {
                    Some(true) => println!("update: up to date — {pol}"),
                    Some(false) => println!(
                        "update: last check failed: {} — {pol}",
                        u["last_check"]["error"].as_str().unwrap_or("?")
                    ),
                    None => println!("update: not checked yet — {pol}"),
                }
            }
            let st = &u["state"];
            match st["state"].as_str() {
                Some("draining") => println!(
                    "        DRAINING for v{} ({}, by {}){}",
                    st["target"].as_str().unwrap_or("?"),
                    st["when"].as_str().unwrap_or("quiet"),
                    st["by"].as_str().unwrap_or("?"),
                    st["waiting_on"]
                        .as_array()
                        .filter(|w| !w.is_empty())
                        .map(|w| format!(
                            " — waiting on: {}",
                            w.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .unwrap_or_default()
                ),
                Some("updating") => println!(
                    "        UPDATING to v{} — the updater is running; this daemon will restart",
                    st["target"].as_str().unwrap_or("?")
                ),
                _ => {}
            }
            let o = &u["last_outcome"];
            if o.is_object() {
                println!(
                    "        last update: {} → {} {}{}",
                    o["from"].as_str().unwrap_or("?"),
                    o["to"].as_str().unwrap_or("?"),
                    if o["rolled_back"].as_bool() == Some(true) {
                        "ROLLED BACK"
                    } else if o["ok"].as_bool() == Some(true) {
                        "ok"
                    } else {
                        "FAILED"
                    },
                    o["error"]
                        .as_str()
                        .map(|e| format!(" — {e}"))
                        .unwrap_or_default()
                );
            }
            if let Some(cv) = u["inventory"]["claude_version"].as_str() {
                println!("harness: {cv}");
            }
            println!();
        }
    }

    // Mesh: live link health when possible, disk configuration otherwise.
    let live_mesh = api
        .as_ref()
        .and_then(|(base, listen)| query(base, "/api/mesh", data_dir, listen).ok());
    match live_mesh {
        Some(m) if m["in_mesh"].as_bool() == Some(true) => {
            println!(
                "mesh '{}' — this node: '{}'",
                m["mesh"].as_str().unwrap_or("?"),
                m["node"].as_str().unwrap_or("?")
            );
            let peers = m["peers"].as_array().cloned().unwrap_or_default();
            if peers.is_empty() {
                println!("  peers: none");
            }
            for p in peers {
                let updown = if p["link_up"].as_bool() == Some(true) {
                    let kind = match p["link_kind"].as_str() {
                        Some("direct") => " (direct)".to_owned(),
                        Some(k) => format!(" (via {})", k.trim_start_matches("relay:")),
                        None => String::new(),
                    };
                    format!(
                        "link UP{kind} · {} agent(s)",
                        p["agents"].as_u64().unwrap_or(0)
                    )
                } else {
                    "link down".into()
                };
                let dial = p["url"]
                    .as_str()
                    .map(|u| format!(" — dials {u}"))
                    .unwrap_or_else(|| " — inbound only (this peer dials us)".into());
                let h = &p["health"];
                let mut svc = String::new();
                if let Some(v) = h["version"].as_str() {
                    svc.push_str(&format!(" · v{v}"));
                    if v != env!("CARGO_PKG_VERSION") {
                        svc.push_str(" (skew)");
                    }
                }
                if let Some(a) = h["update_available"].as_str() {
                    svc.push_str(&format!(" · v{a} available"));
                }
                if let Some(st) = h["service_state"].as_str().filter(|s| *s != "ready") {
                    svc.push_str(&format!(" · {st}"));
                    if let Some(d) = h["service_detail"].as_str() {
                        svc.push_str(&format!(" ({d})"));
                    }
                }
                println!(
                    "  peer '{}'  {updown}{dial}{svc}",
                    p["node"].as_str().unwrap_or("?")
                );
            }
            if let Some(url) = m["relay"]["url"].as_str() {
                let conn = if m["relay"]["connected_at"].is_null() {
                    "NOT connected"
                } else {
                    "connected"
                };
                println!("  relay {url} — {conn}");
            }
        }
        Some(_) => {
            // Daemon says no mesh. If the files say otherwise, the daemon
            // missed a reload (it was unreachable when the mesh command ran).
            let files = aspen_node::mesh::MeshFiles::new(data_dir);
            match (files.load_identity()?, files.load_mesh()?) {
                (Some(id), Some(m)) if id.cert.is_some() => println!(
                    "mesh '{}' is configured on disk but the running daemon hasn't loaded it — run `aspen mesh reload`",
                    m.mesh
                ),
                _ => println!("mesh: none configured (see `aspen mesh init` / `enroll`)"),
            }
        }
        // Daemon down (or unreachable): report what's configured on disk.
        None => disk_mesh(data_dir)?,
    }
    Ok(())
}

fn disk_mesh(data_dir: &Path) -> Result<()> {
    let files = aspen_node::mesh::MeshFiles::new(data_dir);
    match (files.load_identity()?, files.load_mesh()?) {
        (Some(id), Some(mesh)) => {
            println!(
                "mesh '{}' — this node: '{}' ({}){}  [from disk; daemon down]",
                mesh.mesh,
                id.node,
                if id.cert.is_some() {
                    "certified"
                } else {
                    "NOT certified"
                },
                if files.load_root()?.is_some() {
                    ", root key present"
                } else {
                    ""
                },
            );
            let peers = files.verified_peers()?;
            if peers.is_empty() {
                println!("  peers: none");
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
            if let Some(u) = &mesh.relay {
                println!("  relay {u}");
            }
        }
        (Some(id), None) => println!(
            "mesh: identity '{}' exists but no mesh joined (enrolled, awaiting join?)",
            id.node
        ),
        _ => println!("mesh: none configured (see `aspen mesh init` / `enroll`)"),
    }
    Ok(())
}

/// GET a daemon API path. Loopback listeners take no token; non-loopback
/// ones require the node token from the data dir.
fn query(base: &str, path: &str, data_dir: &Path, listen: &str) -> Result<serde_json::Value> {
    let mut req = ureq::get(&format!("{base}{path}")).timeout(std::time::Duration::from_secs(3));
    let loopback = listen
        .parse::<std::net::SocketAddr>()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(true);
    if !loopback {
        if let Ok(token) = std::fs::read_to_string(data_dir.join("api-token")) {
            req = req.set("X-Aspen-Token", token.trim());
        }
    }
    Ok(req.call()?.into_json()?)
}

/// None = unknowable on this platform (fall back to the API probe).
fn pid_alive(pid: u64) -> Option<bool> {
    #[cfg(unix)]
    {
        Some(unsafe { libc::kill(pid as i32, 0) } == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

fn human_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
        _ => format!("{}d {}h", secs / 86400, (secs % 86400) / 3600),
    }
}
